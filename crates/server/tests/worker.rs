//! Converter worker and sandbox tests.

use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use bytes::Bytes;
use deadpool_postgres::Pool;
use fileconv_server::auth::context::OrgContext;
use fileconv_server::config::{MinioConfig, SecretString};
use fileconv_server::database::apply_migrations;
use fileconv_server::db::collections::{self, NewCollection};
use fileconv_server::db::documents::{self, NewDocument};
use fileconv_server::db::error::DbError;
use fileconv_server::db::models::{CollectionVisibility, Job, JobStatus, JobType};
use fileconv_server::db::orgs;
use fileconv_server::db::pool::{create_pool, with_org_txn};
use fileconv_server::jobs::{self, EnqueueJob, JobPayload};
use fileconv_server::services::conversion::ConversionIdentity;
use fileconv_server::services::promotion::PromotionFault;
use fileconv_server::services::quota;
use fileconv_server::storage::keys::quarantine_key;
use fileconv_server::storage::minio::{MinioClient, ObjectIdentityMeta};
use fileconv_server::workers::convert::{
    ConvertWorker, ConvertWorkerConfig, ConvertWorkerPause, ConvertWorkerRun,
};
use fileconv_server::workers::limits::ResourceLimits;
use fileconv_server::workers::sandbox::{
    self, SandboxCancel, SandboxConfig, SandboxExit, SandboxInput,
};
use sha2::{Digest, Sha256};
use tokio::sync::Notify;
use tokio_postgres::NoTls;
use uuid::Uuid;

const INPUT: &str = "{input}";
static SANDBOX_TEST_LOCK: Mutex<()> = Mutex::new(());
const ECHO_INPUT_SCRIPT: &str = r#"while IFS= read -r line; do printf '%s\n' "$line"; done < "$1""#;

fn sandbox_test_guard() -> MutexGuard<'static, ()> {
    SANDBOX_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn sandbox_available() -> bool {
    match sandbox::preflight() {
        Ok(()) => true,
        Err(error) => {
            eprintln!("skipped: sandbox isolation unavailable: {error}");
            false
        }
    }
}

fn base_limits(timeout: Duration) -> ResourceLimits {
    ResourceLimits {
        wall_timeout: timeout,
        memory_bytes: 128 * 1024 * 1024,
        cpu_seconds: 5,
        file_size_bytes: 2 * 1024 * 1024,
        max_processes: 512,
        max_open_files: 256,
        stdout_stderr_bytes: 1024 * 1024,
    }
}

fn shell(script: &str, timeout: Duration) -> SandboxConfig {
    SandboxConfig {
        argv_template: vec![
            "/bin/sh".into(),
            "-c".into(),
            script.into(),
            "sandbox-sh".into(),
            INPUT.into(),
        ],
        limits: base_limits(timeout),
    }
}

fn python(script: &str, timeout: Duration) -> Option<SandboxConfig> {
    if !Path::new("/usr/bin/python3").exists() {
        eprintln!("skipped: /usr/bin/python3 missing");
        return None;
    }
    Some(SandboxConfig {
        argv_template: vec![
            "/usr/bin/python3".into(),
            "-B".into(),
            "-c".into(),
            script.into(),
            INPUT.into(),
        ],
        limits: base_limits(timeout),
    })
}

fn input(ext: &str, bytes: &[u8]) -> SandboxInput {
    SandboxInput {
        bytes: bytes.to_vec(),
        canonical_extension: ext.into(),
    }
}

fn run(config: &SandboxConfig) -> fileconv_server::workers::sandbox::SandboxOutput {
    sandbox::run(config, input("txt", b"hello"), &SandboxCancel::default()).expect("sandbox run")
}

fn markhand_e2e_required() -> bool {
    std::env::var("MARKHAND_E2E").ok().as_deref() == Some("1")
}

fn take_live<T>(value: Option<T>, name: &str) -> Option<T> {
    match value {
        Some(value) => Some(value),
        None if markhand_e2e_required() => panic!("MARKHAND_E2E=1 requires {name}"),
        None => None,
    }
}

fn test_database_url() -> Option<String> {
    match std::env::var("MARKHAND_TEST_DATABASE_URL") {
        Ok(url) if !url.trim().is_empty() => Some(url),
        _ => {
            eprintln!("skipped: MARKHAND_TEST_DATABASE_URL unset");
            None
        }
    }
}

fn rewrite_database_url(base_url: &str, database_name: &str) -> String {
    let (without_query, query) = match base_url.split_once('?') {
        Some((head, tail)) => (head, Some(tail)),
        None => (base_url, None),
    };
    let prefix = without_query
        .rsplit_once('/')
        .map(|(head, _)| head)
        .expect("database URL must include a path");
    match query {
        Some(tail) => format!("{prefix}/{database_name}?{tail}"),
        None => format!("{prefix}/{database_name}"),
    }
}

async fn connect_raw(database_url: &str) -> tokio_postgres::Client {
    let (client, connection) = tokio_postgres::connect(database_url, NoTls)
        .await
        .unwrap_or_else(|error| panic!("connect failed for {database_url}: {error}"));
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client
}

struct EphemeralDb {
    admin_url: String,
    db_name: String,
    url: String,
}

impl EphemeralDb {
    async fn create(base_url: &str) -> Self {
        let db_name = format!("markhand_worker_{}", Uuid::new_v4().simple());
        let admin_url = rewrite_database_url(base_url, "postgres");
        let admin = connect_raw(&admin_url).await;
        admin
            .batch_execute(&format!("CREATE DATABASE \"{db_name}\""))
            .await
            .expect("CREATE DATABASE");
        Self {
            admin_url,
            db_name: db_name.clone(),
            url: rewrite_database_url(base_url, &db_name),
        }
    }

    async fn drop(self) {
        let admin = connect_raw(&self.admin_url).await;
        admin
            .batch_execute(&format!(
                "SELECT pg_terminate_backend(pid) FROM pg_stat_activity                  WHERE datname = '{}' AND pid <> pg_backend_pid()",
                self.db_name
            ))
            .await
            .unwrap_or_else(|error| panic!("terminate backends failed: {error}"));
        admin
            .batch_execute(&format!(
                "DROP DATABASE IF EXISTS \"{}\" WITH (FORCE)",
                self.db_name
            ))
            .await
            .unwrap_or_else(|error| panic!("DROP DATABASE WITH (FORCE) failed: {error}"));
    }
}

async fn boot_pool(base_url: &str) -> (EphemeralDb, Pool) {
    let ephemeral = EphemeralDb::create(base_url).await;
    apply_migrations(&ephemeral.url)
        .await
        .expect("apply migrations");
    let pool = create_pool(&ephemeral.url).expect("pool");
    (ephemeral, pool)
}

fn test_minio_client() -> Option<MinioClient> {
    let endpoint = match std::env::var("MARKHAND_TEST_MINIO_ENDPOINT") {
        Ok(url) if !url.trim().is_empty() => url,
        _ => {
            eprintln!("skipped: MARKHAND_TEST_MINIO_ENDPOINT unset");
            return None;
        }
    };
    let access_key = std::env::var("MARKHAND_TEST_MINIO_ACCESS_KEY").ok()?;
    let secret_key = std::env::var("MARKHAND_TEST_MINIO_SECRET_KEY").ok()?;
    let region = std::env::var("MARKHAND_TEST_MINIO_REGION").unwrap_or_else(|_| "us-east-1".into());
    let bucket = format!("markhand-worker-{}", Uuid::new_v4().simple());
    std::env::set_var("RUST_S3_SKIP_LOCATION_CONSTRAINT", "true");
    let config = MinioConfig::new(
        endpoint,
        SecretString::new(access_key),
        SecretString::new(secret_key),
        bucket,
        region,
        true,
    )
    .expect("minio config");
    let client = MinioClient::from_config(&config).expect("minio client");
    Some(client)
}

fn org_context(org: Uuid, user: Uuid) -> OrgContext {
    OrgContext::try_new(org, user, ["doc.upload"], []).expect("org context")
}

async fn seed_org_collection_document_version(
    pool: &Pool,
    ctx: &OrgContext,
    document_id: Uuid,
    version_id: Uuid,
    original_object_key: &str,
    sha256: &str,
    byte_size: u64,
) -> (Uuid, Uuid) {
    let collection_id = Uuid::new_v4();
    let original_object_key = original_object_key.to_string();
    let sha256 = sha256.to_string();
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                orgs::ensure_exists(txn, &ctx, "worker-org", "Worker Org").await?;
                orgs::ensure_user(txn, &ctx, ctx.user_id(), "worker@example.test", "Worker")
                    .await?;
                orgs::ensure_membership(txn, &ctx).await?;
                txn.execute(
                    "INSERT INTO org_quotas (
                        org_id, max_storage_bytes, max_documents,
                        max_concurrent_jobs, max_monthly_tokens
                     )
                     VALUES ($1, 1000000000, 100000, 100, 1000000000)
                     ON CONFLICT (org_id) DO NOTHING",
                    &[&ctx.org_id()],
                )
                .await?;
                let collection_name = format!("Worker Collection {collection_id}");
                let collection_slug = format!("worker-collection-{collection_id}");
                collections::insert(
                    txn,
                    &ctx,
                    NewCollection {
                        id: collection_id,
                        name: &collection_name,
                        slug: &collection_slug,
                        description: None,
                        visibility: CollectionVisibility::Private,
                    },
                )
                .await?;
                documents::insert(
                    txn,
                    &ctx,
                    NewDocument {
                        id: document_id,
                        collection_id,
                        title: "Worker Doc",
                    },
                )
                .await?;
                txn.execute(
                    "INSERT INTO document_versions (
                        id, org_id, document_id, version_number, content_sha256,
                        original_object_key, source_content_type, byte_size,
                        created_by_user_id
                     )
                     VALUES ($1, $2, $3, 1, $4, $5, 'text/plain', $6, $7)",
                    &[
                        &version_id,
                        &ctx.org_id(),
                        &document_id,
                        &sha256,
                        &original_object_key,
                        &(byte_size as i64),
                        &ctx.user_id(),
                    ],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("seed document version");
    (document_id, version_id)
}

