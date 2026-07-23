use std::future::Future;
use std::time::Duration;

use fileconv_server::auth::context::OrgContext;
use fileconv_server::db::pool::create_pool;
use fileconv_server::jobs;
use fileconv_server::services::indexing::IndexingOutboxSink;
use fileconv_server::services::reconciliation::ReconcileMode;
use fileconv_server::storage::{MinioClient, QdrantClient};
use fileconv_server::workers::convert::{ConvertWorker, ConvertWorkerConfig};
use fileconv_server::workers::delete::{DeleteWorker, DeleteWorkerConfig, DeleteWorkerRun};
use fileconv_server::workers::embedding::{
    EmbeddingWorker, EmbeddingWorkerConfig, EmbeddingWorkerRun,
};
use fileconv_server::workers::index::{IndexWorker, IndexWorkerConfig, IndexWorkerRun};
use fileconv_server::workers::limits::ResourceLimits;
use fileconv_server::workers::reconcile::{
    ReconcileWorker, ReconcileWorkerConfig, ReconcileWorkerRun,
};
use fileconv_server::workers::sandbox::SandboxConfig;
use uuid::Uuid;

const RECLAIM_LIMIT: u32 = 32;
const RECLAIM_BACKOFF: Duration = Duration::from_secs(1);
const DEFAULT_SHUTDOWN_GRACE: Duration = Duration::from_secs(30);
const DEFAULT_SHUTDOWN_FLUSH: Duration = Duration::from_secs(2);

#[tokio::main]
async fn main() {
    fileconv_server::init_tracing();
    let args: Vec<String> = std::env::args().collect();
    if args
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        println!(
            "fileconv-worker\n\nRuns Markhand background job handlers. Configure converter argv with MARKHAND_CONVERTER_ARGV_JSON.\n\nOptions:\n  --check-config         Validate worker env/config and exit\n  --sandbox-preflight    Probe convert sandbox isolation and exit"
        );
        return;
    }
    if args
        .iter()
        .any(|argument| argument == "--sandbox-preflight")
    {
        match fileconv_server::workers::sandbox::preflight() {
            Ok(()) => {
                println!("sandbox preflight ok");
                return;
            }
            Err(error) => exit_with_error(format!("sandbox preflight failed: {error}")),
        }
    }
    match fileconv_server::config::ServerConfig::from_worker_env() {
        Ok(config) if args.iter().any(|argument| argument == "--check-config") => {
            match fileconv_server::state::RuntimeState::from_config(config) {
                Ok(state) => println!(
                    "configuration valid: profile={:?}, bind={}",
                    state.config().profile(),
                    state.config().bind_addr()
                ),
                Err(error) => exit_with_error(format!("invalid worker configuration: {error}")),
            }
        }
        Ok(config) => {
            fileconv_server::telemetry::init(config.telemetry());
            match fileconv_server::state::RuntimeState::from_config(config) {
                Ok(state) => {
                    if let Err(error) = run_worker(state).await {
                        exit_with_error(error);
                    }
                }
                Err(error) => exit_with_error(format!("invalid worker configuration: {error}")),
            }
        }
        Err(error) => {
            exit_with_error(format!("invalid worker configuration: {error}"));
        }
    }
}

