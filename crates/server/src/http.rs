//! HTTP liveness and readiness routes backed by real POC dependencies.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use tokio::time::timeout;
use uuid::Uuid;

use crate::api::ApiError;
use crate::config::RuntimeEndpoints;
use crate::database;

const DEPENDENCY_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone)]
pub struct AppState {
    endpoints: RuntimeEndpoints,
    http_client: reqwest::Client,
}

impl AppState {
    pub fn new(endpoints: RuntimeEndpoints) -> Result<Self, String> {
        let http_client = reqwest::Client::builder()
            .timeout(DEPENDENCY_TIMEOUT)
            .build()
            .map_err(|error| format!("cannot configure HTTP client: {error}"))?;
        Ok(Self {
            endpoints,
            http_client,
        })
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Health {
    status: &'static str,
    request_id: String,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/health/live", get(liveness))
        .route("/api/v1/health/ready", get(readiness))
        .with_state(Arc::new(state))
}

async fn liveness() -> Json<Health> {
    Json(healthy())
}

async fn readiness(State(state): State<Arc<AppState>>) -> Result<Json<Health>, ReadinessError> {
    check_dependencies(state).await.map_err(ReadinessError)?;
    Ok(Json(healthy()))
}

async fn check_dependencies(state: Arc<AppState>) -> Result<(), String> {
    let database = timeout(
        DEPENDENCY_TIMEOUT,
        database::check_connection(state.endpoints.database_url.expose()),
    );
    let qdrant = state
        .http_client
        .get(format!("{}/healthz", state.endpoints.qdrant_url))
        .send();
    let minio = state
        .http_client
        .get(format!("{}/minio/health/live", state.endpoints.minio_url))
        .send();

    let (database, qdrant, minio) = tokio::join!(database, qdrant, minio);
    database.map_err(|_| "PostgreSQL readiness timed out".to_string())??;
    ensure_success(qdrant, "Qdrant").await?;
    ensure_success(minio, "MinIO").await
}

async fn ensure_success(
    response: Result<reqwest::Response, reqwest::Error>,
    dependency: &str,
) -> Result<(), String> {
    let response = response.map_err(|_| format!("{dependency} readiness request failed"))?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!("{dependency} returned {}", response.status()))
    }
}

fn healthy() -> Health {
    Health {
        status: "ok",
        request_id: Uuid::new_v4().to_string(),
    }
}

struct ReadinessError(String);

impl IntoResponse for ReadinessError {
    fn into_response(self) -> Response {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiError {
                code: "dependency_unavailable".into(),
                message: "A required service is unavailable".into(),
                request_id: Uuid::new_v4().to_string(),
                details: None,
            }),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::{router, AppState};
    use crate::config::{RuntimeEndpoints, SecretString};

    #[tokio::test]
    async fn liveness_has_a_contract_compliant_body() {
        let app = router(
            AppState::new(RuntimeEndpoints {
                database_url: SecretString::new("postgres://unused"),
                qdrant_url: "http://127.0.0.1:1".into(),
                minio_url: "http://127.0.0.1:1".into(),
            })
            .unwrap(),
        );
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/health/live")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let health: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(health["status"], "ok");
        assert!(health["requestId"].as_str().is_some());
    }
}
