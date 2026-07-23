//! `POST /api/v1/uploads` — streaming quarantine upload intake.
//!
//! Route layer: auth + multipart parse only. Persistence goes through
//! `services::upload` saga (no detached half-saga after object put).

use std::sync::Arc;
use std::time::Duration;

use axum::extract::multipart::Field;
use axum::extract::DefaultBodyLimit;
use axum::extract::{Multipart, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::api::ApiError;
use crate::auth::middleware::AuthenticatedOrg;
use crate::auth::permissions::require_permission;
use crate::http::AppState;
use crate::services::quota::{self, QuotaError, QuotaSnapshot};
use uuid::Uuid;

use crate::services::upload::{
    run_upload_saga, stream_to_tempfile_with_idle_timeout, Disposition, LimitsConfig,
    QuarantineIdentity, ReasonCode, SagaError, SagaInput, ThreatClass, UploadError, UploadOutcome,
};

pub fn router(max_upload_bytes: usize) -> Router<Arc<AppState>> {
    Router::new().route(
        "/api/v1/uploads",
        post(create_upload).layer(DefaultBodyLimit::max(max_upload_bytes)),
    )
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UploadResponse {
    disposition: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    threat_class: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason_code: Option<String>,
    object_key: String,
    object_id: String,
    sha256: String,
    size_bytes: u64,
    canonical_format: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    original_filename: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    collection_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    document_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    job_id: Option<String>,
    request_id: String,
}

async fn create_upload(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    client_ip: Option<axum::Extension<crate::middleware::ClientIp>>,
    headers: HeaderMap,
    multipart: Multipart,
) -> Result<Response, UploadRouteError> {
    let request_id = auth.request_id.clone();
    if state.ensure_mutations_allowed().await.is_err() {
        return Err(UploadRouteError::MutationsPaused(request_id.clone()));
    }
    let ip = client_ip
        .map(|ext| ext.0 .0.clone())
        .unwrap_or_else(|| "unknown".into());
    if let Err(rejected) = crate::routes::rate_limit_guard::check_user(
        &state,
        &auth.context.org_id().to_string(),
        &auth.context.user_id().to_string(),
        &request_id,
    ) {
        return Err(UploadRouteError::RateLimited(rejected));
    }
    if let Err(rejected) =
        crate::routes::rate_limit_guard::check_route(&state, "upload", &ip, &request_id)
    {
        return Err(UploadRouteError::RateLimited(rejected));
    }
    if require_permission(&auth.context, "doc.upload").is_err() {
        crate::services::audit::record_deny(
            state.pool(),
            &auth.context,
            &request_id,
            "document.upload",
            "document",
            None,
            "permission_denied",
        )
        .await
        .map_err(|_| UploadRouteError::Upload(UploadError::Internal, request_id.clone()))?;
        return Err(UploadRouteError::Upload(
            UploadError::PermissionDenied,
            request_id.clone(),
        ));
    }

    let limits = state.runtime().config().upload().limits;
    let upload_timeout = Duration::from_secs(limits.upload_timeout_secs);

    let pending = tokio::time::timeout(upload_timeout, read_multipart(multipart, limits))
        .await
        .map_err(|_| {
            UploadRouteError::Upload(
                UploadError::MultipartInvalid {
                    reason: ReasonCode::MultipartTimeout,
                },
                request_id.clone(),
            )
        })?
        .map_err(|error| UploadRouteError::Upload(error, request_id.clone()))?;

    let collection_id = pending.collection_id.ok_or_else(|| {
        UploadRouteError::Validation("collectionId is required".into(), request_id.clone())
    })?;
    if !auth.context.allows_collection(collection_id) {
        let resource_id = collection_id.to_string();
        crate::services::audit::record_deny(
            state.pool(),
            &auth.context,
            &request_id,
            "document.upload",
            "collection",
            Some(&resource_id),
            "collection_denied",
        )
        .await
        .map_err(|_| UploadRouteError::Upload(UploadError::Internal, request_id.clone()))?;
        return Err(UploadRouteError::Upload(
            UploadError::PermissionDenied,
            request_id.clone(),
        ));
    }

    let storage = state.object_store().ok_or_else(|| {
        UploadRouteError::Upload(UploadError::StorageUnavailable, request_id.clone())
    })?;
    let (idempotency_key, reservation_key) = upload_operation_keys(&auth, &request_id, &headers)
        .map_err(|message| UploadRouteError::Validation(message, request_id.clone()))?;

    let identity = QuarantineIdentity {
        object_id: Uuid::new_v4(),
        collection_id,
        document_id: Uuid::new_v4(),
        version_id: Uuid::new_v4(),
    };

    // Detach put→register→finalize so a cancelled HTTP request cannot leave a
    // reserved quota half-saga; the child task always reaches a terminal settle.
    // Capture correlation BEFORE spawn — task-locals do not cross join boundaries.
    let corr = crate::telemetry::CorrelationContext::current()
        .unwrap_or_else(|| crate::telemetry::CorrelationContext::new(request_id.clone()));
    let pool = state.pool().clone();
    let storage = storage.clone();
    let ctx = auth.context.clone();
    let saga = tokio::spawn(async move {
        crate::telemetry::scope(corr, async {
            run_upload_saga(
                &pool,
                &storage,
                &ctx,
                &limits,
                SagaInput {
                    collection_id,
                    idempotency_key,
                    reservation_key,
                    streamed: pending.streamed,
                    declared_filename: pending.declared_filename,
                    declared_content_type: pending.declared_content_type,
                    identity,
                },
            )
            .await
        })
        .await
    });
    let success = match saga.await {
        Ok(Ok(success)) => success,
        Ok(Err(error)) => return Err(UploadRouteError::from_saga(error, request_id.clone())),
        Err(_) => {
            return Err(UploadRouteError::Upload(
                UploadError::Internal,
                request_id.clone(),
            ))
        }
    };

    let quota_snapshot = if success.replayed {
        None
    } else {
        Some(success.quota_snapshot)
    };
    Ok(success_response(
        success.outcome,
        &request_id,
        quota_snapshot,
        Some(success.registered),
    ))
}

async fn read_multipart(
    mut multipart: Multipart,
    limits: LimitsConfig,
) -> Result<PendingUpload, UploadError> {
    let mut saw_file = false;
    let mut pending: Option<PendingUpload> = None;
    let mut collection_id = None;
    let mut parts_seen = 0_u32;
    let idle_timeout = Duration::from_secs(limits.upload_idle_timeout_secs);

    while let Some(field) = tokio::time::timeout(idle_timeout, multipart.next_field())
        .await
        .map_err(|_| UploadError::MultipartInvalid {
            reason: ReasonCode::MultipartTimeout,
        })?
        .map_err(|_| UploadError::MultipartInvalid {
            reason: ReasonCode::MultipartMissingFile,
        })?
    {
        parts_seen = parts_seen.saturating_add(1);
        if parts_seen > limits.max_multipart_parts {
            return Err(UploadError::MultipartInvalid {
                reason: ReasonCode::MultipartTooManyParts,
            });
        }
        enforce_part_header_limit(&field, &limits)?;
        let field_name = field.name().unwrap_or("").to_string();
        let is_file = field.file_name().is_some()
            || matches!(field_name.as_str(), "file" | "upload" | "document");
        if !is_file {
            if matches!(field_name.as_str(), "collectionId" | "collection_id") {
                let raw = read_text_field(field, &limits, idle_timeout).await?;
                collection_id = Some(Uuid::parse_str(raw.trim()).map_err(|_| {
                    UploadError::MultipartInvalid {
                        reason: ReasonCode::MultipartMissingFile,
                    }
                })?);
            } else {
                drain_non_file_field(field, &limits, idle_timeout).await?;
            }
            continue;
        }
        if saw_file {
            return Err(UploadError::MultipartInvalid {
                reason: ReasonCode::MultipartTooManyFiles,
            });
        }
        saw_file = true;
        let declared = field.file_name().map(str::to_owned);
        let declared_content_type = field.content_type().map(str::to_owned);
        let streamed = stream_to_tempfile_with_idle_timeout(field, &limits, idle_timeout).await?;
        pending = Some(PendingUpload {
            streamed,
            declared_filename: declared,
            declared_content_type,
            collection_id: None,
        });
    }

    let mut pending = pending.ok_or(UploadError::MultipartInvalid {
        reason: ReasonCode::MultipartMissingFile,
    })?;
    pending.collection_id = collection_id;
    Ok(pending)
}

async fn read_text_field(
    mut field: Field<'_>,
    limits: &LimitsConfig,
    idle_timeout: Duration,
) -> Result<String, UploadError> {
    let mut bytes = Vec::new();
    loop {
        let chunk = tokio::time::timeout(idle_timeout, field.chunk())
            .await
            .map_err(|_| UploadError::MultipartInvalid {
                reason: ReasonCode::MultipartTimeout,
            })?
            .map_err(|_| UploadError::MultipartInvalid {
                reason: ReasonCode::MultipartMissingFile,
            })?;
        let Some(chunk) = chunk else {
            break;
        };
        if bytes.len().saturating_add(chunk.len()) > 256 {
            return Err(UploadError::MultipartInvalid {
                reason: ReasonCode::MultipartHeaderTooLarge,
            });
        }
        bytes.extend_from_slice(&chunk);
        let _ = limits;
    }
    String::from_utf8(bytes).map_err(|_| UploadError::MultipartInvalid {
        reason: ReasonCode::MultipartMissingFile,
    })
}

struct PendingUpload {
    streamed: crate::services::upload::StreamedUpload,
    declared_filename: Option<String>,
    declared_content_type: Option<String>,
    collection_id: Option<Uuid>,
}

fn enforce_part_header_limit(field: &Field<'_>, limits: &LimitsConfig) -> Result<(), UploadError> {
    let header_bytes = field.name().map_or(0, str::len)
        + field.file_name().map_or(0, str::len)
        + field.content_type().map_or(0, str::len);
    if header_bytes > limits.max_part_header_bytes {
        return Err(UploadError::MultipartInvalid {
            reason: ReasonCode::MultipartHeaderTooLarge,
        });
    }
    Ok(())
}

async fn drain_non_file_field(
    mut field: Field<'_>,
    limits: &LimitsConfig,
    idle_timeout: Duration,
) -> Result<(), UploadError> {
    let mut bytes = 0_u64;
    loop {
        let chunk = tokio::time::timeout(idle_timeout, field.chunk())
            .await
            .map_err(|_| UploadError::MultipartInvalid {
                reason: ReasonCode::MultipartTimeout,
            })?
            .map_err(|_| UploadError::MultipartInvalid {
                reason: ReasonCode::MultipartMissingFile,
            })?;
        let Some(chunk) = chunk else {
            break;
        };
        bytes = bytes.saturating_add(chunk.len() as u64);
        if bytes > limits.max_upload_bytes {
            return Err(UploadError::rejected(
                ThreatClass::Oversize,
                ReasonCode::UploadTooLarge,
            ));
        }
    }
    Ok(())
}

fn success_response(
    outcome: UploadOutcome,
    request_id: &str,
    quota_snapshot: Option<QuotaSnapshot>,
    registered: Option<crate::services::upload::RegisteredUpload>,
) -> Response {
    let status = match outcome.disposition {
        Disposition::Accepted | Disposition::Quarantined => StatusCode::CREATED,
        Disposition::Rejected => StatusCode::BAD_REQUEST,
    };
    let body = UploadResponse {
        disposition: outcome.disposition.as_str().to_string(),
        threat_class: outcome
            .threat_class
            .map(|threat: ThreatClass| threat.as_str().to_string()),
        reason_code: outcome
            .reason_code
            .map(|reason| reason.as_str().to_string()),
        object_key: outcome.object_key.as_str(),
        object_id: outcome.object_id.to_string(),
        sha256: outcome.sha256_hex,
        size_bytes: outcome.size_bytes,
        canonical_format: outcome.canonical_format.as_str().to_string(),
        original_filename: outcome.original_filename,
        collection_id: registered
            .as_ref()
            .map(|item| item.collection_id.to_string()),
        document_id: registered.as_ref().map(|item| item.document_id.to_string()),
        version_id: registered.as_ref().map(|item| item.version_id.to_string()),
        job_id: registered
            .as_ref()
            .and_then(|item| item.job_id.map(|id| id.to_string())),
        request_id: request_id.to_string(),
    };
    let mut response = (status, Json(body)).into_response();
    if let Some(snapshot) = quota_snapshot {
        quota::apply_quota_headers(response.headers_mut(), &snapshot);
    }
    response
}

enum UploadRouteError {
    Upload(UploadError, String),
    Quota(QuotaError, String),
    Validation(String, String),
    Conflict(String, String),
    MutationsPaused(String),
    RateLimited(crate::routes::rate_limit_guard::RateLimitRejected),
}

impl UploadRouteError {
    fn from_saga(error: SagaError, request_id: String) -> Self {
        match error {
            SagaError::Upload(error) => Self::Upload(error, request_id),
            SagaError::Quota(error) => Self::Quota(error, request_id),
            SagaError::PermissionDenied => Self::Upload(UploadError::PermissionDenied, request_id),
            SagaError::IdempotencyConflict => Self::Conflict(
                "Idempotency-Key was reused with a different upload digest".into(),
                request_id,
            ),
            SagaError::IdempotencyInProgress => Self::Conflict(
                "Upload with this Idempotency-Key is already in progress".into(),
                request_id,
            ),
            SagaError::NotFound => Self::Upload(UploadError::Internal, request_id),
            SagaError::Database(_) | SagaError::Job(_) | SagaError::Internal => {
                Self::Upload(UploadError::Internal, request_id)
            }
        }
    }
}

impl IntoResponse for UploadRouteError {
    fn into_response(self) -> Response {
        match self {
            Self::Upload(error, request_id) => error.into_response_with_request_id(&request_id),
            Self::Quota(error, request_id) => error.into_response_with_request_id(&request_id),
            Self::Validation(message, request_id) => (
                StatusCode::BAD_REQUEST,
                Json(ApiError {
                    code: "validation_failed".into(),
                    message,
                    request_id,
                    details: None,
                }),
            )
                .into_response(),
            Self::Conflict(message, request_id) => (
                StatusCode::CONFLICT,
                Json(ApiError {
                    code: "idempotency_conflict".into(),
                    message,
                    request_id,
                    details: None,
                }),
            )
                .into_response(),
            Self::MutationsPaused(request_id) => (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ApiError {
                    code: crate::services::ops_fence::MUTATIONS_PAUSED_CODE.into(),
                    message: "Mutations paused while an ops fence is active".into(),
                    request_id,
                    details: None,
                }),
            )
                .into_response(),
            Self::RateLimited(rejected) => rejected.into_response(),
        }
    }
}

fn upload_operation_keys(
    auth: &AuthenticatedOrg,
    request_id: &str,
    headers: &HeaderMap,
) -> Result<(String, String), String> {
    let operation = match headers.get("idempotency-key") {
        Some(value) => {
            let value = value
                .to_str()
                .map_err(|_| "Idempotency-Key must be visible ASCII".to_string())?;
            validate_idempotency_key(value)?;
            format!("client:{value}")
        }
        None => format!("request:{request_id}"),
    };
    let mut hasher = Sha256::new();
    hasher.update(auth.context.org_id().as_bytes());
    hasher.update(auth.context.user_id().as_bytes());
    hasher.update(operation.as_bytes());
    let reservation_key = format!("op.{}", hex::encode(hasher.finalize()));
    Ok((operation, reservation_key))
}

fn validate_idempotency_key(value: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > 128 {
        return Err("Idempotency-Key must be between 1 and 128 bytes".into());
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':'))
    {
        return Err("Idempotency-Key contains unsupported characters".into());
    }
    Ok(())
}
