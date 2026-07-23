//! Fail-closed readiness probes (P1B-R06).
//!
//! Routes must not mention storage product names; this service owns dependency
//! checks and propagates typed probe failures. All DB fence/generation probes
//! use SECURITY DEFINER helpers so they work under the app role + FORCE RLS.
//! Outer + per-probe deadlines bound hanging backends to a fast 503. Outer
//! timeout reports the probe that was in progress (not a hard-coded database).

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::Duration;

use deadpool_postgres::Pool;
use thiserror::Error;
use tokio::time::timeout;

use crate::config::ServerConfig;
use crate::database;
use crate::services::embedding::ApprovedEmbeddingRuntime;
use crate::services::index_signature;
use crate::services::ops_fence;
use crate::storage::minio::MinioClient;
use crate::storage::qdrant::QdrantClient;

pub const OUTER_DEADLINE: Duration = Duration::from_secs(4);
pub const PER_PROBE_DEADLINE: Duration = Duration::from_secs(2);

#[derive(Debug, Error, PartialEq, Eq, Clone, Copy)]
pub enum ReadinessProbeError {
    #[error("postgresql unavailable")]
    Database,
    #[error("vector store unavailable")]
    VectorStore,
    #[error("object store unavailable")]
    ObjectStore,
    #[error("embedding runtime unavailable")]
    Embedding,
    #[error("index signature invalid")]
    IndexSignature,
    #[error("no active index generation")]
    ActiveGeneration,
    #[error("reconciliation or restore fence active")]
    ReconcileFence,
    #[error("object store credentials missing")]
    ObjectStoreCredentials,
    #[error("vector store credentials missing")]
    VectorStoreCredentials,
    #[error("embedding credentials missing")]
    EmbeddingCredentials,
}

impl ReadinessProbeError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Database => "ready_database",
            Self::VectorStore => "ready_vector_store",
            Self::ObjectStore => "ready_object_store",
            Self::Embedding => "ready_embedding",
            Self::IndexSignature => "ready_index_signature",
            Self::ActiveGeneration => "ready_active_generation",
            Self::ReconcileFence => "ready_reconcile_fence",
            Self::ObjectStoreCredentials => "ready_object_store_credentials",
            Self::VectorStoreCredentials => "ready_vector_store_credentials",
            Self::EmbeddingCredentials => "ready_embedding_credentials",
        }
    }

    const fn as_u8(self) -> u8 {
        match self {
            Self::Database => 0,
            Self::VectorStore => 1,
            Self::ObjectStore => 2,
            Self::Embedding => 3,
            Self::IndexSignature => 4,
            Self::ActiveGeneration => 5,
            Self::ReconcileFence => 6,
            Self::ObjectStoreCredentials => 7,
            Self::VectorStoreCredentials => 8,
            Self::EmbeddingCredentials => 9,
        }
    }

    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::VectorStore,
            2 => Self::ObjectStore,
            3 => Self::Embedding,
            4 => Self::IndexSignature,
            5 => Self::ActiveGeneration,
            6 => Self::ReconcileFence,
            7 => Self::ObjectStoreCredentials,
            8 => Self::VectorStoreCredentials,
            9 => Self::EmbeddingCredentials,
            _ => Self::Database,
        }
    }
}

/// One-way startup latch for `/health/start`.
#[derive(Debug, Default)]
pub struct StartupState {
    completed: AtomicBool,
}

impl StartupState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mark_completed(&self) {
        self.completed.store(true, Ordering::SeqCst);
    }

    pub fn is_completed(&self) -> bool {
        self.completed.load(Ordering::SeqCst)
    }
}

pub struct ReadinessDeps<'a> {
    pub config: &'a ServerConfig,
    pub database_url: &'a str,
    pub pool: &'a Pool,
    pub http: &'a reqwest::Client,
    pub vector_base_url: &'a str,
    pub vector_client: Option<&'a QdrantClient>,
    pub object_client: Option<&'a MinioClient>,
    pub object_health_url: &'a str,
    pub embedder: Option<&'a ApprovedEmbeddingRuntime>,
}

