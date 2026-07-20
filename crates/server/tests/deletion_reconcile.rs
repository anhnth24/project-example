//! Live tests for tombstone delete and reconciliation.
//!
//! These tests skip cleanly unless PostgreSQL, MinIO, and Qdrant test endpoints
//! are provided in the environment.

use bytes::Bytes;
use deadpool_postgres::Pool;
use fileconv_knowledge::embedding::{EmbeddingPlan, ProviderDeployment, RUNTIME_VLLM_LOCAL};
use fileconv_server::auth::context::OrgContext;
use fileconv_server::config::{MinioConfig, Profile, SecretString};
use fileconv_server::database::apply_migrations;
use fileconv_server::db::collections::{self, NewCollection};
use fileconv_server::db::documents::{self, NewDocument};
use fileconv_server::db::models::{ArtifactKind, CollectionVisibility, Document, DocumentState};
use fileconv_server::db::orgs;
use fileconv_server::db::pool::{create_pool, with_org_txn};
use fileconv_server::jobs::{self, CheckpointPayload, EventPayload, CURRENT_EVENT_PAYLOAD_VERSION};
use fileconv_server::services::deletion::{request_delete, DeleteRequestOutcome};
use fileconv_server::services::embedding::ApprovedEmbeddingRuntime;
use fileconv_server::services::index_signature::collection_name_for_digest;
use fileconv_server::services::indexing::{
    compensate_batch_points, enqueue_compensation_reconcile, IndexingOutboxSink,
};
use fileconv_server::services::reconciliation::{
    enqueue_reconcile, reconcile_dead_letter_jobs, reconcile_document, ReconcileMode,
};
use fileconv_server::storage::keys::parse_key_for_org;
use fileconv_server::storage::minio::{MinioClient, ObjectIdentityMeta};
use fileconv_server::storage::qdrant::{
    point_id_from_org_collection_and_chunk, ChunkPointPayload, QdrantClient, UpsertPoint,
    VectorScope,
};
use fileconv_server::storage::trusted_key;
use fileconv_server::workers::delete::{DeleteWorker, DeleteWorkerConfig, DeleteWorkerRun};
use fileconv_server::workers::embedding::{
    EmbeddingWorker, EmbeddingWorkerConfig, EmbeddingWorkerRun,
};
use fileconv_server::workers::index::{IndexWorker, IndexWorkerConfig, IndexWorkerRun};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tokio_postgres::NoTls;
use uuid::Uuid;

const TEST_VECTOR_DIMENSIONS: usize = 8;

fn test_database_url() -> Option<String> {
    match std::env::var("MARKHAND_TEST_DATABASE_URL") {
        Ok(url) if !url.trim().is_empty() => Some(url),
        _ => {
            eprintln!("skipped: MARKHAND_TEST_DATABASE_URL unset");
            None
        }
    }
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
    let bucket = format!("markhand-delete-reconcile-{}", Uuid::new_v4().simple());
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
    Some(MinioClient::from_config(&config).expect("minio client"))
}

fn test_qdrant_client() -> Option<QdrantClient> {
    let url = match std::env::var("MARKHAND_TEST_QDRANT_URL") {
        Ok(url) if !url.trim().is_empty() => url,
        _ => {
            eprintln!("skipped: MARKHAND_TEST_QDRANT_URL unset");
            return None;
        }
    };
    let api_key = std::env::var("MARKHAND_TEST_QDRANT_API_KEY")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(SecretString::new);
    Some(QdrantClient::with_api_key(url, api_key).expect("qdrant client"))
}

fn test_embedding_plan(base_url: &str) -> EmbeddingPlan {
    EmbeddingPlan::provider(
        "test",
        "test-embedding",
        "r1",
        ProviderDeployment::from_base_url(Some(base_url)).expect("test deployment"),
        Some(TEST_VECTOR_DIMENSIONS),
        RUNTIME_VLLM_LOCAL,
    )
    .expect("test embedding plan")
}

struct MockEmbeddingProvider {
    base_url: String,
    stopping: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl MockEmbeddingProvider {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock embedding provider");
        listener
            .set_nonblocking(true)
            .expect("set mock listener nonblocking");
        let base_url = format!(
            "http://{}/v1",
            listener.local_addr().expect("mock listener address")
        );
        let stopping = Arc::new(AtomicBool::new(false));
        let thread_stopping = Arc::clone(&stopping);
        let thread = thread::spawn(move || {
            while !thread_stopping.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => respond_to_embedding_request(&mut stream),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("mock embedding provider accept failed: {error}"),
                }
            }
        });
        Self {
            base_url,
            stopping,
            thread: Some(thread),
        }
    }

    fn base_url(&self) -> &str {
        &self.base_url
    }
}

