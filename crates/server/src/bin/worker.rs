use std::future::Future;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
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
use fileconv_server::workers::fairness::OrgRotation;
use fileconv_server::workers::index::{IndexWorker, IndexWorkerConfig, IndexWorkerRun};
use fileconv_server::workers::limits::ResourceLimits;
use fileconv_server::workers::reconcile::{
    ReconcileWorker, ReconcileWorkerConfig, ReconcileWorkerRun,
};
use fileconv_server::workers::sandbox::{SandboxCancel, SandboxConfig, SandboxInput};
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
            "fileconv-worker\n\nRuns Markhand background job handlers. Configure converter argv with MARKHAND_CONVERTER_ARGV_JSON.\n\nOptions:\n  --check-config                    Validate worker env/config and exit\n  --db-role-probe                   Query pg_roles/current_user via worker DB URL and exit\n  --sandbox-preflight               Probe convert sandbox isolation and exit\n  --sandbox-convert-probe <file>    Convert one file through the production sandbox and exit"
        );
        return;
    }
    match sandbox_convert_probe_arg(&args) {
        Ok(Some(path)) => match run_sandbox_convert_probe(Path::new(path)) {
            Ok(()) => return,
            Err(error) => exit_with_error(format!("sandbox conversion probe failed: {error}")),
        },
        Ok(None) => {}
        Err(error) => exit_with_error(error),
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
    if args.iter().any(|argument| argument == "--db-role-probe") {
        match fileconv_server::config::ServerConfig::from_worker_env() {
            Ok(config) => {
                if let Err(error) = run_db_role_probe(&config).await {
                    exit_with_error(error);
                }
                return;
            }
            Err(error) => exit_with_error(format!("invalid worker configuration: {error}")),
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

async fn run_db_role_probe(config: &fileconv_server::config::ServerConfig) -> Result<(), String> {
    let endpoints = config
        .runtime_endpoints()
        .map_err(|error| error.to_string())?;
    let database_url = endpoints.database_url.expose();
    let pool = create_pool(database_url).map_err(|error| error.to_string())?;
    let client = pool.get().await.map_err(|error| error.to_string())?;
    let row = client
        .query_one(
            "SELECT current_user::text, rolsuper, rolbypassrls
             FROM pg_roles
             WHERE rolname = current_user",
            &[],
        )
        .await
        .map_err(|error| error.to_string())?;
    let current_user: String = row.get(0);
    let rolsuper: bool = row.get(1);
    let rolbypassrls: bool = row.get(2);
    let nonce = Uuid::new_v4().simple().to_string();
    let payload = serde_json::json!({
        "schemaVersion": 1,
        "currentUser": current_user,
        "superuser": rolsuper,
        "bypassRls": rolbypassrls,
        "dedicatedDatabaseUrlVerified": database_url.contains("markhand_worker"),
        "databaseUrlRolePath": "markhand_worker",
        "nonce": nonce,
    });
    let encoded = serde_json::to_string(&payload).map_err(|error| error.to_string())?;
    println!("PHASE1C_WORKER_ROLE_PROBE\t{encoded}");
    println!("PHASE1C_WORKER_ROLE_PROBE_EOF\ttrue");
    if current_user != "markhand_worker" {
        return Err(format!(
            "worker runtime role must be markhand_worker, got {current_user}"
        ));
    }
    if rolsuper {
        return Err("worker runtime role must not be superuser".to_string());
    }
    if rolbypassrls {
        return Err("worker runtime role must not bypass RLS".to_string());
    }
    Ok(())
}

fn sandbox_convert_probe_arg(args: &[String]) -> Result<Option<&str>, String> {
    let matches = args
        .iter()
        .enumerate()
        .filter(|(_, argument)| argument.as_str() == "--sandbox-convert-probe")
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Ok(None),
        [index] => {
            let path = args
                .get(index + 1)
                .filter(|value| !value.is_empty() && !value.starts_with('-'))
                .ok_or_else(|| "--sandbox-convert-probe requires one file path".to_string())?;
            if args.len() != index + 2 {
                return Err(
                    "--sandbox-convert-probe cannot be combined with other arguments".to_string(),
                );
            }
            Ok(Some(path))
        }
        _ => Err("--sandbox-convert-probe may be specified only once".to_string()),
    }
}

fn run_sandbox_convert_probe(path: &Path) -> Result<(), String> {
    let canonical_extension = path
        .extension()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "input file must have an extension".to_string())?
        .to_ascii_lowercase();
    let bytes = std::fs::read(path)
        .map_err(|error| format!("cannot read probe input {}: {error}", path.display()))?;
    let output = fileconv_server::workers::sandbox::run(
        &sandbox_config_from_env()?,
        SandboxInput {
            bytes,
            canonical_extension,
        },
        &SandboxCancel::default(),
    )
    .map_err(|error| error.to_string())?;
    if !output.exit.success() {
        return Err(format!(
            "converter exit={:?}; stderr={}",
            output.exit,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    if output.stdout_truncated || output.stderr_truncated {
        return Err("converter output was truncated".to_string());
    }
    std::io::stdout()
        .write_all(&output.stdout)
        .map_err(|error| format!("cannot write probe output: {error}"))?;
    Ok(())
}

async fn run_worker(state: fileconv_server::state::RuntimeState) -> Result<(), String> {
    // `MARKHAND_WORKER_ORG_ID` accepts one UUID (legacy single-org pin) or a
    // comma-separated list. With several orgs, each claim cycle round-robins
    // across them (1C-10): one org's giant backlog cannot starve the others
    // because the rotation serves at most one job per org turn.
    let org_ids = env_uuid_list("MARKHAND_WORKER_ORG_ID")?;
    let user_id = env_uuid("MARKHAND_WORKER_USER_ID")?;
    let kind = std::env::var("MARKHAND_WORKER_KIND")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "convert".into());
    // Validate kind → permissions before any OrgContext is constructed so an
    // unknown MARKHAND_WORKER_KIND cannot start or claim work.
    let contexts = worker_contexts_for_kind(&org_ids, user_id, &kind)?;
    let rotation = Arc::new(OrgRotation::new(contexts)?);
    let worker_id = std::env::var("MARKHAND_WORKER_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("fileconv-worker-{}", std::process::id()));
    // Reconcile oneshot: require a valid document UUID *before* opening a DB pool
    // so missing/empty/malformed IDs exit without contacting Postgres.
    let oneshot = env_truthy("MARKHAND_WORKER_ONESHOT");
    if kind == "reconcile" && oneshot {
        let _ = require_oneshot_reconcile_document_id()?;
        if rotation.len() != 1 {
            return Err(
                "reconcile oneshot requires exactly one MARKHAND_WORKER_ORG_ID".to_string(),
            );
        }
    }
    let endpoints = state.endpoints();
    let pool = create_pool(endpoints.database_url.expose())
        .map_err(|error| format!("database pool failed: {error}"))?;
    let storage_config = state
        .config()
        .storage_config()
        .map_err(|error| format!("invalid storage configuration: {error}"))?;
    match kind.as_str() {
        "convert" => {
            let storage = MinioClient::from_config(storage_config.minio())
                .map_err(|error| format!("storage client failed: {}", error.code()))?;
            run_convert_worker(state, pool, storage, worker_id, rotation).await
        }
        "index" => {
            let storage = MinioClient::from_config(storage_config.minio())
                .map_err(|error| format!("storage client failed: {}", error.code()))?;
            let qdrant = QdrantClient::with_api_key(
                storage_config.qdrant_url(),
                storage_config.qdrant_api_key().cloned(),
            )
            .map_err(|error| format!("qdrant client failed: {}", error.code()))?;
            run_index_worker(state, pool, storage, qdrant, worker_id, rotation).await
        }
        "embedding" => {
            let qdrant = QdrantClient::with_api_key(
                storage_config.qdrant_url(),
                storage_config.qdrant_api_key().cloned(),
            )
            .map_err(|error| format!("qdrant client failed: {}", error.code()))?;
            run_embedding_worker(state, pool, qdrant, worker_id, rotation).await
        }
        "delete" => {
            let storage = MinioClient::from_config(storage_config.minio())
                .map_err(|error| format!("storage client failed: {}", error.code()))?;
            let qdrant = QdrantClient::with_api_key(
                storage_config.qdrant_url(),
                storage_config.qdrant_api_key().cloned(),
            )
            .map_err(|error| format!("qdrant client failed: {}", error.code()))?;
            run_delete_worker(state, pool, storage, qdrant, worker_id, rotation).await
        }
        "reconcile" => {
            let storage = MinioClient::from_config(storage_config.minio())
                .map_err(|error| format!("storage client failed: {}", error.code()))?;
            let qdrant = QdrantClient::with_api_key(
                storage_config.qdrant_url(),
                storage_config.qdrant_api_key().cloned(),
            )
            .map_err(|error| format!("qdrant client failed: {}", error.code()))?;
            run_reconcile_worker(state, pool, storage, qdrant, worker_id, rotation).await
        }
        other => Err(format!("unknown MARKHAND_WORKER_KIND: {other}")),
    }
}

async fn run_convert_worker(
    state: fileconv_server::state::RuntimeState,
    pool: deadpool_postgres::Pool,
    storage: MinioClient,
    worker_id: String,
    rotation: Arc<OrgRotation>,
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
            let rotation = rotation.clone();
            let worker = worker.clone();
            async move {
                let _ = fileconv_server::jobs::observe_queue_metrics(&pool).await;
                rotation
                    .run_cycle(|ctx| {
                        let pool = pool.clone();
                        let worker = worker.clone();
                        async move {
                            reclaim_expired_leases(&pool, &ctx).await;
                            let outcome = worker
                                .run_once(&ctx)
                                .await
                                .map_err(|error| error.to_string())?;
                            Ok(non_idle(outcome, |run| {
                                matches!(
                                    run,
                                    fileconv_server::workers::convert::ConvertWorkerRun::NoJob
                                )
                            }))
                        }
                    })
                    .await
            }
        },
        |outcome| outcome.is_none(),
    )
    .await
}

/// Maps a worker `run_once` outcome to the rotation contract: `None` when the
/// org had nothing claimable, `Some(outcome)` when a job was served.
fn non_idle<T>(outcome: T, is_idle: impl Fn(&T) -> bool) -> Option<T> {
    if is_idle(&outcome) {
        None
    } else {
        Some(outcome)
    }
}

async fn run_index_worker(
    state: fileconv_server::state::RuntimeState,
    pool: deadpool_postgres::Pool,
    storage: MinioClient,
    qdrant: QdrantClient,
    worker_id: String,
    rotation: Arc<OrgRotation>,
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
            let rotation = rotation.clone();
            let worker = worker.clone();
            let sink = sink.clone();
            async move {
                rotation
                    .run_cycle(|ctx| {
                        let pool = pool.clone();
                        let worker = worker.clone();
                        let sink = sink.clone();
                        async move {
                            reclaim_expired_leases(&pool, &ctx).await;
                            jobs::relay_outbox_with_sink(&pool, &ctx, 32, &sink)
                                .await
                                .map_err(|error| error.to_string())?;
                            let outcome = worker
                                .run_once(&ctx)
                                .await
                                .map_err(|error| error.to_string())?;
                            Ok(non_idle(outcome, |run| {
                                matches!(run, IndexWorkerRun::NoJob)
                            }))
                        }
                    })
                    .await
            }
        },
        |outcome| outcome.is_none(),
    )
    .await
}

