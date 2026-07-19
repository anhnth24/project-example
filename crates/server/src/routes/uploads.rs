//! `POST /api/v1/uploads` — streaming quarantine upload intake.
//!
//! Route layer: auth + multipart parse only. Persistence goes through
//! `services::upload` (no direct object-store client types here).

use std::sync::Arc;
use std::time::Duration;

use axum::extract::multipart::Field;
use axum::extract::DefaultBodyLimit;
use axum::extract::{Multipart, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde::Serialize;

use crate::auth::middleware::AuthenticatedOrg;
use crate::auth::permissions::require_permission;
use crate::http::AppState;
use crate::services::quota::{self, QuotaError, QuotaSnapshot};
use crate::services::upload::{
    quota_reserve_hook, stream_to_tempfile_with_idle_timeout, validate_and_quarantine, Disposition,
    LimitsConfig, ReasonCode, StreamedUpload, ThreatClass, UploadError, UploadOutcome,
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
    request_id: String,
}

async fn create_upload(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    multipart: Multipart,
) -> Result<Response, UploadRouteError> {
    let request_id = auth.request_id.clone();
    require_permission(&auth.context, "doc.upload")
        .map_err(|_| UploadRouteError::Upload(UploadError::PermissionDenied, request_id.clone()))?;

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

    let storage = state.object_store().ok_or_else(|| {
        UploadRouteError::Upload(UploadError::StorageUnavailable, request_id.clone())
    })?;
    let reservation_key = upload_reservation_key(&auth, &request_id);
    let quota = quota_reserve_hook(
        state.pool(),
        &auth.context,
        &reservation_key,
        pending.streamed.size_bytes,
    )
    .await
    .map_err(|error| UploadRouteError::Quota(error, request_id.clone()))?;

    let outcome = validate_and_quarantine(
        &auth.context,
        storage,
        &limits,
        pending.streamed,
        pending.declared_filename.as_deref(),
        pending.declared_content_type.as_deref(),
    )
    .await;
    match outcome {
        Ok(outcome) => {
            quota::finalize_upload(state.pool(), &auth.context, &reservation_key)
                .await
                .map_err(|error| UploadRouteError::Quota(error, request_id.clone()))?;
            Ok(success_response(
                outcome,
                &request_id,
                Some(quota.storage_headers()),
            ))
        }
        Err(error) => {
            let _ = quota::refund_upload(state.pool(), &auth.context, &reservation_key).await;
            Err(UploadRouteError::Upload(error, request_id))
        }
    }
}

async fn read_multipart(
    mut multipart: Multipart,
    limits: LimitsConfig,
) -> Result<PendingUpload, UploadError> {
    let mut saw_file = false;
    let mut pending: Option<PendingUpload> = None;
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
        let is_file = field.file_name().is_some()
            || field
                .name()
                .is_some_and(|name| name == "file" || name == "upload" || name == "document");
        if !is_file {
            drain_non_file_field(field, &limits, idle_timeout).await?;
            continue;
        }
        if saw_file {
            return Err(UploadError::MultipartInvalid {
                reason: ReasonCode::MultipartTooManyFiles,
            });
        }
        saw_file = true;
        // Filename is metadata only — never used as a key or filesystem path.
        let declared = field.file_name().map(str::to_owned);
        let declared_content_type = field.content_type().map(str::to_owned);
        let streamed = stream_to_tempfile_with_idle_timeout(field, &limits, idle_timeout).await?;
        pending = Some(PendingUpload {
            streamed,
            declared_filename: declared,
            declared_content_type,
        });
    }

    pending.ok_or(UploadError::MultipartInvalid {
        reason: ReasonCode::MultipartMissingFile,
    })
}

struct PendingUpload {
    streamed: StreamedUpload,
    declared_filename: Option<String>,
    declared_content_type: Option<String>,
}

fn enforce_part_header_limit(field: &Field<'_>, limits: &LimitsConfig) -> Result<(), UploadError> {
    // Secondary per-part metadata bound: the route-level `DefaultBodyLimit`
    // remains the primary whole-request cap, and this check runs before any
    // field body is buffered to disk.
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
}

impl IntoResponse for UploadRouteError {
    fn into_response(self) -> Response {
        match self {
            Self::Upload(error, request_id) => error.into_response_with_request_id(&request_id),
            Self::Quota(error, request_id) => error.into_response_with_request_id(&request_id),
        }
    }
}

fn upload_reservation_key(auth: &AuthenticatedOrg, request_id: &str) -> String {
    format!(
        "{}.{}.{}",
        auth.context.org_id(),
        auth.context.user_id(),
        request_id
    )
}