impl Drop for MockEmbeddingProvider {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn respond_to_embedding_request(stream: &mut TcpStream) {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set mock stream read timeout");
    let request = read_http_request(stream);
    let body_start = request
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("mock request header terminator")
        + 4;
    let request_body = &request[body_start..];
    let input_count = serde_json::from_slice::<serde_json::Value>(request_body)
        .expect("decode embedding request")
        .get("input")
        .and_then(serde_json::Value::as_array)
        .map_or(0, Vec::len);
    let response = json!({
        "data": (0..input_count)
            .map(|index| json!({
                "index": index,
                "embedding": [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            }))
            .collect::<Vec<_>>(),
    });
    let response = serde_json::to_vec(&response).expect("encode embedding response");
    let headers = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response.len()
    );
    stream
        .write_all(headers.as_bytes())
        .expect("write mock headers");
    stream.write_all(&response).expect("write mock response");
    stream.flush().expect("flush mock response");
}

fn read_http_request(stream: &mut TcpStream) -> Vec<u8> {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 1024];
    loop {
        let read = stream.read(&mut buffer).expect("read mock request");
        assert_ne!(read, 0, "mock client closed request before completion");
        request.extend_from_slice(&buffer[..read]);
        let Some(header_end) = request
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|index| index + 4)
        else {
            continue;
        };
        let headers = std::str::from_utf8(&request[..header_end]).expect("mock request headers");
        let content_length = headers
            .lines()
            .find_map(|line| {
                line.split_once(':').and_then(|(name, value)| {
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().expect("content length"))
                })
            })
            .unwrap_or(0);
        if request.len() >= header_end + content_length {
            return request;
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
        .unwrap_or_else(|error| panic!("database connection failed: {error}"));
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
        let db_name = format!("markhand_delete_reconcile_{}", Uuid::new_v4().simple());
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

struct LiveEnv {
    db: EphemeralDb,
    pool: Pool,
    storage: MinioClient,
    qdrant: QdrantClient,
    ctx: OrgContext,
}

impl LiveEnv {
    async fn boot() -> Option<Self> {
        let base_url = test_database_url()?;
        let storage = test_minio_client()?;
        let qdrant = test_qdrant_client()?;
        storage.ensure_bucket().await.expect("ensure bucket");
        let db = EphemeralDb::create(&base_url).await;
        apply_migrations(&db.url).await.expect("apply migrations");
        let pool = create_pool(&db.url).expect("pool");
        let ctx = OrgContext::try_new(Uuid::new_v4(), Uuid::new_v4(), ["doc.upload"], [])
            .expect("org context");
        Some(Self {
            db,
            pool,
            storage,
            qdrant,
            ctx,
        })
    }

    async fn drop(self) {
        self.db.drop().await;
    }
}

async fn ensure_org(pool: &Pool, ctx: &OrgContext) {
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                orgs::ensure_exists(txn, &ctx, "delete-reconcile-org", "Delete Reconcile Org")
                    .await?;
                orgs::ensure_user(
                    txn,
                    &ctx,
                    ctx.user_id(),
                    "delete-reconcile@example.test",
                    "Worker",
                )
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
                Ok(())
            })
        }
    })
    .await
    .expect("ensure org");
}

