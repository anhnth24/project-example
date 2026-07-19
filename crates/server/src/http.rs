//! HTTP liveness, readiness, and API routes backed by real POC dependencies.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use deadpool_postgres::Pool;
use serde::Serialize;
use tokio::time::timeout;
use uuid::Uuid;

use crate::api::ApiError;
use crate::auth::jwt::JwtKeys;
use crate::auth::provider::PasswordAuthProvider;
use crate::database;
use crate::db::pool::create_pool;
use crate::routes;
use crate::state::RuntimeState;

const DEPENDENCY_TIMEOUT: Duration = Duration::from_secs(3);
const READINESS_CACHE_TTL: Duration = Duration::from_secs(1);

pub struct AppState {
    runtime: RuntimeState,
    http_client: reqwest::Client,
    readiness: tokio::sync::Mutex<Option<CachedReadiness>>,
    pool: Pool,
    auth_provider: Option<PasswordAuthProvider>,
    /// Object store adapter (optional when credentials are absent in tests).
    object_store: Option<crate::storage::MinioClient>,
}

struct CachedReadiness {
    checked_at: tokio::time::Instant,
    result: Result<(), ()>,
}

impl AppState {
    pub fn new(runtime: RuntimeState) -> Result<Self, String> {
        if !runtime.is_api_role() {
            return Err("HTTP application requires API runtime configuration".into());
        }
        let http_client = reqwest::Client::builder()
            .timeout(DEPENDENCY_TIMEOUT)
            .build()
            .map_err(|error| format!("cannot configure HTTP client: {error}"))?;
        let pool = create_pool(runtime.endpoints().database_url.expose())
            .map_err(|error| format!("cannot create database pool: {error}"))?;
        let auth_provider = match JwtKeys::from_auth(runtime.config().auth()) {
            Ok(keys) => Some(PasswordAuthProvider::new(
                pool.clone(),
                runtime.config().auth().clone(),
                keys,
            )),
            Err(crate::auth::jwt::JwtError::NotConfigured) => None,
            Err(error) => return Err(format!("cannot configure authentication: {error}")),
        };
        let object_store = match runtime.config().storage_config() {
            Ok(storage) => Some(
                crate::storage::MinioClient::from_config(storage.minio())
                    .map_err(|error| format!("cannot configure object store: {error}"))?,
            ),
            Err(_) => None,
        };
        Ok(Self {
            runtime,
            http_client,
            readiness: tokio::sync::Mutex::new(None),
            pool,
            auth_provider,
            object_store,
        })
    }

    /// Builds state for tests with an explicit pool and optional auth provider.
    pub fn from_parts(
        runtime: RuntimeState,
        pool: Pool,
        auth_provider: Option<PasswordAuthProvider>,
    ) -> Result<Self, String> {
        Self::from_parts_with_store(runtime, pool, auth_provider, None)
    }

    /// Builds state for tests that exercise object-store-backed routes.
    pub fn from_parts_with_store(
        runtime: RuntimeState,
        pool: Pool,
        auth_provider: Option<PasswordAuthProvider>,
        object_store: Option<crate::storage::MinioClient>,
    ) -> Result<Self, String> {
        if !runtime.is_api_role() {
            return Err("HTTP application requires API runtime configuration".into());
        }
        let http_client = reqwest::Client::builder()
            .timeout(DEPENDENCY_TIMEOUT)
            .build()
            .map_err(|error| format!("cannot configure HTTP client: {error}"))?;
        Ok(Self {
            runtime,
            http_client,
            readiness: tokio::sync::Mutex::new(None),
            pool,
            auth_provider,
            object_store,
        })
    }

    pub fn auth_provider(&self) -> Option<&PasswordAuthProvider> {
        self.auth_provider.as_ref()
    }

    pub fn pool(&self) -> &Pool {
        &self.pool
    }

    pub fn runtime(&self) -> &RuntimeState {
        &self.runtime
    }

    pub fn object_store(&self) -> Option<&crate::storage::MinioClient> {
        self.object_store.as_ref()
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
        .merge(routes::auth::router())
        .merge(routes::uploads::router())
        .with_state(Arc::new(state))
}

async fn liveness() -> Json<Health> {
    Json(healthy())
}

async fn readiness(State(state): State<Arc<AppState>>) -> Result<Json<Health>, ReadinessError> {
    check_dependencies(state).await?;
    Ok(Json(healthy()))
}

async fn check_dependencies(state: Arc<AppState>) -> Result<(), ReadinessError> {
    let mut cached = state.readiness.lock().await;
    if let Some(previous) = cached.as_ref() {
        if previous.checked_at.elapsed() < READINESS_CACHE_TTL {
            return previous
                .result
                .as_ref()
                .map(|_| ())
                .map_err(|_| ReadinessError);
        }
    }

    let result = check_dependencies_uncached(&state).await;
    *cached = Some(CachedReadiness {
        checked_at: tokio::time::Instant::now(),
        result: result.as_ref().map(|_| ()).map_err(|_| ()),
    });
    result.map_err(|_| ReadinessError)
}

async fn check_dependencies_uncached(state: &AppState) -> Result<(), String> {
    let database = timeout(
        DEPENDENCY_TIMEOUT,
        database::check_connection(state.runtime.endpoints().database_url.expose()),
    );
    let qdrant = state
        .http_client
        .get(format!("{}/healthz", state.runtime.endpoints().qdrant_url))
        .send();
    let minio = state
        .http_client
        .get(format!(
            "{}/minio/health/live",
            state.runtime.endpoints().minio_url
        ))
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

struct ReadinessError;

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
    use crate::config::{RuntimeEndpoints, SecretString, ServerConfig};
    use crate::db::pool::create_pool;
    use crate::state::RuntimeState;

    #[tokio::test]
    async fn liveness_has_a_contract_compliant_body() {
        let runtime =
            RuntimeState::from_config(ServerConfig::test_with_endpoints(RuntimeEndpoints {
                database_url: SecretString::new("postgres://unused"),
                qdrant_url: "http://127.0.0.1:1".into(),
                minio_url: "http://127.0.0.1:1".into(),
            }))
            .unwrap();
        // Pool construction is lazy; a dummy URL is enough for the liveness route.
        let pool = create_pool("postgres://markhand_app:markhand_app@127.0.0.1:5432/markhand_test")
            .expect("pool");
        let app = router(AppState::from_parts(runtime, pool, None).unwrap());
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

    #[test]
    fn application_rejects_worker_runtime_state() {
        let state =
            RuntimeState::from_config(ServerConfig::test_worker_with_endpoints(RuntimeEndpoints {
                database_url: SecretString::new("postgres://unused"),
                qdrant_url: "http://127.0.0.1:1".into(),
                minio_url: "http://127.0.0.1:1".into(),
            }))
            .unwrap();
        let pool = create_pool("postgres://markhand_app:markhand_app@127.0.0.1:5432/markhand_test")
            .expect("pool");
        assert_eq!(
            AppState::from_parts(state, pool, None).err().as_deref(),
            Some("HTTP application requires API runtime configuration")
        );
    }
}
