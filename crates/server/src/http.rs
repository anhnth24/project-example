//! HTTP application state, middleware stack, and route composition.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::http::HeaderValue;
use axum::middleware::from_fn_with_state;
use axum::response::IntoResponse;
use axum::Router;
use deadpool_postgres::Pool;

use crate::auth::jwt::JwtKeys;
use crate::auth::provider::PasswordAuthProvider;
use crate::config::QuotaSweepConfig;
use crate::db::pool::create_pool;
use crate::middleware::rate_limit::{RateLimitConfig, RateLimiter};
use crate::middleware::{baseline_ip_rate_limit, cors_middleware, inject_request_id};
use crate::routes;
use crate::services::download::CapabilityKeys;
use crate::services::embedding::ApprovedEmbeddingRuntime;
use crate::services::qa::provider::{ChatProvider, OpenAiCompatibleChat};
use crate::services::quota;
use crate::services::readiness::{self, ReadinessDeps};
use crate::state::RuntimeState;
use crate::storage::{MinioClient, QdrantClient};

const READINESS_CACHE_TTL: Duration = Duration::from_secs(1);

pub struct AppState {
    runtime: RuntimeState,
    http_client: reqwest::Client,
    readiness: tokio::sync::Mutex<Option<CachedReadiness>>,
    pool: Pool,
    auth_provider: Option<PasswordAuthProvider>,
    object_store: Option<MinioClient>,
    qdrant: Option<QdrantClient>,
    embedder: Option<ApprovedEmbeddingRuntime>,
    chat_provider: Option<ChatProvider>,
    capability_keys: Option<CapabilityKeys>,
    rate_limiter: RateLimiter,
    cors_origins: Vec<String>,
    trusted_proxies: Vec<IpAddr>,
}

struct CachedReadiness {
    checked_at: tokio::time::Instant,
    result: Result<(), &'static str>,
}

