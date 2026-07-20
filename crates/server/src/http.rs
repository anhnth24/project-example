//! HTTP liveness, readiness, and API routes backed by real POC dependencies.

use std::sync::Arc;
use std::time::Duration;

use axum::middleware::{from_fn, from_fn_with_state};
use axum::Router;
use deadpool_postgres::Pool;

use crate::api::sse::AskStreamRegistry;
use crate::auth::jwt::JwtKeys;
use crate::auth::provider::PasswordAuthProvider;
use crate::config::QuotaSweepConfig;
use crate::db::pool::create_pool;
use crate::middleware::{cors, rate_limit, request_id};
use crate::routes;
use crate::services::download::{CapabilityKey, ConsumedDownloadNonces};
use crate::services::quota;
use crate::state::RuntimeState;

pub(crate) const DEPENDENCY_TIMEOUT: Duration = Duration::from_secs(3);

pub struct AppState {
    runtime: RuntimeState,
    http_client: reqwest::Client,
    readiness: tokio::sync::Mutex<Option<crate::services::health::CachedReadiness>>,
    startup_complete: std::sync::atomic::AtomicBool,
    rate_limiter: rate_limit::RateLimiter,
    pool: Pool,
    auth_provider: Option<PasswordAuthProvider>,
    /// Object store adapter (optional when credentials are absent in tests).
    object_store: Option<crate::storage::MinioClient>,
    qdrant: crate::storage::QdrantClient,
    ask_streams: AskStreamRegistry,
    download_capability_key: Option<CapabilityKey>,
    consumed_download_nonces: ConsumedDownloadNonces,
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
        let (object_store, qdrant) = match runtime.config().storage_config() {
            Ok(storage) => {
                let qdrant = crate::storage::QdrantClient::with_api_key(
                    storage.qdrant_url(),
                    storage.qdrant_api_key().cloned(),
                )
                .map_err(|error| format!("cannot configure qdrant client: {}", error.code()))?;
                let object_store = Some(
                    crate::storage::MinioClient::from_config(storage.minio())
                        .map_err(|error| format!("cannot configure object store: {error}"))?,
                );
                (object_store, qdrant)
            }
            Err(error) => {
                let qdrant = crate::storage::QdrantClient::new(&runtime.endpoints().qdrant_url)
                    .map_err(|error| format!("cannot configure qdrant client: {}", error.code()))?;
                if runtime.config().profile() == crate::config::Profile::Prod {
                    return Err(error);
                }
                (None, qdrant)
            }
        };
        start_quota_sweep(pool.clone(), runtime.config().quota_sweep());
        let download_capability_key = runtime
            .config()
            .auth()
            .signing_key
            .as_ref()
            .map(CapabilityKey::derive_from_auth_signing_key);
        Ok(Self {
            rate_limiter: rate_limit::RateLimiter::new(runtime.config().rate_limits()),
            runtime,
            http_client,
            readiness: tokio::sync::Mutex::new(None),
            startup_complete: std::sync::atomic::AtomicBool::new(true),
            pool,
            auth_provider,
            object_store,
            qdrant,
            ask_streams: AskStreamRegistry::new(),
            download_capability_key,
            consumed_download_nonces: ConsumedDownloadNonces::new(),
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
        let qdrant = crate::storage::QdrantClient::new(&runtime.endpoints().qdrant_url)
            .map_err(|error| format!("cannot configure qdrant client: {}", error.code()))?;
        Self::from_parts_with_clients(runtime, pool, auth_provider, object_store, qdrant)
    }

    /// Builds state for tests with explicit object-store and vector clients.
    pub fn from_parts_with_clients(
        runtime: RuntimeState,
        pool: Pool,
        auth_provider: Option<PasswordAuthProvider>,
        object_store: Option<crate::storage::MinioClient>,
        qdrant: crate::storage::QdrantClient,
    ) -> Result<Self, String> {
        if !runtime.is_api_role() {
            return Err("HTTP application requires API runtime configuration".into());
        }
        let http_client = reqwest::Client::builder()
            .timeout(DEPENDENCY_TIMEOUT)
            .build()
            .map_err(|error| format!("cannot configure HTTP client: {error}"))?;
        let download_capability_key = runtime
            .config()
            .auth()
            .signing_key
            .as_ref()
            .map(CapabilityKey::derive_from_auth_signing_key);
        Ok(Self {
            rate_limiter: rate_limit::RateLimiter::new(runtime.config().rate_limits()),
            runtime,
            http_client,
            readiness: tokio::sync::Mutex::new(None),
            startup_complete: std::sync::atomic::AtomicBool::new(true),
            pool,
            auth_provider,
            object_store,
            qdrant,
            ask_streams: AskStreamRegistry::new(),
            download_capability_key,
            consumed_download_nonces: ConsumedDownloadNonces::new(),
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

    pub(crate) fn http_client(&self) -> &reqwest::Client {
        &self.http_client
    }

    pub(crate) fn readiness_cache(
        &self,
    ) -> &tokio::sync::Mutex<Option<crate::services::health::CachedReadiness>> {
        &self.readiness
    }

    pub(crate) fn startup_complete(&self) -> bool {
        self.startup_complete
            .load(std::sync::atomic::Ordering::Acquire)
    }

    pub(crate) fn rate_limiter(&self) -> &rate_limit::RateLimiter {
        &self.rate_limiter
    }

    #[cfg(test)]
    pub(crate) fn set_startup_complete_for_test(&self, value: bool) {
        self.startup_complete
            .store(value, std::sync::atomic::Ordering::Release);
    }

    pub fn object_store(&self) -> Option<&crate::storage::MinioClient> {
        self.object_store.as_ref()
    }

    pub fn vector_store(&self) -> &crate::storage::QdrantClient {
        &self.qdrant
    }

    pub(crate) fn ask_streams(&self) -> AskStreamRegistry {
        self.ask_streams.clone()
    }

    pub fn download_capability_key(&self) -> Option<&CapabilityKey> {
        self.download_capability_key.as_ref()
    }

    pub fn consumed_download_nonces(&self) -> &ConsumedDownloadNonces {
        &self.consumed_download_nonces
    }
}

fn start_quota_sweep(pool: Pool, config: QuotaSweepConfig) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(config.interval_secs));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            // Admission itself is time-correct (it filters `expires_at` against a
            // lock-scoped `clock_timestamp()` in SQL). This bounded sweep is hygiene
            // so expired reservations become terminal and operational gauges stop
            // showing stale reserved rows.
            match quota::sweep_expired_all_orgs(&pool, config.batch_size).await {
                Ok(expired) if expired > 0 => {
                    eprintln!("fileconv-server: quota expiry sweep marked {expired} reservations");
                }
                Ok(_) => {}
                Err(error) => {
                    eprintln!(
                        "fileconv-server: quota expiry sweep failed: {}",
                        error.code()
                    );
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
        .merge(routes::collections::router())
        .merge(routes::documents::router())
        .merge(routes::jobs::router())
        .merge(routes::search::router())
        .merge(routes::ask::router())
        .merge(routes::events::router())
        .merge(routes::uploads::router(max_upload_bytes))
        .with_state(state.clone())
        .layer(from_fn_with_state(state.clone(), rate_limit::rate_limit))
        .layer(from_fn_with_state(state, cors::cors))
        .layer(from_fn(request_id::ensure_request_id))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use axum::http::header::{ACCESS_CONTROL_ALLOW_ORIGIN, ORIGIN, RETRY_AFTER, VARY};
    use axum::http::{Method, Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::{router, AppState};
    use crate::api::ApiError;
    use crate::config::{RateLimitConfig, RuntimeEndpoints, SecretString, ServerConfig};
    use crate::db::pool::create_pool;
    use crate::middleware::request_id::X_REQUEST_ID;
    use crate::state::RuntimeState;

    fn test_rate_limits(limit: u32) -> RateLimitConfig {
        RateLimitConfig {
            auth_per_ip_per_minute: limit,
            llm_per_user_per_minute: limit,
            authenticated_per_user_per_minute: limit,
            fallback_per_ip_per_minute: limit,
            max_entries: 64,
        }
    }

    fn test_app(config: ServerConfig) -> axum::Router {
        let runtime = RuntimeState::from_config(config).unwrap();
        let pool = create_pool("postgres://markhand_app:markhand_app@127.0.0.1:5432/markhand_test")
            .expect("pool");
        router(AppState::from_parts(runtime, pool, None).unwrap())
    }

    fn config_for_middleware_tests() -> ServerConfig {
        ServerConfig::test_with_endpoints(RuntimeEndpoints {
            database_url: SecretString::new("postgres://unused"),
            qdrant_url: "http://127.0.0.1:1".into(),
            minio_url: "http://127.0.0.1:1".into(),
        })
    }

    fn login_request(ip: &str, forwarded_for: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder()
            .method(Method::POST)
            .uri("/api/v1/auth/login")
            .header("content-type", "application/json");
        if let Some(forwarded_for) = forwarded_for {
            builder = builder.header("x-forwarded-for", forwarded_for);
        }
        let mut request = builder
            .body(Body::from(
                r#"{"email":"a@example.com","password":"secret"}"#,
            ))
            .unwrap();
        request.extensions_mut().insert(ConnectInfo(
            format!("{ip}:12345")
                .parse::<std::net::SocketAddr>()
                .unwrap(),
        ));
        request
    }

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

    #[tokio::test]
    async fn rate_limit_returns_429_with_retry_after_and_api_error() {
        let app =
            test_app(config_for_middleware_tests().with_test_rate_limits(test_rate_limits(2)));
        for _ in 0..2 {
            let response = app
                .clone()
                .oneshot(login_request("192.0.2.10", None))
                .await
                .unwrap();
            assert_ne!(response.status(), axum::http::StatusCode::TOO_MANY_REQUESTS);
        }
        let response = app
            .oneshot(login_request("192.0.2.10", None))
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::TOO_MANY_REQUESTS);
        assert!(response.headers().contains_key(RETRY_AFTER));
        assert!(response.headers().contains_key(&X_REQUEST_ID));
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let error: ApiError = serde_json::from_slice(&body).unwrap();
        assert_eq!(error.code, "rate_limited");
        assert!(!error.request_id.is_empty());
    }

    #[tokio::test]
    async fn rate_limit_keys_are_isolated_and_health_is_exempt() {
        let app =
            test_app(config_for_middleware_tests().with_test_rate_limits(test_rate_limits(1)));
        let first = app
            .clone()
            .oneshot(login_request("192.0.2.10", None))
            .await
            .unwrap();
        assert_ne!(first.status(), axum::http::StatusCode::TOO_MANY_REQUESTS);
        let limited = app
            .clone()
            .oneshot(login_request("192.0.2.10", None))
            .await
            .unwrap();
        assert_eq!(limited.status(), axum::http::StatusCode::TOO_MANY_REQUESTS);
        let isolated = app
            .clone()
            .oneshot(login_request("192.0.2.11", None))
            .await
            .unwrap();
        assert_ne!(isolated.status(), axum::http::StatusCode::TOO_MANY_REQUESTS);

        for _ in 0..3 {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/api/v1/health/live")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), axum::http::StatusCode::OK);
        }
    }

    #[tokio::test]
    async fn missing_connect_info_falls_back_without_panicking() {
        let app =
            test_app(config_for_middleware_tests().with_test_rate_limits(test_rate_limits(1)));
        for expected in [
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            axum::http::StatusCode::TOO_MANY_REQUESTS,
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri("/api/v1/auth/login")
                        .header("content-type", "application/json")
                        .body(Body::from(
                            r#"{"email":"a@example.com","password":"secret"}"#,
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), expected);
        }
    }

    #[tokio::test]
    async fn forwarded_for_is_ignored_unless_trusted_proxy_is_enabled() {
        let untrusted = test_app(
            config_for_middleware_tests()
                .with_test_rate_limits(test_rate_limits(1))
                .with_test_http_hardening(false, Vec::new()),
        );
        let first = untrusted
            .clone()
            .oneshot(login_request("192.0.2.1", Some("198.51.100.1")))
            .await
            .unwrap();
        assert_ne!(first.status(), axum::http::StatusCode::TOO_MANY_REQUESTS);
        let limited = untrusted
            .oneshot(login_request("192.0.2.1", Some("198.51.100.2")))
            .await
            .unwrap();
        assert_eq!(limited.status(), axum::http::StatusCode::TOO_MANY_REQUESTS);

        let trusted = test_app(
            config_for_middleware_tests()
                .with_test_rate_limits(test_rate_limits(1))
                .with_test_http_hardening(true, Vec::new()),
        );
        let first = trusted
            .clone()
            .oneshot(login_request("192.0.2.1", Some("198.51.100.1")))
            .await
            .unwrap();
        assert_ne!(first.status(), axum::http::StatusCode::TOO_MANY_REQUESTS);
        let isolated = trusted
            .oneshot(login_request("192.0.2.1", Some("198.51.100.2")))
            .await
            .unwrap();
        assert_ne!(isolated.status(), axum::http::StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn cors_preflight_and_request_id_headers_are_conservative() {
        let app = test_app(
            config_for_middleware_tests()
                .with_test_http_hardening(false, vec!["https://app.example.test".into()]),
        );
        let allowed = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/health/live")
                    .header(ORIGIN, "https://app.example.test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(allowed.status(), axum::http::StatusCode::OK);
        assert_eq!(
            allowed.headers().get(ACCESS_CONTROL_ALLOW_ORIGIN).unwrap(),
            "https://app.example.test"
        );
        assert!(allowed.headers().contains_key(VARY));
        assert!(allowed.headers().contains_key(&X_REQUEST_ID));

        let disallowed = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/health/live")
                    .header(ORIGIN, "https://evil.example")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(!disallowed
            .headers()
            .contains_key(ACCESS_CONTROL_ALLOW_ORIGIN));
        assert!(disallowed.headers().contains_key(&X_REQUEST_ID));

        let preflight = app
            .oneshot(
                Request::builder()
                    .method(Method::OPTIONS)
                    .uri("/api/v1/health/live")
                    .header(ORIGIN, "https://app.example.test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(preflight.status(), axum::http::StatusCode::NO_CONTENT);
        assert_eq!(
            preflight
                .headers()
                .get(ACCESS_CONTROL_ALLOW_ORIGIN)
                .unwrap(),
            "https://app.example.test"
        );
        assert!(preflight.headers().contains_key(&X_REQUEST_ID));
    }
}