async fn seed_additional_source_version(
    pool: &Pool,
    ctx: &OrgContext,
    document_id: Uuid,
    version_id: Uuid,
    original_object_key: &str,
    sha256: &str,
    byte_size: u64,
) {
    let original_object_key = original_object_key.to_string();
    let sha256 = sha256.to_string();
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "INSERT INTO document_versions (
                        id, org_id, document_id, version_number, content_sha256,
                        original_object_key, source_content_type, byte_size,
                        created_by_user_id
                     )
                     SELECT $1, $2, $3, COALESCE(MAX(version_number), 0)::integer + 1,
                            $4, $5, 'text/plain', $6, $7
                     FROM document_versions
                     WHERE org_id = $2 AND document_id = $3",
                    &[
                        &version_id,
                        &ctx.org_id(),
                        &document_id,
                        &sha256,
                        &original_object_key,
                        &(byte_size as i64),
                        &ctx.user_id(),
                    ],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("seed additional source version");
}

async fn put_quarantine_object(
    storage: &MinioClient,
    ctx: &OrgContext,
    key: &fileconv_server::storage::ObjectKey,
    bytes: &'static [u8],
    canonical_format: &str,
    document_id: Uuid,
    version_id: Uuid,
) -> String {
    storage.ensure_bucket().await.expect("ensure bucket");
    let sha256 = hex::encode(Sha256::digest(bytes));
    let meta = ObjectIdentityMeta {
        org_id: ctx.org_id(),
        collection_id: None,
        document_id: Some(document_id),
        version_id: Some(version_id),
        original_filename: None,
        canonical_format: Some(canonical_format.into()),
        content_sha256: Some(sha256.clone()),
        content_length: Some(bytes.len() as u64),
        disposition: Some("accepted".into()),
    };
    storage
        .put_object(
            ctx.org_id(),
            key,
            Bytes::from_static(bytes),
            &meta,
            "text/plain",
        )
        .await
        .expect("put quarantine");
    sha256
}

async fn enqueue_convert(
    pool: &Pool,
    ctx: &OrgContext,
    document_id: Uuid,
    version_id: Uuid,
) -> Job {
    jobs::enqueue(
        pool,
        ctx,
        EnqueueJob::new(
            JobType::Convert,
            JobPayload {
                document_id: Some(document_id),
                version_id: Some(version_id),
                collection_id: None,
                upload_id: None,
                batch_id: None,
                index_metadata_id: None,
                cleanup_target_job_id: None,
                related_version_id: None,

                request_id: None,
                traceparent: None,
            },
            format!("convert-{version_id}"),
        ),
    )
    .await
    .expect("enqueue")
    .job
}

async fn get_job(pool: &Pool, ctx: &OrgContext, job_id: Uuid) -> Job {
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT id, org_id, job_type, status, payload_version, payload,
                                attempts, max_attempts, lease_owner, lease_expires_at,
                                heartbeat_at, checkpoint, idempotency_key, document_id,
                                version_id, available_at, started_at, finished_at,
                                last_error, created_at, updated_at
                         FROM jobs
                         WHERE org_id = $1 AND id = $2",
                        &[&ctx.org_id(), &job_id],
                    )
                    .await?;
                map_job(&row)
            })
        }
    })
    .await
    .expect("get job")
}

fn map_job(row: &tokio_postgres::Row) -> Result<Job, DbError> {
    let job_type: String = row.get("job_type");
    let status: String = row.get("status");
    Ok(Job {
        id: row.get("id"),
        org_id: row.get("org_id"),
        job_type: JobType::parse(&job_type).map_err(DbError::Config)?,
        status: JobStatus::parse(&status).map_err(DbError::Config)?,
        payload_version: row.get("payload_version"),
        payload: row.get("payload"),
        attempts: row.get("attempts"),
        max_attempts: row.get("max_attempts"),
        lease_owner: row.get("lease_owner"),
        lease_expires_at: row.get("lease_expires_at"),
        heartbeat_at: row.get("heartbeat_at"),
        checkpoint: row.get("checkpoint"),
        idempotency_key: row.get("idempotency_key"),
        document_id: row.get("document_id"),
        version_id: row.get("version_id"),
        available_at: row.get("available_at"),
        started_at: row.get("started_at"),
        finished_at: row.get("finished_at"),
        last_error: row.get("last_error"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

async fn markdown_artifact_key(pool: &Pool, ctx: &OrgContext, version_id: Uuid) -> Option<String> {
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_opt(
                        "SELECT object_key
                         FROM derived_artifacts
                         WHERE org_id = $1 AND version_id = $2 AND artifact_kind = 'markdown'",
                        &[&ctx.org_id(), &version_id],
                    )
                    .await?;
                Ok(row.map(|row| row.get(0)))
            })
        }
    })
    .await
    .expect("artifact query")
}

async fn published_version_for_source(
    pool: &Pool,
    ctx: &OrgContext,
    source_version_id: Uuid,
) -> Option<Uuid> {
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_opt(
                        "SELECT id
                         FROM document_versions
                         WHERE org_id = $1
                           AND parent_version_id = $2
                           AND publication_state = 'published'",
                        &[&ctx.org_id(), &source_version_id],
                    )
                    .await?;
                Ok(row.map(|row| row.get(0)))
            })
        }
    })
    .await
    .expect("published version query")
}

async fn document_current_version(
    pool: &Pool,
    ctx: &OrgContext,
    document_id: Uuid,
) -> Option<Uuid> {
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT current_version_id
                         FROM documents
                         WHERE org_id = $1 AND id = $2",
                        &[&ctx.org_id(), &document_id],
                    )
                    .await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .expect("current version query")
}

async fn document_state(pool: &Pool, ctx: &OrgContext, document_id: Uuid) -> String {
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT state FROM documents WHERE org_id = $1 AND id = $2",
                        &[&ctx.org_id(), &document_id],
                    )
                    .await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .expect("document state query")
}

async fn count_published_versions(pool: &Pool, ctx: &OrgContext, document_id: Uuid) -> i64 {
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT count(*)::bigint
                         FROM document_versions
                         WHERE org_id = $1
                           AND document_id = $2
                           AND publication_state = 'published'",
                        &[&ctx.org_id(), &document_id],
                    )
                    .await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .expect("published count query")
}

async fn count_markdown_artifacts(pool: &Pool, ctx: &OrgContext, document_id: Uuid) -> i64 {
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT count(*)::bigint
                         FROM derived_artifacts
                         WHERE org_id = $1
                           AND document_id = $2
                           AND artifact_kind = 'markdown'",
                        &[&ctx.org_id(), &document_id],
                    )
                    .await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .expect("artifact count query")
}

async fn count_index_outbox(pool: &Pool, ctx: &OrgContext, job_id: Uuid) -> i64 {
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT count(*)::bigint
                         FROM outbox_events
                         WHERE org_id = $1
                           AND job_id = $2
                           AND event_type = 'document.index_requested'",
                        &[&ctx.org_id(), &job_id],
                    )
                    .await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .expect("index outbox count query")
}

async fn quota_reservation_statuses(pool: &Pool, ctx: &OrgContext, job_id: Uuid) -> Vec<String> {
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let rows = txn
                    .query(
                        "SELECT status
                         FROM quota_reservations
                         WHERE org_id = $1 AND job_id = $2
                         ORDER BY created_at, id",
                        &[&ctx.org_id(), &job_id],
                    )
                    .await?;
                Ok(rows.into_iter().map(|row| row.get(0)).collect())
            })
        }
    })
    .await
    .expect("quota status query")
}

fn staging_key_for_claim(
    ctx: &OrgContext,
    document_id: Uuid,
    source_version_id: Uuid,
    job: &Job,
) -> fileconv_server::storage::ObjectKey {
    let identity = ConversionIdentity::new(
        ctx.org_id(),
        document_id,
        source_version_id,
        job.idempotency_key.clone(),
    );
    let lease_token = job.lease_owner.as_deref().expect("claimed job lease");
    fileconv_server::services::artifacts::markdown_key(
        &identity,
        identity.promoted_version_id(),
        job.id,
        job.attempts,
        lease_token,
    )
    .expect("staging key")
}

async fn checkpoint_staged_keys(pool: &Pool, ctx: &OrgContext, job_id: Uuid) -> Vec<String> {
    let job = get_job(pool, ctx, job_id).await;
    job.checkpoint
        .and_then(|value| value.get("staged_object_keys").cloned())
        .and_then(|value| serde_json::from_value::<Vec<String>>(value).ok())
        .unwrap_or_default()
}

async fn first_markdown_artifact_key(pool: &Pool, ctx: &OrgContext, document_id: Uuid) -> String {
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT object_key
                         FROM derived_artifacts
                         WHERE org_id = $1
                           AND document_id = $2
                           AND artifact_kind = 'markdown'",
                        &[&ctx.org_id(), &document_id],
                    )
                    .await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .expect("artifact key query")
}

async fn make_job_available(pool: &Pool, ctx: &OrgContext, job_id: Uuid) {
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "UPDATE jobs
                     SET available_at = clock_timestamp()
                     WHERE org_id = $1 AND id = $2",
                    &[&ctx.org_id(), &job_id],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("make job available");
}

/// Drive a convert worker until `convert_job_id` completes, draining any
/// conversion-cleanup reconcile work enqueued by a prior fault/compensation path.
/// A fresh [`ConvertWorker`] prefers reconcile on its first `run_once`, so a single
/// call is not sufficient when cleanup reconciliation was scheduled.
async fn run_convert_worker_until_completed(
    pool: &Pool,
    storage: &MinioClient,
    ctx: &OrgContext,
    convert_job_id: Uuid,
    config: ConvertWorkerConfig,
) -> ConvertWorkerRun {
    let worker = ConvertWorker::new(pool.clone(), storage.clone(), config).expect("worker");
    for round in 0..12 {
        make_job_available(pool, ctx, convert_job_id).await;
        let outcome = worker.run_once(ctx).await.expect("worker run");
        match outcome {
            ConvertWorkerRun::Completed { job_id, .. } if job_id == convert_job_id => {
                return outcome;
            }
            ConvertWorkerRun::Failed {
                job_id,
                terminal: true,
                ..
            } if job_id == convert_job_id => {
                let job = get_job(pool, ctx, convert_job_id).await;
                panic!("convert job {convert_job_id} dead-lettered on round {round}: {job:?}");
            }
            ConvertWorkerRun::NoJob if round + 1 < 12 => continue,
            other if round + 1 < 12 => {
                let _ = other;
                continue;
            }
            other => panic!(
                "convert job {convert_job_id} did not complete within run budget; last={other:?}"
            ),
        }
    }
    unreachable!("loop bound exhausted");
}

async fn force_lease_expired(pool: &Pool, ctx: &OrgContext, job_id: Uuid) {
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "UPDATE jobs
                     SET lease_expires_at = clock_timestamp() - interval '1 second'
                     WHERE org_id = $1 AND id = $2",
                    &[&ctx.org_id(), &job_id],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("force lease expired");
}