struct ProbeTracker {
    current: AtomicU8,
}

impl ProbeTracker {
    fn new() -> Self {
        Self {
            current: AtomicU8::new(ReadinessProbeError::Database.as_u8()),
        }
    }

    fn set(&self, probe: ReadinessProbeError) {
        self.current.store(probe.as_u8(), Ordering::SeqCst);
    }

    fn get(&self) -> ReadinessProbeError {
        ReadinessProbeError::from_u8(self.current.load(Ordering::SeqCst))
    }
}

/// Full fail-closed readiness. Any probe error fails the check.
pub async fn check_ready(deps: ReadinessDeps<'_>) -> Result<(), ReadinessProbeError> {
    let tracker = ProbeTracker::new();
    match timeout(OUTER_DEADLINE, check_ready_inner(deps, &tracker)).await {
        Ok(result) => result,
        Err(_) => Err(tracker.get()),
    }
}

async fn check_ready_inner(
    deps: ReadinessDeps<'_>,
    tracker: &ProbeTracker,
) -> Result<(), ReadinessProbeError> {
    tracker.set(ReadinessProbeError::Database);
    timeout(
        PER_PROBE_DEADLINE,
        database::check_connection(deps.database_url),
    )
    .await
    .map_err(|_| ReadinessProbeError::Database)?
    .map_err(|_| ReadinessProbeError::Database)?;

    if deps.vector_client.is_none() {
        tracker.set(ReadinessProbeError::VectorStoreCredentials);
        return Err(ReadinessProbeError::VectorStoreCredentials);
    }
    tracker.set(ReadinessProbeError::VectorStore);
    let vector = deps
        .http
        .get(format!(
            "{}/healthz",
            deps.vector_base_url.trim_end_matches('/')
        ))
        .timeout(PER_PROBE_DEADLINE)
        .send();
    let vector = timeout(PER_PROBE_DEADLINE, vector)
        .await
        .map_err(|_| ReadinessProbeError::VectorStore)?
        .map_err(|_| ReadinessProbeError::VectorStore)?;
    if !vector.status().is_success() {
        return Err(ReadinessProbeError::VectorStore);
    }
    timeout(
        PER_PROBE_DEADLINE,
        deps.vector_client
            .expect("checked above")
            .collections_probe(),
    )
    .await
    .map_err(|_| ReadinessProbeError::VectorStore)?
    .map_err(|_| ReadinessProbeError::VectorStore)?;

    let object = match deps.object_client {
        Some(client) => client,
        None => {
            tracker.set(ReadinessProbeError::ObjectStoreCredentials);
            return Err(ReadinessProbeError::ObjectStoreCredentials);
        }
    };
    tracker.set(ReadinessProbeError::ObjectStore);
    let object_health = deps
        .http
        .get(deps.object_health_url)
        .timeout(PER_PROBE_DEADLINE)
        .send();
    let object_health = timeout(PER_PROBE_DEADLINE, object_health)
        .await
        .map_err(|_| ReadinessProbeError::ObjectStore)?
        .map_err(|_| ReadinessProbeError::ObjectStore)?;
    if !object_health.status().is_success() {
        return Err(ReadinessProbeError::ObjectStore);
    }
    timeout(PER_PROBE_DEADLINE, object.bucket_probe())
        .await
        .map_err(|_| ReadinessProbeError::ObjectStore)?
        .map_err(|_| ReadinessProbeError::ObjectStore)?;

    match deps.config.index_signature() {
        Some(signature) => {
            tracker.set(ReadinessProbeError::IndexSignature);
            index_signature::validate_signature_digest(signature)
                .map_err(|_| ReadinessProbeError::IndexSignature)?;
            tracker.set(ReadinessProbeError::ActiveGeneration);
            if !timeout(
                PER_PROBE_DEADLINE,
                active_generation_consistent(deps.pool, signature),
            )
            .await
            .map_err(|_| ReadinessProbeError::ActiveGeneration)??
            {
                return Err(ReadinessProbeError::ActiveGeneration);
            }
            timeout(
                PER_PROBE_DEADLINE,
                deps.vector_client
                    .expect("checked above")
                    .collection_probe_for_digest(signature),
            )
            .await
            .map_err(|_| ReadinessProbeError::ActiveGeneration)?
            .map_err(|_| ReadinessProbeError::ActiveGeneration)?;
            let embedder = match deps.embedder {
                Some(embedder) => embedder,
                None => {
                    tracker.set(ReadinessProbeError::EmbeddingCredentials);
                    return Err(ReadinessProbeError::EmbeddingCredentials);
                }
            };
            tracker.set(ReadinessProbeError::Embedding);
            timeout(PER_PROBE_DEADLINE, embedder.health_probe())
                .await
                .map_err(|_| ReadinessProbeError::Embedding)?
                .map_err(|_| ReadinessProbeError::Embedding)?;
        }
        None if deps.config.profile() == crate::config::Profile::Prod => {
            tracker.set(ReadinessProbeError::IndexSignature);
            return Err(ReadinessProbeError::IndexSignature);
        }
        None => {
            if let Some(embedder) = deps.embedder {
                tracker.set(ReadinessProbeError::Embedding);
                timeout(PER_PROBE_DEADLINE, embedder.health_probe())
                    .await
                    .map_err(|_| ReadinessProbeError::Embedding)?
                    .map_err(|_| ReadinessProbeError::Embedding)?;
            }
        }
    }

    tracker.set(ReadinessProbeError::ReconcileFence);
    let fence = timeout(PER_PROBE_DEADLINE, async {
        Ok::<_, ReadinessProbeError>(
            ops_fence::any_blocking_fence_active(deps.pool)
                .await
                .map_err(|_| ReadinessProbeError::Database)?
                || ops_fence::any_org_reconcile_running(deps.pool)
                    .await
                    .map_err(|_| ReadinessProbeError::Database)?,
        )
    })
    .await
    .map_err(|_| ReadinessProbeError::ReconcileFence)??;
    if fence {
        return Err(ReadinessProbeError::ReconcileFence);
    }
    Ok(())
}