async fn seed_converted_document(env: &LiveEnv, markdown: &str) -> (Uuid, Uuid, Uuid, String) {
    let collection_id = Uuid::new_v4();
    let document_id = Uuid::new_v4();
    let version_id = Uuid::new_v4();
    let artifact_id = Uuid::new_v4();
    let outbox_key = format!("index-request:{version_id}");
    let object_key =
        trusted_key(env.ctx.org_id(), version_id, Uuid::new_v4(), None).expect("trusted key");
    let object_key_string = object_key.as_str();
    let sha256 = hex::encode(Sha256::digest(markdown.as_bytes()));
    let markdown_len = markdown.len() as i64;
    env.storage
        .put_object(
            env.ctx.org_id(),
            &object_key,
            Bytes::copy_from_slice(markdown.as_bytes()),
            &ObjectIdentityMeta {
                org_id: env.ctx.org_id(),
                collection_id: Some(collection_id),
                document_id: Some(document_id),
                version_id: Some(version_id),
                original_filename: None,
                canonical_format: Some("md".into()),
                content_sha256: Some(sha256.clone()),
                content_length: Some(markdown.len() as u64),
                disposition: Some("trusted".into()),
            },
            "text/markdown; charset=utf-8",
        )
        .await
        .expect("put markdown");

    ensure_org(&env.pool, &env.ctx).await;
    with_org_txn(&env.pool, &env.ctx, {
        let ctx = env.ctx.clone();
        let object_key_string = object_key_string.clone();
        let sha256 = sha256.clone();
        move |txn| {
            Box::pin(async move {
                collections::insert(
                    txn,
                    &ctx,
                    NewCollection {
                        id: collection_id,
                        name: "Delete Reconcile Collection",
                        slug: &format!("delete-reconcile-{collection_id}"),
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
                        title: "Delete Reconcile Doc",
                    },
                )
                .await?;
                txn.execute(
                    "INSERT INTO document_versions (
                        id, org_id, document_id, version_number, publication_state,
                        is_current, content_sha256, original_object_key, markdown_object_key,
                        source_content_type, byte_size, created_by_user_id
                     )
                     VALUES ($1, $2, $3, 1, 'published', true, $4, $5, $5,
                             'text/markdown', $6, $7)",
                    &[
                        &version_id,
                        &ctx.org_id(),
                        &document_id,
                        &sha256,
                        &object_key_string,
                        &markdown_len,
                        &ctx.user_id(),
                    ],
                )
                .await?;
                let kind = ArtifactKind::Markdown.as_str();
                txn.execute(
                    "INSERT INTO derived_artifacts (
                        id, org_id, document_id, version_id, artifact_kind,
                        object_key, content_sha256, content_type, byte_size
                     )
                     VALUES ($1, $2, $3, $4, $5, $6, $7,
                             'text/markdown; charset=utf-8', $8)",
                    &[
                        &artifact_id,
                        &ctx.org_id(),
                        &document_id,
                        &version_id,
                        &kind,
                        &object_key_string,
                        &sha256,
                        &markdown_len,
                    ],
                )
                .await?;
                txn.execute(
                    "UPDATE documents
                     SET state = 'converted', current_version_id = $3, updated_at = clock_timestamp()
                     WHERE org_id = $1 AND id = $2",
                    &[&ctx.org_id(), &document_id, &version_id],
                )
                .await?;
                let payload = EventPayload {
                    job_id: None,
                    document_id: Some(document_id),
                    version_id: Some(version_id),
                    outbox_event_id: None,
                }
                .to_json()
                .expect("event payload");
                txn.execute(
                    "INSERT INTO outbox_events (
                        org_id, event_type, payload_version, payload, idempotency_key
                     )
                     VALUES ($1, 'document.index_requested', $2, $3, $4)",
                    &[
                        &ctx.org_id(),
                        &CURRENT_EVENT_PAYLOAD_VERSION,
                        &payload,
                        &outbox_key,
                    ],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("seed converted document");
    (document_id, version_id, collection_id, object_key_string)
}

fn index_worker(env: &LiveEnv, embedding_plan: EmbeddingPlan) -> IndexWorker {
    let mut config = IndexWorkerConfig::new(format!("index-worker-{}", Uuid::new_v4()));
    config.lease_ttl = Duration::from_secs(30);
    config.heartbeat_interval = Duration::from_secs(5);
    config.max_job_duration = Duration::from_secs(60);
    config.embedding_batch_size = 2;
    IndexWorker::new_with_plan(
        env.pool.clone(),
        env.storage.clone(),
        env.qdrant.clone(),
        config,
        None,
        embedding_plan,
    )
    .expect("index worker")
}

fn delete_worker(env: &LiveEnv) -> DeleteWorker {
    let mut config = DeleteWorkerConfig::new(format!("delete-worker-{}", Uuid::new_v4()));
    config.lease_ttl = Duration::from_secs(30);
    config.heartbeat_interval = Duration::from_secs(5);
    config.max_job_duration = Duration::from_secs(60);
    DeleteWorker::new(
        env.pool.clone(),
        env.storage.clone(),
        env.qdrant.clone(),
        config,
    )
    .expect("delete worker")
}

fn embedding_worker(env: &LiveEnv, provider: &MockEmbeddingProvider) -> EmbeddingWorker {
    let mut config = EmbeddingWorkerConfig::new(format!("embedding-worker-{}", Uuid::new_v4()));
    config.lease_ttl = Duration::from_secs(30);
    config.heartbeat_interval = Duration::from_secs(5);
    config.max_job_duration = Duration::from_secs(60);
    let runtime = ApprovedEmbeddingRuntime::new(
        provider.base_url().to_string(),
        "test-api-key".into(),
        "test".into(),
        "test-embedding".into(),
        "r1".into(),
        TEST_VECTOR_DIMENSIONS,
        RUNTIME_VLLM_LOCAL.into(),
        Profile::Test,
        false,
        None,
    )
    .expect("mock embedding runtime");
    EmbeddingWorker::new(env.pool.clone(), env.qdrant.clone(), config, runtime)
        .expect("embedding worker")
}

async fn run_embedding_jobs(env: &LiveEnv, worker: &EmbeddingWorker) {
    for _ in 0..32 {
        match worker.run_once(&env.ctx).await.expect("embedding run") {
            EmbeddingWorkerRun::Completed { .. } => {}
            EmbeddingWorkerRun::NoJob => return,
            outcome => panic!("unexpected embedding run: {outcome:?}"),
        }
    }
    panic!("embedding worker did not drain its jobs");
}

async fn relay(env: &LiveEnv, embedding_plan: &EmbeddingPlan) {
    let sink = Arc::new(IndexingOutboxSink::new(embedding_plan).expect("indexing outbox sink"));
    jobs::relay_outbox_with_sink(&env.pool, &env.ctx, 32, &sink)
        .await
        .expect("relay outbox");
}

async fn index_seeded(
    env: &LiveEnv,
    provider: &MockEmbeddingProvider,
    markdown: &str,
) -> (Uuid, Uuid, Uuid, String, String) {
    let embedding_plan = test_embedding_plan(provider.base_url());
    let (document_id, version_id, collection_id, object_key) =
        seed_converted_document(env, markdown).await;
    relay(env, &embedding_plan).await;
    let run = index_worker(env, embedding_plan.clone())
        .run_once(&env.ctx)
        .await
        .expect("index run");
    let IndexWorkerRun::Completed { .. } = run else {
        panic!("unexpected index run: {run:?}");
    };
    run_embedding_jobs(env, &embedding_worker(env, provider)).await;
    let signature = active_signature(env, collection_id)
        .await
        .expect("active signature");
    (
        document_id,
        version_id,
        collection_id,
        object_key,
        signature,
    )
}

async fn active_signature(env: &LiveEnv, collection_id: Uuid) -> Option<String> {
    with_org_txn(&env.pool, &env.ctx, {
        let ctx = env.ctx.clone();
        move |txn| {
            Box::pin(async move {
                Ok(
                    fileconv_server::db::index_metadata::find_active(
                        txn,
                        &ctx,
                        Some(collection_id),
                    )
                    .await?
                    .map(|metadata| metadata.index_signature_sha256),
                )
            })
        }
    })
    .await
    .expect("active signature")
}

async fn document(env: &LiveEnv, document_id: Uuid) -> Document {
    with_org_txn(&env.pool, &env.ctx, {
        let ctx = env.ctx.clone();
        move |txn| Box::pin(async move { documents::get_by_id(txn, &ctx, document_id).await })
    })
    .await
    .expect("document")
}

async fn chunk_count(env: &LiveEnv, document_id: Uuid) -> i64 {
    with_org_txn(&env.pool, &env.ctx, {
        let ctx = env.ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT count(*)::bigint FROM chunks WHERE org_id = $1 AND document_id = $2",
                        &[&ctx.org_id(), &document_id],
                    )
                    .await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .expect("chunk count")
}

async fn points_for_doc(
    env: &LiveEnv,
    collection_id: Uuid,
    document_id: Uuid,
    signature: &str,
) -> usize {
    let collection = collection_name_for_digest(signature).expect("collection name");
    env.qdrant
        .scroll_points(
            &collection,
            &VectorScope::new(env.ctx.org_id(), [collection_id]),
            &[json!({
                "key": "document_id",
                "match": { "value": document_id.to_string() }
            })],
            1000,
        )
        .await
        .expect("scroll points")
        .len()
}

async fn object_exists(env: &LiveEnv, raw_key: &str) -> bool {
    let key = parse_key_for_org(raw_key, env.ctx.org_id()).expect("parse key");
    env.storage
        .object_exists(env.ctx.org_id(), &key)
        .await
        .expect("object exists")
}

async fn delete_job_count(env: &LiveEnv) -> i64 {
    with_org_txn(&env.pool, &env.ctx, {
        let ctx = env.ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT count(*)::bigint
                         FROM jobs
                         WHERE org_id = $1 AND job_type = 'delete'",
                        &[&ctx.org_id()],
                    )
                    .await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .expect("delete job count")
}

async fn reconcile_job_count(env: &LiveEnv, document_id: Uuid) -> i64 {
    with_org_txn(&env.pool, &env.ctx, {
        let ctx = env.ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT count(*)::bigint
                         FROM jobs
                         WHERE org_id = $1 AND job_type = 'reconcile' AND document_id = $2",
                        &[&ctx.org_id(), &document_id],
                    )
                    .await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .expect("reconcile job count")
}

async fn reconcile_job_count_by_key(env: &LiveEnv, key: &str) -> i64 {
    with_org_txn(&env.pool, &env.ctx, {
        let ctx = env.ctx.clone();
        let key = key.to_string();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT count(*)::bigint
                         FROM jobs
                         WHERE org_id = $1 AND job_type = 'reconcile' AND idempotency_key = $2",
                        &[&ctx.org_id(), &key],
                    )
                    .await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .expect("reconcile job count by key")
}

async fn outbox_count(env: &LiveEnv, event_type: &str) -> i64 {
    with_org_txn(&env.pool, &env.ctx, {
        let ctx = env.ctx.clone();
        let event_type = event_type.to_string();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT count(*)::bigint
                         FROM outbox_events
                         WHERE org_id = $1 AND event_type = $2",
                        &[&ctx.org_id(), &event_type],
                    )
                    .await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .expect("outbox count")
}

async fn audit_count(env: &LiveEnv, action: &str) -> i64 {
    with_org_txn(&env.pool, &env.ctx, {
        let ctx = env.ctx.clone();
        let action = action.to_string();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT count(*)::bigint
                         FROM audit_log
                         WHERE org_id = $1 AND action = $2",
                        &[&ctx.org_id(), &action],
                    )
                    .await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .expect("audit count")
}

async fn immutable_inventory_counts(env: &LiveEnv, document_id: Uuid) -> (i64, i64, i64) {
    with_org_txn(&env.pool, &env.ctx, {
        let ctx = env.ctx.clone();
        move |txn| {
            Box::pin(async move {
                let versions = txn
                    .query_one(
                        "SELECT count(*)::bigint FROM document_versions WHERE org_id = $1 AND document_id = $2",
                        &[&ctx.org_id(), &document_id],
                    )
                    .await?
                    .get(0);
                let artifacts = txn
                    .query_one(
                        "SELECT count(*)::bigint FROM derived_artifacts WHERE org_id = $1 AND document_id = $2",
                        &[&ctx.org_id(), &document_id],
                    )
                    .await?
                    .get(0);
                let metadata = txn
                    .query_one(
                        "SELECT count(*)::bigint FROM index_metadata WHERE org_id = $1",
                        &[&ctx.org_id()],
                    )
                    .await?
                    .get(0);
                Ok((versions, artifacts, metadata))
            })
        }
    })
    .await
    .expect("inventory counts")
}

async fn reset_delete_job_to_pending(env: &LiveEnv) {
    with_org_txn(&env.pool, &env.ctx, {
        let ctx = env.ctx.clone();
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "UPDATE jobs
                     SET status = 'pending', lease_owner = NULL, lease_expires_at = NULL,
                         heartbeat_at = NULL, checkpoint = NULL, available_at = clock_timestamp(),
                         finished_at = NULL, updated_at = clock_timestamp()
                     WHERE org_id = $1 AND job_type = 'delete'",
                    &[&ctx.org_id()],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("reset delete job");
}

async fn tombstone_directly(env: &LiveEnv, document_id: Uuid) {
    request_delete(&env.pool, &env.ctx, document_id)
        .await
        .expect("request delete");
}

async fn insert_dead_letter_with_id(env: &LiveEnv, job_id: Uuid, staged_key: &str) {
    ensure_org(&env.pool, &env.ctx).await;
    with_org_txn(&env.pool, &env.ctx, {
        let ctx = env.ctx.clone();
        let staged_key = staged_key.to_string();
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "INSERT INTO jobs (
                        id, org_id, job_type, status, payload_version, payload,
                        attempts, max_attempts, checkpoint, idempotency_key,
                        available_at, finished_at
                     )
                     VALUES ($1, $2, 'convert', 'dead_letter', 2, $3,
                             5, 5, $4, $5, clock_timestamp(), clock_timestamp())",
                    &[
                        &job_id,
                        &ctx.org_id(),
                        &json!({}),
                        &json!(CheckpointPayload {
                            staged_object_keys: vec![staged_key],
                            ..CheckpointPayload::default()
                        }),
                        &format!("dead-letter-{job_id}"),
                    ],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("insert dead letter");
}

async fn first_chunk_identity(env: &LiveEnv, document_id: Uuid) -> String {
    with_org_txn(&env.pool, &env.ctx, {
        let ctx = env.ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT chunk_identity_sha256
                         FROM chunks
                         WHERE org_id = $1 AND document_id = $2
                         ORDER BY ordinal
                         LIMIT 1",
                        &[&ctx.org_id(), &document_id],
                    )
                    .await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .expect("first chunk")
}

async fn set_document_state_and_delete_chunks(
    env: &LiveEnv,
    document_id: Uuid,
    state: DocumentState,
) {
    with_org_txn(&env.pool, &env.ctx, {
        let ctx = env.ctx.clone();
        move |txn| {
            Box::pin(async move {
                let state = state.as_str();
                txn.execute(
                    "UPDATE documents
                     SET state = $3, updated_at = clock_timestamp()
                     WHERE org_id = $1 AND id = $2",
                    &[&ctx.org_id(), &document_id, &state],
                )
                .await?;
                txn.execute(
                    "DELETE FROM chunks WHERE org_id = $1 AND document_id = $2",
                    &[&ctx.org_id(), &document_id],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("set state and delete chunks");
}

async fn insert_index_job(env: &LiveEnv, document_id: Uuid, version_id: Uuid) -> Uuid {
    let job_id = Uuid::new_v4();
    with_org_txn(&env.pool, &env.ctx, {
        let ctx = env.ctx.clone();
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "INSERT INTO jobs (
                        id, org_id, job_type, status, payload_version, payload,
                        attempts, max_attempts, idempotency_key, document_id, version_id,
                        available_at
                     )
                     VALUES ($1, $2, 'index', 'pending', $3, $4,
                             0, 5, $5, $6, $7, clock_timestamp())",
                    &[
                        &job_id,
                        &ctx.org_id(),
                        &jobs::CURRENT_JOB_PAYLOAD_VERSION,
                        &json!({
                            "document_id": document_id,
                            "version_id": version_id,
                        }),
                        &format!("manual-index-{job_id}"),
                        &document_id,
                        &version_id,
                    ],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("insert index job");
    job_id
}

async fn job_status(env: &LiveEnv, job_id: Uuid) -> String {
    with_org_txn(&env.pool, &env.ctx, {
        let ctx = env.ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT status FROM jobs WHERE org_id = $1 AND id = $2",
                        &[&ctx.org_id(), &job_id],
                    )
                    .await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .expect("job status")
}

async fn upsert_test_point(
    env: &LiveEnv,
    collection_id: Uuid,
    document_id: Uuid,
    version_id: Uuid,
    signature: &str,
    chunk_identity: &str,
) {
    let collection = collection_name_for_digest(signature).expect("collection name");
    env.qdrant
        .upsert_points(
            &collection,
            &VectorScope::new(env.ctx.org_id(), [collection_id]),
            &[UpsertPoint {
                chunk_identity: chunk_identity.to_string(),
                vector: {
                    let mut vector = vec![0.0; TEST_VECTOR_DIMENSIONS];
                    vector[0] = 1.0;
                    vector
                },
                payload: ChunkPointPayload {
                    org_id: env.ctx.org_id(),
                    collection_id,
                    document_id,
                    version_id,
                    chunk_id: chunk_identity.to_string(),
                    ordinal: 0,
                    is_current: true,
                    is_effective: true,
                    index_generation: 1,
                },
            }],
        )
        .await
        .expect("upsert test point");
}

fn sample_markdown() -> &'static str {
    "# Chương I\n\nMở đầu.\n\n## Điều 1\n\nNội dung điều 1.\n\n## Điều 2\n\nNội dung điều 2.\n"
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL, MARKHAND_TEST_MINIO_*, and MARKHAND_TEST_QDRANT_URL"]
async fn live_enqueue_reconcile_is_reason_scoped_and_idempotent() {
    let Some(env) = LiveEnv::boot().await else {
        return;
    };
    let provider = MockEmbeddingProvider::start();
    let (document_id, _version_id, _collection_id, _object_key, _signature) =
        index_seeded(&env, &provider, sample_markdown()).await;

    let first = enqueue_reconcile(&env.pool, &env.ctx, document_id, "manual-hour-1")
        .await
        .expect("enqueue first reconcile");
    assert!(first.created);
    let replay = enqueue_reconcile(&env.pool, &env.ctx, document_id, "manual-hour-1")
        .await
        .expect("replay first reconcile");
    assert!(!replay.created);
    assert_eq!(first.job.id, replay.job.id);
    let second = enqueue_reconcile(&env.pool, &env.ctx, document_id, "manual-hour-2")
        .await
        .expect("enqueue second reconcile");
    assert!(second.created);
    assert_ne!(first.job.id, second.job.id);
    assert_eq!(reconcile_job_count(&env, document_id).await, 2);
    env.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL, MARKHAND_TEST_MINIO_*, and MARKHAND_TEST_QDRANT_URL"]
async fn live_delete_tombstones_then_purges() {
    let Some(env) = LiveEnv::boot().await else {
        return;
    };
    let provider = MockEmbeddingProvider::start();
    let embedding_plan = test_embedding_plan(provider.base_url());
    let markdown = sample_markdown();
    let (document_id, version_id, collection_id, object_key, signature) =
        index_seeded(&env, &provider, markdown).await;
    let orphan_chunk_identity = first_chunk_identity(&env, document_id).await;
    assert!(points_for_doc(&env, collection_id, document_id, &signature).await > 0);

    let outcome = request_delete(&env.pool, &env.ctx, document_id)
        .await
        .expect("request delete");
    let DeleteRequestOutcome::Requested(tombstoned) = outcome else {
        panic!("expected tombstone");
    };
    assert_eq!(tombstoned.state, DocumentState::Tombstoned);
    assert!(tombstoned.deleted_at.is_some());
    assert_eq!(outbox_count(&env, "document.delete_requested").await, 1);

    relay(&env, &embedding_plan).await;
    assert_eq!(delete_job_count(&env).await, 1);
    let run = delete_worker(&env)
        .run_once(&env.ctx)
        .await
        .expect("delete run");
    let DeleteWorkerRun::Completed { .. } = run else {
        panic!("unexpected delete run: {run:?}");
    };

    let purged = document(&env, document_id).await;
    assert_eq!(purged.state, DocumentState::Purged);
    assert_eq!(chunk_count(&env, document_id).await, 0);
    assert_eq!(
        points_for_doc(&env, collection_id, document_id, &signature).await,
        0
    );
    assert!(!object_exists(&env, &object_key).await);
    let (versions, artifacts, metadata) = immutable_inventory_counts(&env, document_id).await;
    assert_eq!(versions, 1);
    assert_eq!(artifacts, 1);
    assert_eq!(metadata, 1);
    assert_eq!(audit_count(&env, "document.purge").await, 1);

    upsert_test_point(
        &env,
        collection_id,
        document_id,
        version_id,
        &signature,
        &orphan_chunk_identity,
    )
    .await;
    assert_eq!(
        points_for_doc(&env, collection_id, document_id, &signature).await,
        1
    );
    let report = reconcile_document(
        &env.pool,
        &env.storage,
        &env.qdrant,
        &env.ctx,
        document_id,
        ReconcileMode::Repair,
    )
    .await
    .expect("post-purge reconcile");
    assert_eq!(report.repaired.orphan_vectors, 1);
    assert_eq!(
        points_for_doc(&env, collection_id, document_id, &signature).await,
        0
    );
    env.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL, MARKHAND_TEST_MINIO_*, and MARKHAND_TEST_QDRANT_URL"]
async fn live_delete_replay_is_idempotent() {
    let Some(env) = LiveEnv::boot().await else {
        return;
    };
    let provider = MockEmbeddingProvider::start();
    let embedding_plan = test_embedding_plan(provider.base_url());
    let (document_id, _version_id, _collection_id, _object_key, _signature) =
        index_seeded(&env, &provider, sample_markdown()).await;
    request_delete(&env.pool, &env.ctx, document_id)
        .await
        .expect("request delete");
    relay(&env, &embedding_plan).await;
    assert!(matches!(
        delete_worker(&env)
            .run_once(&env.ctx)
            .await
            .expect("delete run"),
        DeleteWorkerRun::Completed { .. }
    ));
    reset_delete_job_to_pending(&env).await;
    assert!(matches!(
        delete_worker(&env)
            .run_once(&env.ctx)
            .await
            .expect("delete replay"),
        DeleteWorkerRun::Completed { .. }
    ));
    assert_eq!(
        document(&env, document_id).await.state,
        DocumentState::Purged
    );
    assert_eq!(chunk_count(&env, document_id).await, 0);
    env.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL, MARKHAND_TEST_MINIO_*, and MARKHAND_TEST_QDRANT_URL"]
async fn live_reconcile_repairs_orphan_vectors() {
    let Some(env) = LiveEnv::boot().await else {
        return;
    };
    let provider = MockEmbeddingProvider::start();
    let (document_id, _version_id, collection_id, _object_key, signature) =
        index_seeded(&env, &provider, sample_markdown()).await;
    let before = points_for_doc(&env, collection_id, document_id, &signature).await;
    assert!(before > 0);
    tombstone_directly(&env, document_id).await;

    let dry = reconcile_document(
        &env.pool,
        &env.storage,
        &env.qdrant,
        &env.ctx,
        document_id,
        ReconcileMode::DryRun,
    )
    .await
    .expect("dry run");
    assert_eq!(dry.orphan_vectors, before);
    assert_eq!(
        points_for_doc(&env, collection_id, document_id, &signature).await,
        before
    );

    let repaired = reconcile_document(
        &env.pool,
        &env.storage,
        &env.qdrant,
        &env.ctx,
        document_id,
        ReconcileMode::Repair,
    )
    .await
    .expect("repair");
    assert_eq!(repaired.repaired.orphan_vectors, before);
    assert_eq!(
        points_for_doc(&env, collection_id, document_id, &signature).await,
        0
    );
    let repeat = reconcile_document(
        &env.pool,
        &env.storage,
        &env.qdrant,
        &env.ctx,
        document_id,
        ReconcileMode::Repair,
    )
    .await
    .expect("repeat repair");
    assert_eq!(repeat.orphan_vectors, 0);
    env.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL, MARKHAND_TEST_MINIO_*, and MARKHAND_TEST_QDRANT_URL"]
async fn live_reconcile_does_not_delete_in_flight_vectors() {
    let Some(env) = LiveEnv::boot().await else {
        return;
    };
    let provider = MockEmbeddingProvider::start();
    let (document_id, version_id, collection_id, _object_key, signature) =
        index_seeded(&env, &provider, sample_markdown()).await;
    let chunk_identity = first_chunk_identity(&env, document_id).await;
    set_document_state_and_delete_chunks(&env, document_id, DocumentState::Indexing).await;
    assert_eq!(chunk_count(&env, document_id).await, 0);
    assert!(points_for_doc(&env, collection_id, document_id, &signature).await > 0);

    let report = reconcile_document(
        &env.pool,
        &env.storage,
        &env.qdrant,
        &env.ctx,
        document_id,
        ReconcileMode::Repair,
    )
    .await
    .expect("repair skips in-flight");
    assert!(report.in_flight_vectors > 0);
    assert_eq!(report.repaired.orphan_vectors, 0);
    assert!(points_for_doc(&env, collection_id, document_id, &signature).await > 0);

    upsert_test_point(
        &env,
        collection_id,
        document_id,
        version_id,
        &signature,
        &chunk_identity,
    )
    .await;
    env.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL, MARKHAND_TEST_MINIO_*, and MARKHAND_TEST_QDRANT_URL"]
async fn live_delete_cancels_index_jobs_and_rejects_stale_index_attempts() {
    let Some(env) = LiveEnv::boot().await else {
        return;
    };
    let provider = MockEmbeddingProvider::start();
    let embedding_plan = test_embedding_plan(provider.base_url());
    let (document_id, version_id, collection_id, _object_key, signature) =
        index_seeded(&env, &provider, sample_markdown()).await;
    let chunks_before = chunk_count(&env, document_id).await;
    let pending_job_id = insert_index_job(&env, document_id, version_id).await;
    request_delete(&env.pool, &env.ctx, document_id)
        .await
        .expect("request delete");
    assert_eq!(job_status(&env, pending_job_id).await, "cancelled");

    let stale_job_id = insert_index_job(&env, document_id, version_id).await;
    let run = index_worker(&env, embedding_plan.clone())
        .run_once(&env.ctx)
        .await
        .expect("stale index worker");
    let IndexWorkerRun::Failed { job_id, .. } = run else {
        panic!("unexpected stale index run: {run:?}");
    };
    assert_eq!(job_id, stale_job_id);
    assert_eq!(chunk_count(&env, document_id).await, chunks_before);

    relay(&env, &embedding_plan).await;
    assert!(matches!(
        delete_worker(&env)
            .run_once(&env.ctx)
            .await
            .expect("delete run"),
        DeleteWorkerRun::Completed { .. }
    ));
    assert_eq!(
        points_for_doc(&env, collection_id, document_id, &signature).await,
        0
    );
    assert_eq!(
        document(&env, document_id).await.state,
        DocumentState::Purged
    );
    env.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL, MARKHAND_TEST_MINIO_*, and MARKHAND_TEST_QDRANT_URL"]
async fn live_reconcile_exact_point_ids_do_not_delete_other_document_payloads() {
    let Some(env) = LiveEnv::boot().await else {
        return;
    };
    let provider = MockEmbeddingProvider::start();
    let (document_id, version_id, collection_id, _object_key, signature) =
        index_seeded(&env, &provider, sample_markdown()).await;
    let chunk_identity = first_chunk_identity(&env, document_id).await;
    let other_document_id = Uuid::new_v4();
    upsert_test_point(
        &env,
        collection_id,
        other_document_id,
        version_id,
        &signature,
        &chunk_identity,
    )
    .await;
    set_document_state_and_delete_chunks(&env, document_id, DocumentState::Indexed).await;

    let report = reconcile_document(
        &env.pool,
        &env.storage,
        &env.qdrant,
        &env.ctx,
        document_id,
        ReconcileMode::Repair,
    )
    .await
    .expect("repair target document");
    // The target document's own points (its chunks were deleted) are GC'd by exact
    // point id; the foreign point that merely reuses the same chunk_identity string
    // but carries a different payload.document_id must NOT be collaterally deleted.
    assert_eq!(report.repaired.orphan_vectors, 2);
    assert_eq!(
        points_for_doc(&env, collection_id, document_id, &signature).await,
        0
    );
    assert_eq!(
        points_for_doc(&env, collection_id, other_document_id, &signature).await,
        1
    );
    env.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL, MARKHAND_TEST_MINIO_*, and MARKHAND_TEST_QDRANT_URL"]
async fn live_compensate_batch_points_deletes_only_target_document_ids() {
    let Some(env) = LiveEnv::boot().await else {
        return;
    };
    let provider = MockEmbeddingProvider::start();
    let (document_id, version_id, collection_id, _object_key, signature) =
        index_seeded(&env, &provider, sample_markdown()).await;
    let other_document_id = Uuid::new_v4();
    let target_chunk_a = format!("{:064x}", 10_u8);
    let target_chunk_b = format!("{:064x}", 11_u8);
    let other_chunk = format!("{:064x}", 12_u8);
    upsert_test_point(
        &env,
        collection_id,
        document_id,
        version_id,
        &signature,
        &target_chunk_a,
    )
    .await;
    upsert_test_point(
        &env,
        collection_id,
        document_id,
        version_id,
        &signature,
        &target_chunk_b,
    )
    .await;
    upsert_test_point(
        &env,
        collection_id,
        other_document_id,
        version_id,
        &signature,
        &other_chunk,
    )
    .await;

    let collection = collection_name_for_digest(&signature).expect("collection name");
    let scope = VectorScope::new(env.ctx.org_id(), [collection_id]);
    let target_ids = vec![
        point_id_from_org_collection_and_chunk(env.ctx.org_id(), collection_id, &target_chunk_a)
            .expect("point id a"),
        point_id_from_org_collection_and_chunk(env.ctx.org_id(), collection_id, &target_chunk_b)
            .expect("point id b"),
    ];
    let other_id =
        point_id_from_org_collection_and_chunk(env.ctx.org_id(), collection_id, &other_chunk)
            .expect("other point id");

    let mut requested_delete_ids = target_ids.clone();
    requested_delete_ids.push(other_id);
    compensate_batch_points(
        &env.qdrant,
        &collection,
        &scope,
        document_id,
        &requested_delete_ids,
    )
    .await
    .expect("compensate target batch");

    assert!(env
        .qdrant
        .get_points(&collection, &scope, &target_ids)
        .await
        .expect("target points")
        .is_empty());
    assert_eq!(
        env.qdrant
            .get_points(&collection, &scope, &[other_id])
            .await
            .expect("other point")
            .len(),
        1
    );
    env.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL, MARKHAND_TEST_MINIO_*, and MARKHAND_TEST_QDRANT_URL"]
async fn live_compensation_failure_incident_reconcile_is_enqueued_and_cleans_orphan() {
    let Some(env) = LiveEnv::boot().await else {
        return;
    };
    let provider = MockEmbeddingProvider::start();
    let embedding_plan = test_embedding_plan(provider.base_url());
    let (document_id, version_id, collection_id, _object_key, signature) =
        index_seeded(&env, &provider, sample_markdown()).await;
    request_delete(&env.pool, &env.ctx, document_id)
        .await
        .expect("request delete");
    relay(&env, &embedding_plan).await;
    assert!(matches!(
        delete_worker(&env)
            .run_once(&env.ctx)
            .await
            .expect("delete run"),
        DeleteWorkerRun::Completed { .. }
    ));
    assert_eq!(
        document(&env, document_id).await.state,
        DocumentState::Purged
    );

    let orphan_chunk = format!("{:064x}", 42_u8);
    upsert_test_point(
        &env,
        collection_id,
        document_id,
        version_id,
        &signature,
        &orphan_chunk,
    )
    .await;
    assert_eq!(
        points_for_doc(&env, collection_id, document_id, &signature).await,
        1
    );

    // Models the production path where compensation itself failed; the helper is
    // best-effort and schedules a unique reconcile incident for durable cleanup.
    let index_job_id = Uuid::new_v4();
    enqueue_compensation_reconcile(&env.pool, &env.ctx, document_id, index_job_id, 3, 0).await;
    let incident_key = format!("reconcile:{document_id}:{index_job_id}:3:0");
    assert_eq!(reconcile_job_count_by_key(&env, &incident_key).await, 1);
    enqueue_compensation_reconcile(&env.pool, &env.ctx, document_id, index_job_id, 3, 0).await;
    assert_eq!(reconcile_job_count_by_key(&env, &incident_key).await, 1);

    let report = reconcile_document(
        &env.pool,
        &env.storage,
        &env.qdrant,
        &env.ctx,
        document_id,
        ReconcileMode::Repair,
    )
    .await
    .expect("incident reconcile repair");
    assert_eq!(report.repaired.orphan_vectors, 1);
    assert_eq!(
        points_for_doc(&env, collection_id, document_id, &signature).await,
        0
    );
    env.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL, MARKHAND_TEST_MINIO_*, and MARKHAND_TEST_QDRANT_URL"]
async fn live_reconcile_dead_letter_staging_gc() {
    let Some(env) = LiveEnv::boot().await else {
        return;
    };
    let provider = MockEmbeddingProvider::start();
    let (referenced_document_id, _referenced_version_id, _collection_id, referenced_key, _sig) =
        index_seeded(&env, &provider, sample_markdown()).await;
    let version_id = Uuid::new_v4();
    let document_id = Uuid::new_v4();
    let collection_id = Uuid::new_v4();
    let key = trusted_key(env.ctx.org_id(), version_id, Uuid::new_v4(), None).expect("trusted key");
    let key_string = key.as_str();
    env.storage
        .put_object(
            env.ctx.org_id(),
            &key,
            Bytes::from_static(b"staged"),
            &ObjectIdentityMeta {
                org_id: env.ctx.org_id(),
                collection_id: Some(collection_id),
                document_id: Some(document_id),
                version_id: Some(version_id),
                original_filename: None,
                canonical_format: Some("md".into()),
                content_sha256: Some(hex::encode(Sha256::digest(b"staged"))),
                content_length: Some(6),
                disposition: Some("trusted".into()),
            },
            "text/markdown; charset=utf-8",
        )
        .await
        .expect("put staged");
    for index in 1..=500_u128 {
        insert_dead_letter_with_id(&env, Uuid::from_u128(index), &referenced_key).await;
    }
    insert_dead_letter_with_id(&env, Uuid::from_u128(501), &key_string).await;
    assert!(object_exists(&env, &key_string).await);
    assert!(object_exists(&env, &referenced_key).await);

    let report =
        reconcile_dead_letter_jobs(&env.pool, &env.storage, &env.ctx, ReconcileMode::Repair)
            .await
            .expect("dead letter repair");
    assert_eq!(report.repaired.staged_objects, 1);
    assert!(!object_exists(&env, &key_string).await);
    assert!(object_exists(&env, &referenced_key).await);
    assert_eq!(
        document(&env, referenced_document_id).await.state,
        DocumentState::Indexed
    );
    reconcile_dead_letter_jobs(&env.pool, &env.storage, &env.ctx, ReconcileMode::Repair)
        .await
        .expect("dead letter repeat");
    env.drop().await;
}
