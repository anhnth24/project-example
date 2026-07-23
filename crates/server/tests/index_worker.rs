//! Live tests for the durable index worker.
//!
//! These tests require PostgreSQL, MinIO, and Qdrant. They use a local
//! OpenAI-compatible mock for embeddings and are explicitly ignored unless
//! the live storage environment is configured.
//!
//! Database access runs as non-superuser `markhand_app` (FORCE RLS).

mod common;

use bytes::Bytes;
use common::{
    admin_database_url, app_database_url, assert_markhand_app_role, boot_app_pool,
    DualRoleEphemeralDb,
};
use deadpool_postgres::Pool;
use fileconv_knowledge::embedding::{EmbeddingPlan, ProviderDeployment, RUNTIME_VLLM_LOCAL};
use fileconv_server::auth::context::OrgContext;
use fileconv_server::config::{MinioConfig, Profile, SecretString};
use fileconv_server::db::collections::{self, NewCollection};
use fileconv_server::db::documents::{self, NewDocument};
use fileconv_server::db::models::{ArtifactKind, CollectionVisibility, DocumentState};
use fileconv_server::db::orgs;
use fileconv_server::db::pool::with_org_txn;
use fileconv_server::jobs::{self, EventPayload, CURRENT_EVENT_PAYLOAD_VERSION};
use fileconv_server::services::chunking::prepare_chunks;
use fileconv_server::services::embedding::ApprovedEmbeddingRuntime;
use fileconv_server::services::indexing::IndexingOutboxSink;
use fileconv_server::services::lifecycle;
use fileconv_server::storage::minio::{MinioClient, ObjectIdentityMeta};
use fileconv_server::storage::qdrant::{
    point_id_from_org_collection_and_chunk, ChunkPointPayload, QdrantClient, UpsertPoint,
    VectorScope,
};
use fileconv_server::storage::trusted_key;
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
use uuid::Uuid;

fn test_minio_client() -> Result<MinioClient, String> {
    let endpoint = match std::env::var("MARKHAND_TEST_MINIO_ENDPOINT") {
        Ok(url) if !url.trim().is_empty() => url,
        _ => return Err("MARKHAND_TEST_MINIO_ENDPOINT is required".into()),
    };
    let access_key = std::env::var("MARKHAND_TEST_MINIO_ACCESS_KEY")
        .map_err(|_| "MARKHAND_TEST_MINIO_ACCESS_KEY is required".to_string())?;
    let secret_key = std::env::var("MARKHAND_TEST_MINIO_SECRET_KEY")
        .map_err(|_| "MARKHAND_TEST_MINIO_SECRET_KEY is required".to_string())?;
    let region = std::env::var("MARKHAND_TEST_MINIO_REGION").unwrap_or_else(|_| "us-east-1".into());
    let bucket = format!("markhand-index-worker-{}", Uuid::new_v4().simple());
    std::env::set_var("RUST_S3_SKIP_LOCATION_CONSTRAINT", "true");
    let config = MinioConfig::new(
        endpoint,
        SecretString::new(access_key),
        SecretString::new(secret_key),
        bucket,
        region,
        true,
    )
    .map_err(|error| format!("invalid test MinIO configuration: {error}"))?;
    MinioClient::from_config(&config).map_err(|error| format!("test MinIO client: {error}"))
}

fn test_qdrant_client() -> Result<QdrantClient, String> {
    let url = match std::env::var("MARKHAND_TEST_QDRANT_URL") {
        Ok(url) if !url.trim().is_empty() => url,
        _ => return Err("MARKHAND_TEST_QDRANT_URL is required".into()),
    };
    let api_key = std::env::var("MARKHAND_TEST_QDRANT_API_KEY")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(SecretString::new);
    QdrantClient::with_api_key(url, api_key).map_err(|error| format!("test Qdrant client: {error}"))
}