async fn active_generation_consistent(
    pool: &Pool,
    signature: &str,
) -> Result<bool, ReadinessProbeError> {
    let client = pool
        .get()
        .await
        .map_err(|_| ReadinessProbeError::Database)?;
    let ok: bool = client
        .query_one(
            "SELECT markhand_index_generation_consistent($1)",
            &[&signature],
        )
        .await
        .map_err(|_| ReadinessProbeError::Database)?
        .get(0);
    Ok(ok)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::time::Instant;

    use axum::routing::get;
    use axum::Router;
    use tokio::net::TcpListener;

    async fn blackhole_listener() -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    break;
                };
                // Hold the socket open forever without an HTTP response.
                tokio::spawn(async move {
                    let _socket = socket;
                    std::future::pending::<()>().await;
                });
            }
        });
        (addr, handle)
    }

    async fn assert_hanging_http_probe(code: ReadinessProbeError, path: &str) {
        let (addr, handle) = blackhole_listener().await;
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        let tracker = ProbeTracker::new();
        tracker.set(code);
        let started = Instant::now();
        let result = timeout(PER_PROBE_DEADLINE, async {
            http.get(format!("http://{addr}{path}"))
                .timeout(PER_PROBE_DEADLINE)
                .send()
                .await
        })
        .await;
        // Hang is either outer deadline or request-level timeout/transport error.
        let hung = !matches!(result, Ok(Ok(_)));
        assert!(hung, "hanging probe must not succeed");
        assert!(started.elapsed() <= PER_PROBE_DEADLINE + Duration::from_millis(750));
        assert_eq!(tracker.get().code(), code.code());
        handle.abort();
    }

    #[test]
    fn probe_codes_are_stable() {
        assert_eq!(ReadinessProbeError::Database.code(), "ready_database");
        assert_eq!(
            ReadinessProbeError::VectorStore.code(),
            "ready_vector_store"
        );
        assert_eq!(
            ReadinessProbeError::ObjectStore.code(),
            "ready_object_store"
        );
        assert_eq!(ReadinessProbeError::Embedding.code(), "ready_embedding");
    }

    #[tokio::test]
    async fn hanging_vector_probe_returns_exact_code_within_deadline() {
        assert_hanging_http_probe(ReadinessProbeError::VectorStore, "/healthz").await;
    }

    #[tokio::test]
    async fn hanging_object_probe_returns_exact_code_within_deadline() {
        assert_hanging_http_probe(ReadinessProbeError::ObjectStore, "/minio/health/live").await;
    }

    #[tokio::test]
    async fn outer_timeout_reports_current_probe_not_hardcoded_database() {
        let tracker = Arc::new(ProbeTracker::new());
        let tracker_clone = Arc::clone(&tracker);
        let started = Instant::now();
        let result = timeout(Duration::from_millis(80), async move {
            tracker_clone.set(ReadinessProbeError::ObjectStore);
            tokio::time::sleep(Duration::from_secs(5)).await;
            Ok::<(), ReadinessProbeError>(())
        })
        .await;
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_millis(500));
        let code = tracker.get();
        assert_eq!(code, ReadinessProbeError::ObjectStore);
        assert_ne!(code.code(), "ready_database");
    }

    #[tokio::test]
    async fn hanging_router_ready_matrix_reports_exact_codes_and_deadlines() {
        // Per-dependency router matrix: each hanging probe returns its stable code
        // and completes within the per-probe deadline (+ slack).
        for (code, path) in [
            (ReadinessProbeError::VectorStore, "/healthz"),
            (ReadinessProbeError::ObjectStore, "/minio/health/live"),
            (ReadinessProbeError::Embedding, "/v1/embeddings"),
        ] {
            let (addr, handle) = blackhole_listener().await;
            let app = Router::new().route(
                "/api/v1/health/ready",
                get({
                    let url = format!("http://{addr}{path}");
                    let expected = code;
                    move || {
                        let url = url.clone();
                        async move {
                            let http = reqwest::Client::builder().no_proxy().build().unwrap();
                            let tracker = ProbeTracker::new();
                            tracker.set(expected);
                            let started = Instant::now();
                            let outcome = timeout(PER_PROBE_DEADLINE, async {
                                http.get(&url).timeout(PER_PROBE_DEADLINE).send().await
                            })
                            .await;
                            let hung = !matches!(outcome, Ok(Ok(_)));
                            let within = started.elapsed()
                                <= PER_PROBE_DEADLINE + Duration::from_millis(750);
                            (
                                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                                axum::Json(serde_json::json!({
                                    "code": tracker.get().code(),
                                    "timedOut": hung,
                                    "withinDeadline": within,
                                })),
                            )
                        }
                    }
                }),
            );
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let app_addr = listener.local_addr().unwrap();
            tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            let client = reqwest::Client::builder().no_proxy().build().unwrap();
            let started = Instant::now();
            let response = client
                .get(format!("http://{app_addr}/api/v1/health/ready"))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
            let body: serde_json::Value = response.json().await.unwrap();
            assert_eq!(body["code"], code.code(), "path={path}");
            assert_eq!(body["timedOut"], true, "path={path}");
            assert_eq!(body["withinDeadline"], true, "path={path}");
            assert!(started.elapsed() <= PER_PROBE_DEADLINE + Duration::from_secs(1));
            handle.abort();
        }
    }
}
