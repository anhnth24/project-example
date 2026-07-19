//! Document citation, preview, and original-download routes.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use serde::Deserialize;
use uuid::Uuid;

use crate::api::ApiError;
use crate::auth::middleware::AuthenticatedOrg;
use crate::auth::permissions::require_permission;
use crate::http::AppState;
use crate::services::citation::{self, CitationPin};
use crate::services::download::{self, DownloadError};
use crate::services::preview::{self, PreviewError, MARKDOWN_CONTENT_TYPE};
use crate::storage::StorageError;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/v1/documents/{documentId}/versions/{versionId}/citations:resolve",
            post(resolve_citation),
        )
        .route(
            "/api/v1/documents/{documentId}/versions/{versionId}/preview",
            get(markdown_preview),
        )
        .route(
            "/api/v1/documents/{documentId}/versions/{versionId}/download",
            post(authorize_download),
        )
        .route("/api/v1/documents/download/{token}", get(redeem_download))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VersionPath {
    document_id: Uuid,
    version_id: Uuid,
}

#[derive(Debug, Deserialize)]
struct TokenPath {
    token: String,
}

async fn resolve_citation(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Path(path): Path<VersionPath>,
    Json(pin): Json<CitationPin>,
) -> Result<Response, DocumentRouteError> {
    let request_id = auth.request_id.clone();
    require_permission(&auth.context, "qa.query")
        .map_err(|_| DocumentRouteError::forbidden(request_id.clone()))?;
    if path.document_id != pin.document_id || path.version_id != pin.version_id {
        return Err(DocumentRouteError::validation(
            "Citation pin does not match the requested document version",
            request_id,
        ));
    }
    let resolved = citation::resolve_citation(state.pool(), &auth.context, pin)
        .await
        .map_err(|error| DocumentRouteError::citation(error, request_id.clone()))?;
    Ok(Json(resolved).into_response())
}

async fn markdown_preview(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Path(path): Path<VersionPath>,
) -> Result<Response, DocumentRouteError> {
    let request_id = auth.request_id.clone();
    require_permission(&auth.context, "qa.query")
        .map_err(|_| DocumentRouteError::forbidden(request_id.clone()))?;
    let storage = state
        .object_store()
        .ok_or_else(|| DocumentRouteError::service_unavailable(request_id.clone()))?;
    let preview = preview::fetch_markdown_preview(
        state.pool(),
        storage,
        &auth.context,
        path.document_id,
        path.version_id,
    )
    .await
    .map_err(|error| DocumentRouteError::preview(error, request_id.clone()))?;
    let mut headers = HeaderMap::new();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static(MARKDOWN_CONTENT_TYPE),
    );
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    Ok((headers, Body::from(preview.bytes)).into_response())
}

async fn authorize_download(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Path(path): Path<VersionPath>,
) -> Result<Response, DocumentRouteError> {
    let request_id = auth.request_id.clone();
    require_permission(&auth.context, "qa.query")
        .map_err(|_| DocumentRouteError::forbidden(request_id.clone()))?;
    let storage = state
        .object_store()
        .ok_or_else(|| DocumentRouteError::service_unavailable(request_id.clone()))?;
    let key = state
        .download_capability_key()
        .ok_or_else(|| DocumentRouteError::service_unavailable(request_id.clone()))?;
    let capability = download::authorize_download(
        state.pool(),
        storage,
        &auth.context,
        path.document_id,
        path.version_id,
        key,
        Utc::now(),
    )
    .await
    .map_err(|error| DocumentRouteError::download(error, request_id.clone()))?;
    Ok(Json(capability).into_response())
}

async fn redeem_download(
    State(state): State<Arc<AppState>>,
    Path(path): Path<TokenPath>,
) -> Result<Response, DocumentRouteError> {
    let request_id = Uuid::new_v4().to_string();
    let storage = state
        .object_store()
        .ok_or_else(|| DocumentRouteError::service_unavailable(request_id.clone()))?;
    let key = state
        .download_capability_key()
        .ok_or_else(|| DocumentRouteError::service_unavailable(request_id.clone()))?;
    let stream = download::redeem_download(
        state.pool(),
        storage,
        key,
        state.consumed_download_nonces(),
        &path.token,
        Utc::now(),
    )
    .await
    .map_err(|error| DocumentRouteError::download(error, request_id.clone()))?;
    let disposition = download::content_disposition_value(&stream.filename);
    let mut headers = HeaderMap::new();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_str(&stream.content_type)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    headers.insert(
        CONTENT_DISPOSITION,
        HeaderValue::from_str(&disposition)
            .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
    );
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    Ok((headers, Body::from(stream.bytes)).into_response())
}

struct DocumentRouteError {
    status: StatusCode,
    code: &'static str,
    message: &'static str,
    request_id: String,
}

impl DocumentRouteError {
    fn validation(message: &'static str, request_id: String) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "validation_failed",
            message,
            request_id,
        }
    }

    fn forbidden(request_id: String) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code: "permission_denied",
            message: "Permission denied",
            request_id,
        }
    }

    fn not_found(request_id: String) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "not_found",
            message: "Document source was not found",
            request_id,
        }
    }

    fn internal(request_id: String) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal_error",
            message: "Document operation failed",
            request_id,
        }
    }

    fn service_unavailable(request_id: String) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "dependency_unavailable",
            message: "A required document service is unavailable",
            request_id,
        }
    }

    fn citation(error: citation::CitationError, request_id: String) -> Self {
        match error {
            citation::CitationError::NotFound => Self::not_found(request_id),
            citation::CitationError::Db(_) => Self::internal(request_id),
        }
    }

    fn preview(error: PreviewError, request_id: String) -> Self {
        match error {
            PreviewError::NotFound
            | PreviewError::Storage(
                StorageError::NotFound
                | StorageError::InvalidKey
                | StorageError::KeyOrgMismatch
                | StorageError::OwnershipConflict
                | StorageError::MissingScope,
            ) => Self::not_found(request_id),
            PreviewError::Db(_) | PreviewError::Storage(_) | PreviewError::Integrity => {
                Self::internal(request_id)
            }
        }
    }

    fn download(error: DownloadError, request_id: String) -> Self {
        match error {
            DownloadError::NotFound
            | DownloadError::InvalidToken
            | DownloadError::Expired
            | DownloadError::Replay
            | DownloadError::Storage(
                StorageError::NotFound
                | StorageError::InvalidKey
                | StorageError::KeyOrgMismatch
                | StorageError::OwnershipConflict
                | StorageError::MissingScope,
            ) => Self::not_found(request_id),
            DownloadError::CapabilityUnavailable => Self::service_unavailable(request_id),
            DownloadError::Db(_) | DownloadError::Storage(_) | DownloadError::Integrity => {
                Self::internal(request_id)
            }
        }
    }
}

impl IntoResponse for DocumentRouteError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ApiError {
                code: self.code.into(),
                message: self.message.into(),
                request_id: self.request_id,
                details: None,
            }),
        )
            .into_response()
    }
}
