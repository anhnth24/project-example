//! Liveness, readiness, and startup probes (P1B-R06).
//!
//! - `GET|HEAD /live` — process liveness (no dependency I/O)
//! - `GET|HEAD /ready` — required deps + signature + reconciliation (fail closed)
//! - `GET|HEAD /startup` — one-way startup completion / degraded
//!
//! Compatibility aliases remain at `/api/v1/health/{live,ready,startup}`.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use tokio::sync::Mutex;
use tokio::time::{timeout, Instant};

use crate::api::ApiError;
use crate::http::AppState;
use crate::middleware::ResolvedRequestId;

const DEFAULT_DEPENDENCY_TIMEOUT: Duration = Duration::from_secs(2);
const READINESS_CACHE_TTL: Duration = Duration::from_secs(1);

/// Operator-internal reason codes (never returned to unauthenticated callers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeReason {
    Postgres,
    Minio,
    Qdrant,
    Config,
    Signature,
    Reconciliation,
    Timeout,
    Embedding,
}

impl ProbeReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Postgres => "postgres",
            Self::Minio => "minio",
            Self::Qdrant => "qdrant",
            Self::Config => "config",
            Self::Signature => "signature",
            Self::Reconciliation => "reconciliation",
            Self::Timeout => "timeout",
            Self::Embedding => "embedding",
        }
    }
}

/// One-way startup latch. Once completed it never returns to incomplete.
#[derive(Debug, Default)]
pub struct StartupState {
    completed: AtomicBool,
    degraded: AtomicBool,
}

impl StartupState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mark_completed(&self, degraded: bool) {
        self.degraded.store(degraded, Ordering::SeqCst);
        self.completed.store(true, Ordering::SeqCst);
    }

    pub fn is_completed(&self) -> bool {
        self.completed.load(Ordering::SeqCst)
    }

    pub fn is_degraded(&self) -> bool {
        self.degraded.load(Ordering::SeqCst)
    }
}

/// Dynamic reconciliation/restore fence used by readiness.
///
/// Defaults to **not ready** until durable startup reconciliation completes.
#[derive(Debug, Default)]
pub struct ReconciliationGate {
    ready: AtomicBool,
}

impl ReconciliationGate {
    pub fn new_ready() -> Self {
        Self {
            ready: AtomicBool::new(true),
        }
    }

    pub fn new_blocked() -> Self {
        Self {
            ready: AtomicBool::new(false),
        }
    }

    pub fn set_ready(&self, ready: bool) {
        self.ready.store(ready, Ordering::SeqCst);
    }

    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::SeqCst)
    }
}

/// Controllable probes for hermetic tests (no external stack).
#[derive(Debug, Clone)]
pub struct FakeHealthProbes {
    pub postgres: bool,
    pub minio: bool,
    pub qdrant: bool,
    pub config: bool,
    pub signature: bool,
    pub reconciliation: bool,
    /// Shared counter of `run` invocations (cache tests).
    pub runs: Arc<AtomicUsize>,
}

impl FakeHealthProbes {
    pub fn all_ok() -> Self {
        Self {
            postgres: true,
            minio: true,
            qdrant: true,
            config: true,
            signature: true,
            reconciliation: true,
            runs: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn run_count(&self) -> usize {
        self.runs.load(Ordering::SeqCst)
    }

    pub async fn run(&self) -> Result<(), ProbeReason> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        if !self.config {
            return Err(ProbeReason::Config);
        }
        if !self.signature {
            return Err(ProbeReason::Signature);
        }
        if !self.reconciliation {
            return Err(ProbeReason::Reconciliation);
        }
        if !self.postgres {
            return Err(ProbeReason::Postgres);
        }
        if !self.minio {
            return Err(ProbeReason::Minio);
        }
        if !self.qdrant {
            return Err(ProbeReason::Qdrant);
        }
        Ok(())
    }
}

/// Probe backend selected by [`AppState`].
#[derive(Debug, Clone)]
pub enum HealthProbeBackend {
    Live { timeout: Duration },
    Fake(FakeHealthProbes),
}

impl Default for HealthProbeBackend {
    fn default() -> Self {
        Self::Live {
            timeout: DEFAULT_DEPENDENCY_TIMEOUT,
        }
    }
}

#[derive(Debug)]
pub struct ReadinessCache {
    /// Held across refresh so concurrent `/ready` callers single-flight.
    inner: Mutex<Option<CachedReadiness>>,
}

#[derive(Debug, Clone)]
struct CachedReadiness {
    checked_at: Instant,
    result: Result<(), ProbeReason>,
}

impl Default for ReadinessCache {
    fn default() -> Self {
        Self {
            inner: Mutex::new(None),
        }
    }
}

impl ReadinessCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn get_or_run<F, Fut>(&self, run: F) -> Result<(), ProbeReason>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<(), ProbeReason>>,
    {
        let mut guard = self.inner.lock().await;
        if let Some(previous) = guard.as_ref() {
            if previous.checked_at.elapsed() < READINESS_CACHE_TTL {
                return previous.result;
            }
        }
        // Keep the lock while refreshing — waiters queue instead of stampeding.
        let result = run().await;
        *guard = Some(CachedReadiness {
            checked_at: Instant::now(),
            result,
        });
        result
    }

    pub async fn invalidate(&self) {
        *self.inner.lock().await = None;
    }
}

