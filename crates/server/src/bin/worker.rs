use std::future::Future;
use std::time::Duration;

use fileconv_server::auth::context::OrgContext;
use fileconv_server::db::pool::create_pool;
use fileconv_server::jobs;
use fileconv_server::services::indexing::OutboxJobSink;
use fileconv_server::services::reconciliation::ReconcileMode;
use fileconv_server::storage::{MinioClient, QdrantClient};
use fileconv_server::telemetry::metrics::MetricsRegistry;
use fileconv_server::workers::convert::{ConvertWorker, ConvertWorkerConfig};
use fileconv_server::workers::delete::{DeleteWorker, DeleteWorkerConfig, DeleteWorkerRun};
use fileconv_server::workers::index::{IndexWorker, IndexWorkerConfig, IndexWorkerRun};
use fileconv_server::workers::limits::ResourceLimits;
use fileconv_server::workers::reconcile::{
    ReconcileWorker, ReconcileWorkerConfig, ReconcileWorkerRun,
};
use fileconv_server::workers::sandbox::SandboxConfig;
use std::sync::Arc;
use uuid::Uuid;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        println!(
            "fileconv-worker\n\nRuns Markhand background job handlers. Configure converter argv with MARKHAND_CONVERTER_ARGV_JSON."
        );
        return;
    }
    match fileconv_server::config::ServerConfig::from_worker_env() {
        Ok(config) if args.iter().any(|argument| argument == "--check-config") => {
            fileconv_server::telemetry::logging::init_tracing(&config);
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
            fileconv_server::telemetry::logging::init_tracing(&config);
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
    let metrics = Arc::new(MetricsRegistry::new());
    match kind.as_str() {
        "convert" => {
            tracing::info!(worker_kind = "convert", "fileconv-worker starting");
            let storage = MinioClient::from_config(storage_config.minio())
                .map_err(|error| format!("storage client failed: {}", error.code()))?;
            run_convert_worker(state, pool, storage, worker_id, ctx, metrics).await
        }
        "index" => {
            tracing::info!(worker_kind = "index", "fileconv-worker starting");
            let storage = MinioClient::from_config(storage_config.minio())
                .map_err(|error| format!("storage client failed: {}", error.code()))?;
            let qdrant = QdrantClient::with_api_key(
                storage_config.qdrant_url(),
                storage_config.qdrant_api_key().cloned(),
            )
            .map_err(|error| format!("qdrant client failed: {}", error.code()))?;
            run_index_worker(state, pool, storage, qdrant, worker_id, ctx, metrics).await
        }
        "delete" => {
            tracing::info!(worker_kind = "delete", "fileconv-worker starting");
            let storage = MinioClient::from_config(storage_config.minio())
                .map_err(|error| format!("storage client failed: {}", error.code()))?;
            let qdrant = QdrantClient::with_api_key(
                storage_config.qdrant_url(),
                storage_config.qdrant_api_key().cloned(),
            )
            .map_err(|error| format!("qdrant client failed: {}", error.code()))?;
            run_delete_worker(state, pool, storage, qdrant, worker_id, ctx, metrics).await
        }
        "reconcile" => {
            tracing::info!(worker_kind = "reconcile", "fileconv-worker starting");
            let storage = MinioClient::from_config(storage_config.minio())
                .map_err(|error| format!("storage client failed: {}", error.code()))?;
            let qdrant = QdrantClient::with_api_key(
                storage_config.qdrant_url(),
                storage_config.qdrant_api_key().cloned(),
            )
            .map_err(|error| format!("qdrant client failed: {}", error.code()))?;
            run_reconcile_worker(state, pool, storage, qdrant, worker_id, ctx, metrics).await
        }
        _ => Err(
            "unknown MARKHAND_WORKER_KIND; expected convert, index, delete, or reconcile".into(),
        ),
    }
}

async fn run_convert_worker(
    state: fileconv_server::state::RuntimeState,
    pool: deadpool_postgres::Pool,
    storage: MinioClient,
    worker_id: String,
    ctx: OrgContext,
    metrics: Arc<MetricsRegistry>,
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
    let worker = ConvertWorker::new(pool, storage, config)
        .map_err(|error| format!("converter worker initialization failed: {error}"))?;
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("fileconv-worker shutdown requested");
                break;
            }
            poll = instrument_worker_once(metrics.clone(), "convert", worker.run_once(&ctx)) => {
                match poll.result {
                    Ok(fileconv_server::workers::convert::ConvertWorkerRun::NoJob) => {
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                    Ok(outcome) => record_convert_run(&metrics, outcome, poll.duration_seconds),
                    Err(error) => {
                        metrics.record_job_processed("convert", "error", poll.duration_seconds);
                        tracing::error!(
                            job_type = "convert",
                            error_code = error.safe_job_error(),
                            "convert worker error"
                        );
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                }
            }
        }
    }
    Ok(())
}