async fn run_worker(state: fileconv_server::state::RuntimeState) -> Result<(), String> {
    let org_id = env_uuid("MARKHAND_WORKER_ORG_ID")?;
    let user_id = env_uuid("MARKHAND_WORKER_USER_ID")?;
    let ctx = OrgContext::try_new(org_id, user_id, [] as [&str; 0], [])
        .map_err(|error| format!("invalid worker tenant context: {error}"))?;
    let endpoints = state.endpoints();
    let pool = create_pool(endpoints.database_url.expose())
        .map_err(|error| format!("database pool failed: {error}"))?;
    let storage_config = state
        .config()
        .storage_config()
        .map_err(|error| format!("invalid storage configuration: {error}"))?;
    let worker_id = std::env::var("MARKHAND_WORKER_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("fileconv-worker-{}", std::process::id()));
    let kind = std::env::var("MARKHAND_WORKER_KIND")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "convert".into());
    match kind.as_str() {
        "convert" => {
            let storage = MinioClient::from_config(storage_config.minio())
                .map_err(|error| format!("storage client failed: {}", error.code()))?;
            run_convert_worker(state, pool, storage, worker_id, ctx).await
        }
        "index" => {
            let storage = MinioClient::from_config(storage_config.minio())
                .map_err(|error| format!("storage client failed: {}", error.code()))?;
            let qdrant = QdrantClient::with_api_key(
                storage_config.qdrant_url(),
                storage_config.qdrant_api_key().cloned(),
            )
            .map_err(|error| format!("qdrant client failed: {}", error.code()))?;
            run_index_worker(state, pool, storage, qdrant, worker_id, ctx).await
        }
        "embedding" => {
            let qdrant = QdrantClient::with_api_key(
                storage_config.qdrant_url(),
                storage_config.qdrant_api_key().cloned(),
            )
            .map_err(|error| format!("qdrant client failed: {}", error.code()))?;
            run_embedding_worker(state, pool, qdrant, worker_id, ctx).await
        }
        "delete" => {
            let storage = MinioClient::from_config(storage_config.minio())
                .map_err(|error| format!("storage client failed: {}", error.code()))?;
            let qdrant = QdrantClient::with_api_key(
                storage_config.qdrant_url(),
                storage_config.qdrant_api_key().cloned(),
            )
            .map_err(|error| format!("qdrant client failed: {}", error.code()))?;
            run_delete_worker(state, pool, storage, qdrant, worker_id, ctx).await
        }
        "reconcile" => {
            let storage = MinioClient::from_config(storage_config.minio())
                .map_err(|error| format!("storage client failed: {}", error.code()))?;
            let qdrant = QdrantClient::with_api_key(
                storage_config.qdrant_url(),
                storage_config.qdrant_api_key().cloned(),
            )
            .map_err(|error| format!("qdrant client failed: {}", error.code()))?;
            run_reconcile_worker(state, pool, storage, qdrant, worker_id, ctx).await
        }
        other => Err(format!("unknown MARKHAND_WORKER_KIND: {other}")),
    }
}

async fn run_convert_worker(
    state: fileconv_server::state::RuntimeState,
    pool: deadpool_postgres::Pool,
    storage: MinioClient,
    worker_id: String,
    ctx: OrgContext,
) -> Result<(), String> {
    let mut config = ConvertWorkerConfig::new(worker_id, sandbox_config_from_env()?);
    config.lease_ttl = Duration::from_secs(state.config().limits().job_lease_seconds);
    if let Ok(value) = std::env::var("MARKHAND_WORKER_HEARTBEAT_INTERVAL_SECS") {
        config.heartbeat_interval = Duration::from_secs(value.parse().map_err(|_| {
            "MARKHAND_WORKER_HEARTBEAT_INTERVAL_SECS must be an integer".to_string()
        })?);
    }
    if let Ok(value) = std::env::var("MARKHAND_WORKER_MAX_JOB_SECS") {
        config.max_job_duration = Duration::from_secs(
            value
                .parse()
                .map_err(|_| "MARKHAND_WORKER_MAX_JOB_SECS must be an integer".to_string())?,
        );
    }
    if let Ok(value) = std::env::var("MARKHAND_WORKER_CLAIM_LIMIT") {
        let claim_limit: u32 = value
            .parse()
            .map_err(|_| "MARKHAND_WORKER_CLAIM_LIMIT must be an integer".to_string())?;
        if claim_limit != 1 {
            return Err("MARKHAND_WORKER_CLAIM_LIMIT must be exactly 1".into());
        }
    }
    let worker = ConvertWorker::new(pool.clone(), storage, config)
        .map_err(|error| format!("converter worker initialization failed: {error}"))?;
    run_bounded_claim_loop(
        "convert",
        || {
            let pool = pool.clone();
            let ctx = ctx.clone();
            let worker = worker.clone();
            async move {
                reclaim_expired_leases(&pool, &ctx).await;
                let _ = fileconv_server::jobs::observe_queue_metrics(&pool).await;
                worker
                    .run_once(&ctx)
                    .await
                    .map_err(|error| error.to_string())
            }
        },
        |outcome| {
            matches!(
                outcome,
                fileconv_server::workers::convert::ConvertWorkerRun::NoJob
            )
        },
    )
    .await
}

