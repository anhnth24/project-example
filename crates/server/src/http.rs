//! HTTP application wiring: middleware, health, and `/api/v1` routes.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::{from_fn, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use deadpool_postgres::Pool;
use uuid::Uuid;

use crate::api::ApiError;
use crate::auth::jwt::JwtKeys;
use crate::auth::provider::PasswordAuthProvider;
use crate::config::{QuotaSweepConfig, TrustedProxies};
use crate::db::pool::create_pool;
use crate::middleware::{
    rate_limit_middleware, request_id_middleware, InMemoryRateLimiter, RequestId,
};
use crate::routes;
use crate::services::download::DownloadFetchBudget;
use crate::services::embedding::{ApprovedEmbeddingRuntime, EmbeddingError};
use crate::services::health::{
    dependency_timeout, FakeHealthProbes, HealthProbeBackend, ProbeReason, ReadinessCache,
    ReconciliationGate, StartupState,
};
use crate::services::qa::provider::{ConfiguredProvider, ProviderError, QaProviderConfig};
use crate::services::quota;
use crate::state::RuntimeState;
use crate::storage::qdrant::QdrantClient;

pub struct AppState {
    runtime: RuntimeState,
    http_client: reqwest::Client,
    pool: Pool,
    auth_provider: Option<PasswordAuthProvider>,
    /// Object store adapter (optional when credentials are absent in tests).
    object_store: Option<crate::storage::MinioClient>,
    /// Process-wide concurrent download byte budget / concurrency limiter.
    download_budget: Arc<DownloadFetchBudget>,
    /// Vector search client (FTS-only still works when the backend is down).
    qdrant: Option<QdrantClient>,
    /// Optional approved embedding runtime for hybrid retrieval.
    embedder: Option<ApprovedEmbeddingRuntime>,
    /// Preserved embedding init/signature error for readiness reporting.
    embedding_init_error: Option<ProbeReason>,
    /// Optional grounded-QA provider (absent → deterministic extractive).
    qa_provider: Option<ConfiguredProvider>,
    rate_limiter: Arc<InMemoryRateLimiter>,
    trusted_proxies: TrustedProxies,
    startup: Arc<StartupState>,
    /// Defaults blocked until durable startup reconciliation marks ready.
    reconciliation: Arc<ReconciliationGate>,
    probes: HealthProbeBackend,
    /// Short-lived readiness probe cache (ready path only).
    readiness_cache: ReadinessCache,
}