pub async fn run_live_readiness(state: &AppState, bound: Duration) -> Result<(), ProbeReason> {
    match timeout(bound, run_live_readiness_inner(state)).await {
        Ok(result) => result,
        Err(_) => Err(ProbeReason::Timeout),
    }
}

async fn run_live_readiness_inner(state: &AppState) -> Result<(), ProbeReason> {
    state
        .runtime()
        .config()
        .runtime_endpoints()
        .map_err(|_| ProbeReason::Config)?;

    check_signature_alignment(state)?;

    // Dynamically refresh from durable marker — never a permanent in-memory latch.
    let durable_ready = state.refresh_reconciliation_gate().await?;
    if !durable_ready {
        return Err(ProbeReason::Reconciliation);
    }

    let postgres = check_postgres_pool(state);
    let qdrant = state
        .http_client()
        .get(format!(
            "{}/healthz",
            state.runtime().endpoints().qdrant_url
        ))
        .send();
    let minio = state
        .http_client()
        .get(format!(
            "{}/minio/health/ready",
            state.runtime().endpoints().minio_url
        ))
        .send();

    let (postgres, qdrant, minio) = tokio::join!(postgres, qdrant, minio);
    postgres?;
    ensure_http(qdrant, ProbeReason::Qdrant)?;
    ensure_http(minio, ProbeReason::Minio)?;
    Ok(())
}

pub fn check_signature_alignment(state: &AppState) -> Result<(), ProbeReason> {
    let configured = state.runtime().config().index_signature();
    match (configured, state.embedder(), state.embedding_init_error()) {
        (_, _, Some(ProbeReason::Signature)) => Err(ProbeReason::Signature),
        (_, _, Some(ProbeReason::Embedding)) => Err(ProbeReason::Embedding),
        (_, _, Some(other)) => Err(other),
        (Some(configured), Some(runtime), None) => {
            let derived = runtime
                .signature_digest()
                .map_err(|_| ProbeReason::Signature)?;
            if derived != configured {
                return Err(ProbeReason::Signature);
            }
            Ok(())
        }
        (Some(_), None, None) => {
            // Configured signature without a usable runtime in prod → fail closed.
            if state.runtime().config().profile() == crate::config::Profile::Prod {
                Err(ProbeReason::Signature)
            } else {
                Ok(())
            }
        }
        (None, _, None) if state.runtime().config().profile() == crate::config::Profile::Prod => {
            Err(ProbeReason::Signature)
        }
        (None, _, None) => Ok(()),
    }
}

async fn check_postgres_pool(state: &AppState) -> Result<(), ProbeReason> {
    // Bounded by the outer whole-probe timeout in `run_live_readiness`.
    let client = state
        .pool()
        .get()
        .await
        .map_err(|_| ProbeReason::Postgres)?;
    client
        .simple_query("SELECT 1")
        .await
        .map_err(|_| ProbeReason::Postgres)?;
    Ok(())
}

fn ensure_http(
    response: Result<reqwest::Response, reqwest::Error>,
    reason: ProbeReason,
) -> Result<(), ProbeReason> {
    match response {
        Ok(response) if response.status().is_success() => Ok(()),
        _ => Err(reason),
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HealthBody {
    status: &'static str,
    request_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StartupBody {
    status: &'static str,
    completed: bool,
    degraded: bool,
    request_id: String,
}

fn live_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/live", get(liveness).head(liveness))
        .route("/ready", get(readiness).head(readiness))
        .route("/startup", get(startup).head(startup))
}

fn compat_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/health/live", get(liveness).head(liveness))
        .route("/api/v1/health/ready", get(readiness).head(readiness))
        .route("/api/v1/health/startup", get(startup).head(startup))
}

pub fn router() -> Router<Arc<AppState>> {
    live_routes().merge(compat_routes())
}

async fn liveness(ResolvedRequestId(request_id): ResolvedRequestId) -> Json<HealthBody> {
    Json(HealthBody {
        status: "ok",
        request_id,
    })
}

async fn startup(
    State(state): State<Arc<AppState>>,
    ResolvedRequestId(request_id): ResolvedRequestId,
) -> Response {
    let completed = state.startup_state().is_completed();
    let degraded = state.startup_state().is_degraded();
    if !completed {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(StartupBody {
                status: "starting",
                completed: false,
                degraded: false,
                request_id,
            }),
        )
            .into_response();
    }
    let status = if degraded { "degraded" } else { "ok" };
    (
        StatusCode::OK,
        Json(StartupBody {
            status,
            completed: true,
            degraded,
            request_id,
        }),
    )
        .into_response()
}

async fn readiness(
    State(state): State<Arc<AppState>>,
    ResolvedRequestId(request_id): ResolvedRequestId,
) -> Response {
    match state.check_readiness().await {
        Ok(()) => (
            StatusCode::OK,
            Json(HealthBody {
                status: "ok",
                request_id,
            }),
        )
            .into_response(),
        Err(reason) => {
            tracing::warn!(
                target: "readiness",
                reason = reason.as_str(),
                "dependency readiness check failed"
            );
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ApiError {
                    code: "dependency_unavailable".into(),
                    message: "A required service is unavailable".into(),
                    request_id,
                    details: None,
                }),
            )
                .into_response()
        }
    }
}

pub fn dependency_timeout() -> Duration {
    DEFAULT_DEPENDENCY_TIMEOUT
}

pub fn readiness_cache_ttl() -> Duration {
    READINESS_CACHE_TTL
}
