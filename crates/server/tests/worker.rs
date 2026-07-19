//! Converter worker and sandbox tests.

use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
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
use fileconv_server::storage::keys::{quarantine_key, trusted_key};
use fileconv_server::storage::minio::{MinioClient, ObjectIdentityMeta};
use fileconv_server::workers::convert::{ConvertWorker, ConvertWorkerConfig, ConvertWorkerRun};
use fileconv_server::workers::limits::ResourceLimits;
use fileconv_server::workers::sandbox::{
    self, SandboxCancel, SandboxConfig, SandboxExit, SandboxInput,
};
use sha2::{Digest, Sha256};
use tokio_postgres::NoTls;
use uuid::Uuid;

const INPUT: &str = "{input}";
static SANDBOX_TEST_LOCK: Mutex<()> = Mutex::new(());

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
        let _ = admin
            .batch_execute(&format!(
                "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
                 WHERE datname = '{}' AND pid <> pg_backend_pid(); \
                 DROP DATABASE IF EXISTS \"{}\"",
                self.db_name, self.db_name
            ))
            .await;
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
    print("daemon-ready", flush=True)
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
async fn live_convert_worker_completes_and_stores_trusted_markdown() {
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
    let artifact_key = markdown_artifact_key(&pool, &ctx, version_id)
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
    ephemeral.drop().await;
}

#[tokio::test]
async fn live_convert_worker_cancel_loses_lease_and_kills_sandbox() {
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
    ephemeral.drop().await;
}

#[tokio::test]
async fn live_convert_worker_cancel_after_upload_cleans_generated_object() {
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
    let generated_trusted =
        trusted_key(ctx.org_id(), version_id, job.id, None).expect("trusted key");
    let mut config = stub_worker_config("printf converted-after-upload", 50);
    config.post_upload_settlement_delay = Duration::from_secs(1);
    let worker = ConvertWorker::new(pool.clone(), storage.clone(), config).expect("worker");
    let run_ctx = ctx.clone();
    let run_worker = worker.clone();
    let handle = tokio::spawn(async move { run_worker.run_once(&run_ctx).await });
    tokio::time::sleep(Duration::from_millis(250)).await;
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
    assert!(!storage
        .object_exists(ctx.org_id(), &generated_trusted)
        .await
        .expect("trusted object existence"));
    ephemeral.drop().await;
}

#[tokio::test]
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