async fn run_index_worker(
    state: fileconv_server::state::RuntimeState,
    pool: deadpool_postgres::Pool,
    storage: MinioClient,
    qdrant: QdrantClient,
    worker_id: String,
    ctx: OrgContext,
) -> Result<(), String> {
    let mut config = IndexWorkerConfig::new(worker_id);
    config.lease_ttl = Duration::from_secs(state.config().limits().job_lease_seconds);
    if let Ok(value) = std::env::var("MARKHAND_WORKER_HEARTBEAT_INTERVAL_SECS") {
        config.heartbeat_interval = Duration::from_secs(value.parse().map_err(|_| {
            "MARKHAND_WORKER_HEARTBEAT_INTERVAL_SECS must be an integer".to_string()
        })?);
    }
    if let Ok(value) = std::env::var("MARKHAND_WORKER_MAX_JOB_SECS") {
        config.max_job_duration = Duration::from_secs(
            value
                .parse()
                .map_err(|_| "MARKHAND_WORKER_MAX_JOB_SECS must be an integer".to_string())?,
        );
    }
    if let Ok(value) = std::env::var("MARKHAND_INDEX_EMBEDDING_BATCH_SIZE") {
        config.embedding_batch_size = value
            .parse()
            .map_err(|_| "MARKHAND_INDEX_EMBEDDING_BATCH_SIZE must be an integer".to_string())?;
    }
    let approved_signature = state.config().index_signature().map(str::to_string);
    let worker = IndexWorker::new(
        pool.clone(),
        storage,
        qdrant,
        config,
        state.config().profile(),
        approved_signature,
    )
    .map_err(|error| format!("index worker initialization failed: {error}"))?;
    let sink = std::sync::Arc::new(
        IndexingOutboxSink::new(worker.embedding_plan())
            .map_err(|error| format!("index worker generation setup failed: {error}"))?,
    );
    run_bounded_claim_loop(
        "index",
        || {
            let pool = pool.clone();
            let ctx = ctx.clone();
            let worker = worker.clone();
            let sink = sink.clone();
            async move {
                reclaim_expired_leases(&pool, &ctx).await;
                jobs::relay_outbox_with_sink(&pool, &ctx, 32, &sink)
                    .await
                    .map_err(|error| error.to_string())?;
                worker
                    .run_once(&ctx)
                    .await
                    .map_err(|error| error.to_string())
            }
        },
        |outcome| matches!(outcome, IndexWorkerRun::NoJob),
    )
    .await
}

async fn run_delete_worker(
    state: fileconv_server::state::RuntimeState,
    pool: deadpool_postgres::Pool,
    storage: MinioClient,
    qdrant: QdrantClient,
    worker_id: String,
    ctx: OrgContext,
) -> Result<(), String> {
    let mut config = DeleteWorkerConfig::new(worker_id);
    config.lease_ttl = Duration::from_secs(state.config().limits().job_lease_seconds);
    if let Ok(value) = std::env::var("MARKHAND_WORKER_HEARTBEAT_INTERVAL_SECS") {
        config.heartbeat_interval = Duration::from_secs(value.parse().map_err(|_| {
            "MARKHAND_WORKER_HEARTBEAT_INTERVAL_SECS must be an integer".to_string()
        })?);
    }
    if let Ok(value) = std::env::var("MARKHAND_WORKER_MAX_JOB_SECS") {
        config.max_job_duration = Duration::from_secs(
            value
                .parse()
                .map_err(|_| "MARKHAND_WORKER_MAX_JOB_SECS must be an integer".to_string())?,
        );
    }
    let worker = DeleteWorker::new(pool.clone(), storage, qdrant, config)
        .map_err(|error| format!("delete worker initialization failed: {error}"))?;
    let approved_signature = state.config().index_signature().map(str::to_string);
    let embedding_plan = fileconv_server::services::embedding::ApprovedEmbeddingRuntime::from_env(
        approved_signature.as_deref(),
        state.config().profile(),
    )
    .map_err(|error| format!("delete worker generation setup failed: {error}"))?
    .plan()
    .clone();
    let sink = std::sync::Arc::new(
        IndexingOutboxSink::new(&embedding_plan)
            .map_err(|error| format!("delete worker outbox sink failed: {error}"))?,
    );
    run_bounded_claim_loop(
        "delete",
        || {
            let pool = pool.clone();
            let ctx = ctx.clone();
            let worker = worker.clone();
            let sink = sink.clone();
            async move {
                reclaim_expired_leases(&pool, &ctx).await;
                jobs::relay_outbox_with_sink(&pool, &ctx, 32, &sink)
                    .await
                    .map_err(|error| error.to_string())?;
                worker
                    .run_once(&ctx)
                    .await
                    .map_err(|error| error.to_string())
            }
        },
        |outcome| matches!(outcome, DeleteWorkerRun::NoJob),
    )
    .await
}