fn test_embedding_plan(base_url: &str) -> EmbeddingPlan {
    EmbeddingPlan::provider(
        "test",
        "test-embedding",
        "r1",
        ProviderDeployment::from_base_url(Some(base_url)).expect("test deployment"),
        Some(8),
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
            thread.join().expect("join mock embedding provider");
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

struct LiveEnv {
    db: DualRoleEphemeralDb,
    pool: Pool,
    storage: MinioClient,
    qdrant: QdrantClient,
    ctx: OrgContext,
}

impl LiveEnv {
    async fn boot() -> Result<Self, String> {
        let admin_url = admin_database_url()
            .ok_or_else(|| "MARKHAND_TEST_DATABASE_URL is required".to_string())?;
        let app_url = app_database_url()
            .ok_or_else(|| "MARKHAND_TEST_APP_DATABASE_URL is required".to_string())?;
        let storage = test_minio_client()?;
        let qdrant = test_qdrant_client()?;
        storage
            .ensure_bucket()
            .await
            .map_err(|error| format!("ensure test bucket: {error}"))?;
        let (db, pool) = boot_app_pool(&admin_url, &app_url).await;
        assert_markhand_app_role(&pool).await;
        let ctx = OrgContext::try_new(Uuid::new_v4(), Uuid::new_v4(), ["doc.upload"], [])
            .map_err(|error| format!("create test org context: {error}"))?;
        let env = Self {
            db,
            pool,
            storage,
            qdrant,
            ctx,
        };
        assert_cross_org_raw_query_is_zero(&env).await;
        Ok(env)
    }

    async fn drop(self) {
        self.db.drop().await;
    }
}

async fn assert_cross_org_raw_query_is_zero(env: &LiveEnv) {
    let other = OrgContext::try_new(Uuid::new_v4(), Uuid::new_v4(), ["doc.upload"], [])
        .expect("other org context");
    let other_collection = Uuid::new_v4();
    with_org_txn(&env.pool, &other, {
        let ctx = other.clone();
        move |txn| {
            Box::pin(async move {
                orgs::ensure_exists(txn, &ctx, "other-org", "Other Org").await?;
                orgs::ensure_user(txn, &ctx, ctx.user_id(), "other@example.test", "Other").await?;
                orgs::ensure_membership(txn, &ctx).await?;
                collections::insert(
                    txn,
                    &ctx,
                    NewCollection {
                        id: other_collection,
                        name: "other",
                        slug: "other",
                        description: None,
                        visibility: CollectionVisibility::Private,
                    },
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("seed other org");
    let visible: i64 = with_org_txn(&env.pool, &env.ctx, {
        let other_org = other.org_id();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT count(*)::bigint FROM collections WHERE org_id = $1",
                        &[&other_org],
                    )
                    .await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .expect("cross-org probe");
    assert_eq!(
        visible, 0,
        "app-role FORCE RLS must hide other-org rows from raw queries"
    );
}

async fn seed_converted_document(
    pool: &Pool,
    storage: &MinioClient,
    ctx: &OrgContext,
    markdown: &str,
) -> (Uuid, Uuid, Uuid) {
    let collection_id = Uuid::new_v4();
    let document_id = Uuid::new_v4();
    let version_id = Uuid::new_v4();
    let artifact_id = Uuid::new_v4();
    let outbox_key = format!("index-request:{version_id}");
    let object_key =
        trusted_key(ctx.org_id(), version_id, Uuid::new_v4(), None).expect("trusted key");
    let object_key_string = object_key.as_str();
    let sha256 = hex::encode(Sha256::digest(markdown.as_bytes()));
    let markdown_len = markdown.len() as i64;
    storage
        .put_object(
            ctx.org_id(),
            &object_key,
            Bytes::copy_from_slice(markdown.as_bytes()),
            &ObjectIdentityMeta {
                org_id: ctx.org_id(),
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

    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        let object_key_string = object_key_string.clone();
        let sha256 = sha256.clone();
        move |txn| {
            Box::pin(async move {
                orgs::ensure_exists(txn, &ctx, "index-worker-org", "Index Worker Org").await?;
                orgs::ensure_user(
                    txn,
                    &ctx,
                    ctx.user_id(),
                    "index-worker@example.test",
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
                let collection_name = format!("Index Worker Collection {collection_id}");
                let collection_slug = format!("index-worker-{collection_id}");
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
                        title: "Index Worker Doc",
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
                let converted = DocumentState::Converted.as_str();
                txn.execute(
                    "UPDATE documents
                     SET state = $3, current_version_id = $4, updated_at = clock_timestamp()
                     WHERE org_id = $1 AND id = $2",
                    &[&ctx.org_id(), &document_id, &converted, &version_id],
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
    (document_id, version_id, collection_id)
}

async fn seed_promoted_current_version(
    env: &LiveEnv,
    document_id: Uuid,
    collection_id: Uuid,
    previous_version_id: Uuid,
    markdown: &str,
) -> Uuid {
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
        .expect("put promoted markdown");

    with_org_txn(&env.pool, &env.ctx, {
        let ctx = env.ctx.clone();
        let object_key_string = object_key_string.clone();
        let sha256 = sha256.clone();
        move |txn| {
            Box::pin(async move {
                let effective_to = txn
                    .query_one("SELECT clock_timestamp()", &[])
                    .await?
                    .get::<_, chrono::DateTime<chrono::Utc>>(0);
                txn.execute(
                    "UPDATE document_versions
                     SET is_current = false, effective_to = $4
                     WHERE org_id = $1 AND document_id = $2 AND id = $3",
                    &[
                        &ctx.org_id(),
                        &document_id,
                        &previous_version_id,
                        &effective_to,
                    ],
                )
                .await?;
                txn.execute(
                    "INSERT INTO document_versions (
                        id, org_id, document_id, version_number, publication_state,
                        is_current, content_sha256, original_object_key, markdown_object_key,
                        source_content_type, byte_size, created_by_user_id
                     )
                     SELECT $1, $2, $3, COALESCE(MAX(version_number), 0)::integer + 1,
                            'published', true, $4, $5, $5, 'text/markdown', $6, $7
                     FROM document_versions
                     WHERE org_id = $2 AND document_id = $3",
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
                     SET current_version_id = $3, state = 'converted', updated_at = clock_timestamp()
                     WHERE org_id = $1 AND id = $2",
                    &[&ctx.org_id(), &document_id, &version_id],
                )
                .await?;
                // Same durable enqueue path production promotion uses: lifecycle
                // refresh for the demoted version in the pointer-swap transaction.
                lifecycle::enqueue_refresh_within_txn(
                    txn,
                    &ctx,
                    document_id,
                    collection_id,
                    previous_version_id,
                    version_id,
                )
                .await
                .map_err(|error| {
                    fileconv_server::db::error::DbError::Config(format!("{error:?}"))
                })?;
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
    .expect("seed promoted current version");
    version_id
}

fn index_worker(
    env: &LiveEnv,
    approved_signature: Option<String>,
    embedding_plan: EmbeddingPlan,
) -> Result<IndexWorker, fileconv_server::workers::index::IndexWorkerError> {
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
        approved_signature,
        embedding_plan,
    )
}

fn embedding_worker(
    env: &LiveEnv,
    provider: &MockEmbeddingProvider,
) -> Result<EmbeddingWorker, fileconv_server::workers::embedding::EmbeddingWorkerError> {
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
        8,
        RUNTIME_VLLM_LOCAL.into(),
        Profile::Test,
        false,
        None,
    )
    .expect("mock embedding runtime");
    EmbeddingWorker::new(env.pool.clone(), env.qdrant.clone(), config, runtime)
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

async fn index_job_count(env: &LiveEnv) -> i64 {
    with_org_txn(&env.pool, &env.ctx, {
        let ctx = env.ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT count(*)::bigint
                         FROM jobs
                         WHERE org_id = $1 AND job_type = 'index'",
                        &[&ctx.org_id()],
                    )
                    .await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .expect("index job count")
}

async fn lifecycle_job_count(env: &LiveEnv) -> i64 {
    with_org_txn(&env.pool, &env.ctx, {
        let ctx = env.ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT count(*)::bigint
                         FROM jobs
                         WHERE org_id = $1 AND job_type = 'lifecycle_refresh'",
                        &[&ctx.org_id()],
                    )
                    .await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .expect("lifecycle job count")
}

async fn lifecycle_job_succeeded_count(env: &LiveEnv) -> i64 {
    with_org_txn(&env.pool, &env.ctx, {
        let ctx = env.ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT count(*)::bigint
                         FROM jobs
                         WHERE org_id = $1
                           AND job_type = 'lifecycle_refresh'
                           AND status = 'succeeded'",
                        &[&ctx.org_id()],
                    )
                    .await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .expect("lifecycle succeeded count")
}

async fn lifecycle_job_generation_ids(env: &LiveEnv) -> Vec<Uuid> {
    with_org_txn(&env.pool, &env.ctx, {
        let ctx = env.ctx.clone();
        move |txn| {
            Box::pin(async move {
                let rows = txn
                    .query(
                        "SELECT DISTINCT (payload->>'index_metadata_id')::uuid AS generation_id
                         FROM jobs
                         WHERE org_id = $1 AND job_type = 'lifecycle_refresh'
                         ORDER BY generation_id",
                        &[&ctx.org_id()],
                    )
                    .await?;
                Ok(rows
                    .iter()
                    .map(|row| row.get::<_, Uuid>("generation_id"))
                    .collect::<Vec<_>>())
            })
        }
    })
    .await
    .expect("lifecycle generation ids")
}

async fn lifecycle_pending_count(env: &LiveEnv) -> i64 {
    with_org_txn(&env.pool, &env.ctx, {
        let ctx = env.ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT count(*)::bigint
                         FROM jobs
                         WHERE org_id = $1
                           AND job_type = 'lifecycle_refresh'
                           AND status = 'pending'",
                        &[&ctx.org_id()],
                    )
                    .await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .expect("lifecycle pending count")
}

/// Materializes version chunks under a second (inactive) generation so promotion
/// enqueues one lifecycle job per generation.
async fn materialize_second_generation_for_version(
    env: &LiveEnv,
    collection_id: Uuid,
    document_id: Uuid,
    version_id: Uuid,
    markdown: &str,
) -> Uuid {
    let chunks = prepare_chunks(document_id, version_id, markdown, "");
    let second_generation = Uuid::new_v4();
    let second_collection = Uuid::new_v4();
    let signature = "f".repeat(64);
    with_org_txn(&env.pool, &env.ctx, {
        let ctx = env.ctx.clone();
        move |txn| {
            Box::pin(async move {
                // Second collection + inactive generation (distinct Qdrant routing key).
                let collection_name = format!("Second Gen {second_collection}");
                let collection_slug = format!("second-gen-{second_collection}");
                collections::insert(
                    txn,
                    &ctx,
                    NewCollection {
                        id: second_collection,
                        name: &collection_name,
                        slug: &collection_slug,
                        description: None,
                        visibility: CollectionVisibility::Private,
                    },
                )
                .await?;
                txn.execute(
                    "INSERT INTO index_metadata (
                        id, org_id, collection_id, index_signature_sha256, embedding_family,
                        embedding_revision, dimensions, runtime_path, generation, is_active, state
                     ) VALUES (
                        $1, $2, $3, $4, 'test', 'r1', 8, 'vllm-local', 1, false, 'retired'
                     )",
                    &[
                        &second_generation,
                        &ctx.org_id(),
                        &second_collection,
                        &signature,
                    ],
                )
                .await?;
                for (ordinal, chunk) in chunks.iter().enumerate() {
                    let ordinal = i32::try_from(ordinal).expect("ordinal");
                    txn.execute(
                        "INSERT INTO chunks (
                            id, org_id, document_id, version_id, ordinal, body,
                            chunk_identity_sha256, index_metadata_id, index_signature, tsv
                         ) VALUES (
                            $1, $2, $3, $4, $5, $6, $7, $8, $9,
                            to_tsvector('simple', $6)
                         )",
                        &[
                            &Uuid::new_v4(),
                            &ctx.org_id(),
                            &document_id,
                            &version_id,
                            &ordinal,
                            &chunk.body,
                            &chunk.chunk_identity,
                            &second_generation,
                            &signature,
                        ],
                    )
                    .await?;
                }
                let _ = collection_id;
                Ok(second_generation)
            })
        }
    })
    .await
    .expect("materialize second generation")
}

async fn reset_lifecycle_job_to_pending(env: &LiveEnv) {
    with_org_txn(&env.pool, &env.ctx, {
        let ctx = env.ctx.clone();
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "UPDATE jobs
                     SET status = 'pending', lease_owner = NULL, lease_expires_at = NULL,
                         heartbeat_at = NULL, available_at = clock_timestamp(),
                         finished_at = NULL, last_error = NULL, updated_at = clock_timestamp()
                     WHERE org_id = $1 AND job_type = 'lifecycle_refresh'",
                    &[&ctx.org_id()],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("reset lifecycle job");
}

async fn drain_index_and_lifecycle(env: &LiveEnv, worker: &IndexWorker) {
    for _ in 0..16 {
        match worker.run_once(&env.ctx).await.expect("drain run") {
            IndexWorkerRun::NoJob => return,
            IndexWorkerRun::Completed { .. }
            | IndexWorkerRun::Failed { .. }
            | IndexWorkerRun::LeaseLost { .. } => {}
        }
    }
    panic!("index/lifecycle worker did not drain");
}

async fn chunk_count(env: &LiveEnv, version_id: Uuid) -> i64 {
    with_org_txn(&env.pool, &env.ctx, {
        let ctx = env.ctx.clone();
        move |txn| {
            Box::pin(async move {
                fileconv_server::db::chunks::count_by_version(txn, &ctx, version_id).await
            })
        }
    })
    .await
    .expect("chunk count")
}

async fn document_state(env: &LiveEnv, document_id: Uuid) -> DocumentState {
    with_org_txn(&env.pool, &env.ctx, {
        let ctx = env.ctx.clone();
        move |txn| Box::pin(async move { documents::get_by_id(txn, &ctx, document_id).await })
    })
    .await
    .expect("document")
    .state
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

async fn fetched_points(
    env: &LiveEnv,
    embedding_plan: &EmbeddingPlan,
    collection_id: Uuid,
    document_id: Uuid,
    version_id: Uuid,
    markdown: &str,
) -> Vec<(Uuid, ChunkPointPayload)> {
    let chunks = prepare_chunks(document_id, version_id, markdown, "");
    let signature = embedding_plan.index_signature(8).unwrap();
    let collection_name = env
        .qdrant
        .ensure_collection_for_signature(&signature)
        .await
        .expect("ensure qdrant collection");
    let ids = chunks
        .iter()
        .map(|chunk| {
            point_id_from_org_collection_and_chunk(
                env.ctx.org_id(),
                collection_id,
                &chunk.chunk_identity,
            )
            .expect("point id")
        })
        .collect::<Vec<_>>();
    env.qdrant
        .get_points(
            &collection_name,
            &VectorScope::new(env.ctx.org_id(), [collection_id]),
            &ids,
        )
        .await
        .expect("get points")
}

async fn reset_job_to_pending(env: &LiveEnv) {
    with_org_txn(&env.pool, &env.ctx, {
        let ctx = env.ctx.clone();
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "UPDATE jobs
                     SET status = 'pending', lease_owner = NULL, lease_expires_at = NULL,
                         heartbeat_at = NULL, available_at = clock_timestamp(),
                         finished_at = NULL, updated_at = clock_timestamp()
                     WHERE org_id = $1 AND job_type = 'index'",
                    &[&ctx.org_id()],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("reset job");
}

async fn reset_index_job_for_version(env: &LiveEnv, version_id: Uuid) {
    with_org_txn(&env.pool, &env.ctx, {
        let ctx = env.ctx.clone();
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "UPDATE jobs
                     SET status = 'pending', lease_owner = NULL, lease_expires_at = NULL,
                         heartbeat_at = NULL, checkpoint = NULL, available_at = clock_timestamp(),
                         finished_at = NULL, updated_at = clock_timestamp()
                     WHERE org_id = $1 AND job_type = 'index' AND version_id = $2",
                    &[&ctx.org_id(), &version_id],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("reset version job");
}

async fn insert_duplicate_index_outbox(env: &LiveEnv, document_id: Uuid, version_id: Uuid) {
    with_org_txn(&env.pool, &env.ctx, {
        let ctx = env.ctx.clone();
        move |txn| {
            Box::pin(async move {
                let payload = EventPayload {
                    job_id: None,
                    document_id: Some(document_id),
                    version_id: Some(version_id),
                    outbox_event_id: None,
                }
                .to_json()
                .expect("event payload");
                let key = format!("index-request-duplicate:{version_id}");
                txn.execute(
                    "INSERT INTO outbox_events (
                        org_id, event_type, payload_version, payload, idempotency_key
                     )
                     VALUES ($1, 'document.index_requested', $2, $3, $4)",
                    &[
                        &ctx.org_id(),
                        &CURRENT_EVENT_PAYLOAD_VERSION,
                        &payload,
                        &key,
                    ],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("duplicate outbox");
}

async fn seed_first_batch_and_checkpoint(
    env: &LiveEnv,
    embedding_plan: &EmbeddingPlan,
    collection_id: Uuid,
    document_id: Uuid,
    version_id: Uuid,
    markdown: &str,
) {
    let chunks = prepare_chunks(document_id, version_id, markdown, "");
    assert!(chunks.len() > 1);
    let first = &chunks[0];
    let signature = embedding_plan.index_signature(8).unwrap();
    let signature_digest = signature.digest();
    let collection_name = env
        .qdrant
        .ensure_collection_for_signature(&signature)
        .await
        .expect("ensure collection");
    let vector = vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    let metadata = with_org_txn(&env.pool, &env.ctx, {
        let ctx = env.ctx.clone();
        let signature_digest = signature_digest.clone();
        let chunking_version = signature.chunking_version.to_string();
        let body_text_version = signature.body_text_version.to_string();
        let query_normalization_version = signature.query_normalization_version.to_string();
        let embedding_family = signature.embedding_family.to_string();
        let embedding_revision = signature.embedding_revision.to_string();
        let normalized = signature.normalized;
        move |txn| {
            Box::pin(async move {
                fileconv_server::db::index_metadata::ensure_active_generation(
                    txn,
                    &ctx,
                    fileconv_server::db::index_metadata::EnsureGeneration {
                        collection_id: Some(collection_id),
                        signature_sha256: &signature_digest,
                        chunking_version: &chunking_version,
                        body_text_version: &body_text_version,
                        query_normalization_version: &query_normalization_version,
                        embedding_family: &embedding_family,
                        embedding_revision: &embedding_revision,
                        dimensions: 8,
                        normalized,
                        runtime_path: fileconv_server::db::models::EmbeddingRuntimePath::VllmLocal,
                    },
                )
                .await
            })
        }
    })
    .await
    .expect("ensure metadata");
    env.qdrant
        .upsert_points(
            &collection_name,
            &VectorScope::new(env.ctx.org_id(), [collection_id]),
            &[UpsertPoint {
                chunk_identity: first.chunk_identity.clone(),
                vector,
                payload: ChunkPointPayload {
                    org_id: env.ctx.org_id(),
                    collection_id,
                    document_id,
                    version_id,
                    chunk_id: first.chunk_identity.clone(),
                    ordinal: first.ordinal as u64,
                    is_current: true,
                    is_effective: true,
                    index_generation: metadata.generation as u32,
                },
            }],
        )
        .await
        .expect("upsert first point");
    with_org_txn(&env.pool, &env.ctx, {
        let ctx = env.ctx.clone();
        let first = first.clone();
        let signature_digest = signature_digest.clone();
        move |txn| {
            Box::pin(async move {
                fileconv_server::db::chunks::insert_if_absent(
                    txn,
                    &ctx,
                    fileconv_server::db::chunks::NewChunk {
                        id: Uuid::new_v4(),
                        document_id,
                        version_id,
                        ordinal: first.ordinal,
                        heading_path: &first.heading_path,
                        body: &first.body,
                        body_text_version: fileconv_knowledge::identity::BODY_TEXT_VERSION,
                        chunk_identity_sha256: &first.chunk_identity,
                        index_metadata_id: metadata.id,
                        index_signature: &signature_digest,
                        page: first.page,
                        slide: first.slide,
                        sheet: first.sheet.as_deref(),
                        span_start: Some(first.span_start),
                        span_end: Some(first.span_end),
                    },
                )
                .await?;
                txn.execute(
                    "UPDATE documents
                     SET state = 'indexing', updated_at = clock_timestamp()
                     WHERE org_id = $1 AND id = $2",
                    &[&ctx.org_id(), &document_id],
                )
                .await?;
                txn.execute(
                    "UPDATE jobs
                     SET checkpoint = $2
                     WHERE org_id = $1 AND job_type = 'index'",
                    &[&ctx.org_id(), &json!({ "offset": 1_u64 })],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("seed checkpoint");
}

fn sample_markdown() -> &'static str {
    "# Chương I\n\nMở đầu.\n\n## Điều 1\n\nNội dung điều 1.\n\n## Điều 2\n\nNội dung điều 2.\n"
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL, MARKHAND_TEST_MINIO_*, and MARKHAND_TEST_QDRANT_URL"]
async fn live_index_worker_indexes_converted_document() {
    let env = match LiveEnv::boot().await {
        Ok(env) => env,
        Err(error) => {
            eprintln!("skipped: {error}");
            return;
        }
    };
    let provider = MockEmbeddingProvider::start();
    let embedding_plan = test_embedding_plan(provider.base_url());
    let markdown = sample_markdown();
    let (document_id, version_id, collection_id) =
        seed_converted_document(&env.pool, &env.storage, &env.ctx, markdown).await;
    relay(&env, &embedding_plan).await;
    assert_eq!(index_job_count(&env).await, 1);

    let run = index_worker(&env, None, embedding_plan.clone())
        .expect("worker")
        .run_once(&env.ctx)
        .await
        .expect("run");
    let IndexWorkerRun::Completed { chunks, .. } = run else {
        panic!("unexpected run: {run:?}");
    };
    let expected = prepare_chunks(document_id, version_id, markdown, "");
    assert_eq!(chunks, expected.len());
    let embedding_worker = embedding_worker(&env, &provider).expect("embedding worker");
    run_embedding_jobs(&env, &embedding_worker).await;
    assert_eq!(
        document_state(&env, document_id).await,
        DocumentState::Indexed
    );
    assert_eq!(chunk_count(&env, version_id).await, expected.len() as i64);
    let signature = embedding_plan.index_signature(8).unwrap().digest();
    assert_eq!(active_signature(&env, collection_id).await, Some(signature));
    let points = fetched_points(
        &env,
        &embedding_plan,
        collection_id,
        document_id,
        version_id,
        markdown,
    )
    .await;
    assert_eq!(points.len(), expected.len());
    for (_, payload) in points {
        assert_eq!(payload.org_id, env.ctx.org_id());
        assert_eq!(payload.collection_id, collection_id);
        assert_eq!(payload.document_id, document_id);
        assert_eq!(payload.version_id, version_id);
        assert!(payload.is_current);
        assert!(payload.is_effective);
    }
    env.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL, MARKHAND_TEST_MINIO_*, and MARKHAND_TEST_QDRANT_URL"]
async fn live_index_worker_replay_is_idempotent() {
    let env = match LiveEnv::boot().await {
        Ok(env) => env,
        Err(error) => {
            eprintln!("skipped: {error}");
            return;
        }
    };
    let provider = MockEmbeddingProvider::start();
    let embedding_plan = test_embedding_plan(provider.base_url());
    let markdown = sample_markdown();
    let (document_id, version_id, collection_id) =
        seed_converted_document(&env.pool, &env.storage, &env.ctx, markdown).await;
    relay(&env, &embedding_plan).await;
    let worker = index_worker(&env, None, embedding_plan.clone()).expect("worker");
    assert!(matches!(
        worker.run_once(&env.ctx).await.expect("first run"),
        IndexWorkerRun::Completed { .. }
    ));
    let embedding_worker = embedding_worker(&env, &provider).expect("embedding worker");
    run_embedding_jobs(&env, &embedding_worker).await;
    let expected = prepare_chunks(document_id, version_id, markdown, "");
    assert_eq!(chunk_count(&env, version_id).await, expected.len() as i64);
    insert_duplicate_index_outbox(&env, document_id, version_id).await;
    relay(&env, &embedding_plan).await;
    assert_eq!(index_job_count(&env).await, 1);
    reset_job_to_pending(&env).await;
    assert!(matches!(
        worker.run_once(&env.ctx).await.expect("replay"),
        IndexWorkerRun::Completed { chunks: 0, .. }
    ));
    assert_eq!(chunk_count(&env, version_id).await, expected.len() as i64);
    assert_eq!(
        fetched_points(
            &env,
            &embedding_plan,
            collection_id,
            document_id,
            version_id,
            markdown,
        )
        .await
        .len(),
        expected.len()
    );
    env.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL, MARKHAND_TEST_MINIO_*, and MARKHAND_TEST_QDRANT_URL"]
async fn live_index_worker_signature_mismatch_fails_closed() {
    let env = match LiveEnv::boot().await {
        Ok(env) => env,
        Err(error) => {
            eprintln!("skipped: {error}");
            return;
        }
    };
    let embedding_plan = test_embedding_plan("http://embedding.test/v1");
    let markdown = sample_markdown();
    let (document_id, version_id, collection_id) =
        seed_converted_document(&env.pool, &env.storage, &env.ctx, markdown).await;
    relay(&env, &embedding_plan).await;
    let bad_signature =
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string();
    assert!(index_worker(&env, Some(bad_signature), embedding_plan.clone()).is_err());
    assert_eq!(chunk_count(&env, version_id).await, 0);
    assert_eq!(
        document_state(&env, document_id).await,
        DocumentState::Converted
    );
    assert!(fetched_points(
        &env,
        &embedding_plan,
        collection_id,
        document_id,
        version_id,
        markdown,
    )
    .await
    .is_empty());
    env.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL, MARKHAND_TEST_MINIO_*, and MARKHAND_TEST_QDRANT_URL"]
async fn live_index_worker_stale_version_does_not_mark_current_indexed() {
    let env = match LiveEnv::boot().await {
        Ok(env) => env,
        Err(error) => {
            eprintln!("skipped: {error}");
            return;
        }
    };
    let provider = MockEmbeddingProvider::start();
    let embedding_plan = test_embedding_plan(provider.base_url());
    let markdown_a = sample_markdown();
    let markdown_b = "# Chương II\n\nBản mới.\n\n## Điều 3\n\nNội dung điều 3.\n";
    let (document_id, version_a, collection_id) =
        seed_converted_document(&env.pool, &env.storage, &env.ctx, markdown_a).await;
    relay(&env, &embedding_plan).await;
    let worker = index_worker(&env, None, embedding_plan.clone()).expect("worker");
    assert!(matches!(
        worker.run_once(&env.ctx).await.expect("index version a"),
        IndexWorkerRun::Completed { .. }
    ));
    let embedding_worker = embedding_worker(&env, &provider).expect("embedding worker");
    run_embedding_jobs(&env, &embedding_worker).await;
    assert_eq!(
        document_state(&env, document_id).await,
        DocumentState::Indexed
    );

    // Natural A→B: promote B (durably enqueues lifecycle_refresh for A) without
    // resetting A's succeeded index job. Fairness may claim lifecycle before
    // index B, so drain both job types then embeddings.
    let version_b =
        seed_promoted_current_version(&env, document_id, collection_id, version_a, markdown_b)
            .await;
    assert_eq!(lifecycle_job_count(&env).await, 1);
    relay(&env, &embedding_plan).await;
    drain_index_and_lifecycle(&env, &worker).await;
    run_embedding_jobs(&env, &embedding_worker).await;
    drain_index_and_lifecycle(&env, &worker).await;
    assert_eq!(
        document_state(&env, document_id).await,
        DocumentState::Indexed
    );
    assert_eq!(lifecycle_job_succeeded_count(&env).await, 1);
    assert_eq!(
        chunk_count(&env, version_b).await,
        prepare_chunks(document_id, version_b, markdown_b, "").len() as i64
    );

    let points_a = fetched_points(
        &env,
        &embedding_plan,
        collection_id,
        document_id,
        version_a,
        markdown_a,
    )
    .await;
    assert!(!points_a.is_empty());
    assert!(points_a
        .iter()
        .all(|(_, payload)| !payload.is_current && !payload.is_effective));
    let points_b = fetched_points(
        &env,
        &embedding_plan,
        collection_id,
        document_id,
        version_b,
        markdown_b,
    )
    .await;
    assert!(!points_b.is_empty());
    assert!(points_b
        .iter()
        .all(|(_, payload)| payload.is_current && payload.is_effective));
    assert_eq!(
        chunk_count(&env, version_b).await,
        prepare_chunks(document_id, version_b, markdown_b, "").len() as i64
    );
    env.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL, MARKHAND_TEST_MINIO_*, and MARKHAND_TEST_QDRANT_URL"]
async fn live_index_worker_resumes_from_indexing_state() {
    let env = match LiveEnv::boot().await {
        Ok(env) => env,
        Err(error) => {
            eprintln!("skipped: {error}");
            return;
        }
    };
    let provider = MockEmbeddingProvider::start();
    let embedding_plan = test_embedding_plan(provider.base_url());
    let markdown = sample_markdown();
    let (document_id, version_id, collection_id) =
        seed_converted_document(&env.pool, &env.storage, &env.ctx, markdown).await;
    relay(&env, &embedding_plan).await;
    seed_first_batch_and_checkpoint(
        &env,
        &embedding_plan,
        collection_id,
        document_id,
        version_id,
        markdown,
    )
    .await;
    let run = index_worker(&env, None, embedding_plan.clone())
        .expect("worker")
        .run_once(&env.ctx)
        .await
        .expect("run");
    assert!(matches!(run, IndexWorkerRun::Completed { .. }));
    let embedding_worker = embedding_worker(&env, &provider).expect("embedding worker");
    run_embedding_jobs(&env, &embedding_worker).await;
    let expected = prepare_chunks(document_id, version_id, markdown, "");
    assert_eq!(chunk_count(&env, version_id).await, expected.len() as i64);
    assert_eq!(
        fetched_points(
            &env,
            &embedding_plan,
            collection_id,
            document_id,
            version_id,
            markdown,
        )
        .await
        .len(),
        expected.len()
    );
    assert_eq!(
        document_state(&env, document_id).await,
        DocumentState::Indexed
    );
    env.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL, MARKHAND_TEST_MINIO_*, and MARKHAND_TEST_QDRANT_URL"]
async fn live_qdrant_set_payload_mixed_scope_ids_only_target_changes() {
    let env = match LiveEnv::boot().await {
        Ok(env) => env,
        Err(error) => {
            eprintln!("skipped: {error}");
            return;
        }
    };
    let embedding_plan = test_embedding_plan("http://embedding.test/v1");
    let signature = embedding_plan.index_signature(8).unwrap();
    let collection_name = env
        .qdrant
        .ensure_collection_for_signature(&signature)
        .await
        .expect("ensure collection");

    let org = env.ctx.org_id();
    let other_org = Uuid::new_v4();
    let collection = Uuid::new_v4();
    let other_collection = Uuid::new_v4();
    let document = Uuid::new_v4();
    let version_target = Uuid::new_v4();
    let version_other = Uuid::new_v4();
    let id_target = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let id_other_version = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let id_other_collection = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    let id_other_org = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
    let point_target =
        point_id_from_org_collection_and_chunk(org, collection, id_target).expect("target");
    let point_other_version =
        point_id_from_org_collection_and_chunk(org, collection, id_other_version)
            .expect("other version");
    let point_other_collection =
        point_id_from_org_collection_and_chunk(org, other_collection, id_other_collection)
            .expect("other collection");
    let point_other_org =
        point_id_from_org_collection_and_chunk(other_org, collection, id_other_org)
            .expect("other org");
    let vector = vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    let points = [
        (org, collection, id_target, document, version_target),
        (org, collection, id_other_version, document, version_other),
        (
            org,
            other_collection,
            id_other_collection,
            document,
            version_target,
        ),
        (
            other_org,
            collection,
            id_other_org,
            document,
            version_target,
        ),
    ];
    for (scope_org, scope_col, identity, document_id, version_id) in points {
        env.qdrant
            .upsert_points(
                &collection_name,
                &VectorScope::new(scope_org, [scope_col]),
                &[UpsertPoint {
                    chunk_identity: identity.into(),
                    vector: vector.clone(),
                    payload: ChunkPointPayload {
                        org_id: scope_org,
                        collection_id: scope_col,
                        document_id,
                        version_id,
                        chunk_id: identity.into(),
                        ordinal: 0,
                        is_current: true,
                        is_effective: true,
                        index_generation: 1,
                    },
                }],
            )
            .await
            .expect("upsert mixed-scope point");
    }

    // Mixed has_id set: only target must change when org/collection/version filter holds.
    // Dropping version_id would flip same-org/collection other-version; body `points`
    // without filter would flip foreign IDs — both regressions fail this assertion.
    let mixed_ids = [
        point_target,
        point_other_version,
        point_other_collection,
        point_other_org,
    ];
    env.qdrant
        .set_payload_fields(
            &collection_name,
            &VectorScope::new(org, [collection]),
            &mixed_ids,
            &json!({ "is_current": false, "is_effective": false }),
            &[json!({
                "key": "version_id",
                "match": { "value": version_target.to_string() }
            })],
        )
        .await
        .expect("mixed-scope filter-only update");

    let target = env
        .qdrant
        .get_points(
            &collection_name,
            &VectorScope::new(org, [collection]),
            &[point_target],
        )
        .await
        .expect("get target");
    assert_eq!(target.len(), 1);
    assert!(!target[0].1.is_current && !target[0].1.is_effective);

    let other_version = env
        .qdrant
        .get_points(
            &collection_name,
            &VectorScope::new(org, [collection]),
            &[point_other_version],
        )
        .await
        .expect("get other version");
    assert!(
        other_version[0].1.is_current && other_version[0].1.is_effective,
        "other version must stay current when version filter is present"
    );

    let other_collection_pts = env
        .qdrant
        .get_points(
            &collection_name,
            &VectorScope::new(org, [other_collection]),
            &[point_other_collection],
        )
        .await
        .expect("get other collection");
    assert!(other_collection_pts[0].1.is_current && other_collection_pts[0].1.is_effective);

    let other_org_pts = env
        .qdrant
        .get_points(
            &collection_name,
            &VectorScope::new(other_org, [collection]),
            &[point_other_org],
        )
        .await
        .expect("get other org");
    assert!(other_org_pts[0].1.is_current && other_org_pts[0].1.is_effective);
    env.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL, MARKHAND_TEST_MINIO_*, and MARKHAND_TEST_QDRANT_URL"]
async fn live_lifecycle_refresh_retries_converge_without_losing_work() {
    let env = match LiveEnv::boot().await {
        Ok(env) => env,
        Err(error) => {
            eprintln!("skipped: {error}");
            return;
        }
    };
    let provider = MockEmbeddingProvider::start();
    let embedding_plan = test_embedding_plan(provider.base_url());
    let markdown_a = sample_markdown();
    let markdown_b = "# Chương II\n\nBản retry.\n\n## Điều 3\n\nNội dung.\n";
    let (document_id, version_a, collection_id) =
        seed_converted_document(&env.pool, &env.storage, &env.ctx, markdown_a).await;
    relay(&env, &embedding_plan).await;
    let worker = index_worker(&env, None, embedding_plan.clone()).expect("worker");
    assert!(matches!(
        worker.run_once(&env.ctx).await.expect("index a"),
        IndexWorkerRun::Completed { .. }
    ));
    let embedding_worker = embedding_worker(&env, &provider).expect("embedding worker");
    run_embedding_jobs(&env, &embedding_worker).await;
    let _version_b =
        seed_promoted_current_version(&env, document_id, collection_id, version_a, markdown_b)
            .await;
    relay(&env, &embedding_plan).await;
    drain_index_and_lifecycle(&env, &worker).await;
    run_embedding_jobs(&env, &embedding_worker).await;
    drain_index_and_lifecycle(&env, &worker).await;
    assert_eq!(lifecycle_job_succeeded_count(&env).await, 1);

    // Corrupt markers, then re-queue the same durable lifecycle job and converge.
    let points_a = fetched_points(
        &env,
        &embedding_plan,
        collection_id,
        document_id,
        version_a,
        markdown_a,
    )
    .await;
    let point_ids = points_a.iter().map(|(id, _)| *id).collect::<Vec<_>>();
    let signature = embedding_plan.index_signature(8).unwrap();
    let collection_name = env
        .qdrant
        .ensure_collection_for_signature(&signature)
        .await
        .expect("collection");
    env.qdrant
        .set_payload_fields(
            &collection_name,
            &VectorScope::new(env.ctx.org_id(), [collection_id]),
            &point_ids,
            &json!({ "is_current": true, "is_effective": true }),
            &[json!({
                "key": "version_id",
                "match": { "value": version_a.to_string() }
            })],
        )
        .await
        .expect("corrupt markers");
    reset_lifecycle_job_to_pending(&env).await;
    assert!(matches!(
        worker.run_once(&env.ctx).await.expect("lifecycle retry"),
        IndexWorkerRun::Completed { chunks: 0, .. }
    ));
    let repaired = fetched_points(
        &env,
        &embedding_plan,
        collection_id,
        document_id,
        version_a,
        markdown_a,
    )
    .await;
    assert!(repaired
        .iter()
        .all(|(_, payload)| !payload.is_current && !payload.is_effective));
    assert_eq!(lifecycle_job_count(&env).await, 1);
    env.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL, MARKHAND_TEST_MINIO_*, and MARKHAND_TEST_QDRANT_URL"]
async fn live_replay_a_races_promotion_b_under_barrier() {
    let env = match LiveEnv::boot().await {
        Ok(env) => env,
        Err(error) => {
            eprintln!("skipped: {error}");
            return;
        }
    };
    let provider = MockEmbeddingProvider::start();
    let embedding_plan = test_embedding_plan(provider.base_url());
    let markdown_a = sample_markdown();
    let markdown_b = "# Chương II\n\nBản race.\n\n## Điều 3\n\nNội dung race.\n";
    let (document_id, version_a, collection_id) =
        seed_converted_document(&env.pool, &env.storage, &env.ctx, markdown_a).await;
    relay(&env, &embedding_plan).await;
    let worker = Arc::new(index_worker(&env, None, embedding_plan.clone()).expect("worker"));
    assert!(matches!(
        worker.run_once(&env.ctx).await.expect("index a"),
        IndexWorkerRun::Completed { .. }
    ));
    let embedding_worker = embedding_worker(&env, &provider).expect("embedding worker");
    run_embedding_jobs(&env, &embedding_worker).await;

    // Race: replay superseded A vs promoting B (lifecycle enqueue) at a barrier.
    reset_index_job_for_version(&env, version_a).await;
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let barrier_replay = barrier.clone();
    let worker_replay = worker.clone();
    let ctx_replay = env.ctx.clone();
    let replay = tokio::spawn(async move {
        barrier_replay.wait().await;
        worker_replay.run_once(&ctx_replay).await
    });
    barrier.wait().await;
    let version_b =
        seed_promoted_current_version(&env, document_id, collection_id, version_a, markdown_b)
            .await;
    let replay_outcome = replay.await.expect("join replay").expect("replay run");
    assert!(matches!(
        replay_outcome,
        IndexWorkerRun::Completed { .. } | IndexWorkerRun::NoJob | IndexWorkerRun::LeaseLost { .. }
    ));
    relay(&env, &embedding_plan).await;
    drain_index_and_lifecycle(&env, &worker).await;
    run_embedding_jobs(&env, &embedding_worker).await;
    drain_index_and_lifecycle(&env, &worker).await;

    let points_a = fetched_points(
        &env,
        &embedding_plan,
        collection_id,
        document_id,
        version_a,
        markdown_a,
    )
    .await;
    let points_b = fetched_points(
        &env,
        &embedding_plan,
        collection_id,
        document_id,
        version_b,
        markdown_b,
    )
    .await;
    assert!(!points_a.is_empty());
    assert!(points_a
        .iter()
        .all(|(_, payload)| !payload.is_current && !payload.is_effective));
    assert!(!points_b.is_empty());
    assert!(points_b
        .iter()
        .all(|(_, payload)| payload.is_current && payload.is_effective));
    assert_eq!(
        document_state(&env, document_id).await,
        DocumentState::Indexed
    );
    env.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL, MARKHAND_TEST_MINIO_*, and MARKHAND_TEST_QDRANT_URL"]
async fn live_promotion_enqueues_lifecycle_per_materialized_generation() {
    let env = match LiveEnv::boot().await {
        Ok(env) => env,
        Err(error) => {
            eprintln!("skipped: {error}");
            return;
        }
    };
    let provider = MockEmbeddingProvider::start();
    let embedding_plan = test_embedding_plan(provider.base_url());
    let markdown_a = sample_markdown();
    let markdown_b = "# Chương II\n\nHai generation.\n\n## Điều 3\n\nNội dung.\n";
    let (document_id, version_a, collection_id) =
        seed_converted_document(&env.pool, &env.storage, &env.ctx, markdown_a).await;
    relay(&env, &embedding_plan).await;
    let worker = index_worker(&env, None, embedding_plan.clone()).expect("worker");
    assert!(matches!(
        worker.run_once(&env.ctx).await.expect("index a"),
        IndexWorkerRun::Completed { .. }
    ));
    let embedding_worker = embedding_worker(&env, &provider).expect("embedding worker");
    run_embedding_jobs(&env, &embedding_worker).await;

    let second_generation = materialize_second_generation_for_version(
        &env,
        collection_id,
        document_id,
        version_a,
        markdown_a,
    )
    .await;
    let second_collection = with_org_txn(&env.pool, &env.ctx, {
        let ctx = env.ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT collection_id FROM index_metadata
                         WHERE org_id = $1 AND id = $2",
                        &[&ctx.org_id(), &second_generation],
                    )
                    .await?;
                Ok(row.get::<_, Uuid>("collection_id"))
            })
        }
    })
    .await
    .expect("second collection");

    let second_signature = "f".repeat(64);
    let collection_name =
        fileconv_server::services::index_signature::collection_name_for_digest(&second_signature)
            .expect("collection name");
    env.qdrant
        .ensure_collection_for_digest(&second_signature, 8, true)
        .await
        .expect("ensure second qdrant collection");

    let chunks = prepare_chunks(document_id, version_a, markdown_a, "");
    let vector = vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    for chunk in &chunks {
        env.qdrant
            .upsert_points(
                &collection_name,
                &VectorScope::new(env.ctx.org_id(), [second_collection]),
                &[UpsertPoint {
                    chunk_identity: chunk.chunk_identity.clone(),
                    vector: vector.clone(),
                    payload: ChunkPointPayload {
                        org_id: env.ctx.org_id(),
                        collection_id: second_collection,
                        document_id,
                        version_id: version_a,
                        chunk_id: chunk.chunk_identity.clone(),
                        ordinal: u64::try_from(chunk.ordinal).expect("ordinal"),
                        is_current: true,
                        is_effective: true,
                        index_generation: 1,
                    },
                }],
            )
            .await
            .expect("upsert second gen point");
    }

    let version_b =
        seed_promoted_current_version(&env, document_id, collection_id, version_a, markdown_b)
            .await;
    assert_eq!(lifecycle_job_count(&env).await, 2);
    let generations = lifecycle_job_generation_ids(&env).await;
    assert_eq!(generations.len(), 2);
    assert!(generations.contains(&second_generation));

    with_org_txn(&env.pool, &env.ctx, {
        let ctx = env.ctx.clone();
        move |txn| {
            Box::pin(async move {
                let outcomes = lifecycle::enqueue_refresh_within_txn(
                    txn,
                    &ctx,
                    document_id,
                    collection_id,
                    version_a,
                    version_b,
                )
                .await
                .map_err(|error| {
                    fileconv_server::db::error::DbError::Config(format!("{error:?}"))
                })?;
                assert_eq!(outcomes.len(), 2);
                assert!(outcomes.iter().all(|outcome| !outcome.created));
                Ok(())
            })
        }
    })
    .await
    .expect("idempotent re-enqueue");
    assert_eq!(lifecycle_job_count(&env).await, 2);

    relay(&env, &embedding_plan).await;
    drain_index_and_lifecycle(&env, &worker).await;
    run_embedding_jobs(&env, &embedding_worker).await;
    drain_index_and_lifecycle(&env, &worker).await;
    assert_eq!(lifecycle_job_succeeded_count(&env).await, 2);

    let points_primary = fetched_points(
        &env,
        &embedding_plan,
        collection_id,
        document_id,
        version_a,
        markdown_a,
    )
    .await;
    assert!(points_primary
        .iter()
        .all(|(_, payload)| !payload.is_current && !payload.is_effective));

    let second_ids = chunks
        .iter()
        .map(|chunk| {
            point_id_from_org_collection_and_chunk(
                env.ctx.org_id(),
                second_collection,
                &chunk.chunk_identity,
            )
            .expect("id")
        })
        .collect::<Vec<_>>();
    let points_second = env
        .qdrant
        .get_points(
            &collection_name,
            &VectorScope::new(env.ctx.org_id(), [second_collection]),
            &second_ids,
        )
        .await
        .expect("second gen points");
    assert!(!points_second.is_empty());
    assert!(points_second
        .iter()
        .all(|(_, payload)| !payload.is_current && !payload.is_effective));
    env.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL, MARKHAND_TEST_MINIO_*, and MARKHAND_TEST_QDRANT_URL"]
async fn live_index_worker_fairness_claims_lifecycle_within_two_run_once() {
    let env = match LiveEnv::boot().await {
        Ok(env) => env,
        Err(error) => {
            eprintln!("skipped: {error}");
            return;
        }
    };
    let provider = MockEmbeddingProvider::start();
    let embedding_plan = test_embedding_plan(provider.base_url());
    let markdown_a = sample_markdown();
    let markdown_b = "# Chương II\n\nFairness.\n\n## Điều 3\n\nNội dung.\n";
    let markdown_c = "# Chương III\n\nBacklog.\n\n## Điều 4\n\nThêm index.\n";

    let (document_a, version_a, collection_a) =
        seed_converted_document(&env.pool, &env.storage, &env.ctx, markdown_a).await;
    relay(&env, &embedding_plan).await;
    let worker = index_worker(&env, None, embedding_plan.clone()).expect("worker");
    assert!(matches!(
        worker.run_once(&env.ctx).await.expect("index a"),
        IndexWorkerRun::Completed { .. }
    ));
    let embedding_worker = embedding_worker(&env, &provider).expect("embedding worker");
    run_embedding_jobs(&env, &embedding_worker).await;

    let _version_b =
        seed_promoted_current_version(&env, document_a, collection_a, version_a, markdown_b).await;
    let (_document_c, _version_c, _collection_c) =
        seed_converted_document(&env.pool, &env.storage, &env.ctx, markdown_c).await;
    relay(&env, &embedding_plan).await;
    assert!(lifecycle_pending_count(&env).await >= 1);
    let index_pending_before = with_org_txn(&env.pool, &env.ctx, {
        let ctx = env.ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT count(*)::bigint FROM jobs
                         WHERE org_id = $1 AND job_type = 'index' AND status = 'pending'",
                        &[&ctx.org_id()],
                    )
                    .await?;
                Ok(row.get::<_, i64>(0))
            })
        }
    })
    .await
    .expect("index pending");
    assert!(
        index_pending_before >= 2,
        "need continuous index backlog, got {index_pending_before}"
    );

    worker.run_once(&env.ctx).await.expect("run_once 1");
    worker.run_once(&env.ctx).await.expect("run_once 2");
    assert!(
        lifecycle_pending_count(&env).await < lifecycle_job_count(&env).await,
        "lifecycle must be claimed within 2 run_once despite index backlog"
    );
    env.drop().await;
}