async fn run_index_worker(
    state: fileconv_server::state::RuntimeState,
    pool: deadpool_postgres::Pool,
    storage: MinioClient,
    qdrant: QdrantClient,
    worker_id: String,
    ctx: OrgContext,
    metrics: Arc<MetricsRegistry>,
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
    let worker = IndexWorker::new(pool.clone(), storage, qdrant, config, approved_signature)
        .map_err(|error| format!("index worker initialization failed: {error}"))?;
    let sink = std::sync::Arc::new(OutboxJobSink::new());
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("fileconv-worker shutdown requested");
                break;
            }
            poll = instrument_worker_once(metrics.clone(), "index", async {
                jobs::relay_outbox_with_sink(&pool, &ctx, 32, &sink)
                    .await
                    .map_err(|_| "outbox_relay_error")?;
                worker.run_once(&ctx).await.map_err(|error| error.safe_job_error())
            }) => {
                match poll.result {
                    Ok(IndexWorkerRun::NoJob) => {
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                    Ok(outcome) => record_index_run(&metrics, outcome, poll.duration_seconds),
                    Err(error) => {
                        metrics.record_job_processed("index", "error", poll.duration_seconds);
                        tracing::error!(
                            job_type = "index",
                            error_code = %error,
                            "index worker error"
                        );
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                }
            }
        }
    }
    Ok(())
}

async fn run_delete_worker(
    state: fileconv_server::state::RuntimeState,
    pool: deadpool_postgres::Pool,
    storage: MinioClient,
    qdrant: QdrantClient,
    worker_id: String,
    ctx: OrgContext,
    metrics: Arc<MetricsRegistry>,
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
    let sink = std::sync::Arc::new(OutboxJobSink::new());
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("fileconv-worker shutdown requested");
                break;
            }
            poll = instrument_worker_once(metrics.clone(), "delete", async {
                jobs::relay_outbox_with_sink(&pool, &ctx, 32, &sink)
                    .await
                    .map_err(|_| "outbox_relay_error")?;
                worker.run_once(&ctx).await.map_err(|error| error.safe_job_error())
            }) => {
                match poll.result {
                    Ok(DeleteWorkerRun::NoJob) => {
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                    Ok(outcome) => record_delete_run(&metrics, outcome, poll.duration_seconds),
                    Err(error) => {
                        metrics.record_job_processed("delete", "error", poll.duration_seconds);
                        tracing::error!(
                            job_type = "delete",
                            error_code = %error,
                            "delete worker error"
                        );
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                }
            }
        }
    }
    Ok(())
}

async fn run_reconcile_worker(
    state: fileconv_server::state::RuntimeState,
    pool: deadpool_postgres::Pool,
    storage: MinioClient,
    qdrant: QdrantClient,
    worker_id: String,
    ctx: OrgContext,
    metrics: Arc<MetricsRegistry>,
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
    let sink = std::sync::Arc::new(OutboxJobSink::new());
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("fileconv-worker shutdown requested");
                break;
            }
            poll = instrument_worker_once(metrics.clone(), "reconcile", async {
                jobs::relay_outbox_with_sink(&pool, &ctx, 32, &sink)
                    .await
                    .map_err(|_| "outbox_relay_error")?;
                worker.run_once(&ctx).await.map_err(|error| error.safe_job_error())
            }) => {
                match poll.result {
                    Ok(ReconcileWorkerRun::NoJob) => {
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                    Ok(outcome) => record_reconcile_run(&metrics, outcome, poll.duration_seconds),
                    Err(error) => {
                        metrics.record_job_processed("reconcile", "error", poll.duration_seconds);
                        tracing::error!(
                            job_type = "reconcile",
                            error_code = %error,
                            "reconcile worker error"
                        );
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                }
            }
        }
    }
    Ok(())
}

struct WorkerPoll<T, E> {
    result: Result<T, E>,
    duration_seconds: f64,
}

async fn instrument_worker_once<T, E>(
    metrics: Arc<MetricsRegistry>,
    job_type: &'static str,
    future: impl Future<Output = Result<T, E>>,
) -> WorkerPoll<T, E> {
    let span = tracing::info_span!("worker.job", job_type = job_type);
    let started = std::time::Instant::now();
    metrics.increment_jobs_in_flight();
    let result = {
        use tracing::Instrument;
        future.instrument(span).await
    };
    metrics.decrement_jobs_in_flight();
    WorkerPoll {
        result,
        duration_seconds: started.elapsed().as_secs_f64(),
    }
}