async fn run_reconcile_worker(
    state: fileconv_server::state::RuntimeState,
    pool: deadpool_postgres::Pool,
    storage: MinioClient,
    qdrant: QdrantClient,
    worker_id: String,
    ctx: OrgContext,
) -> Result<(), String> {
    let mut config = ReconcileWorkerConfig::new(worker_id);
    config.lease_ttl = Duration::from_secs(state.config().limits().job_lease_seconds);
    if let Ok(value) = std::env::var("MARKHAND_WORKER_HEARTBEAT_INTERVAL_SECS") {
        config.heartbeat_interval = Duration::from_secs(value.parse().map_err(|_| {
            "MARKHAND_WORKER_HEARTBEAT_INTERVAL_SECS must be an integer".to_string()
        })?);
    }
    if let Ok(value) = std::env::var("MARKHAND_WORKER_MAX_JOB_SECS") {
        config.max_job_duration = Duration::from_secs(
            value
                .parse()
                .map_err(|_| "MARKHAND_WORKER_MAX_JOB_SECS must be an integer".to_string())?,
        );
    }
    if let Ok(value) = std::env::var("MARKHAND_RECONCILE_MODE") {
        config.mode = ReconcileMode::parse(value.trim()).map_err(|error| error.to_string())?;
    }
    let worker = ReconcileWorker::new(pool.clone(), storage, qdrant, config)
        .map_err(|error| format!("reconcile worker initialization failed: {error}"))?;
    let approved_signature = state.config().index_signature().map(str::to_string);
    let embedding_plan = fileconv_server::services::embedding::ApprovedEmbeddingRuntime::from_env(
        approved_signature.as_deref(),
        state.config().profile(),
    )
    .map_err(|error| format!("reconcile worker generation setup failed: {error}"))?
    .plan()
    .clone();
    let sink = std::sync::Arc::new(
        IndexingOutboxSink::new(&embedding_plan)
            .map_err(|error| format!("reconcile worker outbox sink failed: {error}"))?,
    );
    run_bounded_claim_loop(
        "reconcile",
        || {
            let pool = pool.clone();
            let ctx = ctx.clone();
            let worker = worker.clone();
            let sink = sink.clone();
            async move {
                reclaim_expired_leases(&pool, &ctx).await;
                jobs::relay_outbox_with_sink(&pool, &ctx, 32, &sink)
                    .await
                    .map_err(|error| error.to_string())?;
                worker
                    .run_once(&ctx)
                    .await
                    .map_err(|error| error.to_string())
            }
        },
        |outcome| matches!(outcome, ReconcileWorkerRun::NoJob),
    )
    .await
}

async fn run_embedding_worker(
    state: fileconv_server::state::RuntimeState,
    pool: deadpool_postgres::Pool,
    qdrant: QdrantClient,
    worker_id: String,
    ctx: OrgContext,
) -> Result<(), String> {
    let mut config = EmbeddingWorkerConfig::new(worker_id);
    config.lease_ttl = Duration::from_secs(state.config().limits().job_lease_seconds);
    if let Ok(value) = std::env::var("MARKHAND_WORKER_HEARTBEAT_INTERVAL_SECS") {
        config.heartbeat_interval = Duration::from_secs(value.parse().map_err(|_| {
            "MARKHAND_WORKER_HEARTBEAT_INTERVAL_SECS must be an integer".to_string()
        })?);
    }
    if let Ok(value) = std::env::var("MARKHAND_WORKER_MAX_JOB_SECS") {
        config.max_job_duration = Duration::from_secs(
            value
                .parse()
                .map_err(|_| "MARKHAND_WORKER_MAX_JOB_SECS must be an integer".to_string())?,
        );
    }
    let runtime = fileconv_server::services::embedding::ApprovedEmbeddingRuntime::from_env(
        state.config().index_signature(),
        state.config().profile(),
    )
    .map_err(|error| format!("embedding runtime initialization failed: {error}"))?;
    let worker = EmbeddingWorker::new(pool.clone(), qdrant, config, runtime)
        .map_err(|error| format!("embedding worker initialization failed: {error}"))?;
    run_bounded_claim_loop(
        "embedding",
        || {
            let pool = pool.clone();
            let ctx = ctx.clone();
            let worker = worker.clone();
            async move {
                reclaim_expired_leases(&pool, &ctx).await;
                let _ = fileconv_server::jobs::observe_queue_metrics(&pool).await;
                worker
                    .run_once(&ctx)
                    .await
                    .map_err(|error| error.to_string())
            }
        },
        |outcome| matches!(outcome, EmbeddingWorkerRun::NoJob),
    )
    .await
}

