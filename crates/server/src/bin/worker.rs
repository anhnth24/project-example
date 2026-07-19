use std::time::Duration;

use fileconv_server::auth::context::OrgContext;
use fileconv_server::db::pool::create_pool;
use fileconv_server::jobs;
use fileconv_server::services::indexing::OutboxJobSink;
use fileconv_server::services::reconciliation::ReconcileMode;
use fileconv_server::storage::{MinioClient, QdrantClient};
use fileconv_server::workers::convert::{ConvertWorker, ConvertWorkerConfig};
use fileconv_server::workers::delete::{DeleteWorker, DeleteWorkerConfig, DeleteWorkerRun};
use fileconv_server::workers::index::{IndexWorker, IndexWorkerConfig, IndexWorkerRun};
use fileconv_server::workers::limits::ResourceLimits;
use fileconv_server::workers::reconcile::{
    ReconcileWorker, ReconcileWorkerConfig, ReconcileWorkerRun,
};
use fileconv_server::workers::sandbox::SandboxConfig;
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
            match fileconv_server::state::RuntimeState::from_config(config) {
                Ok(state) => println!(
                    "configuration valid: profile={:?}, bind={}",
                    state.config().profile(),
                    state.config().bind_addr()
                ),
                Err(error) => exit_with_error(format!("invalid worker configuration: {error}")),
            }
        }
        Ok(config) => match fileconv_server::state::RuntimeState::from_config(config) {
            Ok(state) => {
                if let Err(error) = run_worker(state).await {
                    exit_with_error(error);
                }
            }
            Err(error) => exit_with_error(format!("invalid worker configuration: {error}")),
        },
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
    let worker = ConvertWorker::new(pool, storage, config)
        .map_err(|error| format!("converter worker initialization failed: {error}"))?;
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                println!("fileconv-worker: shutdown requested");
                break;
            }
            result = worker.run_once(&ctx) => {
                match result {
                    Ok(fileconv_server::workers::convert::ConvertWorkerRun::NoJob) => {
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                    Ok(outcome) => println!("fileconv-worker: {outcome:?}"),
                    Err(error) => {
                        eprintln!("fileconv-worker: convert worker error: {error}");
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
                println!("fileconv-worker: shutdown requested");
                break;
            }
            result = async {
                jobs::relay_outbox_with_sink(&pool, &ctx, 32, &sink)
                    .await
                    .map_err(|error| error.to_string())?;
                worker.run_once(&ctx).await.map_err(|error| error.to_string())
            } => {
                match result {
                    Ok(IndexWorkerRun::NoJob) => {
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                    Ok(outcome) => println!("fileconv-worker: {outcome:?}"),
                    Err(error) => {
                        eprintln!("fileconv-worker: index worker error: {error}");
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
                println!("fileconv-worker: shutdown requested");
                break;
            }
            result = async {
                jobs::relay_outbox_with_sink(&pool, &ctx, 32, &sink)
                    .await
                    .map_err(|error| error.to_string())?;
                worker.run_once(&ctx).await.map_err(|error| error.to_string())
            } => {
                match result {
                    Ok(DeleteWorkerRun::NoJob) => {
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                    Ok(outcome) => println!("fileconv-worker: {outcome:?}"),
                    Err(error) => {
                        eprintln!("fileconv-worker: delete worker error: {error}");
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
                println!("fileconv-worker: shutdown requested");
                break;
            }
            result = async {
                jobs::relay_outbox_with_sink(&pool, &ctx, 32, &sink)
                    .await
                    .map_err(|error| error.to_string())?;
                worker.run_once(&ctx).await.map_err(|error| error.to_string())
            } => {
                match result {
                    Ok(ReconcileWorkerRun::NoJob) => {
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                    Ok(outcome) => println!("fileconv-worker: {outcome:?}"),
                    Err(error) => {
                        eprintln!("fileconv-worker: reconcile worker error: {error}");
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                }
            }
        }
    }
    Ok(())
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