impl AppState {
    pub fn new(runtime: RuntimeState) -> Result<Self, String> {
        if !runtime.is_api_role() {
            return Err("HTTP application requires API runtime configuration".into());
        }
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(3))
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
                MinioClient::from_config(storage.minio())
                    .map_err(|error| format!("cannot configure object store: {error}"))?,
            ),
            Err(_) => None,
        };
        let qdrant = match runtime.config().storage_config() {
            Ok(storage) => Some(
                QdrantClient::with_api_key(storage.qdrant_url(), storage.qdrant_api_key().cloned())
                    .map_err(|error| format!("cannot configure qdrant: {}", error.code()))?,
            ),
            Err(_) => None,
        };
        let embedder = ApprovedEmbeddingRuntime::from_env(
            runtime.config().index_signature(),
            runtime.config().profile(),
        )
        .ok();
        let chat_provider = OpenAiCompatibleChat::from_env()
            .ok()
            .map(ChatProvider::OpenAi);
        let capability_keys = runtime
            .config()
            .auth()
            .signing_key
            .as_ref()
            .and_then(|key| CapabilityKeys::from_auth_signing_key(key).ok());
        let cors_origins = std::env::var("MARKHAND_CORS_ORIGINS")
            .ok()
            .map(|value| {
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let trusted_proxies = parse_trusted_proxies_env()?;
        let rate_config = RateLimitConfig::from_env()?;
        start_quota_sweep(pool.clone(), runtime.config().quota_sweep());
        Ok(Self {
            runtime,
            http_client,
            readiness: tokio::sync::Mutex::new(None),
            pool,
            auth_provider,
            object_store,
            qdrant,
            embedder,
            chat_provider,
            capability_keys,
            rate_limiter: RateLimiter::new(rate_config),
            cors_origins,
            trusted_proxies,
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
        object_store: Option<MinioClient>,
    ) -> Result<Self, String> {
        if !runtime.is_api_role() {
            return Err("HTTP application requires API runtime configuration".into());
        }
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(3))
            .build()
            .map_err(|error| format!("cannot configure HTTP client: {error}"))?;
        let capability_keys = runtime
            .config()
            .auth()
            .signing_key
            .as_ref()
            .and_then(|key| CapabilityKeys::from_auth_signing_key(key).ok());
        Ok(Self {
            runtime,
            http_client,
            readiness: tokio::sync::Mutex::new(None),
            pool,
            auth_provider,
            object_store,
            qdrant: None,
            embedder: None,
            chat_provider: None,
            capability_keys,
            rate_limiter: RateLimiter::new(RateLimitConfig::default()),
            cors_origins: Vec::new(),
            trusted_proxies: Vec::new(),
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

    pub fn object_store(&self) -> Option<&MinioClient> {
        self.object_store.as_ref()
    }

    /// Vector index client. Named without storage product tokens for route call sites.
    pub fn vector_index(&self) -> Option<&QdrantClient> {
        self.qdrant.as_ref()
    }

    pub fn embedder(&self) -> Option<&ApprovedEmbeddingRuntime> {
        self.embedder.as_ref()
    }

    pub fn chat_provider(&self) -> Option<&ChatProvider> {
        self.chat_provider.as_ref()
    }

    /// Test helper: inject a chat provider (failing/timeout/static) into app state.
    pub fn with_chat_provider(mut self, provider: ChatProvider) -> Self {
        self.chat_provider = Some(provider);
        self
    }

    /// Test helper: attach Qdrant + embedder for ask/search route suites.
    pub fn with_retrieval_backends(
        mut self,
        qdrant: QdrantClient,
        embedder: Option<ApprovedEmbeddingRuntime>,
    ) -> Self {
        self.qdrant = Some(qdrant);
        self.embedder = embedder;
        self
    }

    pub fn capability_keys(&self) -> Option<&CapabilityKeys> {
        self.capability_keys.as_ref()
    }

    pub fn rate_limiter(&self) -> &RateLimiter {
        &self.rate_limiter
    }

    pub fn cors_origins(&self) -> &[String] {
        &self.cors_origins
    }

    pub fn trusted_proxies(&self) -> &[IpAddr] {
        &self.trusted_proxies
    }

    pub fn http_client(&self) -> &reqwest::Client {
        &self.http_client
    }

    pub async fn check_readiness(&self) -> Result<(), &'static str> {
        let mut cached = self.readiness.lock().await;
        if let Some(previous) = cached.as_ref() {
            if previous.checked_at.elapsed() < READINESS_CACHE_TTL {
                return previous.result;
            }
        }
        let result = self.check_dependencies_uncached().await;
        if let Err(reason) = result {
            tracing::warn!(target: "readiness", reason, "dependency readiness check failed");
            *cached = Some(CachedReadiness {
                checked_at: tokio::time::Instant::now(),
                result: Err(reason),
            });
            return Err(reason);
        }
        *cached = Some(CachedReadiness {
            checked_at: tokio::time::Instant::now(),
            result: Ok(()),
        });
        Ok(())
    }

    async fn check_dependencies_uncached(&self) -> Result<(), &'static str> {
        let endpoints = self.runtime().endpoints();
        let object_health_url = format!(
            "{}/minio/health/live",
            endpoints.minio_url.trim_end_matches('/')
        );
        readiness::check_ready(ReadinessDeps {
            config: self.runtime().config(),
            database_url: endpoints.database_url.expose(),
            pool: self.pool(),
            http: self.http_client(),
            vector_base_url: &endpoints.qdrant_url,
            vector_client: self.vector_index(),
            object_client: self.object_store(),
            object_health_url: &object_health_url,
        })
        .await
        .map_err(|error| error.code())
    }
}

fn parse_trusted_proxies_env() -> Result<Vec<IpAddr>, String> {
    let raw = std::env::var("MARKHAND_TRUSTED_PROXIES").unwrap_or_default();
    let mut out = Vec::new();
    for token in raw
        .split(',')
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        let ip = token
            .parse::<IpAddr>()
            .map_err(|_| format!("invalid MARKHAND_TRUSTED_PROXIES entry: {token}"))?;
        out.push(ip);
    }
    Ok(out)
}

fn start_quota_sweep(pool: Pool, config: QuotaSweepConfig) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(config.interval_secs));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            match quota::sweep_expired_all_orgs(&pool, config.batch_size).await {
                Ok(expired) if expired > 0 => {
                    tracing::info!(target: "quota", expired, "quota expiry sweep marked reservations");
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(target: "quota", code = error.code(), "quota expiry sweep failed");
                }
            }
        }
    });
}

pub fn router(state: AppState) -> Router {
    let max_upload_bytes = state.runtime.config().upload().limits.max_upload_bytes as usize;
    let state = Arc::new(state);
    Router::new()
        .merge(routes::health::router())
        .merge(routes::auth::router())
        .merge(routes::uploads::router(max_upload_bytes))
        .merge(routes::collections::router())
        .merge(routes::documents::router())
        .merge(routes::jobs::router())
        .merge(routes::search::router())
        .merge(routes::ask::router())
        .merge(routes::events::router())
        .route("/api/v1/openapi.yaml", axum::routing::get(openapi_yaml))
        .layer(from_fn_with_state(state.clone(), baseline_ip_rate_limit))
        .layer(from_fn_with_state(state.clone(), cors_middleware))
        .layer(from_fn_with_state(state.clone(), inject_request_id))
        .with_state(state)
}

async fn openapi_yaml() -> impl IntoResponse {
    (
        [(
            axum::http::header::CONTENT_TYPE,
            HeaderValue::from_static("application/yaml"),
        )],
        crate::api::embedded_openapi_yaml(),
    )
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
        let header_id = response
            .headers()
            .get(crate::middleware::REQUEST_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .expect("x-request-id header")
            .to_string();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let health: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(health["status"], "ok");
        assert_eq!(health["requestId"].as_str(), Some(header_id.as_str()));
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