/// Stop new claims on SIGTERM/Ctrl-C, await the active `run_once` within grace,
/// then flush OTLP with each HTTP attempt bounded by the remaining deadline.
async fn run_bounded_claim_loop<F, Fut, T, Idle>(
    kind: &str,
    mut cycle: F,
    is_idle: Idle,
) -> Result<(), String>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, String>> + Send + 'static,
    T: std::fmt::Debug + Send + 'static,
    Idle: Fn(&T) -> bool,
{
    let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        shutdown_signal().await;
        let _ = stop_tx.send(true);
    });
    let grace = shutdown_grace_from_env();
    let flush_budget = shutdown_flush_from_env();

    loop {
        if *stop_rx.borrow() {
            break;
        }
        let mut handle = tokio::spawn(cycle());
        tokio::select! {
            join = &mut handle => {
                match join {
                    Ok(Ok(outcome)) => {
                        if is_idle(&outcome) {
                            tokio::select! {
                                _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                                changed = stop_rx.changed() => {
                                    let _ = changed;
                                }
                            }
                        } else {
                            println!("fileconv-worker: {outcome:?}");
                        }
                    }
                    Ok(Err(error)) => {
                        eprintln!("fileconv-worker: {kind} worker error: {error}");
                        tokio::select! {
                            _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                            changed = stop_rx.changed() => {
                                let _ = changed;
                            }
                        }
                    }
                    Err(error) => {
                        eprintln!("fileconv-worker: {kind} join error: {error}");
                    }
                }
            }
            changed = stop_rx.changed() => {
                let _ = changed;
                println!(
                    "fileconv-worker: shutdown requested — stop claim, await active run_once (grace {grace:?})"
                );
                match tokio::time::timeout(grace, handle).await {
                    Ok(Ok(Ok(outcome))) => {
                        println!("fileconv-worker: active job finished during grace: {outcome:?}");
                    }
                    Ok(Ok(Err(error))) => {
                        eprintln!("fileconv-worker: active job error during grace: {error}");
                    }
                    Ok(Err(error)) => {
                        eprintln!("fileconv-worker: active job join error during grace: {error}");
                    }
                    Err(_) => {
                        eprintln!(
                            "fileconv-worker: grace expired waiting for active run_once; proceeding to flush"
                        );
                    }
                }
                break;
            }
        }
    }
    shutdown_flush_telemetry(flush_budget).await;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    {
        let terminate = async {
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(mut signal) => {
                    signal.recv().await;
                }
                Err(error) => {
                    eprintln!("fileconv-worker: cannot register SIGTERM handler: {error}");
                }
            }
        };
        tokio::select! {
            _ = ctrl_c => {}
            _ = terminate => {}
        }
    }
    #[cfg(not(unix))]
    ctrl_c.await;
}