async fn run_delete_worker(
    state: fileconv_server::state::RuntimeState,
    pool: deadpool_postgres::Pool,
    storage: MinioClient,
    qdrant: QdrantClient,
    worker_id: String,
    rotation: Arc<OrgRotation>,
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
            let rotation = rotation.clone();
            let worker = worker.clone();
            let sink = sink.clone();
            async move {
                rotation
                    .run_cycle(|ctx| {
                        let pool = pool.clone();
                        let worker = worker.clone();
                        let sink = sink.clone();
                        async move {
                            reclaim_expired_leases(&pool, &ctx).await;
                            jobs::relay_outbox_with_sink(&pool, &ctx, 32, &sink)
                                .await
                                .map_err(|error| error.to_string())?;
                            let outcome = worker
                                .run_once(&ctx)
                                .await
                                .map_err(|error| error.to_string())?;
                            Ok(non_idle(outcome, |run| {
                                matches!(run, DeleteWorkerRun::NoJob)
                            }))
                        }
                    })
                    .await
            }
        },
        |outcome| outcome.is_none(),
    )
    .await
}

async fn run_reconcile_worker(
    state: fileconv_server::state::RuntimeState,
    pool: deadpool_postgres::Pool,
    storage: MinioClient,
    qdrant: QdrantClient,
    worker_id: String,
    rotation: Arc<OrgRotation>,
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
    let oneshot = env_truthy("MARKHAND_WORKER_ONESHOT");
    if oneshot {
        config.document_id = Some(require_oneshot_reconcile_document_id()?);
    } else if let Ok(value) = std::env::var("MARKHAND_RECONCILE_DOCUMENT_ID") {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            config.document_id = Some(
                Uuid::parse_str(trimmed)
                    .map_err(|_| "MARKHAND_RECONCILE_DOCUMENT_ID must be a UUID".to_string())?,
            );
        }
    }
    // Document-scoped oneshot: ensure exactly one durable drift job exists so the
    // worker has a single job/document unit of work (idempotent key).
    if oneshot {
        let document_id = config
            .document_id
            .ok_or_else(|| "reconcile oneshot missing document id".to_string())?;
        // run_worker already rejects oneshot with more than one org.
        let ctx = rotation
            .contexts()
            .first()
            .ok_or_else(|| "reconcile oneshot requires an org context".to_string())?;
        fileconv_server::services::reconciliation::enqueue_reconcile(
            &pool,
            ctx,
            document_id,
            "oneshot-scope",
        )
        .await
        .map_err(|error| format!("reconcile oneshot enqueue failed: {error}"))?;
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
            let rotation = rotation.clone();
            let worker = worker.clone();
            let sink = sink.clone();
            async move {
                rotation
                    .run_cycle(|ctx| {
                        let pool = pool.clone();
                        let worker = worker.clone();
                        let sink = sink.clone();
                        async move {
                            reclaim_expired_leases(&pool, &ctx).await;
                            jobs::relay_outbox_with_sink(&pool, &ctx, 32, &sink)
                                .await
                                .map_err(|error| error.to_string())?;
                            let outcome = worker
                                .run_once(&ctx)
                                .await
                                .map_err(|error| error.to_string())?;
                            Ok(non_idle(outcome, |run| {
                                matches!(run, ReconcileWorkerRun::NoJob)
                            }))
                        }
                    })
                    .await
            }
        },
        |outcome| outcome.is_none(),
    )
    .await
}