fn record_convert_run(
    metrics: &MetricsRegistry,
    run: fileconv_server::workers::convert::ConvertWorkerRun,
    duration_seconds: f64,
) {
    match run {
        fileconv_server::workers::convert::ConvertWorkerRun::NoJob => {}
        fileconv_server::workers::convert::ConvertWorkerRun::Completed {
            job_id,
            markdown_bytes,
        } => {
            metrics.record_job_processed("convert", "success", duration_seconds);
            tracing::info!(
                job_type = "convert",
                job_id = %job_id,
                markdown_bytes = markdown_bytes,
                outcome = "success",
                "worker job completed"
            );
        }
        fileconv_server::workers::convert::ConvertWorkerRun::Failed { job_id, terminal } => {
            metrics.record_job_processed("convert", "failed", duration_seconds);
            tracing::warn!(
                job_type = "convert",
                job_id = %job_id,
                terminal = terminal,
                outcome = "failed",
                "worker job failed"
            );
        }
        fileconv_server::workers::convert::ConvertWorkerRun::LeaseLost { job_id } => {
            metrics.record_job_processed("convert", "lease_lost", duration_seconds);
            tracing::warn!(
                job_type = "convert",
                job_id = %job_id,
                outcome = "lease_lost",
                "worker job lease lost"
            );
        }
    }
}

fn record_index_run(metrics: &MetricsRegistry, run: IndexWorkerRun, duration_seconds: f64) {
    match run {
        IndexWorkerRun::NoJob => {}
        IndexWorkerRun::Completed { job_id, chunks } => {
            metrics.record_job_processed("index", "success", duration_seconds);
            tracing::info!(
                job_type = "index",
                job_id = %job_id,
                chunks = chunks,
                outcome = "success",
                "worker job completed"
            );
        }
        IndexWorkerRun::Failed { job_id, terminal } => {
            metrics.record_job_processed("index", "failed", duration_seconds);
            tracing::warn!(
                job_type = "index",
                job_id = %job_id,
                terminal = terminal,
                outcome = "failed",
                "worker job failed"
            );
        }
        IndexWorkerRun::LeaseLost { job_id } => {
            metrics.record_job_processed("index", "lease_lost", duration_seconds);
            tracing::warn!(
                job_type = "index",
                job_id = %job_id,
                outcome = "lease_lost",
                "worker job lease lost"
            );
        }
    }
}

fn record_delete_run(metrics: &MetricsRegistry, run: DeleteWorkerRun, duration_seconds: f64) {
    match run {
        DeleteWorkerRun::NoJob => {}
        DeleteWorkerRun::Completed {
            job_id,
            deleted_chunks,
        } => {
            metrics.record_job_processed("delete", "success", duration_seconds);
            tracing::info!(
                job_type = "delete",
                job_id = %job_id,
                deleted_chunks = deleted_chunks,
                outcome = "success",
                "worker job completed"
            );
        }
        DeleteWorkerRun::Failed { job_id, terminal } => {
            metrics.record_job_processed("delete", "failed", duration_seconds);
            tracing::warn!(
                job_type = "delete",
                job_id = %job_id,
                terminal = terminal,
                outcome = "failed",
                "worker job failed"
            );
        }
        DeleteWorkerRun::LeaseLost { job_id } => {
            metrics.record_job_processed("delete", "lease_lost", duration_seconds);
            tracing::warn!(
                job_type = "delete",
                job_id = %job_id,
                outcome = "lease_lost",
                "worker job lease lost"
            );
        }
    }
}

fn record_reconcile_run(metrics: &MetricsRegistry, run: ReconcileWorkerRun, duration_seconds: f64) {
    match run {
        ReconcileWorkerRun::NoJob => {}
        ReconcileWorkerRun::Completed { job_id, report } => {
            metrics.record_job_processed("reconcile", "success", duration_seconds);
            tracing::info!(
                job_type = "reconcile",
                job_id = %job_id,
                orphan_vectors_repaired = report.repaired.orphan_vectors,
                stale_vectors_repaired = report.repaired.stale_vectors,
                staged_objects_repaired = report.repaired.staged_objects,
                outcome = "success",
                "worker job completed"
            );
        }
        ReconcileWorkerRun::Failed { job_id, terminal } => {
            metrics.record_job_processed("reconcile", "failed", duration_seconds);
            tracing::warn!(
                job_type = "reconcile",
                job_id = %job_id,
                terminal = terminal,
                outcome = "failed",
                "worker job failed"
            );
        }
        ReconcileWorkerRun::LeaseLost { job_id } => {
            metrics.record_job_processed("reconcile", "lease_lost", duration_seconds);
            tracing::warn!(
                job_type = "reconcile",
                job_id = %job_id,
                outcome = "lease_lost",
                "worker job lease lost"
            );
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