async fn reclaim_job_to_pending(pool: &Pool, ctx: &OrgContext, job_id: Uuid) -> Job {
    const MAX_ATTEMPTS: usize = 40;
    for attempt in 0..MAX_ATTEMPTS {
        force_lease_expired(pool, ctx, job_id).await;
        let reclaimed = jobs::reclaim_expired(pool, ctx, 10, Duration::from_secs(1))
            .await
            .expect("reclaim expired");
        if let Some(job) = reclaimed.into_iter().find(|job| job.id == job_id) {
            assert_eq!(
                job.status,
                JobStatus::Pending,
                "reclaimed job must return to pending for retry"
            );
            make_job_available(pool, ctx, job_id).await;
            return get_job(pool, ctx, job_id).await;
        }
        tokio::time::sleep(Duration::from_millis(25 * (attempt as u64 + 1))).await;
    }
    let job = get_job(pool, ctx, job_id).await;
    panic!(
        "job {job_id} was not reclaimed to pending within retry budget; status={:?}",
        job.status
    );
}

async fn version_inherits_document_collection(
    pool: &Pool,
    ctx: &OrgContext,
    document_id: Uuid,
    version_id: Uuid,
) -> bool {
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT d.collection_id = d2.collection_id
                         FROM documents d
                         JOIN document_versions v
                           ON v.org_id = d.org_id AND v.document_id = d.id
                         JOIN documents d2
                           ON d2.org_id = v.org_id AND d2.id = v.document_id
                         WHERE d.org_id = $1 AND d.id = $2 AND v.id = $3",
                        &[&ctx.org_id(), &document_id, &version_id],
                    )
                    .await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .expect("collection inheritance query")
}

async fn version_current_and_expired(
    pool: &Pool,
    ctx: &OrgContext,
    version_id: Uuid,
) -> (bool, bool) {
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT is_current, effective_to IS NOT NULL AS expired
                         FROM document_versions
                         WHERE org_id = $1 AND id = $2",
                        &[&ctx.org_id(), &version_id],
                    )
                    .await?;
                Ok((row.get(0), row.get(1)))
            })
        }
    })
    .await
    .expect("version current query")
}

async fn illegal_version_content_update_is_rejected(
    pool: &Pool,
    ctx: &OrgContext,
    version_id: Uuid,
) -> bool {
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "UPDATE document_versions
                     SET content_sha256 = repeat('a', 64)
                     WHERE org_id = $1 AND id = $2",
                    &[&ctx.org_id(), &version_id],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .is_err()
}

async fn illegal_original_key_update_is_rejected(
    pool: &Pool,
    ctx: &OrgContext,
    version_id: Uuid,
) -> bool {
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "UPDATE document_versions
                     SET original_object_key = original_object_key || '-mutated'
                     WHERE org_id = $1 AND id = $2",
                    &[&ctx.org_id(), &version_id],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .is_err()
}

fn stub_worker_config(script: &str, heartbeat_ms: u64) -> ConvertWorkerConfig {
    let mut config = ConvertWorkerConfig::new(
        format!("test-worker-{}", Uuid::new_v4()),
        shell(script, Duration::from_secs(10)),
    );
    config.heartbeat_interval = Duration::from_millis(heartbeat_ms);
    config.lease_ttl = Duration::from_secs(2);
    config
}

#[test]
fn sandbox_denies_network_connect() {
    let _guard = sandbox_test_guard();
    if !sandbox_available() {
        return;
    }
    let listener = TcpListener::bind("127.0.0.1:0").expect("parent loopback listener");
    let port = listener.local_addr().expect("listener addr").port();
    let Some(config) = python(
        &format!(
            r#"
import errno, socket, sys
s = socket.socket()
s.settimeout(0.2)
try:
    s.connect(("127.0.0.1", {port}))
    print("connected")
    sys.exit(1)
except OSError as exc:
    print("network-denied", exc.errno)
"#,
        ),
        Duration::from_secs(5),
    ) else {
        return;
    };
    let output = run(&config);
    assert_eq!(output.exit, SandboxExit::Success);
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("network-denied"),
        "stdout={}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(!output.workspace_path.exists());
}

#[test]
fn sandbox_denies_host_filesystem_and_clears_environment() {
    let _guard = sandbox_test_guard();
    if !sandbox_available() {
        return;
    }
    let outside = tempfile::tempdir().expect("outside tempdir");
    let secret_path = outside.path().join("host-secret.txt");
    fs::write(&secret_path, b"top secret").expect("write secret");
    std::env::set_var("FILECONV_LLM_API_KEY", "must-not-leak");
    let script = format!(
        r#"
import os, sys
secret = {secret_path:?}
if os.environ.get("FILECONV_LLM_API_KEY"):
    print("env-leaked")
    sys.exit(1)
try:
    open(secret, "rb").read()
    print("host-file-readable")
    sys.exit(1)
except OSError:
    print("host-file-denied env-cleared")
"#,
        secret_path = secret_path.display().to_string()
    );
    let Some(config) = python(&script, Duration::from_secs(5)) else {
        return;
    };
    let output = run(&config);
    assert_eq!(output.exit, SandboxExit::Success);
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("host-file-denied env-cleared"),
        "stdout={}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(!output.workspace_path.exists());
    std::env::remove_var("FILECONV_LLM_API_KEY");
}

#[test]
fn sandbox_timeout_kills_descendant_process_tree() {
    let _guard = sandbox_test_guard();
    if !sandbox_available() {
        return;
    }
    let Some(config) = python(
        r#"
import os, time, sys
pid = os.fork()
if pid == 0:
    os.setsid()
    while True:
        time.sleep(60)
else:
    time.sleep(600)
"#,
        Duration::from_millis(200),
    ) else {
        return;
    };
    let output = run(&config);
    assert_eq!(output.exit, SandboxExit::TimedOut);
    // The forked child calls setsid() to try to escape the process group, but a
    // setsid() only changes session/pgroup — it cannot leave the sandbox PID
    // namespace. When the sandbox kills pid 1 of that namespace, the kernel
    // SIGKILLs every remaining process in it (including the setsid descendant)
    // and tears the namespace down. The descendant's host pid is not observable
    // from inside the namespace, so pid 1's host pid disappearing is the
    // authoritative proof that the whole tree — descendant included — is gone.
    assert_process_exits(
        output.pid1_host_pid.expect("pidns pid1"),
        Duration::from_secs(2),
    );
    assert!(!output.workspace_path.exists());
}

#[test]
fn sandbox_cleans_workspace_on_success_failure_timeout_and_cancel() {
    let _guard = sandbox_test_guard();
    if !sandbox_available() {
        return;
    }
    let success = run(&shell("printf ok", Duration::from_secs(5)));
    assert_eq!(success.exit, SandboxExit::Success);
    assert!(!success.workspace_path.exists());

    let failure = run(&shell("exit 7", Duration::from_secs(5)));
    assert_eq!(failure.exit, SandboxExit::Exit(7));
    assert!(!failure.workspace_path.exists());

    let timeout = run(
        &python("import time; time.sleep(600)", Duration::from_millis(100)).expect("python config"),
    );
    assert_eq!(timeout.exit, SandboxExit::TimedOut);
    assert!(!timeout.workspace_path.exists());

    let cancel = SandboxCancel::default();
    let cancel_for_thread = cancel.clone();
    let config = python(
        r#"
import os, time
pid = os.fork()
if pid == 0:
    os.setsid()
    while True:
        time.sleep(60)
else:
    time.sleep(600)
"#,
        Duration::from_secs(30),
    )
    .expect("python config");
    let handle = std::thread::spawn(move || {
        sandbox::run(&config, input("txt", b"hello"), &cancel_for_thread).expect("sandbox run")
    });
    std::thread::sleep(Duration::from_millis(100));
    cancel.cancel();
    let cancelled = handle.join().expect("join");
    assert_eq!(cancelled.exit, SandboxExit::Cancelled);
    // As in the timeout test, the setsid descendant cannot escape the sandbox
    // PID namespace; pid 1's host pid disappearing proves the namespace (and
    // every process in it) was torn down on cancel.
    assert_process_exits(
        cancelled.pid1_host_pid.expect("pidns pid1"),
        Duration::from_secs(2),
    );
    assert!(!cancelled.workspace_path.exists());
}

#[test]
fn sandbox_limits_fork_bomb_disk_and_ram() {
    let _guard = sandbox_test_guard();
    if !sandbox_available() {
        return;
    }
    let mut fork_limits = base_limits(Duration::from_millis(500));
    fork_limits.max_processes = 8;
    let fork = sandbox::run(
        &SandboxConfig {
            argv_template: python(
                r#"
import os, time
children = []
while True:
    try:
        pid = os.fork()
    except OSError:
        print("fork-limited", len(children), flush=True)
        time.sleep(60)
        break
    if pid == 0:
        time.sleep(60)
    children.append(pid)
"#,
                Duration::from_millis(500),
            )
            .expect("python config")
            .argv_template,
            limits: fork_limits,
        },
        input("txt", b"hello"),
        &SandboxCancel::default(),
    )
    .expect("fork run");
    assert!(!fork.exit.success(), "fork bomb should not succeed");
    assert!(!fork.workspace_path.exists());

    let Some(mut disk_config) = python(
        r#"
with open("big.bin", "wb") as f:
    while True:
        f.write(b"x" * 4096)
"#,
        Duration::from_secs(5),
    ) else {
        return;
    };
    disk_config.limits.file_size_bytes = 64 * 1024;
    let disk = run(&disk_config);
    assert!(!disk.exit.success(), "disk limit should stop writer");
    assert!(!disk.workspace_path.exists());

    let Some(mut ram_config) = python(
        r#"
chunks = []
while True:
    chunks.append(bytearray(16 * 1024 * 1024))
"#,
        Duration::from_secs(5),
    ) else {
        return;
    };
    ram_config.limits.memory_bytes = 64 * 1024 * 1024;
    let ram = run(&ram_config);
    assert!(!ram.exit.success(), "RAM limit should stop allocator");
    assert!(!ram.workspace_path.exists());
}

#[test]
fn sandbox_reports_malformed_converter_exit() {
    let _guard = sandbox_test_guard();
    if !sandbox_available() {
        return;
    }
    let output = run(&shell(
        "printf malformed >&2; exit 42",
        Duration::from_secs(5),
    ));
    assert_eq!(output.exit, SandboxExit::Exit(42));
    assert!(String::from_utf8_lossy(&output.stderr).contains("malformed"));
    assert!(!output.workspace_path.exists());
}

