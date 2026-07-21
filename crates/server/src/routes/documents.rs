//! Minimal document citation / preview / download routes (P1B-R02).
//!
//! Full collection/document CRUD belongs to P1B-R04. These routes only expose
//! the R02 service contracts with fresh auth on every call.

use std::fmt;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::ApiError;
use crate::auth::middleware::AuthenticatedOrg;
use crate::db::download_capabilities::DownloadPurpose;
use crate::http::AppState;
use crate::services::citation::{self, CitationError, CitationResolveRequest, StableCitation};
use crate::services::download::{
    self, CapabilitySigner, DownloadError, DEFAULT_CAPABILITY_TTL, MAX_CAPABILITY_TTL,
};
use crate::services::preview::{self, PreviewError};

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/citations/resolve", post(resolve_citations))
        .route(
            "/api/v1/documents/{document_id}/versions/{version_id}/preview",
            get(preview_markdown),
        )
        .route(
            "/api/v1/documents/{document_id}/versions/{version_id}/download-capabilities",
            post(mint_download_capability),
        )
        .route(
            "/api/v1/download-capabilities/redeem",
            post(redeem_download_capability),
        )
}

/// Wire token field: serializes normally, Debug never prints the raw secret.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
struct RedactedToken(String);

impl fmt::Debug for RedactedToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResolveCitationsBody {
    citations: Vec<ResolveCitationItem>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResolveCitationItem {
    chunk_id: Uuid,
    #[serde(default)]
    expected_version_id: Option<Uuid>,
    #[serde(default)]
    expected_document_id: Option<Uuid>,
    #[serde(default)]
    expected_content_sha256: Option<String>,
    #[serde(default)]
    expected_quote: Option<String>,
    #[serde(default)]
    expected_span_start: Option<usize>,
    #[serde(default)]
    expected_span_end: Option<usize>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StableCitationResponse {
    org_id: Uuid,
    logical_document_id: Uuid,
    version_id: Uuid,
    version_number: i32,
    content_sha256: String,
    chunk_id: Uuid,
    chunk_identity_sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    page: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    slide: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sheet: Option<String>,
    span_start: usize,
    span_end: usize,
    quote: String,
    effective_from: chrono::DateTime<chrono::Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    effective_to: Option<chrono::DateTime<chrono::Utc>>,
    is_current: bool,
    heading: String,
}

impl From<StableCitation> for StableCitationResponse {
    fn from(value: StableCitation) -> Self {
        Self {
            org_id: value.org_id,
            logical_document_id: value.logical_document_id,
            version_id: value.version_id,
            version_number: value.version_number,
            content_sha256: value.content_sha256,
            chunk_id: value.chunk_id,
            chunk_identity_sha256: value.chunk_identity_sha256,
            page: value.page,
            slide: value.slide,
            sheet: value.sheet,
            span_start: value.span_start,
            span_end: value.span_end,
            quote: value.quote,
            effective_from: value.effective_from,
            effective_to: value.effective_to,
            is_current: value.is_current,
            heading: value.heading,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ResolveCitationsResponse {
    citations: Vec<StableCitationResponse>,
    request_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PreviewResponse {
    document_id: Uuid,
    version_id: Uuid,
    version_number: i32,
    content_sha256: String,
    markdown_sha256: String,
    is_current: bool,
    markdown: String,
    request_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MintDownloadBody {
    purpose: String,
    #[serde(default)]
    ttl_secs: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MintDownloadResponse {
    capability_id: Uuid,
    token: RedactedToken,
    purpose: String,
    document_id: Uuid,
    version_id: Uuid,
    expires_at: chrono::DateTime<chrono::Utc>,
    request_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RedeemDownloadBody {
    token: RedactedToken,
}

async fn resolve_citations(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Json(body): Json<ResolveCitationsBody>,
) -> Result<Json<ResolveCitationsResponse>, DocumentRouteError> {
    let request_id = auth.request_id.clone();
    if body.citations.is_empty() || body.citations.len() > 40 {
        return Err(DocumentRouteError::validation(
            "citations must contain 1..=40 items",
            request_id,
        ));
    }
    let requests: Vec<CitationResolveRequest> = body
        .citations
        .into_iter()
        .map(|item| CitationResolveRequest {
            chunk_id: item.chunk_id,
            expected_version_id: item.expected_version_id,
            expected_document_id: item.expected_document_id,
            expected_content_sha256: item.expected_content_sha256,
            expected_quote: item.expected_quote,
            expected_span_start: item.expected_span_start,
            expected_span_end: item.expected_span_end,
        })
        .collect();
    let storage = state
        .object_store()
        .ok_or_else(|| DocumentRouteError::Citation(CitationError::Storage, request_id.clone()))?;
    let citations = citation::resolve_citations(
        state.pool(),
        storage,
        auth.context.org_id(),
        auth.context.user_id(),
        &requests,
    )
    .await
    .map_err(|error| DocumentRouteError::Citation(error, request_id.clone()))?;
    Ok(Json(ResolveCitationsResponse {
        citations: citations.into_iter().map(Into::into).collect(),
        request_id,
    }))
}

async fn preview_markdown(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Path((document_id, version_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<PreviewResponse>, DocumentRouteError> {
    let request_id = auth.request_id.clone();
    let storage = state.object_store().ok_or_else(|| {
        DocumentRouteError::Preview(PreviewError::StorageUnavailable, request_id.clone())
    })?;
    let preview = preview::fetch_trusted_markdown(
        state.pool(),
        storage,
        auth.context.org_id(),
        auth.context.user_id(),
        document_id,
        version_id,
    )
    .await
    .map_err(|error| DocumentRouteError::Preview(error, request_id.clone()))?;
    Ok(Json(PreviewResponse {
        document_id: preview.document_id,
        version_id: preview.version_id,
        version_number: preview.version_number,
        content_sha256: preview.content_sha256,
        markdown_sha256: preview.markdown_sha256,
        is_current: preview.is_current,
        markdown: preview.markdown,
        request_id,
    }))
}

async fn mint_download_capability(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Path((document_id, version_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<MintDownloadBody>,
) -> Result<Json<MintDownloadResponse>, DocumentRouteError> {
    let request_id = auth.request_id.clone();
    let purpose = DownloadPurpose::parse(&body.purpose).map_err(|_| {
        DocumentRouteError::Download(DownloadError::InvalidPurpose, request_id.clone())
    })?;
    let ttl = body
        .ttl_secs
        .map(std::time::Duration::from_secs)
        .unwrap_or(DEFAULT_CAPABILITY_TTL);
    if ttl > MAX_CAPABILITY_TTL {
        return Err(DocumentRouteError::Download(
            DownloadError::InvalidTtl,
            request_id,
        ));
    }
    let signer = capability_signer(&state)
        .map_err(|error| DocumentRouteError::Download(error, request_id.clone()))?;
    let minted = download::mint_download_capability(
        state.pool(),
        &signer,
        auth.context.org_id(),
        auth.context.user_id(),
        document_id,
        version_id,
        purpose,
        ttl,
    )
    .await
    .map_err(|error| DocumentRouteError::Download(error, request_id.clone()))?;
    Ok(Json(MintDownloadResponse {
        capability_id: minted.capability_id,
        token: RedactedToken(minted.token.expose().to_string()),
        purpose: minted.purpose.as_str().into(),
        document_id: minted.document_id,
        version_id: minted.version_id,
        expires_at: minted.expires_at,
        request_id,
    }))
}

async fn redeem_download_capability(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Json(body): Json<RedeemDownloadBody>,
) -> Result<Response, DocumentRouteError> {
    let request_id = auth.request_id.clone();
    if body.token.0.is_empty() || body.token.0.len() > 512 {
        return Err(DocumentRouteError::Download(
            DownloadError::InvalidToken,
            request_id,
        ));
    }
    let storage = state.object_store().ok_or_else(|| {
        DocumentRouteError::Download(DownloadError::StorageUnavailable, request_id.clone())
    })?;
    let signer = capability_signer(&state)
        .map_err(|error| DocumentRouteError::Download(error, request_id.clone()))?;
    let artifact = download::redeem_download_capability(
        state.pool(),
        storage,
        &signer,
        state.download_budget(),
        auth.context.org_id(),
        auth.context.user_id(),
        &body.token.0,
    )
    .await
    .map_err(|error| DocumentRouteError::Download(error, request_id.clone()))?;

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&artifact.content_type)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    headers.insert(
        header::HeaderName::from_static("x-content-sha256"),
        HeaderValue::from_str(&artifact.content_sha256)
            .unwrap_or_else(|_| HeaderValue::from_static("")),
    );
    headers.insert(
        header::HeaderName::from_static("x-request-id"),
        HeaderValue::from_str(&request_id).unwrap_or_else(|_| HeaderValue::from_static("")),
    );
    // Defense: never surface object keys; Content-Disposition uses safe filename only.
    if let Some(filename) = artifact.filename.as_deref() {
        let safe = filename
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                    ch
                } else {
                    '_'
                }
            })
            .collect::<String>();
        if let Ok(value) = HeaderValue::from_str(&format!("attachment; filename=\"{safe}\"")) {
            headers.insert(header::CONTENT_DISPOSITION, value);
        }
    }
    // Body owns the download budget permit until Hyper finishes / cancels / drops it.
    let mut response = Response::new(Body::new(artifact.body));
    *response.status_mut() = StatusCode::OK;
    *response.headers_mut() = headers;
    Ok(response)
}

fn capability_signer(state: &AppState) -> Result<CapabilitySigner, DownloadError> {
    CapabilitySigner::from_auth_signing_key(state.runtime().config().auth().signing_key.as_ref())
}

enum DocumentRouteError {
    Citation(CitationError, String),
    Preview(PreviewError, String),
    Download(DownloadError, String),
    Validation(String, String),
}

impl DocumentRouteError {
    fn validation(message: impl Into<String>, request_id: String) -> Self {
        Self::Validation(message.into(), request_id)
    }
}

fn download_status(error: &DownloadError) -> StatusCode {
    match error {
        DownloadError::PermissionDenied => StatusCode::FORBIDDEN,
        DownloadError::NotFound => StatusCode::NOT_FOUND,
        DownloadError::Expired | DownloadError::Replay | DownloadError::InvalidToken => {
            StatusCode::GONE
        }
        DownloadError::SignerNotConfigured
        | DownloadError::StorageUnavailable
        | DownloadError::Busy => StatusCode::SERVICE_UNAVAILABLE,
        DownloadError::InvalidPurpose | DownloadError::InvalidTtl => StatusCode::BAD_REQUEST,
        DownloadError::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        DownloadError::Integrity => StatusCode::CONFLICT,
        DownloadError::Storage | DownloadError::Database => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

impl IntoResponse for DocumentRouteError {
    fn into_response(self) -> Response {
        let (status, code, message, request_id) = match self {
            Self::Citation(error, request_id) => {
                let status = match error {
                    CitationError::PermissionDenied => StatusCode::FORBIDDEN,
                    CitationError::NotFound | CitationError::MarkdownMissing => {
                        StatusCode::NOT_FOUND
                    }
                    CitationError::InvalidRequest => StatusCode::BAD_REQUEST,
                    CitationError::QuoteMismatch
                    | CitationError::VersionMismatch
                    | CitationError::InvalidAnchor
                    | CitationError::Integrity => StatusCode::CONFLICT,
                    CitationError::Storage | CitationError::Database => {
                        StatusCode::INTERNAL_SERVER_ERROR
                    }
                };
                (status, error.code(), error.to_string(), request_id)
            }
            Self::Preview(error, request_id) => {
                let status = match error {
                    PreviewError::PermissionDenied => StatusCode::FORBIDDEN,
                    PreviewError::NotFound | PreviewError::MarkdownMissing => StatusCode::NOT_FOUND,
                    PreviewError::StorageUnavailable => StatusCode::SERVICE_UNAVAILABLE,
                    PreviewError::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
                    PreviewError::Integrity | PreviewError::InvalidUtf8 => StatusCode::CONFLICT,
                    PreviewError::Storage | PreviewError::Database => {
                        StatusCode::INTERNAL_SERVER_ERROR
                    }
                };
                (status, error.code(), error.to_string(), request_id)
            }
            Self::Download(error, request_id) => (
                download_status(&error),
                error.code(),
                error.to_string(),
                request_id,
            ),
            Self::Validation(message, request_id) => (
                StatusCode::BAD_REQUEST,
                "validation_failed",
                message,
                request_id,
            ),
        };
        (
            status,
            Json(ApiError {
                code: code.into(),
                message,
                request_id,
                details: None,
            }),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;

    #[test]
    fn redeem_body_debug_redacts_token() {
        let body = RedeemDownloadBody {
            token: RedactedToken("mhdl1.aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa.deadbeef".into()),
        };
        let debug = format!("{body:?}");
        assert!(debug.contains("REDACTED"));
        assert!(!debug.contains("deadbeef"));
        assert!(!debug.contains("mhdl1"));
    }

    #[test]
    fn mint_response_debug_redacts_token() {
        let response = MintDownloadResponse {
            capability_id: Uuid::nil(),
            token: RedactedToken("mhdl1.secret-token-value".into()),
            purpose: "markdown".into(),
            document_id: Uuid::nil(),
            version_id: Uuid::nil(),
            expires_at: chrono::Utc::now(),
            request_id: "r".into(),
        };
        let debug = format!("{response:?}");
        assert!(debug.contains("REDACTED"));
        assert!(!debug.contains("secret-token-value"));
    }

    #[tokio::test]
    async fn download_error_mapping_distinguishes_expired_replay_busy() {
        for (error, status, code) in [
            (DownloadError::Expired, StatusCode::GONE, "download_expired"),
            (DownloadError::Replay, StatusCode::GONE, "download_replay"),
            (
                DownloadError::Busy,
                StatusCode::SERVICE_UNAVAILABLE,
                "download_busy",
            ),
            (
                DownloadError::Integrity,
                StatusCode::CONFLICT,
                "download_integrity",
            ),
            (
                DownloadError::TooLarge,
                StatusCode::PAYLOAD_TOO_LARGE,
                "download_too_large",
            ),
        ] {
            let response = DocumentRouteError::Download(error, "req-1".into()).into_response();
            assert_eq!(response.status(), status);
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(json["code"], code);
            assert_eq!(json["requestId"], "req-1");
        }
    }
}
