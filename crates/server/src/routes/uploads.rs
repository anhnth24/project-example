//! `POST /api/v1/uploads` — streaming quarantine upload intake.
//!
//! Route layer: auth + multipart parse only. Persistence goes through
//! `services::upload` (no direct object-store client types here).

use std::sync::Arc;
use std::time::Duration;

use axum::extract::multipart::Field;
use axum::extract::DefaultBodyLimit;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::api::{ApiError, AppMultipart};
use crate::auth::middleware::AuthenticatedOrg;
use crate::auth::permissions::require_permission;
use crate::db::api_idempotency::{self, IdempotencyClaim, IdempotencyScope};
use crate::db::pool::with_org_txn;
use crate::http::AppState;
use crate::services::quota::{self, QuotaError, QuotaSnapshot};
use crate::services::upload::{
    quota_reserve_hook, spawn_quota_settled_quarantine, stream_to_tempfile_with_idle_timeout,
    Disposition, LimitsConfig, QuotaSettledUploadError, ReasonCode, StreamedUpload, ThreatClass,
    UploadError, UploadOutcome,
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
    /// Opaque upload identity only — never the quarantine object key.
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
    headers: HeaderMap,
    AppMultipart(multipart): AppMultipart,
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

    let client_idempotency = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    if let Some(ref key) = client_idempotency {
        validate_idempotency_key(key)
            .map_err(|message| UploadRouteError::Validation(message, request_id.clone()))?;
    }
    let filename = pending.declared_filename.as_deref().unwrap_or("");
    let content_type = pending.declared_content_type.as_deref().unwrap_or("");
    let size_bytes = pending.streamed.size_bytes.to_string();
    let request_hash = api_idempotency::hash_request_parts(&[
        b"upload",
        pending.streamed.sha256_hex.as_bytes(),
        size_bytes.as_bytes(),
        filename.as_bytes(),
        content_type.as_bytes(),
    ]);

    // Claim before any quota/storage side effects so failed claims never orphan reservations.
    let mut claimed_key: Option<String> = None;
    if let Some(ref key) = client_idempotency {
        match claim_upload_idempotency(state.pool(), &auth.context, key, &request_hash, &request_id)
            .await?
        {
            Some(replay) => return Ok(replay),
            None => claimed_key = Some(key.clone()),
        }
    }

    let storage = match state.object_store() {
        Some(store) => store,
        None => {
            if let Some(ref key) = claimed_key {
                let _ = abandon_upload_idempotency(state.pool(), &auth.context, key, &request_hash)
                    .await;
            }
            return Err(UploadRouteError::Upload(
                UploadError::StorageUnavailable,
                request_id.clone(),
            ));
        }
    };
    let reservation_key = match upload_reservation_key(&auth, &request_id, &headers) {
        Ok(key) => key,
        Err(message) => {
            if let Some(ref key) = claimed_key {
                let _ = abandon_upload_idempotency(state.pool(), &auth.context, key, &request_hash)
                    .await;
            }
            return Err(UploadRouteError::Validation(message, request_id.clone()));
        }
    };
    let reservation = match quota_reserve_hook(
        state.pool(),
        &auth.context,
        &reservation_key,
        pending.streamed.size_bytes,
    )
    .await
    {
        Ok(reservation) => reservation,
        Err(error) => {
            if let Some(ref key) = claimed_key {
                let _ = abandon_upload_idempotency(state.pool(), &auth.context, key, &request_hash)
                    .await;
            }
            return Err(UploadRouteError::Quota(error, request_id.clone()));
        }
    };
    if !reservation.storage.created || !reservation.document.created {
        if let Some(ref key) = claimed_key {
            let _ =
                abandon_upload_idempotency(state.pool(), &auth.context, key, &request_hash).await;
        }
        return Err(UploadRouteError::Quota(
            QuotaError::ReservationConflict,
            request_id,
        ));
    }

    let handle = spawn_quota_settled_quarantine(
        state.pool().clone(),
        auth.context.clone(),
        storage.clone(),
        limits,
        pending.streamed,
        pending.declared_filename,
        pending.declared_content_type,
        reservation_key,
    );
    match handle.await {
        Ok(Ok(success)) => {
            let (status, body) = upload_response_parts(success.outcome, &request_id);
            if let Some(ref key) = claimed_key {
                let body_json = serde_json::to_value(&body).map_err(|_| {
                    UploadRouteError::Upload(UploadError::Internal, request_id.clone())
                })?;
                finalize_upload_idempotency(
                    state.pool(),
                    &auth.context,
                    key,
                    &request_hash,
                    status.as_u16() as i32,
                    &body_json,
                    &request_id,
                )
                .await?;
            }
            Ok(success_response_from_parts(
                status,
                body,
                Some(success.quota_snapshot),
            ))
        }
        Ok(Err(QuotaSettledUploadError::Upload(error))) => {
            if let Some(ref key) = claimed_key {
                let _ = abandon_upload_idempotency(state.pool(), &auth.context, key, &request_hash)
                    .await;
            }
            Err(UploadRouteError::Upload(error, request_id.clone()))
        }
        Ok(Err(QuotaSettledUploadError::Quota { error, .. })) => {
            if let Some(ref key) = claimed_key {
                let _ = abandon_upload_idempotency(state.pool(), &auth.context, key, &request_hash)
                    .await;
            }
            Err(UploadRouteError::Quota(error, request_id.clone()))
        }
        Err(_) => {
            if let Some(ref key) = claimed_key {
                let _ = abandon_upload_idempotency(state.pool(), &auth.context, key, &request_hash)
                    .await;
            }
            Err(UploadRouteError::Upload(
                UploadError::Internal,
                request_id.clone(),
            ))
        }
    }
}