async fn run_embedding_worker(
    state: fileconv_server::state::RuntimeState,
    pool: deadpool_postgres::Pool,
    qdrant: QdrantClient,
    worker_id: String,
    rotation: Arc<OrgRotation>,
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
            let rotation = rotation.clone();
            let worker = worker.clone();
            async move {
                let _ = fileconv_server::jobs::observe_queue_metrics(&pool).await;
                rotation
                    .run_cycle(|ctx| {
                        let pool = pool.clone();
                        let worker = worker.clone();
                        async move {
                            reclaim_expired_leases(&pool, &ctx).await;
                            let outcome = worker
                                .run_once(&ctx)
                                .await
                                .map_err(|error| error.to_string())?;
                            Ok(non_idle(outcome, |run| {
                                matches!(run, EmbeddingWorkerRun::NoJob)
                            }))
                        }
                    })
                    .await
            }
        },
        |outcome| outcome.is_none(),
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
    let oneshot = env_truthy("MARKHAND_WORKER_ONESHOT");
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
                        if oneshot {
                            println!("fileconv-worker: {outcome:?}");
                            println!(
                                "fileconv-worker: MARKHAND_WORKER_ONESHOT=1 — finite exit after one {kind} cycle"
                            );
                            break;
                        }
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
                        if oneshot {
                            shutdown_flush_telemetry(flush_budget).await;
                            return Err(format!("{kind} oneshot failed: {error}"));
                        }
                        tokio::select! {
                            _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                            changed = stop_rx.changed() => {
                                let _ = changed;
                            }
                        }
                    }
                    Err(error) => {
                        eprintln!("fileconv-worker: {kind} join error: {error}");
                        if oneshot {
                            shutdown_flush_telemetry(flush_budget).await;
                            return Err(format!("{kind} oneshot join failed: {error}"));
                        }
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

fn env_truthy(name: &str) -> bool {
    match std::env::var(name) {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
}

/// Validate `MARKHAND_RECONCILE_DOCUMENT_ID` for oneshot reconcile (no DB I/O).
fn require_oneshot_reconcile_document_id() -> Result<Uuid, String> {
    match std::env::var("MARKHAND_RECONCILE_DOCUMENT_ID") {
        Err(_) => Err("MARKHAND_RECONCILE_DOCUMENT_ID is required for reconcile oneshot".into()),
        Ok(value) if value.trim().is_empty() => {
            Err("MARKHAND_RECONCILE_DOCUMENT_ID is required for reconcile oneshot".into())
        }
        Ok(value) => Uuid::parse_str(value.trim())
            .map_err(|_| "MARKHAND_RECONCILE_DOCUMENT_ID must be a UUID".to_string()),
    }
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

/// One UUID or a comma-separated list ("a,b,c"); blank segments are ignored.
fn env_uuid_list(name: &str) -> Result<Vec<Uuid>, String> {
    let raw = std::env::var(name).map_err(|_| format!("{name} is required"))?;
    parse_uuid_list(name, &raw)
}

fn parse_uuid_list(name: &str, raw: &str) -> Result<Vec<Uuid>, String> {
    let mut ids = Vec::new();
    for part in raw.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        ids.push(
            Uuid::parse_str(trimmed)
                .map_err(|_| format!("{name} must be a UUID or comma-separated UUID list"))?,
        );
    }
    if ids.is_empty() {
        return Err(format!("{name} is required"));
    }
    Ok(ids)
}

fn exit_with_error(error: String) -> ! {
    eprintln!("fileconv-worker: {error}");
    std::process::exit(1);
}

/// Least-privilege permission set for a background worker kind (Phase 1C / 1C-08).
///
/// Unknown kinds are a configuration error: the process must not construct an
/// `OrgContext` or claim work until the kind is validated.
fn worker_permissions(kind: &str) -> Result<&'static [&'static str], String> {
    match kind {
        "convert" | "index" | "embedding" => Ok(&["jobs.system", "doc.upload"]),
        "delete" | "reconcile" => Ok(&["jobs.system", "doc.delete"]),
        other => Err(format!("unknown MARKHAND_WORKER_KIND: {other}")),
    }
}

/// Build org contexts only after `worker_permissions` accepts the kind.
fn worker_contexts_for_kind(
    org_ids: &[Uuid],
    user_id: Uuid,
    kind: &str,
) -> Result<Vec<OrgContext>, String> {
    let permissions = worker_permissions(kind)?;
    let mut contexts = Vec::with_capacity(org_ids.len());
    for &org_id in org_ids {
        contexts.push(
            OrgContext::try_new(org_id, user_id, permissions.iter().copied(), [])
                .map_err(|error| format!("invalid worker tenant context: {error}"))?,
        );
    }
    Ok(contexts)
}

#[cfg(test)]
mod worker_permissions_tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn convert_index_embedding_get_jobs_system_and_doc_upload_only() {
        for kind in ["convert", "index", "embedding"] {
            let perms = worker_permissions(kind).expect(kind);
            assert_eq!(
                perms,
                &["jobs.system", "doc.upload"][..],
                "{kind} must be jobs.system + doc.upload exactly"
            );
            let forbidden = ["member.manage", "audit.view", "doc.delete", "qa.query"];
            for code in forbidden {
                assert!(!perms.contains(&code), "{kind} must not include {code}");
            }
        }
    }

    #[test]
    fn delete_reconcile_get_jobs_system_and_doc_delete_only() {
        for kind in ["delete", "reconcile"] {
            let perms = worker_permissions(kind).expect(kind);
            assert_eq!(
                perms,
                &["jobs.system", "doc.delete"][..],
                "{kind} must be jobs.system + doc.delete exactly"
            );
            let forbidden = ["member.manage", "audit.view", "doc.upload", "qa.query"];
            for code in forbidden {
                assert!(!perms.contains(&code), "{kind} must not include {code}");
            }
        }
    }

    #[test]
    fn unknown_worker_kind_is_configuration_error() {
        let err = worker_permissions("not-a-real-kind").expect_err("unknown kind");
        assert!(
            err.contains("unknown") || err.contains("MARKHAND_WORKER_KIND"),
            "error should name the configuration problem, got: {err}"
        );
    }

    #[test]
    fn worker_contexts_carry_exact_permissions_without_superset() {
        let org = Uuid::from_u128(0x1111);
        let user = Uuid::from_u128(0x2222);
        let contexts = worker_contexts_for_kind(&[org], user, "convert").expect("convert");
        assert_eq!(contexts.len(), 1);
        let expected: BTreeSet<String> = ["jobs.system", "doc.upload"]
            .into_iter()
            .map(str::to_string)
            .collect();
        assert_eq!(contexts[0].permissions(), &expected);
        assert!(!contexts[0].has_permission("member.manage"));
        assert!(!contexts[0].has_permission("audit.view"));
        assert!(!contexts[0].has_permission("doc.delete"));
    }

    #[test]
    fn unknown_kind_cannot_build_worker_contexts() {
        let org = Uuid::from_u128(0x1111);
        let user = Uuid::from_u128(0x2222);
        assert!(worker_contexts_for_kind(&[org], user, "ghost").is_err());
    }
}