impl AppState {
    pub fn new(runtime: RuntimeState) -> Result<Self, String> {
        if !runtime.is_api_role() {
            return Err("HTTP application requires API runtime configuration".into());
        }
        let http_client = reqwest::Client::builder()
            .timeout(dependency_timeout())
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
        let qdrant = Some(build_qdrant(&runtime)?);
        let (embedder, embedding_init_error) = load_embedder(
            runtime.config().index_signature(),
            runtime.config().profile(),
        );
        let qa_provider = match QaProviderConfig::from_env(runtime.config().profile()) {
            Ok(config) => Some(
                ConfiguredProvider::new(config)
                    .map_err(|error| format!("cannot configure QA provider: {}", error.code()))?,
            ),
            Err(ProviderError::Unavailable) => None,
            Err(error) => return Err(format!("cannot configure QA provider: {}", error.code())),
        };
        start_quota_sweep(pool.clone(), runtime.config().quota_sweep());
        let rate_limiter = Arc::new(InMemoryRateLimiter::new(
            runtime.config().rate_limit().clone(),
        ));
        let trusted_proxies = runtime.config().trusted_proxies().clone();
        let startup = Arc::new(StartupState::new());
        // Fail closed until durable startup reconciliation completes.
        let reconciliation = Arc::new(ReconciliationGate::new_blocked());
        let state = Self {
            runtime,
            http_client,
            pool,
            auth_provider,
            object_store,
            download_budget: DownloadFetchBudget::default_production(),
            qdrant,
            embedder,
            embedding_init_error,
            qa_provider,
            rate_limiter,
            trusted_proxies,
            startup,
            reconciliation,
            probes: HealthProbeBackend::default(),
            readiness_cache: ReadinessCache::new(),
        };
        // One-way startup completion after successful construction.
        state.startup.mark_completed(false);
        Ok(state)
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
            .timeout(dependency_timeout())
            .build()
            .map_err(|error| format!("cannot configure HTTP client: {error}"))?;
        let qdrant = build_qdrant(&runtime).ok();
        let rate_limiter = Arc::new(InMemoryRateLimiter::new(
            runtime.config().rate_limit().clone(),
        ));
        let trusted_proxies = runtime.config().trusted_proxies().clone();
        let startup = Arc::new(StartupState::new());
        startup.mark_completed(false);
        let probes = FakeHealthProbes::all_ok();
        // Hermetic tests still default the in-memory gate from the fake probe set.
        let reconciliation = Arc::new(ReconciliationGate::new_blocked());
        reconciliation.set_ready(probes.reconciliation);
        Ok(Self {
            runtime,
            http_client,
            pool,
            auth_provider,
            object_store,
            download_budget: DownloadFetchBudget::for_tests(),
            qdrant,
            embedder: None,
            embedding_init_error: None,
            qa_provider: None,
            rate_limiter,
            trusted_proxies,
            startup,
            reconciliation,
            probes: HealthProbeBackend::Fake(probes),
            readiness_cache: ReadinessCache::new(),
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

    pub fn download_budget(&self) -> &Arc<DownloadFetchBudget> {
        &self.download_budget
    }

    pub fn qdrant(&self) -> Option<&QdrantClient> {
        self.qdrant.as_ref()
    }

    /// Vector-store backend for retrieval services (route-safe name; ADR 0001).
    pub fn vector_store(&self) -> Option<&QdrantClient> {
        self.qdrant.as_ref()
    }

    pub fn embedder(&self) -> Option<&ApprovedEmbeddingRuntime> {
        self.embedder.as_ref()
    }

    pub fn embedding_init_error(&self) -> Option<ProbeReason> {
        self.embedding_init_error
    }

    pub fn qa_provider(&self) -> Option<&ConfiguredProvider> {
        self.qa_provider.as_ref()
    }

    pub fn rate_limiter(&self) -> &Arc<InMemoryRateLimiter> {
        &self.rate_limiter
    }

    pub fn trusted_proxies(&self) -> &TrustedProxies {
        &self.trusted_proxies
    }

    pub fn startup_state(&self) -> &StartupState {
        &self.startup
    }

    pub fn reconciliation_gate(&self) -> &ReconciliationGate {
        &self.reconciliation
    }

    pub fn http_client(&self) -> &reqwest::Client {
        &self.http_client
    }

    /// Test helper: attach retrieval/QA dependencies without rebuilding auth/pool.
    pub fn with_retrieval_deps(
        mut self,
        qdrant: Option<QdrantClient>,
        embedder: Option<ApprovedEmbeddingRuntime>,
        qa_provider: Option<ConfiguredProvider>,
    ) -> Self {
        self.qdrant = qdrant;
        self.embedder = embedder;
        self.embedding_init_error = None;
        self.qa_provider = qa_provider;
        self
    }

    /// Test helper: preserve a signature/init failure for readiness checks.
    pub fn with_embedding_init_error(mut self, reason: ProbeReason) -> Self {
        self.embedder = None;
        self.embedding_init_error = Some(reason);
        self
    }

    /// Test helper: replace readiness probes with controllable fakes.
    pub fn with_fake_probes(mut self, probes: FakeHealthProbes) -> Self {
        self.reconciliation.set_ready(probes.reconciliation);
        self.probes = HealthProbeBackend::Fake(probes);
        self
    }

    /// Test helper: override trusted proxy CIDRs after construction.
    pub fn with_trusted_proxies(mut self, proxies: TrustedProxies) -> Self {
        self.trusted_proxies = proxies;
        self
    }

    /// Test helper: replace the in-memory rate limiter config/state.
    pub fn with_rate_limiter(mut self, limiter: InMemoryRateLimiter) -> Self {
        self.rate_limiter = Arc::new(limiter);
        self
    }

    /// Explicit startup bootstrap: open a readiness generation, then certify if idle.
    pub async fn bootstrap_startup_reconciliation(&self) -> Result<bool, String> {
        let ready = crate::services::reconciliation::bootstrap_startup_reconciliation(self.pool())
            .await
            .map_err(|error| error.to_string())?;
        // Refresh cached view; durable marker remains the authority.
        self.reconciliation.set_ready(ready);
        Ok(ready)
    }

    /// Refresh the in-memory view from the durable marker (never a permanent latch).
    pub async fn refresh_reconciliation_gate(&self) -> Result<bool, ProbeReason> {
        crate::services::health::refresh_reconciliation_gate(self.pool(), &self.reconciliation)
            .await
    }

    pub async fn check_readiness(&self) -> Result<(), ProbeReason> {
        crate::services::health::check_readiness(
            self,
            &self.readiness_cache,
            &self.probes,
            &self.reconciliation,
        )
        .await
    }
}

fn load_embedder(
    approved_signature: Option<&str>,
    profile: crate::config::Profile,
) -> (Option<ApprovedEmbeddingRuntime>, Option<ProbeReason>) {
    match ApprovedEmbeddingRuntime::from_env(approved_signature, profile) {
        Ok(runtime) => (Some(runtime), None),
        Err(EmbeddingError::SignatureMismatch) => (None, Some(ProbeReason::Signature)),
        Err(_) if approved_signature.is_some() || profile == crate::config::Profile::Prod => {
            (None, Some(ProbeReason::Embedding))
        }
        Err(_) => (None, None),
    }
}

fn build_qdrant(runtime: &RuntimeState) -> Result<QdrantClient, String> {
    let url = runtime.endpoints().qdrant_url.clone();
    let api_key = runtime
        .config()
        .storage_config()
        .ok()
        .and_then(|cfg| cfg.qdrant_api_key().cloned());
    QdrantClient::with_api_key(url, api_key)
        .map_err(|error| format!("cannot configure qdrant: {error}"))
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
    let cors = state.runtime.config().cors().clone();
    let state = Arc::new(state);

    // Innermost → outermost for request flow is reverse of layer order.
    // Desired request order: request ID → error envelope → CORS → rate → auth(extractor).
    let api = Router::new()
        .merge(routes::health::router())
        .merge(routes::auth::router())
        .merge(routes::uploads::router(max_upload_bytes))
        .merge(routes::collections::router())
        .merge(routes::documents::router())
        .merge(routes::jobs::router())
        .merge(routes::search::router())
        .merge(routes::ask::router())
        .merge(routes::events::router())
        .fallback(api_not_found)
        .method_not_allowed_fallback(api_method_not_allowed)
        .layer(from_fn_with_state(state.clone(), rate_limit_middleware))
        .layer(from_fn(move |request, next| {
            let cors = cors.clone();
            async move { crate::middleware::cors_layer(cors, request, next).await }
        }))
        .layer(from_fn(error_envelope_middleware))
        .layer(from_fn(request_id_middleware))
        .with_state(state);

    api
}

async fn error_envelope_middleware(request: Request, next: axum::middleware::Next) -> Response {
    // Ensures request-id is available to fallbacks via extensions; body mapping stays
    // in typed rejections. This layer intentionally does not swallow successes.
    next.run(request).await
}

async fn api_not_found(request: Request) -> Response {
    let request_id = request
        .extensions()
        .get::<RequestId>()
        .map(|id| id.0.clone())
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    (
        StatusCode::NOT_FOUND,
        Json(ApiError::new("not_found", "Resource not found", request_id)),
    )
        .into_response()
}

async fn api_method_not_allowed(request: Request) -> Response {
    let request_id = request
        .extensions()
        .get::<RequestId>()
        .map(|id| id.0.clone())
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    (
        StatusCode::METHOD_NOT_ALLOWED,
        Json(ApiError::new(
            "method_not_allowed",
            "Method not allowed",
            request_id,
        )),
    )
        .into_response()
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

    fn test_app() -> axum::Router {
        let runtime =
            RuntimeState::from_config(ServerConfig::test_with_endpoints(RuntimeEndpoints {
                database_url: SecretString::new("postgres://unused"),
                qdrant_url: "http://127.0.0.1:1".into(),
                minio_url: "http://127.0.0.1:1".into(),
            }))
            .unwrap();
        let pool = create_pool("postgres://markhand_app:markhand_app@127.0.0.1:5432/markhand_test")
            .expect("pool");
        router(AppState::from_parts(runtime, pool, None).unwrap())
    }

    #[tokio::test]
    async fn liveness_has_a_contract_compliant_body() {
        let app = test_app();
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/live")
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

    #[tokio::test]
    async fn compat_health_live_still_works() {
        let response = test_app()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/health/live")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn unknown_route_returns_canonical_json_404() {
        let response = test_app()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/does-not-exist")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["code"], "not_found");
        assert!(json["requestId"].as_str().is_some());
    }

    #[tokio::test]
    async fn wrong_method_returns_canonical_json_405() {
        let response = test_app()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/live")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            axum::http::StatusCode::METHOD_NOT_ALLOWED
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["code"], "method_not_allowed");
        assert!(json["requestId"].as_str().is_some());
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
