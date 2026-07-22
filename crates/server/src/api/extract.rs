//! Canonical extractors that map Path/Query/Json/Multipart rejections to [`ApiError`].

use std::sync::Arc;

use axum::extract::multipart::MultipartRejection;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{FromRequest, FromRequestParts, Multipart, Request};
use axum::http::request::Parts;
use axum::http::StatusCode;
use serde::de::DeserializeOwned;
use uuid::Uuid;

use super::error::{ApiError, ApiRejection};
use crate::http::AppState;

fn rejection_request_id(parts: &Parts) -> String {
    parts
        .extensions
        .get::<crate::auth::middleware::AuthenticatedOrg>()
        .map(|auth| auth.request_id.clone())
        .unwrap_or_else(|| Uuid::new_v4().to_string())
}

fn map_status_message(status: StatusCode, fallback: &str) -> (StatusCode, &'static str, String) {
    if status == StatusCode::PAYLOAD_TOO_LARGE {
        (
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload_too_large",
            "Request body exceeds the configured limit".into(),
        )
    } else {
        (
            StatusCode::BAD_REQUEST,
            "validation_failed",
            fallback.into(),
        )
    }
}

/// Path params with canonical API error envelope on malformed IDs.
pub struct AppPath<T>(pub T);

impl<T> FromRequestParts<Arc<AppState>> for AppPath<T>
where
    T: DeserializeOwned + Send,
{
    type Rejection = ApiRejection;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let request_id = rejection_request_id(parts);
        match axum::extract::Path::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Path(value)) => Ok(Self(value)),
            Err(PathRejection::FailedToDeserializePathParams(_)) => Err(ApiRejection::validation(
                "Path parameters are malformed",
                request_id,
            )),
            Err(PathRejection::MissingPathParams(_)) => Err(ApiRejection::validation(
                "Path parameters are missing",
                request_id,
            )),
            Err(other) => {
                let (status, code, message) =
                    map_status_message(other.status(), "Path parameters are invalid");
                Err(ApiRejection::new(status, code, message, request_id))
            }
        }
    }
}

/// Query params with canonical API error envelope on malformed query.
pub struct AppQuery<T>(pub T);

impl<T> FromRequestParts<Arc<AppState>> for AppQuery<T>
where
    T: DeserializeOwned,
{
    type Rejection = ApiRejection;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let request_id = rejection_request_id(parts);
        match axum::extract::Query::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Query(value)) => Ok(Self(value)),
            Err(QueryRejection::FailedToDeserializeQueryString(_)) => Err(
                ApiRejection::validation("Query parameters are malformed", request_id),
            ),
            Err(other) => {
                let (status, code, message) =
                    map_status_message(other.status(), "Query parameters are invalid");
                Err(ApiRejection::new(status, code, message, request_id))
            }
        }
    }
}

/// JSON body with canonical API error envelope on malformed/oversized bodies.
pub struct AppJson<T>(pub T);

impl<T> FromRequest<Arc<AppState>> for AppJson<T>
where
    T: DeserializeOwned,
{
    type Rejection = ApiRejection;

    async fn from_request(req: Request, state: &Arc<AppState>) -> Result<Self, Self::Rejection> {
        let request_id = req
            .extensions()
            .get::<crate::auth::middleware::AuthenticatedOrg>()
            .map(|auth| auth.request_id.clone())
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        match axum::Json::<T>::from_request(req, state).await {
            Ok(axum::Json(value)) => Ok(Self(value)),
            Err(JsonRejection::JsonDataError(_))
            | Err(JsonRejection::JsonSyntaxError(_))
            | Err(JsonRejection::MissingJsonContentType(_)) => Err(ApiRejection::validation(
                "Request body is malformed",
                request_id,
            )),
            Err(JsonRejection::BytesRejection(_)) => Err(ApiRejection::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "payload_too_large",
                "Request body exceeds the configured limit",
                request_id,
            )),
            Err(other) => {
                let (status, code, message) =
                    map_status_message(other.status(), "Request body is invalid");
                Err(ApiRejection::new(status, code, message, request_id))
            }
        }
    }
}

/// Multipart form with canonical API error envelope on boundary/stream failures.
pub struct AppMultipart(pub Multipart);

impl FromRequest<Arc<AppState>> for AppMultipart {
    type Rejection = ApiRejection;

    async fn from_request(req: Request, state: &Arc<AppState>) -> Result<Self, Self::Rejection> {
        let request_id = req
            .extensions()
            .get::<crate::auth::middleware::AuthenticatedOrg>()
            .map(|auth| auth.request_id.clone())
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        match Multipart::from_request(req, state).await {
            Ok(multipart) => Ok(Self(multipart)),
            Err(MultipartRejection::InvalidBoundary(_)) => Err(ApiRejection::validation(
                "Invalid multipart boundary",
                request_id,
            )),
            Err(other) => {
                let (status, code, message) =
                    map_status_message(other.status(), "Multipart request is invalid");
                Err(ApiRejection::new(status, code, message, request_id))
            }
        }
    }
}

/// Helper used by tests / readiness mapping for body-limit style errors.
pub fn body_limit_error(request_id: impl Into<String>) -> ApiError {
    ApiError::new(
        "payload_too_large",
        "Request body exceeds the configured limit",
        request_id,
    )
}

/// Map a multipart stream/read failure (including body-limit) to [`ApiRejection`].
pub fn map_multipart_stream_error(
    status: StatusCode,
    request_id: impl Into<String>,
) -> ApiRejection {
    let request_id = request_id.into();
    if status == StatusCode::PAYLOAD_TOO_LARGE {
        ApiRejection::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload_too_large",
            "Request body exceeds the configured limit",
            request_id,
        )
    } else {
        ApiRejection::validation("Multipart stream is invalid", request_id)
    }
}