#[cfg(test)]
mod shutdown_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn uuid_list_accepts_single_and_multi_and_rejects_garbage() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        assert_eq!(
            parse_uuid_list("X", &a.to_string()).expect("single"),
            vec![a]
        );
        assert_eq!(
            parse_uuid_list("X", &format!(" {a} , {b} ,")).expect("multi"),
            vec![a, b]
        );
        assert!(parse_uuid_list("X", "").is_err());
        assert!(parse_uuid_list("X", "not-a-uuid").is_err());
        assert!(parse_uuid_list("X", &format!("{a},oops")).is_err());
    }

    #[test]
    fn sandbox_convert_probe_accepts_exactly_one_input() {
        let args = [
            "fileconv-worker".to_string(),
            "--sandbox-convert-probe".to_string(),
            "/tmp/input.txt".to_string(),
        ];
        assert_eq!(
            sandbox_convert_probe_arg(&args).expect("valid probe args"),
            Some("/tmp/input.txt")
        );
    }

    #[test]
    fn sandbox_convert_probe_rejects_missing_or_extra_inputs() {
        let missing = [
            "fileconv-worker".to_string(),
            "--sandbox-convert-probe".to_string(),
        ];
        assert!(sandbox_convert_probe_arg(&missing).is_err());

        let extra = [
            "fileconv-worker".to_string(),
            "--sandbox-convert-probe".to_string(),
            "/tmp/input.txt".to_string(),
            "--check-config".to_string(),
        ];
        assert!(sandbox_convert_probe_arg(&extra).is_err());
    }

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
