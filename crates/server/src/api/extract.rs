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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::extract::DefaultBodyLimit;
    use axum::http::{Request, StatusCode};
    use axum::routing::{get, post};
    use axum::{Extension, Json, Router};
    use http_body_util::BodyExt;
    use serde::Deserialize;
    use serde_json::{json, Value};
    use tower::ServiceExt;
    use uuid::Uuid;

    use super::{AppJson, AppMultipart, AppPath, AppQuery};
    use crate::auth::context::OrgContext;
    use crate::auth::jwt::AccessClaims;
    use crate::auth::middleware::AuthenticatedOrg;
    use crate::config::{RuntimeEndpoints, SecretString, ServerConfig};
    use crate::db::pool::create_pool;
    use crate::http::AppState;
    use crate::state::RuntimeState;

    #[derive(Deserialize)]
    struct QueryInput {
        limit: u32,
    }

    #[derive(Deserialize)]
    struct JsonInput {
        name: String,
    }

    async fn path_handler(AppPath(_id): AppPath<Uuid>) -> StatusCode {
        StatusCode::OK
    }

    async fn query_handler(AppQuery(query): AppQuery<QueryInput>) -> Json<Value> {
        Json(json!({ "limit": query.limit }))
    }

    async fn json_handler(AppJson(input): AppJson<JsonInput>) -> Json<Value> {
        Json(json!({ "name": input.name }))
    }

    async fn multipart_handler(AppMultipart(_multipart): AppMultipart) -> StatusCode {
        StatusCode::OK
    }

    fn test_state() -> Arc<AppState> {
        let runtime =
            RuntimeState::from_config(ServerConfig::test_with_endpoints(RuntimeEndpoints {
                database_url: SecretString::new("postgres://unused"),
                qdrant_url: "http://127.0.0.1:1".into(),
                minio_url: "http://127.0.0.1:1".into(),
            }))
            .unwrap();
        let pool = create_pool("postgres://markhand_app:markhand_app@127.0.0.1:5432/markhand_test")
            .expect("pool");
        Arc::new(AppState::from_parts(runtime, pool, None).unwrap())
    }

    fn extractor_auth() -> AuthenticatedOrg {
        let org = Uuid::new_v4();
        let user = Uuid::new_v4();
        AuthenticatedOrg {
            context: OrgContext::try_new(org, user, [] as [&str; 0], []).unwrap(),
            claims: AccessClaims {
                sub: user.to_string(),
                iss: "test".into(),
                aud: "test".into(),
                iat: 1,
                nbf: 1,
                exp: i64::MAX,
                org_id: org.to_string(),
                sid: Uuid::new_v4().to_string(),
            },
            request_id: "extractor-request".into(),
        }
    }

    fn test_app() -> Router {
        Router::new()
            .route("/path/{id}", get(path_handler))
            .route("/query", get(query_handler))
            .route("/json", post(json_handler).layer(DefaultBodyLimit::max(32)))
            .route("/multipart", post(multipart_handler))
            .layer(Extension(extractor_auth()))
            .with_state(test_state())
    }

    async fn json_body(response: axum::response::Response) -> Value {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    async fn assert_error(request: Request<Body>, status: StatusCode, code: &str) {
        let response = test_app().oneshot(request).await.unwrap();
        assert_eq!(response.status(), status);
        let body = json_body(response).await;
        assert_eq!(body["code"], code, "{body}");
        assert_eq!(body["requestId"], "extractor-request", "{body}");
    }

    #[tokio::test]
    async fn path_query_and_json_rejections_are_canonical() {
        assert_error(
            Request::builder()
                .uri("/path/not-a-uuid")
                .body(Body::empty())
                .unwrap(),
            StatusCode::BAD_REQUEST,
            "validation_failed",
        )
        .await;

        assert_error(
            Request::builder()
                .uri("/query?limit=not-a-number")
                .body(Body::empty())
                .unwrap(),
            StatusCode::BAD_REQUEST,
            "validation_failed",
        )
        .await;

        assert_error(
            Request::builder()
                .method("POST")
                .uri("/json")
                .header("content-type", "application/json")
                .body(Body::from("{not-json"))
                .unwrap(),
            StatusCode::BAD_REQUEST,
            "validation_failed",
        )
        .await;
    }

    #[tokio::test]
    async fn oversized_json_and_invalid_multipart_have_stable_errors() {
        assert_error(
            Request::builder()
                .method("POST")
                .uri("/json")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({ "name": "x".repeat(128) })).unwrap(),
                ))
                .unwrap(),
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload_too_large",
        )
        .await;

        assert_error(
            Request::builder()
                .method("POST")
                .uri("/multipart")
                .header("content-type", "multipart/form-data")
                .body(Body::empty())
                .unwrap(),
            StatusCode::BAD_REQUEST,
            "validation_failed",
        )
        .await;
    }
}