#[test]
fn real_fileconv_smoke_for_simple_formats_when_built() {
    let _guard = sandbox_test_guard();
    if !sandbox_available() {
        return;
    }
    let Some(fileconv) = fileconv_binary() else {
        eprintln!("skipped: target/debug/fileconv not built");
        return;
    };
    let argv = vec![fileconv.display().to_string(), "one".into(), INPUT.into()];
    for (ext, bytes, expected) in [
        ("txt", b"Xin chao Markhand\n".as_slice(), "Xin chao"),
        ("csv", b"name,value\nalpha,1\n", "alpha"),
        (
            "html",
            b"<html><body><h1>Tieu de</h1><p>Noi dung</p></body></html>",
            "Tieu de",
        ),
        ("md", b"# Heading\n\nBody\n", "Heading"),
    ] {
        let output = sandbox::run(
            &SandboxConfig {
                argv_template: argv.clone(),
                limits: base_limits(Duration::from_secs(10)),
            },
            input(ext, bytes),
            &SandboxCancel::default(),
        )
        .expect("fileconv smoke");
        assert_eq!(
            output.exit,
            SandboxExit::Success,
            "stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains(expected),
            "ext={ext} stdout={}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(!output.workspace_path.exists());
    }
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL and MARKHAND_TEST_MINIO_*"]
async fn live_convert_worker_promotes_immutable_markdown_version() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let Some(storage) = test_minio_client() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let ctx = org_context(Uuid::new_v4(), Uuid::new_v4());
    let document_id = Uuid::new_v4();
    let version_id = Uuid::new_v4();
    let quarantine = quarantine_key(ctx.org_id(), Uuid::new_v4(), None).expect("quarantine key");
    let payload = b"hello from quarantine\n";
    let sha256 = put_quarantine_object(
        &storage,
        &ctx,
        &quarantine,
        payload,
        "txt",
        document_id,
        version_id,
    )
    .await;
    let (document_id, version_id) = seed_org_collection_document_version(
        &pool,
        &ctx,
        document_id,
        version_id,
        &quarantine.as_str(),
        &sha256,
        payload.len() as u64,
    )
    .await;
    let job = enqueue_convert(&pool, &ctx, document_id, version_id).await;
    let worker = ConvertWorker::new(
        pool.clone(),
        storage.clone(),
        stub_worker_config(
            r#"while IFS= read -r line; do printf '%s\n' "$line"; done < "$1""#,
            50,
        ),
    )
    .expect("worker");

    let outcome = worker.run_once(&ctx).await.expect("run once");
    assert!(matches!(
        outcome,
        ConvertWorkerRun::Completed {
            job_id,
            markdown_bytes: 22
        } if job_id == job.id
    ));
    assert_eq!(
        get_job(&pool, &ctx, job.id).await.status,
        JobStatus::Succeeded
    );
    let promoted_version_id = published_version_for_source(&pool, &ctx, version_id)
        .await
        .expect("published promoted version");
    assert_ne!(promoted_version_id, version_id);
    assert_eq!(
        document_current_version(&pool, &ctx, document_id).await,
        Some(promoted_version_id)
    );
    assert!(
        version_inherits_document_collection(&pool, &ctx, document_id, promoted_version_id).await
    );
    assert_eq!(count_published_versions(&pool, &ctx, document_id).await, 1);
    assert_eq!(count_markdown_artifacts(&pool, &ctx, document_id).await, 1);
    assert_eq!(count_index_outbox(&pool, &ctx, job.id).await, 1);
    assert_eq!(
        quota_reservation_statuses(&pool, &ctx, job.id).await,
        vec!["finalized".to_string()]
    );
    assert!(markdown_artifact_key(&pool, &ctx, version_id)
        .await
        .is_none());
    let artifact_key = markdown_artifact_key(&pool, &ctx, promoted_version_id)
        .await
        .expect("artifact key");
    let trusted = fileconv_server::storage::parse_key_for_org(&artifact_key, ctx.org_id())
        .expect("trusted key");
    let markdown = storage
        .get_object(ctx.org_id(), &trusted)
        .await
        .expect("get trusted markdown");
    assert_eq!(markdown.as_ref(), b"hello from quarantine\n");
    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL and MARKHAND_TEST_MINIO_*"]
async fn live_convert_worker_duplicate_enqueue_converges_to_one_promotion() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let Some(storage) = test_minio_client() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let ctx = org_context(Uuid::new_v4(), Uuid::new_v4());
    let document_id = Uuid::new_v4();
    let version_id = Uuid::new_v4();
    let quarantine = quarantine_key(ctx.org_id(), Uuid::new_v4(), None).expect("quarantine key");
    let payload = b"idempotent retry\n";
    let sha256 = put_quarantine_object(
        &storage,
        &ctx,
        &quarantine,
        payload,
        "txt",
        document_id,
        version_id,
    )
    .await;
    let (document_id, version_id) = seed_org_collection_document_version(
        &pool,
        &ctx,
        document_id,
        version_id,
        &quarantine.as_str(),
        &sha256,
        payload.len() as u64,
    )
    .await;
    let job = enqueue_convert(&pool, &ctx, document_id, version_id).await;
    let duplicate = enqueue_convert(&pool, &ctx, document_id, version_id).await;
    assert_eq!(duplicate.id, job.id);
    let worker = ConvertWorker::new(
        pool.clone(),
        storage.clone(),
        stub_worker_config(ECHO_INPUT_SCRIPT, 50),
    )
    .expect("worker");

    assert!(matches!(
        worker.run_once(&ctx).await.expect("first run"),
        ConvertWorkerRun::Completed { job_id, .. } if job_id == job.id
    ));
    assert_eq!(
        worker.run_once(&ctx).await.expect("second run"),
        ConvertWorkerRun::NoJob
    );
    assert_eq!(count_published_versions(&pool, &ctx, document_id).await, 1);
    assert_eq!(count_markdown_artifacts(&pool, &ctx, document_id).await, 1);
    assert_eq!(count_index_outbox(&pool, &ctx, job.id).await, 1);
    let promoted = published_version_for_source(&pool, &ctx, version_id)
        .await
        .expect("promoted version");
    assert_eq!(
        document_current_version(&pool, &ctx, document_id).await,
        Some(promoted)
    );
    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL and MARKHAND_TEST_MINIO_*"]
async fn live_convert_worker_fault_injection_rolls_back_and_retries_promotion() {
    let Some(base_url) = take_live(test_database_url(), "MARKHAND_TEST_DATABASE_URL") else {
        return;
    };
    let Some(storage) = take_live(test_minio_client(), "MARKHAND_TEST_MINIO_*") else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let ctx = org_context(Uuid::new_v4(), Uuid::new_v4());

    for fault in [
        PromotionFault::AfterStagingPut,
        PromotionFault::AfterVersionInsert,
        PromotionFault::AfterPointerSwap,
        PromotionFault::AfterOutboxInsert,
    ] {
        let document_id = Uuid::new_v4();
        let version_id = Uuid::new_v4();
        let quarantine =
            quarantine_key(ctx.org_id(), Uuid::new_v4(), None).expect("quarantine key");
        let payload = b"fault retry\n";
        let sha256 = put_quarantine_object(
            &storage,
            &ctx,
            &quarantine,
            payload,
            "txt",
            document_id,
            version_id,
        )
        .await;
        let (document_id, version_id) = seed_org_collection_document_version(
            &pool,
            &ctx,
            document_id,
            version_id,
            &quarantine.as_str(),
            &sha256,
            payload.len() as u64,
        )
        .await;
        let job = jobs::enqueue(
            &pool,
            &ctx,
            EnqueueJob::new(
                JobType::Convert,
                JobPayload {
                    document_id: Some(document_id),
                    version_id: Some(version_id),
                    collection_id: None,
                    upload_id: None,
                    batch_id: None,
                    index_metadata_id: None,
                    cleanup_target_job_id: None,
                    related_version_id: None,

                    request_id: None,
                    traceparent: None,
                },
                format!("convert-fault-{fault:?}-{version_id}"),
            ),
        )
        .await
        .expect("enqueue")
        .job;
        let mut fault_config = stub_worker_config(ECHO_INPUT_SCRIPT, 50);
        fault_config.promotion_fault = Some(fault);
        let fault_worker =
            ConvertWorker::new(pool.clone(), storage.clone(), fault_config).expect("worker");

        assert!(matches!(
            fault_worker.run_once(&ctx).await.expect("fault run"),
            ConvertWorkerRun::Failed {
                job_id,
                terminal: false
            } if job_id == job.id
        ));
        assert!(published_version_for_source(&pool, &ctx, version_id)
            .await
            .is_none());
        assert_eq!(
            document_current_version(&pool, &ctx, document_id).await,
            None
        );
        assert_eq!(count_markdown_artifacts(&pool, &ctx, document_id).await, 0);
        assert_eq!(count_index_outbox(&pool, &ctx, job.id).await, 0);
        assert_eq!(
            quota_reservation_statuses(&pool, &ctx, job.id).await,
            vec!["refunded".to_string()]
        );
        for staged_key in checkpoint_staged_keys(&pool, &ctx, job.id).await {
            let staged_key = fileconv_server::storage::parse_key_for_org(&staged_key, ctx.org_id())
                .expect("checkpoint staged key");
            assert!(!storage
                .object_exists(ctx.org_id(), &staged_key)
                .await
                .expect("staged object existence"));
        }

        make_job_available(&pool, &ctx, job.id).await;
        let retry_outcome = run_convert_worker_until_completed(
            &pool,
            &storage,
            &ctx,
            job.id,
            stub_worker_config(ECHO_INPUT_SCRIPT, 50),
        )
        .await;
        assert!(matches!(
            retry_outcome,
            ConvertWorkerRun::Completed { job_id, .. } if job_id == job.id
        ));
        assert_eq!(count_published_versions(&pool, &ctx, document_id).await, 1);
        assert_eq!(count_markdown_artifacts(&pool, &ctx, document_id).await, 1);
        assert_eq!(count_index_outbox(&pool, &ctx, job.id).await, 1);
        assert_eq!(
            quota_reservation_statuses(&pool, &ctx, job.id).await,
            vec!["refunded".to_string(), "finalized".to_string()]
        );
    }

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL and MARKHAND_TEST_MINIO_*"]
async fn live_convert_worker_post_commit_ack_loss_preserves_committed_artifact() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let Some(storage) = test_minio_client() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let ctx = org_context(Uuid::new_v4(), Uuid::new_v4());
    let document_id = Uuid::new_v4();
    let version_id = Uuid::new_v4();
    let quarantine = quarantine_key(ctx.org_id(), Uuid::new_v4(), None).expect("quarantine key");
    let payload = b"post commit acknowledgement loss\n";
    let sha256 = put_quarantine_object(
        &storage,
        &ctx,
        &quarantine,
        payload,
        "txt",
        document_id,
        version_id,
    )
    .await;
    let (document_id, version_id) = seed_org_collection_document_version(
        &pool,
        &ctx,
        document_id,
        version_id,
        &quarantine.as_str(),
        &sha256,
        payload.len() as u64,
    )
    .await;
    let job = enqueue_convert(&pool, &ctx, document_id, version_id).await;
    let mut config = stub_worker_config(ECHO_INPUT_SCRIPT, 50);
    config.promotion_fault = Some(PromotionFault::AfterCommit);
    let worker = ConvertWorker::new(pool.clone(), storage.clone(), config).expect("worker");

    assert!(matches!(
        worker.run_once(&ctx).await.expect("post-commit run"),
        ConvertWorkerRun::ReconciliationNeeded { job_id } if job_id == job.id
    ));
    assert_eq!(
        get_job(&pool, &ctx, job.id).await.status,
        JobStatus::Succeeded
    );
    let promoted = published_version_for_source(&pool, &ctx, version_id)
        .await
        .expect("committed promoted version");
    let artifact_key = markdown_artifact_key(&pool, &ctx, promoted)
        .await
        .expect("committed artifact");
    let artifact = fileconv_server::storage::parse_key_for_org(&artifact_key, ctx.org_id())
        .expect("artifact key");
    assert_eq!(
        storage
            .get_object(ctx.org_id(), &artifact)
            .await
            .expect("committed object")
            .as_ref(),
        payload
    );

    assert!(matches!(
        worker.run_once(&ctx).await.expect("reconciliation run"),
        ConvertWorkerRun::Reconciled { .. }
    ));
    assert_eq!(
        storage
            .get_object(ctx.org_id(), &artifact)
            .await
            .expect("object survives reconciliation")
            .as_ref(),
        payload
    );
    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL and MARKHAND_TEST_MINIO_*"]
async fn live_convert_worker_reconciliation_cleans_terminal_parent_leak() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let Some(storage) = test_minio_client() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let ctx = org_context(Uuid::new_v4(), Uuid::new_v4());
    let document_id = Uuid::new_v4();
    let version_id = Uuid::new_v4();
    let quarantine = quarantine_key(ctx.org_id(), Uuid::new_v4(), None).expect("quarantine key");
    let payload = b"terminal cleanup intent\n";
    let sha256 = put_quarantine_object(
        &storage,
        &ctx,
        &quarantine,
        payload,
        "txt",
        document_id,
        version_id,
    )
    .await;
    let (document_id, version_id) = seed_org_collection_document_version(
        &pool,
        &ctx,
        document_id,
        version_id,
        &quarantine.as_str(),
        &sha256,
        payload.len() as u64,
    )
    .await;
    let mut enqueue = EnqueueJob::new(
        JobType::Convert,
        JobPayload {
            document_id: Some(document_id),
            version_id: Some(version_id),
            collection_id: None,
            upload_id: None,
            batch_id: None,
            index_metadata_id: None,
            cleanup_target_job_id: None,
            related_version_id: None,

            request_id: None,
            traceparent: None,
        },
        format!("convert-terminal-cleanup-{version_id}"),
    );
    enqueue.max_attempts = 1;
    let job = jobs::enqueue(&pool, &ctx, enqueue)
        .await
        .expect("enqueue")
        .job;
    let mut failing_config = stub_worker_config(ECHO_INPUT_SCRIPT, 50);
    failing_config.promotion_fault = Some(PromotionFault::AfterStagingPut);
    failing_config.fail_cleanup_delete = true;
    let failing_worker =
        ConvertWorker::new(pool.clone(), storage.clone(), failing_config).expect("worker");

    assert!(matches!(
        failing_worker.run_once(&ctx).await.expect("failed run"),
        ConvertWorkerRun::Failed {
            job_id,
            terminal: true
        } if job_id == job.id
    ));
    let staged_key = checkpoint_staged_keys(&pool, &ctx, job.id)
        .await
        .into_iter()
        .next()
        .expect("staged key");
    let staged =
        fileconv_server::storage::parse_key_for_org(&staged_key, ctx.org_id()).expect("staged key");
    assert!(storage
        .object_exists(ctx.org_id(), &staged)
        .await
        .expect("staged object remains before reconciliation"));

    let cleanup_worker = ConvertWorker::new(
        pool.clone(),
        storage.clone(),
        stub_worker_config(ECHO_INPUT_SCRIPT, 50),
    )
    .expect("cleanup worker");
    assert!(matches!(
        cleanup_worker
            .run_once(&ctx)
            .await
            .expect("terminal cleanup reconciliation"),
        ConvertWorkerRun::Reconciled { .. }
    ));
    assert!(!storage
        .object_exists(ctx.org_id(), &staged)
        .await
        .expect("terminal cleanup removed staged object"));
    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL and MARKHAND_TEST_MINIO_*"]
async fn live_convert_worker_reconciliation_runs_with_pending_convert_work() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let Some(storage) = test_minio_client() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let ctx = org_context(Uuid::new_v4(), Uuid::new_v4());
    let document_id = Uuid::new_v4();
    let version_id = Uuid::new_v4();
    let quarantine = quarantine_key(ctx.org_id(), Uuid::new_v4(), None).expect("quarantine key");
    let payload = b"fair reconciliation scheduling\n";
    let sha256 = put_quarantine_object(
        &storage,
        &ctx,
        &quarantine,
        payload,
        "txt",
        document_id,
        version_id,
    )
    .await;
    let (document_id, version_id) = seed_org_collection_document_version(
        &pool,
        &ctx,
        document_id,
        version_id,
        &quarantine.as_str(),
        &sha256,
        payload.len() as u64,
    )
    .await;
    let first_convert = jobs::enqueue(
        &pool,
        &ctx,
        EnqueueJob::new(
            JobType::Convert,
            JobPayload {
                document_id: Some(document_id),
                version_id: Some(version_id),
                collection_id: None,
                upload_id: None,
                batch_id: None,
                index_metadata_id: None,
                cleanup_target_job_id: None,
                related_version_id: None,

                request_id: None,
                traceparent: None,
            },
            format!("convert-fair-first-{version_id}"),
        ),
    )
    .await
    .expect("enqueue first convert")
    .job;
    let second_convert = jobs::enqueue(
        &pool,
        &ctx,
        EnqueueJob::new(
            JobType::Convert,
            JobPayload {
                document_id: Some(document_id),
                version_id: Some(version_id),
                collection_id: None,
                upload_id: None,
                batch_id: None,
                index_metadata_id: None,
                cleanup_target_job_id: None,
                related_version_id: None,

                request_id: None,
                traceparent: None,
            },
            format!("convert-fair-second-{version_id}"),
        ),
    )
    .await
    .expect("enqueue second convert")
    .job;
    let parent = jobs::enqueue(
        &pool,
        &ctx,
        EnqueueJob::new(
            JobType::Convert,
            JobPayload {
                document_id: Some(document_id),
                version_id: Some(version_id),
                collection_id: None,
                upload_id: None,
                batch_id: None,
                index_metadata_id: None,
                cleanup_target_job_id: None,
                related_version_id: None,

                request_id: None,
                traceparent: None,
            },
            format!("convert-fair-cleanup-parent-{version_id}"),
        ),
    )
    .await
    .expect("enqueue cleanup parent")
    .job;
    jobs::cancel(&pool, &ctx, parent.id)
        .await
        .expect("cancel cleanup parent");
    jobs::enqueue(
        &pool,
        &ctx,
        EnqueueJob::new(
            JobType::Reconcile,
            JobPayload {
                document_id: Some(document_id),
                version_id: Some(version_id),
                collection_id: None,
                upload_id: None,
                batch_id: None,
                index_metadata_id: None,
                cleanup_target_job_id: Some(parent.id),
                related_version_id: None,

                request_id: None,
                traceparent: None,
            },
            format!("convert.cleanup:{}", parent.id),
        ),
    )
    .await
    .expect("enqueue reconciliation");
    let worker = ConvertWorker::new(
        pool.clone(),
        storage,
        stub_worker_config(ECHO_INPUT_SCRIPT, 50),
    )
    .expect("worker");

    assert!(matches!(
        worker.run_once(&ctx).await.expect("first scheduling run"),
        ConvertWorkerRun::Completed { .. }
    ));
    assert!(matches!(
        worker
            .run_once(&ctx)
            .await
            .expect("fair reconciliation run"),
        ConvertWorkerRun::Reconciled { .. }
    ));
    assert!(
        matches!(
            get_job(&pool, &ctx, first_convert.id).await.status,
            JobStatus::Pending
        ) || matches!(
            get_job(&pool, &ctx, second_convert.id).await.status,
            JobStatus::Pending
        ),
        "one conversion must remain pending when reconciliation is selected"
    );
    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL and MARKHAND_TEST_MINIO_*"]
async fn live_convert_worker_tombstone_before_promotion_does_not_regress_document_state() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let Some(storage) = test_minio_client() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let ctx = org_context(Uuid::new_v4(), Uuid::new_v4());
    let document_id = Uuid::new_v4();
    let version_id = Uuid::new_v4();
    let quarantine = quarantine_key(ctx.org_id(), Uuid::new_v4(), None).expect("quarantine key");
    let payload = b"tombstone before promotion\n";
    let sha256 = put_quarantine_object(
        &storage,
        &ctx,
        &quarantine,
        payload,
        "txt",
        document_id,
        version_id,
    )
    .await;
    let (document_id, version_id) = seed_org_collection_document_version(
        &pool,
        &ctx,
        document_id,
        version_id,
        &quarantine.as_str(),
        &sha256,
        payload.len() as u64,
    )
    .await;
    let job = enqueue_convert(&pool, &ctx, document_id, version_id).await;
    let pause = ConvertWorkerPause {
        staged: Arc::new(Notify::new()),
        release: Arc::new(Notify::new()),
    };
    let mut config = stub_worker_config(ECHO_INPUT_SCRIPT, 50);
    config.pause_after_staging = Some(pause.clone());
    let worker = ConvertWorker::new(pool.clone(), storage.clone(), config).expect("worker");
    let staged_signal = Arc::clone(&pause.staged);
    let worker_ctx = ctx.clone();
    let worker_handle = tokio::spawn(async move { worker.run_once(&worker_ctx).await });

    staged_signal.notified().await;
    with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "UPDATE documents
                     SET state = 'tombstoned', deleted_at = clock_timestamp()
                     WHERE org_id = $1 AND id = $2",
                    &[&ctx.org_id(), &document_id],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("concurrent tombstone");
    pause.release.notify_waiters();

    assert!(matches!(
        worker_handle.await.expect("worker join").expect("worker run"),
        ConvertWorkerRun::Failed {
            job_id,
            terminal: false
        } if job_id == job.id
    ));
    assert_eq!(document_state(&pool, &ctx, document_id).await, "tombstoned");
    assert_eq!(count_published_versions(&pool, &ctx, document_id).await, 0);
    assert_eq!(count_markdown_artifacts(&pool, &ctx, document_id).await, 0);
    assert_eq!(count_index_outbox(&pool, &ctx, job.id).await, 0);
    for staged_key in checkpoint_staged_keys(&pool, &ctx, job.id).await {
        let staged_key = fileconv_server::storage::parse_key_for_org(&staged_key, ctx.org_id())
            .expect("staged key");
        assert!(!storage
            .object_exists(ctx.org_id(), &staged_key)
            .await
            .expect("staged object cleaned"));
    }
    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL and MARKHAND_TEST_MINIO_*"]
async fn live_convert_worker_reclaim_style_retry_keeps_committed_attempt_object_present() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let Some(storage) = test_minio_client() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let ctx = org_context(Uuid::new_v4(), Uuid::new_v4());
    let document_id = Uuid::new_v4();
    let version_id = Uuid::new_v4();
    let quarantine = quarantine_key(ctx.org_id(), Uuid::new_v4(), None).expect("quarantine key");
    let payload = b"race retry\n";
    let sha256 = put_quarantine_object(
        &storage,
        &ctx,
        &quarantine,
        payload,
        "txt",
        document_id,
        version_id,
    )
    .await;
    let (document_id, version_id) = seed_org_collection_document_version(
        &pool,
        &ctx,
        document_id,
        version_id,
        &quarantine.as_str(),
        &sha256,
        payload.len() as u64,
    )
    .await;
    let job = jobs::enqueue(
        &pool,
        &ctx,
        EnqueueJob::new(
            JobType::Convert,
            JobPayload {
                document_id: Some(document_id),
                version_id: Some(version_id),
                collection_id: None,
                upload_id: None,
                batch_id: None,
                index_metadata_id: None,
                cleanup_target_job_id: None,
                related_version_id: None,

                request_id: None,
                traceparent: None,
            },
            format!("convert-reclaim-race-{version_id}"),
        ),
    )
    .await
    .expect("enqueue")
    .job;
    let mut first_config = stub_worker_config(ECHO_INPUT_SCRIPT, 50);
    first_config.promotion_fault = Some(PromotionFault::AfterStagingPut);
    first_config.fail_cleanup_delete = true;
    let first_worker =
        ConvertWorker::new(pool.clone(), storage.clone(), first_config).expect("first worker");
    assert!(matches!(
        first_worker.run_once(&ctx).await.expect("first attempt"),
        ConvertWorkerRun::Failed { job_id, .. } if job_id == job.id
    ));
    let attempt_one_key_raw = checkpoint_staged_keys(&pool, &ctx, job.id)
        .await
        .into_iter()
        .next()
        .expect("attempt one staged key");
    let attempt_one_key =
        fileconv_server::storage::parse_key_for_org(&attempt_one_key_raw, ctx.org_id())
            .expect("attempt one key");
    assert!(storage
        .object_exists(ctx.org_id(), &attempt_one_key)
        .await
        .expect("attempt one exists"));

    make_job_available(&pool, &ctx, job.id).await;
    let retry_worker = ConvertWorker::new(
        pool.clone(),
        storage.clone(),
        stub_worker_config(ECHO_INPUT_SCRIPT, 50),
    )
    .expect("retry worker");
    assert!(matches!(
        retry_worker.run_once(&ctx).await.expect("retry attempt"),
        ConvertWorkerRun::Completed { job_id, .. } if job_id == job.id
    ));

    assert_eq!(count_published_versions(&pool, &ctx, document_id).await, 1);
    assert_eq!(count_markdown_artifacts(&pool, &ctx, document_id).await, 1);
    let committed_key = first_markdown_artifact_key(&pool, &ctx, document_id).await;
    assert_ne!(committed_key, attempt_one_key_raw);
    let committed = fileconv_server::storage::parse_key_for_org(&committed_key, ctx.org_id())
        .expect("committed key");
    assert!(storage
        .object_exists(ctx.org_id(), &committed)
        .await
        .expect("committed exists"));
    assert!(!storage
        .object_exists(ctx.org_id(), &attempt_one_key)
        .await
        .expect("attempt one cleaned"));

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL and MARKHAND_TEST_MINIO_*"]
async fn live_convert_worker_barrier_reclaim_promote_before_old_compensation_keeps_committed_object(
) {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let Some(storage) = test_minio_client() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let ctx = org_context(Uuid::new_v4(), Uuid::new_v4());
    let document_id = Uuid::new_v4();
    let version_id = Uuid::new_v4();
    let quarantine = quarantine_key(ctx.org_id(), Uuid::new_v4(), None).expect("quarantine key");
    let payload = b"barrier reclaim\n";
    let sha256 = put_quarantine_object(
        &storage,
        &ctx,
        &quarantine,
        payload,
        "txt",
        document_id,
        version_id,
    )
    .await;
    let (document_id, version_id) = seed_org_collection_document_version(
        &pool,
        &ctx,
        document_id,
        version_id,
        &quarantine.as_str(),
        &sha256,
        payload.len() as u64,
    )
    .await;
    let job = jobs::enqueue(
        &pool,
        &ctx,
        EnqueueJob::new(
            JobType::Convert,
            JobPayload {
                document_id: Some(document_id),
                version_id: Some(version_id),
                collection_id: None,
                upload_id: None,
                batch_id: None,
                index_metadata_id: None,
                cleanup_target_job_id: None,
                related_version_id: None,

                request_id: None,
                traceparent: None,
            },
            format!("convert-barrier-reclaim-{version_id}"),
        ),
    )
    .await
    .expect("enqueue")
    .job;

    let pause = ConvertWorkerPause {
        staged: Arc::new(Notify::new()),
        release: Arc::new(Notify::new()),
    };
    let mut a_config = stub_worker_config(ECHO_INPUT_SCRIPT, 50);
    a_config.lease_ttl = Duration::from_secs(1);
    a_config.heartbeat_interval = Duration::from_millis(100);
    a_config.promotion_fault = Some(PromotionFault::AfterStagingPut);
    a_config.pause_after_staging = Some(pause.clone());
    let worker_a = ConvertWorker::new(pool.clone(), storage.clone(), a_config).expect("worker a");
    let run_ctx = ctx.clone();
    let run_worker = worker_a.clone();
    let staged_signal = Arc::clone(&pause.staged);
    let handle_a = tokio::spawn(async move { run_worker.run_once(&run_ctx).await });

    staged_signal.notified().await;
    let claimed_a = get_job(&pool, &ctx, job.id).await;
    let key_a = staging_key_for_claim(&ctx, document_id, version_id, &claimed_a);
    assert!(storage
        .object_exists(ctx.org_id(), &key_a)
        .await
        .expect("key A exists after stage"));

    let reclaimed = reclaim_job_to_pending(&pool, &ctx, job.id).await;
    assert_eq!(reclaimed.id, job.id);
    assert_eq!(reclaimed.status, JobStatus::Pending);

    let worker_b = ConvertWorker::new(
        pool.clone(),
        storage.clone(),
        stub_worker_config(ECHO_INPUT_SCRIPT, 50),
    )
    .expect("worker b");
    let worker_b_result = worker_b.run_once(&ctx).await.expect("worker b promote");
    assert!(
        matches!(
            &worker_b_result,
            ConvertWorkerRun::Completed { job_id, .. } if *job_id == job.id
        ),
        "worker b promote: {worker_b_result:?}"
    );
    let committed_key = first_markdown_artifact_key(&pool, &ctx, document_id).await;
    assert_ne!(committed_key, key_a.as_str());
    let committed = fileconv_server::storage::parse_key_for_org(&committed_key, ctx.org_id())
        .expect("committed key");
    let committed_bytes = storage
        .get_object(ctx.org_id(), &committed)
        .await
        .expect("committed object before A release");
    assert_eq!(committed_bytes.as_ref(), payload);

    pause.release.notify_waiters();
    match handle_a.await.expect("worker a join") {
        Ok(ConvertWorkerRun::LeaseLost { job_id }) if job_id == job.id => {}
        Err(_) => {}
        other => panic!("unexpected worker A outcome: {other:?}"),
    }

    assert_eq!(count_published_versions(&pool, &ctx, document_id).await, 1);
    assert_eq!(count_markdown_artifacts(&pool, &ctx, document_id).await, 1);
    assert_eq!(
        first_markdown_artifact_key(&pool, &ctx, document_id).await,
        committed_key
    );
    let committed_after = storage
        .get_object(ctx.org_id(), &committed)
        .await
        .expect("committed object after A compensation");
    assert_eq!(committed_after.as_ref(), payload);
    assert!(!storage
        .object_exists(ctx.org_id(), &key_a)
        .await
        .expect("key A cleaned"));

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL and MARKHAND_TEST_MINIO_*"]
async fn live_convert_worker_checkpointed_key_cleans_ambiguous_after_put() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let Some(storage) = test_minio_client() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let ctx = org_context(Uuid::new_v4(), Uuid::new_v4());
    let document_id = Uuid::new_v4();
    let version_id = Uuid::new_v4();
    let quarantine = quarantine_key(ctx.org_id(), Uuid::new_v4(), None).expect("quarantine key");
    let payload = b"ambiguous put\n";
    let sha256 = put_quarantine_object(
        &storage,
        &ctx,
        &quarantine,
        payload,
        "txt",
        document_id,
        version_id,
    )
    .await;
    let (document_id, version_id) = seed_org_collection_document_version(
        &pool,
        &ctx,
        document_id,
        version_id,
        &quarantine.as_str(),
        &sha256,
        payload.len() as u64,
    )
    .await;
    let job = enqueue_convert(&pool, &ctx, document_id, version_id).await;
    let mut ambiguous_config = stub_worker_config(ECHO_INPUT_SCRIPT, 50);
    ambiguous_config.lose_staged_handle_after_put = true;
    let ambiguous_worker =
        ConvertWorker::new(pool.clone(), storage.clone(), ambiguous_config).expect("worker");
    assert!(matches!(
        ambiguous_worker.run_once(&ctx).await.expect("ambiguous run"),
        ConvertWorkerRun::Failed { job_id, .. } if job_id == job.id
    ));
    for staged_key in checkpoint_staged_keys(&pool, &ctx, job.id).await {
        let staged_key = fileconv_server::storage::parse_key_for_org(&staged_key, ctx.org_id())
            .expect("checkpoint staged key");
        assert!(!storage
            .object_exists(ctx.org_id(), &staged_key)
            .await
            .expect("checkpoint cleanup"));
    }

    make_job_available(&pool, &ctx, job.id).await;
    let retry_worker = ConvertWorker::new(
        pool.clone(),
        storage.clone(),
        stub_worker_config(ECHO_INPUT_SCRIPT, 50),
    )
    .expect("retry worker");
    assert!(matches!(
        retry_worker.run_once(&ctx).await.expect("retry"),
        ConvertWorkerRun::Completed { job_id, .. } if job_id == job.id
    ));
    assert_eq!(count_published_versions(&pool, &ctx, document_id).await, 1);
    assert_eq!(count_markdown_artifacts(&pool, &ctx, document_id).await, 1);

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL and MARKHAND_TEST_MINIO_*"]
async fn live_convert_worker_delete_failure_is_surfaced_and_retry_cleans() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let Some(storage) = test_minio_client() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let ctx = org_context(Uuid::new_v4(), Uuid::new_v4());
    let document_id = Uuid::new_v4();
    let version_id = Uuid::new_v4();
    let quarantine = quarantine_key(ctx.org_id(), Uuid::new_v4(), None).expect("quarantine key");
    let payload = b"delete failure\n";
    let sha256 = put_quarantine_object(
        &storage,
        &ctx,
        &quarantine,
        payload,
        "txt",
        document_id,
        version_id,
    )
    .await;
    let (document_id, version_id) = seed_org_collection_document_version(
        &pool,
        &ctx,
        document_id,
        version_id,
        &quarantine.as_str(),
        &sha256,
        payload.len() as u64,
    )
    .await;
    let job = enqueue_convert(&pool, &ctx, document_id, version_id).await;
    let mut config = stub_worker_config(ECHO_INPUT_SCRIPT, 50);
    config.promotion_fault = Some(PromotionFault::AfterStagingPut);
    config.fail_cleanup_delete = true;
    let worker = ConvertWorker::new(pool.clone(), storage.clone(), config).expect("worker");
    assert!(matches!(
        worker.run_once(&ctx).await.expect("delete failure run"),
        ConvertWorkerRun::Failed { job_id, .. } if job_id == job.id
    ));
    assert_eq!(
        get_job(&pool, &ctx, job.id).await.last_error.as_deref(),
        Some("convert compensation deferred")
    );
    let attempt_one_key_raw = checkpoint_staged_keys(&pool, &ctx, job.id)
        .await
        .into_iter()
        .next()
        .expect("attempt one staged key");
    let attempt_one_key =
        fileconv_server::storage::parse_key_for_org(&attempt_one_key_raw, ctx.org_id())
            .expect("attempt one key");
    assert!(storage
        .object_exists(ctx.org_id(), &attempt_one_key)
        .await
        .expect("left for retry"));

    make_job_available(&pool, &ctx, job.id).await;
    let retry_worker = ConvertWorker::new(
        pool.clone(),
        storage.clone(),
        stub_worker_config(ECHO_INPUT_SCRIPT, 50),
    )
    .expect("retry worker");
    assert!(matches!(
        retry_worker.run_once(&ctx).await.expect("retry"),
        ConvertWorkerRun::Completed { job_id, .. } if job_id == job.id
    ));
    assert!(!storage
        .object_exists(ctx.org_id(), &attempt_one_key)
        .await
        .expect("cleaned on retry"));
    assert_eq!(count_published_versions(&pool, &ctx, document_id).await, 1);
    assert_eq!(count_markdown_artifacts(&pool, &ctx, document_id).await, 1);

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL and MARKHAND_TEST_MINIO_*"]
async fn live_convert_worker_refund_failure_expires_via_quota_sweep_and_retries() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let Some(storage) = test_minio_client() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let ctx = org_context(Uuid::new_v4(), Uuid::new_v4());
    let document_id = Uuid::new_v4();
    let version_id = Uuid::new_v4();
    let quarantine = quarantine_key(ctx.org_id(), Uuid::new_v4(), None).expect("quarantine key");
    let payload = b"refund failure\n";
    let sha256 = put_quarantine_object(
        &storage,
        &ctx,
        &quarantine,
        payload,
        "txt",
        document_id,
        version_id,
    )
    .await;
    let (document_id, version_id) = seed_org_collection_document_version(
        &pool,
        &ctx,
        document_id,
        version_id,
        &quarantine.as_str(),
        &sha256,
        payload.len() as u64,
    )
    .await;
    let job = enqueue_convert(&pool, &ctx, document_id, version_id).await;
    let mut config = stub_worker_config(ECHO_INPUT_SCRIPT, 50);
    config.promotion_fault = Some(PromotionFault::AfterStagingPut);
    config.fail_quota_refund = true;
    config.quota_reservation_ttl = Duration::from_secs(1);
    let worker = ConvertWorker::new(pool.clone(), storage.clone(), config).expect("worker");
    assert!(matches!(
        worker.run_once(&ctx).await.expect("refund failure run"),
        ConvertWorkerRun::Failed { job_id, .. } if job_id == job.id
    ));
    assert_eq!(
        get_job(&pool, &ctx, job.id).await.last_error.as_deref(),
        Some("convert compensation deferred")
    );
    assert_eq!(
        quota_reservation_statuses(&pool, &ctx, job.id).await,
        vec!["reserved".to_string()]
    );
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(
        quota::expire_reserved(&pool, &ctx, 10)
            .await
            .expect("quota sweep"),
        1
    );
    assert_eq!(
        quota_reservation_statuses(&pool, &ctx, job.id).await,
        vec!["expired".to_string()]
    );

    make_job_available(&pool, &ctx, job.id).await;
    let retry_worker = ConvertWorker::new(
        pool.clone(),
        storage.clone(),
        stub_worker_config(ECHO_INPUT_SCRIPT, 50),
    )
    .expect("retry worker");
    assert!(matches!(
        retry_worker.run_once(&ctx).await.expect("retry"),
        ConvertWorkerRun::Completed { job_id, .. } if job_id == job.id
    ));
    assert_eq!(count_published_versions(&pool, &ctx, document_id).await, 1);
    assert_eq!(count_markdown_artifacts(&pool, &ctx, document_id).await, 1);
    assert_eq!(
        quota_reservation_statuses(&pool, &ctx, job.id).await,
        vec!["expired".to_string(), "finalized".to_string()]
    );

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL and MARKHAND_TEST_MINIO_*"]
async fn live_convert_worker_second_promotion_demotes_current_and_preserves_original() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let Some(storage) = test_minio_client() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let ctx = org_context(Uuid::new_v4(), Uuid::new_v4());
    let document_id = Uuid::new_v4();
    let source_one = Uuid::new_v4();
    let quarantine_one =
        quarantine_key(ctx.org_id(), Uuid::new_v4(), None).expect("quarantine key");
    let payload_one = b"first source\n";
    let sha_one = put_quarantine_object(
        &storage,
        &ctx,
        &quarantine_one,
        payload_one,
        "txt",
        document_id,
        source_one,
    )
    .await;
    let (document_id, source_one) = seed_org_collection_document_version(
        &pool,
        &ctx,
        document_id,
        source_one,
        &quarantine_one.as_str(),
        &sha_one,
        payload_one.len() as u64,
    )
    .await;
    let first_job = enqueue_convert(&pool, &ctx, document_id, source_one).await;
    let worker = ConvertWorker::new(
        pool.clone(),
        storage.clone(),
        stub_worker_config(ECHO_INPUT_SCRIPT, 50),
    )
    .expect("worker");
    assert!(matches!(
        worker.run_once(&ctx).await.expect("first promotion"),
        ConvertWorkerRun::Completed { job_id, .. } if job_id == first_job.id
    ));
    let first_promoted = published_version_for_source(&pool, &ctx, source_one)
        .await
        .expect("first promoted");

    let source_two = Uuid::new_v4();
    let quarantine_two =
        quarantine_key(ctx.org_id(), Uuid::new_v4(), None).expect("quarantine key");
    let payload_two = b"second source\n";
    let sha_two = put_quarantine_object(
        &storage,
        &ctx,
        &quarantine_two,
        payload_two,
        "txt",
        document_id,
        source_two,
    )
    .await;
    seed_additional_source_version(
        &pool,
        &ctx,
        document_id,
        source_two,
        &quarantine_two.as_str(),
        &sha_two,
        payload_two.len() as u64,
    )
    .await;
    let second_job = enqueue_convert(&pool, &ctx, document_id, source_two).await;
    assert!(matches!(
        worker.run_once(&ctx).await.expect("second promotion"),
        ConvertWorkerRun::Completed { job_id, .. } if job_id == second_job.id
    ));
    let second_promoted = published_version_for_source(&pool, &ctx, source_two)
        .await
        .expect("second promoted");

    assert_eq!(count_published_versions(&pool, &ctx, document_id).await, 2);
    assert_eq!(
        document_current_version(&pool, &ctx, document_id).await,
        Some(second_promoted)
    );
    assert_eq!(
        version_current_and_expired(&pool, &ctx, first_promoted).await,
        (false, true)
    );
    assert_eq!(
        version_current_and_expired(&pool, &ctx, second_promoted).await,
        (true, false)
    );
    assert!(illegal_version_content_update_is_rejected(&pool, &ctx, second_promoted).await);
    assert!(illegal_original_key_update_is_rejected(&pool, &ctx, source_one).await);
    let original = storage
        .get_object(ctx.org_id(), &quarantine_one)
        .await
        .expect("original quarantine object");
    assert_eq!(original.as_ref(), payload_one);
    assert!(version_inherits_document_collection(&pool, &ctx, document_id, second_promoted).await);

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL and MARKHAND_TEST_MINIO_*"]
async fn live_convert_worker_converter_error_retries_job() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let Some(storage) = test_minio_client() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let ctx = org_context(Uuid::new_v4(), Uuid::new_v4());
    let document_id = Uuid::new_v4();
    let version_id = Uuid::new_v4();
    let quarantine = quarantine_key(ctx.org_id(), Uuid::new_v4(), None).expect("quarantine key");
    let payload = b"bad input\n";
    let sha256 = put_quarantine_object(
        &storage,
        &ctx,
        &quarantine,
        payload,
        "txt",
        document_id,
        version_id,
    )
    .await;
    let (document_id, version_id) = seed_org_collection_document_version(
        &pool,
        &ctx,
        document_id,
        version_id,
        &quarantine.as_str(),
        &sha256,
        payload.len() as u64,
    )
    .await;
    let job = enqueue_convert(&pool, &ctx, document_id, version_id).await;
    let worker = ConvertWorker::new(
        pool.clone(),
        storage,
        stub_worker_config("printf malformed >&2; exit 9", 50),
    )
    .expect("worker");

    let outcome = worker.run_once(&ctx).await.expect("run once");
    assert!(matches!(
        outcome,
        ConvertWorkerRun::Failed {
            job_id,
            terminal: false
        } if job_id == job.id
    ));
    let stored = get_job(&pool, &ctx, job.id).await;
    assert_eq!(stored.status, JobStatus::Pending);
    assert_eq!(
        stored.last_error.as_deref(),
        Some("converter exited unsuccessfully")
    );
    assert!(markdown_artifact_key(&pool, &ctx, version_id)
        .await
        .is_none());
    assert!(published_version_for_source(&pool, &ctx, version_id)
        .await
        .is_none());
    assert_eq!(
        document_current_version(&pool, &ctx, document_id).await,
        None
    );
    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL and MARKHAND_TEST_MINIO_*"]
async fn live_convert_worker_cancel_loses_lease_and_kills_sandbox() {
    let Some(base_url) = take_live(test_database_url(), "MARKHAND_TEST_DATABASE_URL") else {
        return;
    };
    let Some(storage) = take_live(test_minio_client(), "MARKHAND_TEST_MINIO_*") else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let ctx = org_context(Uuid::new_v4(), Uuid::new_v4());
    let document_id = Uuid::new_v4();
    let version_id = Uuid::new_v4();
    let quarantine = quarantine_key(ctx.org_id(), Uuid::new_v4(), None).expect("quarantine key");
    let payload = b"cancel me\n";
    let sha256 = put_quarantine_object(
        &storage,
        &ctx,
        &quarantine,
        payload,
        "txt",
        document_id,
        version_id,
    )
    .await;
    let (document_id, version_id) = seed_org_collection_document_version(
        &pool,
        &ctx,
        document_id,
        version_id,
        &quarantine.as_str(),
        &sha256,
        payload.len() as u64,
    )
    .await;
    let job = enqueue_convert(&pool, &ctx, document_id, version_id).await;
    let mut config = ConvertWorkerConfig::new(
        format!("test-worker-{}", Uuid::new_v4()),
        python(
            r#"
import os, time
pid = os.fork()
if pid == 0:
    os.setsid()
    while True:
        time.sleep(60)
else:
    time.sleep(600)
"#,
            Duration::from_secs(10),
        )
        .expect("python config"),
    );
    config.heartbeat_interval = Duration::from_millis(50);
    config.lease_ttl = Duration::from_secs(2);
    let worker = ConvertWorker::new(pool.clone(), storage, config).expect("worker");
    let run_ctx = ctx.clone();
    let run_worker = worker.clone();
    let handle = tokio::spawn(async move { run_worker.run_once(&run_ctx).await });
    tokio::time::sleep(Duration::from_millis(150)).await;
    jobs::cancel(&pool, &ctx, job.id).await.expect("cancel");

    let outcome = handle.await.expect("join").expect("run once");
    assert_eq!(outcome, ConvertWorkerRun::LeaseLost { job_id: job.id });
    assert_eq!(
        get_job(&pool, &ctx, job.id).await.status,
        JobStatus::Cancelled
    );
    assert!(markdown_artifact_key(&pool, &ctx, version_id)
        .await
        .is_none());
    assert!(published_version_for_source(&pool, &ctx, version_id)
        .await
        .is_none());
    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL and MARKHAND_TEST_MINIO_*"]
async fn live_convert_worker_cancel_after_upload_cleans_generated_object_via_reconciliation() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let Some(storage) = test_minio_client() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let ctx = org_context(Uuid::new_v4(), Uuid::new_v4());
    let document_id = Uuid::new_v4();
    let version_id = Uuid::new_v4();
    let quarantine = quarantine_key(ctx.org_id(), Uuid::new_v4(), None).expect("quarantine key");
    let payload = b"stale upload cleanup\n";
    let sha256 = put_quarantine_object(
        &storage,
        &ctx,
        &quarantine,
        payload,
        "txt",
        document_id,
        version_id,
    )
    .await;
    let (document_id, version_id) = seed_org_collection_document_version(
        &pool,
        &ctx,
        document_id,
        version_id,
        &quarantine.as_str(),
        &sha256,
        payload.len() as u64,
    )
    .await;
    let job = enqueue_convert(&pool, &ctx, document_id, version_id).await;
    let mut config = stub_worker_config("printf converted-after-upload", 50);
    config.post_upload_settlement_delay = Duration::from_secs(1);
    let worker = ConvertWorker::new(pool.clone(), storage.clone(), config).expect("worker");
    let run_ctx = ctx.clone();
    let run_worker = worker.clone();
    let handle = tokio::spawn(async move { run_worker.run_once(&run_ctx).await });
    tokio::time::sleep(Duration::from_millis(250)).await;
    let staged_key_raw = checkpoint_staged_keys(&pool, &ctx, job.id)
        .await
        .into_iter()
        .next()
        .expect("checkpoint staged key");
    let generated_trusted =
        fileconv_server::storage::parse_key_for_org(&staged_key_raw, ctx.org_id())
            .expect("trusted key");
    jobs::cancel(&pool, &ctx, job.id).await.expect("cancel");

    let outcome = handle.await.expect("join").expect("run once");
    assert_eq!(outcome, ConvertWorkerRun::LeaseLost { job_id: job.id });
    assert_eq!(
        get_job(&pool, &ctx, job.id).await.status,
        JobStatus::Cancelled
    );
    assert!(markdown_artifact_key(&pool, &ctx, version_id)
        .await
        .is_none());
    assert!(published_version_for_source(&pool, &ctx, version_id)
        .await
        .is_none());
    assert!(matches!(
        worker
            .run_once(&ctx)
            .await
            .expect("reconciliation run after cancellation"),
        ConvertWorkerRun::Reconciled { .. }
    ));
    assert!(!storage
        .object_exists(ctx.org_id(), &generated_trusted)
        .await
        .expect("trusted object existence"));
    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL and MARKHAND_TEST_MINIO_*"]
async fn live_convert_worker_resource_failures_are_bounded_job_failures() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let Some(storage) = test_minio_client() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let ctx = org_context(Uuid::new_v4(), Uuid::new_v4());
    let cases = [
        (
            "ram",
            r#"
chunks = []
while True:
    chunks.append(bytearray(16 * 1024 * 1024))
"#,
        ),
        (
            "disk",
            r#"
with open("huge.bin", "wb") as f:
    while True:
        f.write(b"x" * 4096)
"#,
        ),
        (
            "fork",
            r#"
import os, time
while True:
    try:
        pid = os.fork()
    except OSError:
        time.sleep(60)
        break
    if pid == 0:
        time.sleep(60)
"#,
        ),
    ];
    for (name, script) in cases {
        let document_id = Uuid::new_v4();
        let version_id = Uuid::new_v4();
        let quarantine =
            quarantine_key(ctx.org_id(), Uuid::new_v4(), None).expect("quarantine key");
        let payload = b"resource failure\n";
        let sha256 = put_quarantine_object(
            &storage,
            &ctx,
            &quarantine,
            payload,
            "txt",
            document_id,
            version_id,
        )
        .await;
        let (document_id, version_id) = seed_org_collection_document_version(
            &pool,
            &ctx,
            document_id,
            version_id,
            &quarantine.as_str(),
            &sha256,
            payload.len() as u64,
        )
        .await;
        let job = jobs::enqueue(
            &pool,
            &ctx,
            EnqueueJob::new(
                JobType::Convert,
                JobPayload {
                    document_id: Some(document_id),
                    version_id: Some(version_id),
                    collection_id: None,
                    upload_id: None,
                    batch_id: None,
                    index_metadata_id: None,
                    cleanup_target_job_id: None,
                    related_version_id: None,

                    request_id: None,
                    traceparent: None,
                },
                format!("convert-resource-{name}-{version_id}"),
            ),
        )
        .await
        .expect("enqueue")
        .job;
        let mut sandbox = python(script, Duration::from_secs(5)).expect("python config");
        match name {
            "ram" => sandbox.limits.memory_bytes = 64 * 1024 * 1024,
            "disk" => sandbox.limits.file_size_bytes = 64 * 1024,
            "fork" => {
                sandbox.limits.max_processes = 8;
                sandbox.limits.wall_timeout = Duration::from_millis(500);
            }
            _ => unreachable!("known resource case"),
        }
        let mut config = ConvertWorkerConfig::new(format!("worker-{name}"), sandbox);
        config.heartbeat_interval = Duration::from_millis(50);
        config.lease_ttl = Duration::from_secs(2);
        let worker = ConvertWorker::new(pool.clone(), storage.clone(), config).expect("worker");
        let outcome = worker.run_once(&ctx).await.expect("run once");
        assert!(matches!(
            outcome,
            ConvertWorkerRun::Failed {
                job_id,
                terminal: false
            } if job_id == job.id
        ));
        assert_eq!(
            get_job(&pool, &ctx, job.id).await.status,
            JobStatus::Pending
        );
        assert!(markdown_artifact_key(&pool, &ctx, version_id)
            .await
            .is_none());
    }
    assert!(
        sandbox_available(),
        "host should remain able to spawn sandbox"
    );
    ephemeral.drop().await;
}

fn fileconv_binary() -> Option<PathBuf> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/fileconv");
    path.exists().then_some(path)
}

fn assert_process_exits(pid: u32, timeout: Duration) {
    let proc_path = PathBuf::from(format!("/proc/{pid}"));
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !proc_path.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("process {pid} survived sandbox kill");
}