/// Returns `Some(replay)` when a completed identical request exists; `None` after a fresh claim.
async fn claim_upload_idempotency(
    pool: &deadpool_postgres::Pool,
    ctx: &crate::auth::context::OrgContext,
    key: &str,
    request_hash: &str,
    request_id: &str,
) -> Result<Option<Response>, UploadRouteError> {
    let claim = with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        let key = key.to_string();
        let request_hash = request_hash.to_string();
        move |txn| {
            Box::pin(async move {
                api_idempotency::claim_or_replay(
                    txn,
                    &ctx,
                    IdempotencyScope::Upload,
                    &key,
                    &request_hash,
                    api_idempotency::DEFAULT_IN_PROGRESS_TTL,
                )
                .await
            })
        }
    })
    .await
    .map_err(|error| map_upload_idempotency_db(error, request_id))?;
    match claim {
        IdempotencyClaim::Proceed => Ok(None),
        IdempotencyClaim::Replay(stored) => {
            let status =
                StatusCode::from_u16(stored.response_status as u16).unwrap_or(StatusCode::CREATED);
            Ok(Some((status, Json(stored.response_body)).into_response()))
        }
    }
}

async fn finalize_upload_idempotency(
    pool: &deadpool_postgres::Pool,
    ctx: &crate::auth::context::OrgContext,
    key: &str,
    request_hash: &str,
    response_status: i32,
    response_body: &serde_json::Value,
    request_id: &str,
) -> Result<(), UploadRouteError> {
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        let key = key.to_string();
        let request_hash = request_hash.to_string();
        let response_body = response_body.clone();
        move |txn| {
            Box::pin(async move {
                api_idempotency::finalize(
                    txn,
                    &ctx,
                    IdempotencyScope::Upload,
                    &key,
                    &request_hash,
                    response_status,
                    &response_body,
                )
                .await
            })
        }
    })
    .await
    .map_err(|error| map_upload_idempotency_db(error, request_id))
}

async fn abandon_upload_idempotency(
    pool: &deadpool_postgres::Pool,
    ctx: &crate::auth::context::OrgContext,
    key: &str,
    request_hash: &str,
) -> Result<(), ()> {
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        let key = key.to_string();
        let request_hash = request_hash.to_string();
        move |txn| {
            Box::pin(async move {
                api_idempotency::abandon(txn, &ctx, IdempotencyScope::Upload, &key, &request_hash)
                    .await
            })
        }
    })
    .await
    .map_err(|_| ())
}

fn map_upload_idempotency_db(
    error: crate::db::error::DbError,
    request_id: &str,
) -> UploadRouteError {
    match error {
        crate::db::error::DbError::Config(ref message) if message == "idempotency_key_conflict" => {
            UploadRouteError::IdempotencyConflict(request_id.to_string())
        }
        crate::db::error::DbError::Config(ref message) if message == "idempotency_in_progress" => {
            UploadRouteError::IdempotencyInProgress(request_id.to_string())
        }
        _ => UploadRouteError::Upload(UploadError::Internal, request_id.to_string()),
    }
}

async fn read_multipart(
    mut multipart: axum::extract::Multipart,
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
        .map_err(|error| {
            if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
                UploadError::rejected(ThreatClass::Oversize, ReasonCode::UploadTooLarge)
            } else {
                UploadError::MultipartInvalid {
                    reason: ReasonCode::MultipartMissingFile,
                }
            }
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

fn upload_response_parts(outcome: UploadOutcome, request_id: &str) -> (StatusCode, UploadResponse) {
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
        object_id: outcome.object_id.to_string(),
        sha256: outcome.sha256_hex,
        size_bytes: outcome.size_bytes,
        canonical_format: outcome.canonical_format.as_str().to_string(),
        original_filename: outcome.original_filename,
        request_id: request_id.to_string(),
    };
    (status, body)
}

fn success_response_from_parts(
    status: StatusCode,
    body: UploadResponse,
    quota_snapshot: Option<QuotaSnapshot>,
) -> Response {
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
    IdempotencyConflict(String),
    IdempotencyInProgress(String),
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
            Self::IdempotencyConflict(request_id) => (
                StatusCode::CONFLICT,
                Json(ApiError {
                    code: "idempotency_key_conflict".into(),
                    message: "Idempotency-Key was reused with a different request".into(),
                    request_id,
                    details: None,
                }),
            )
                .into_response(),
            Self::IdempotencyInProgress(request_id) => (
                StatusCode::CONFLICT,
                Json(ApiError {
                    code: "idempotency_in_progress".into(),
                    message: "Idempotency-Key request is still in progress".into(),
                    request_id,
                    details: None,
                }),
            )
                .into_response(),
        }
    }
}

fn upload_reservation_key(
    auth: &AuthenticatedOrg,
    request_id: &str,
    headers: &HeaderMap,
) -> Result<String, String> {
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
    Ok(format!("op.{}", hex::encode(hasher.finalize())))
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
