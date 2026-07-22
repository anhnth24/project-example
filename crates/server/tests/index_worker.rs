//! Live tests for the durable index worker.
//!
//! These tests require PostgreSQL, MinIO, and Qdrant. They use a local
//! OpenAI-compatible mock for embeddings and are explicitly ignored unless
//! the live storage environment is configured.

use bytes::Bytes;
use deadpool_postgres::Pool;
use fileconv_knowledge::embedding::{EmbeddingPlan, ProviderDeployment, RUNTIME_VLLM_LOCAL};
use fileconv_server::auth::context::OrgContext;
use fileconv_server::config::{MinioConfig, Profile, SecretString};
use fileconv_server::database::apply_migrations;
use fileconv_server::db::collections::{self, NewCollection};
use fileconv_server::db::documents::{self, NewDocument};
use fileconv_server::db::models::{ArtifactKind, CollectionVisibility, DocumentState};
use fileconv_server::db::orgs;
use fileconv_server::db::pool::{create_pool, with_org_txn};
use fileconv_server::jobs::{self, EventPayload, CURRENT_EVENT_PAYLOAD_VERSION};
use fileconv_server::services::chunking::prepare_chunks;
use fileconv_server::services::embedding::ApprovedEmbeddingRuntime;
use fileconv_server::services::indexing::IndexingOutboxSink;
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
use tokio_postgres::NoTls;
use uuid::Uuid;

fn test_database_url() -> Result<String, String> {
    match std::env::var("MARKHAND_TEST_DATABASE_URL") {
        Ok(url) if !url.trim().is_empty() => Ok(url),
        _ => Err("MARKHAND_TEST_DATABASE_URL is required".into()),
    }
}

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
        let db_name = format!("markhand_index_worker_{}", Uuid::new_v4().simple());
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
    async fn boot() -> Result<Self, String> {
        let base_url = test_database_url()?;
        let storage = test_minio_client()?;
        let qdrant = test_qdrant_client()?;
        storage
            .ensure_bucket()
            .await
            .map_err(|error| format!("ensure test bucket: {error}"))?;
        let db = EphemeralDb::create(&base_url).await;
        apply_migrations(&db.url)
            .await
            .map_err(|error| format!("apply test migrations: {error}"))?;
        let pool = create_pool(&db.url).map_err(|error| format!("create test pool: {error}"))?;
        let ctx = OrgContext::try_new(Uuid::new_v4(), Uuid::new_v4(), ["doc.upload"], [])
            .map_err(|error| format!("create test org context: {error}"))?;
        Ok(Self {
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
    let signature_digest = signature.digest().unwrap();
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
    let signature = embedding_plan.index_signature(8).unwrap().digest().unwrap();
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
#[ignore = "requires MARKHAND_TEST_DATABASE_URL, MARKHAND_TEST_MINIO_*, and MARKHAND_TEST_QDRANT_URL"]
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

    let version_b =
        seed_promoted_current_version(&env, document_id, collection_id, version_a, markdown_b)
            .await;
    reset_index_job_for_version(&env, version_a).await;
    relay(&env, &embedding_plan).await;

    let stale_run = worker.run_once(&env.ctx).await.expect("stale run");
    assert!(matches!(
        stale_run,
        IndexWorkerRun::Completed { chunks, .. } if chunks == prepare_chunks(document_id, version_a, markdown_a, "").len()
    ));
    assert_eq!(
        document_state(&env, document_id).await,
        DocumentState::Converted
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

    let current_run = worker.run_once(&env.ctx).await.expect("current run");
    assert!(matches!(
        current_run,
        IndexWorkerRun::Completed { chunks, .. } if chunks == prepare_chunks(document_id, version_b, markdown_b, "").len()
    ));
    run_embedding_jobs(&env, &embedding_worker).await;
    assert_eq!(
        document_state(&env, document_id).await,
        DocumentState::Indexed
    );
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
