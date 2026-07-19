//! `POST /api/v1/uploads` — streaming quarantine upload intake.
//!
//! Route layer: auth + multipart parse only. Persistence goes through
//! `services::upload` (no direct object-store client types here).

use std::sync::Arc;

use axum::extract::{Multipart, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde::Serialize;

use crate::auth::middleware::AuthenticatedOrg;
use crate::auth::permissions::require_permission;
use crate::http::AppState;
use crate::services::upload::{
    stream_to_tempfile, validate_and_quarantine, Disposition, ReasonCode, ThreatClass, UploadError,
    UploadOutcome,
};

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/api/v1/uploads", post(create_upload))
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
    mut multipart: Multipart,
) -> Result<Response, UploadRouteError> {
    let request_id = auth.request_id.clone();
    require_permission(&auth.context, "doc.upload")
        .map_err(|_| UploadRouteError(UploadError::PermissionDenied, request_id.clone()))?;

    let Some(storage) = state.object_store() else {
        return Err(UploadRouteError(
            UploadError::StorageUnavailable,
            request_id.clone(),
        ));
    };
    let limits = state.runtime().config().upload().limits;

    let mut saw_file = false;
    let mut outcome: Option<UploadOutcome> = None;

    while let Some(field) = multipart.next_field().await.map_err(|_| {
        UploadRouteError(
            UploadError::MultipartInvalid {
                reason: ReasonCode::MultipartMissingFile,
            },
            request_id.clone(),
        )
    })? {
        let is_file = field.file_name().is_some()
            || field
                .name()
                .is_some_and(|name| name == "file" || name == "upload" || name == "document");
        if !is_file {
            let _ = field.bytes().await;
            continue;
        }
        if saw_file {
            return Err(UploadRouteError(
                UploadError::MultipartInvalid {
                    reason: ReasonCode::MultipartTooManyFiles,
                },
                request_id.clone(),
            ));
        }
        saw_file = true;
        // Filename is metadata only — never used as a key or filesystem path.
        let declared = field.file_name().map(str::to_owned);
        let streamed = stream_to_tempfile(field, &limits)
            .await
            .map_err(|error| UploadRouteError(error, request_id.clone()))?;
        let result = validate_and_quarantine(
            &auth.context,
            storage,
            &limits,
            streamed,
            declared.as_deref(),
        )
        .await
        .map_err(|error| UploadRouteError(error, request_id.clone()))?;
        outcome = Some(result);
    }

    let outcome = outcome.ok_or(UploadRouteError(
        UploadError::MultipartInvalid {
            reason: ReasonCode::MultipartMissingFile,
        },
        request_id.clone(),
    ))?;
    Ok(success_response(outcome, &request_id))
}

fn success_response(outcome: UploadOutcome, request_id: &str) -> Response {
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
    (status, Json(body)).into_response()
}

struct UploadRouteError(UploadError, String);

impl IntoResponse for UploadRouteError {
    fn into_response(self) -> Response {
        self.0.into_response_with_request_id(&self.1)
    }
}