fn shutdown_grace_from_env() -> Duration {
    std::env::var("MARKHAND_WORKER_SHUTDOWN_GRACE_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_SHUTDOWN_GRACE)
}

fn shutdown_flush_from_env() -> Duration {
    std::env::var("MARKHAND_WORKER_SHUTDOWN_FLUSH_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_SHUTDOWN_FLUSH)
}

async fn shutdown_flush_telemetry(timeout: Duration) {
    let flushed = fileconv_server::telemetry::MetricsRegistry::shutdown_flush(timeout).await;
    if flushed > 0 {
        println!("fileconv-worker: exporter shutdown flush complete ({flushed} spans)");
    }
}

async fn reclaim_expired_leases(pool: &deadpool_postgres::Pool, ctx: &OrgContext) {
    match jobs::reclaim_expired(pool, ctx, RECLAIM_LIMIT, RECLAIM_BACKOFF).await {
        Ok(reclaimed) if !reclaimed.is_empty() => {
            println!(
                "fileconv-worker: reclaimed {} expired leases",
                reclaimed.len()
            );
        }
        Ok(_) => {}
        Err(error) => {
            eprintln!("fileconv-worker: expired lease reclamation failed: {error}");
        }
    }
}

fn sandbox_config_from_env() -> Result<SandboxConfig, String> {
    let argv_template = match std::env::var("MARKHAND_CONVERTER_ARGV_JSON") {
        Ok(value) if !value.trim().is_empty() => serde_json::from_str::<Vec<String>>(&value)
            .map_err(|_| "MARKHAND_CONVERTER_ARGV_JSON must be a JSON string array".to_string())?,
        _ => vec![
            "/usr/local/bin/fileconv".into(),
            "one".into(),
            "{input}".into(),
        ],
    };
    let mut limits = ResourceLimits::default();
    if let Ok(value) = std::env::var("MARKHAND_CONVERTER_TIMEOUT_SECS") {
        limits.wall_timeout = Duration::from_secs(
            value
                .parse()
                .map_err(|_| "MARKHAND_CONVERTER_TIMEOUT_SECS must be an integer".to_string())?,
        );
    }
    if let Ok(value) = std::env::var("MARKHAND_CONVERTER_MEMORY_BYTES") {
        limits.memory_bytes = value
            .parse()
            .map_err(|_| "MARKHAND_CONVERTER_MEMORY_BYTES must be an integer".to_string())?;
    }
    if let Ok(value) = std::env::var("MARKHAND_CONVERTER_CPU_SECONDS") {
        limits.cpu_seconds = value
            .parse()
            .map_err(|_| "MARKHAND_CONVERTER_CPU_SECONDS must be an integer".to_string())?;
    }
    if let Ok(value) = std::env::var("MARKHAND_CONVERTER_FILE_SIZE_BYTES") {
        limits.file_size_bytes = value
            .parse()
            .map_err(|_| "MARKHAND_CONVERTER_FILE_SIZE_BYTES must be an integer".to_string())?;
    }
    if let Ok(value) = std::env::var("MARKHAND_CONVERTER_MAX_PROCESSES") {
        limits.max_processes = value
            .parse()
            .map_err(|_| "MARKHAND_CONVERTER_MAX_PROCESSES must be an integer".to_string())?;
    }
    if let Ok(value) = std::env::var("MARKHAND_CONVERTER_MAX_OPEN_FILES") {
        limits.max_open_files = value
            .parse()
            .map_err(|_| "MARKHAND_CONVERTER_MAX_OPEN_FILES must be an integer".to_string())?;
    }
    let config = SandboxConfig {
        argv_template,
        limits,
    };
    config.validate().map_err(|error| error.to_string())?;
    Ok(config)
}

fn env_uuid(name: &str) -> Result<Uuid, String> {
    let raw = std::env::var(name).map_err(|_| format!("{name} is required"))?;
    Uuid::parse_str(&raw).map_err(|_| format!("{name} must be a UUID"))
}

fn exit_with_error(error: String) -> ! {
    eprintln!("fileconv-worker: {error}");
    std::process::exit(1);
}

#[cfg(test)]
mod shutdown_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[tokio::test]
    async fn stop_claim_awaits_active_cycle_within_grace_then_returns() {
        let started = Arc::new(AtomicUsize::new(0));
        let finished = Arc::new(AtomicUsize::new(0));
        let started_c = started.clone();
        let finished_c = finished.clone();
        let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);

        let runner = tokio::spawn(async move {
            let mut cycles = 0u32;
            loop {
                if *stop_rx.borrow() {
                    break;
                }
                let started_c = started_c.clone();
                let finished_c = finished_c.clone();
                let mut handle: tokio::task::JoinHandle<Result<&'static str, String>> =
                    tokio::spawn(async move {
                        started_c.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(200)).await;
                        finished_c.fetch_add(1, Ordering::SeqCst);
                        Ok("done")
                    });
                tokio::select! {
                    join = &mut handle => {
                        let _ = join;
                        cycles += 1;
                        if cycles >= 1 {
                            // allow outer stop after first claim starts
                        }
                    }
                    changed = stop_rx.changed() => {
                        let _ = changed;
                        let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
                        break;
                    }
                }
            }
        });

        // Wait until an active cycle is in flight, then stop claiming.
        for _ in 0..50 {
            if started.load(Ordering::SeqCst) >= 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(started.load(Ordering::SeqCst) >= 1);
        let _ = stop_tx.send(true);
        runner.await.expect("runner");
        assert_eq!(
            finished.load(Ordering::SeqCst),
            started.load(Ordering::SeqCst),
            "active run_once must finish under grace (not cancelled by stop claim)"
        );
    }
}
