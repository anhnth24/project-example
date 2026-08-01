//! Shared HTTP upload → ConvertWorker → IndexWorker fixture helpers.
//!
//! Used by citation authz (history/IDOR/delete) and the retrieval vertical slice.
//! Does **not** SQL-seed `document_versions`, `derived_artifacts`, or `chunks`.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use deadpool_postgres::Pool;
use fileconv_knowledge::embedding::{EmbeddingPlan, ProviderDeployment, RUNTIME_VLLM_LOCAL};
use fileconv_server::auth::context::OrgContext;
use fileconv_server::config::Profile;
use fileconv_server::db::models::JobStatus;
use fileconv_server::db::pool::with_org_txn;
use fileconv_server::jobs;
use fileconv_server::services::embedding::ApprovedEmbeddingRuntime;
use fileconv_server::services::index_signature::{collection_name_for_signature, CollectionName};
use fileconv_server::services::indexing::IndexingOutboxSink;
use fileconv_server::storage::minio::MinioClient;
use fileconv_server::storage::qdrant::{QdrantAdminClient, QdrantClient};
use fileconv_server::workers::convert::{ConvertWorker, ConvertWorkerConfig, ConvertWorkerRun};
use fileconv_server::workers::embedding::{
    EmbeddingWorker, EmbeddingWorkerConfig, EmbeddingWorkerRun,
};
use fileconv_server::workers::index::{IndexWorker, IndexWorkerConfig, IndexWorkerRun};
use fileconv_server::workers::limits::ResourceLimits;
use fileconv_server::workers::sandbox::SandboxConfig;
use http_body_util::BodyExt;
use tower::ServiceExt;
use uuid::Uuid;

use super::{build_router, login_access_token, seed_user_with_permissions};

const BOUNDARY: &str = "----markhandWorkerPipelineBoundary";

/// Locate the `fileconv` binary used by ConvertWorker sandboxes.
pub fn fileconv_binary() -> Option<PathBuf> {
    super::fileconv_binary()
}

/// Live Qdrant client when `MARKHAND_TEST_QDRANT_URL` is set.
pub fn test_qdrant() -> Option<QdrantClient> {
    super::test_qdrant_client()
}

/// Admin client for collection cleanup.
pub fn test_qdrant_admin() -> Option<QdrantAdminClient> {
    super::test_qdrant_admin_client()
}

fn multipart(
    filename: &str,
    content_type: &str,
    bytes: &[u8],
    collection_id: Uuid,
    document_id: Option<Uuid>,
) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"collectionId\"\r\n\r\n{collection_id}\r\n"
        )
        .as_bytes(),
    );
    if let Some(document_id) = document_id {
        body.extend_from_slice(
            format!(
                "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"documentId\"\r\n\r\n{document_id}\r\n"
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(
        format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: {content_type}\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    body
}

async fn json_post(
    app: axum::Router,
    uri: &str,
    token: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| serde_json::json!({ "raw": String::from_utf8_lossy(&bytes) }));
    (status, json)
}

/// Document/version/chunk identity produced solely by upload + workers.
#[derive(Debug, Clone)]
pub struct WorkerProducedDoc {
    pub org: Uuid,
    pub user: Uuid,
    pub collection_id: Uuid,
    pub document_id: Uuid,
    pub version_id: Uuid,
    pub source_sha: String,
    pub markdown_sha: String,
    pub chunk_id: Uuid,
    pub span_start: usize,
    pub span_end: usize,
    pub quote: String,
    /// Exact permissions seeded for this principal (and used for worker `OrgContext`).
    pub permissions: BTreeSet<String>,
}

/// Existing document identity and source submitted through the production upload route.
pub struct ExistingDocumentRevision<'a> {
    pub access_token: &'a str,
    pub org_id: Uuid,
    pub user_id: Uuid,
    pub collection_id: Uuid,
    pub document_id: Uuid,
    pub permissions: &'a [&'a str],
    pub filename: &'a str,
    pub content_type: &'a str,
    pub source: &'a [u8],
    pub label: &'a str,
}

/// Build the worker `OrgContext` from the caller's effective permission set only.
///
/// Does not inject an unrequested permission superset — regressions that drop
/// required codes (e.g. `jobs.system`) must surface when workers run.
pub fn worker_org_context(
    org: Uuid,
    user: Uuid,
    collection_id: Uuid,
    permissions: impl IntoIterator<Item = impl Into<String>>,
) -> OrgContext {
    OrgContext::try_new(org, user, permissions, [collection_id]).expect("worker org context")
}

#[test]
fn worker_org_context_preserves_exact_permissions_without_superset() {
    let org = Uuid::from_u128(0x1111);
    let user = Uuid::from_u128(0x2222);
    let collection = Uuid::from_u128(0x3333);
    let supplied = ["doc.upload", "jobs.system"];
    let ctx = worker_org_context(org, user, collection, supplied);
    let expected: BTreeSet<String> = supplied.iter().map(|s| (*s).to_string()).collect();
    assert_eq!(ctx.permissions(), &expected);
    assert!(!ctx.has_permission("qa.history"));
    assert!(!ctx.has_permission("doc.delete"));
    assert!(!ctx.has_permission("qa.query"));
}

/// In-process mock OpenAI-compatible embedding server (8-dim unit vectors).
pub struct MockEmbedding {
    base_url: String,
    stopping: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl MockEmbedding {
    pub fn start() -> Self {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.set_nonblocking(true).expect("nonblocking");
        let base_url = format!("http://{}/v1", listener.local_addr().expect("addr"));
        let stopping = Arc::new(AtomicBool::new(false));
        let thread_stopping = Arc::clone(&stopping);
        let thread = thread::spawn(move || {
            while !thread_stopping.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_read_timeout(Some(Duration::from_secs(5)))
                            .expect("read timeout");
                        let mut buf = Vec::new();
                        let mut tmp = [0u8; 1024];
                        let body_start = loop {
                            if let Some(offset) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                                break Some(offset + 4);
                            }
                            match stream.read(&mut tmp) {
                                Ok(0) => break None,
                                Ok(n) => buf.extend_from_slice(&tmp[..n]),
                                Err(_) => break None,
                            }
                        };
                        let Some(body_start) = body_start else {
                            continue;
                        };
                        let headers = String::from_utf8_lossy(&buf[..body_start]);
                        let content_length = headers
                            .lines()
                            .filter_map(|line| line.split_once(':'))
                            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                            .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        while buf.len() < body_start + content_length {
                            match stream.read(&mut tmp) {
                                Ok(0) => break,
                                Ok(n) => buf.extend_from_slice(&tmp[..n]),
                                Err(_) => break,
                            }
                        }
                        let input_count = serde_json::from_slice::<serde_json::Value>(
                            &buf[body_start..buf.len().min(body_start + content_length)],
                        )
                        .ok()
                        .and_then(|value| value["input"].as_array().map(Vec::len))
                        .unwrap_or(1);
                        let data = (0..input_count)
                            .map(|index| {
                                serde_json::json!({
                                    "index": index,
                                    "embedding": [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
                                })
                            })
                            .collect::<Vec<_>>();
                        let body = serde_json::to_vec(&serde_json::json!({ "data": data }))
                            .expect("embedding response");
                        let headers = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = stream.write_all(headers.as_bytes());
                        let _ = stream.write_all(&body);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            base_url,
            stopping,
            thread: Some(thread),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }
}

impl Drop for MockEmbedding {
    fn drop(&mut self) {
        self.stopping
            .store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Shared convert/index/embedding workers bound to one ephemeral pool + store.
pub struct WorkerPipeline {
    pool: Pool,
    store: MinioClient,
    app: axum::Router,
    fileconv: PathBuf,
    mock: MockEmbedding,
    sink: Arc<IndexingOutboxSink>,
    index_worker: IndexWorker,
    embedding_worker: EmbeddingWorker,
    qdrant_admin: QdrantAdminClient,
    qdrant_collection: CollectionName,
}

impl WorkerPipeline {
    /// Bootstrap workers after live DB + MinIO are already available.
    ///
    /// Missing Qdrant URL or `fileconv` panics (fail clearly) — callers soft-skip
    /// only before DB/MinIO bootstrap, never after partial CI setup.
    pub async fn boot(pool: Pool, store: MinioClient, app_database_url: &str) -> Self {
        let qdrant = test_qdrant().unwrap_or_else(|| {
            panic!(
                "MARKHAND_TEST_QDRANT_URL is required once live DB/MinIO are configured \
                 (worker-produced citation fixtures need IndexWorker)"
            )
        });
        let qdrant_admin = test_qdrant_admin().unwrap_or_else(|| {
            panic!(
                "MARKHAND_TEST_QDRANT_ADMIN_API_KEY (or default operator key) is required \
                 once MARKHAND_TEST_QDRANT_URL is set"
            )
        });
        let fileconv = fileconv_binary().unwrap_or_else(|| {
            panic!("target/debug/fileconv missing — build fileconv-cli for worker fixtures")
        });

        store.ensure_bucket().await.expect("bucket");
        let app = build_router(pool.clone(), app_database_url, Some(store.clone()));

        let mock = MockEmbedding::start();
        let embedding_revision = format!("r1-{}", Uuid::new_v4().simple());
        let embedding_plan = EmbeddingPlan::provider(
            "test",
            "test-embedding",
            embedding_revision.clone(),
            ProviderDeployment::from_base_url(Some(mock.base_url())).expect("deployment"),
            Some(8),
            RUNTIME_VLLM_LOCAL,
        )
        .expect("plan");
        let qdrant_collection = {
            let signature = embedding_plan.index_signature(8).expect("index signature");
            collection_name_for_signature(&signature).expect("qdrant collection name")
        };
        let sink = Arc::new(IndexingOutboxSink::new(&embedding_plan).expect("sink"));
        let mut index_config = IndexWorkerConfig::new(format!("pipeline-index-{}", Uuid::new_v4()));
        index_config.lease_ttl = Duration::from_secs(30);
        index_config.heartbeat_interval = Duration::from_secs(5);
        index_config.max_job_duration = Duration::from_secs(60);
        index_config.embedding_batch_size = 8;
        let index_worker = IndexWorker::new_with_plan(
            pool.clone(),
            store.clone(),
            qdrant.clone(),
            index_config,
            None,
            embedding_plan,
        )
        .expect("index worker");
        let mut embedding_config =
            EmbeddingWorkerConfig::new(format!("pipeline-embedding-{}", Uuid::new_v4()));
        embedding_config.lease_ttl = Duration::from_secs(30);
        embedding_config.heartbeat_interval = Duration::from_secs(5);
        embedding_config.max_job_duration = Duration::from_secs(60);
        let embedding_runtime = ApprovedEmbeddingRuntime::new(
            mock.base_url().to_string(),
            "test-api-key".into(),
            "test".into(),
            "test-embedding".into(),
            embedding_revision,
            8,
            RUNTIME_VLLM_LOCAL.into(),
            Profile::Test,
            false,
            None,
        )
        .expect("embedding runtime");
        let embedding_worker =
            EmbeddingWorker::new(pool.clone(), qdrant, embedding_config, embedding_runtime)
                .expect("embedding worker");

        Self {
            pool,
            store,
            app,
            fileconv,
            mock,
            sink,
            index_worker,
            embedding_worker,
            qdrant_admin,
            qdrant_collection,
        }
    }

    pub fn pool(&self) -> &Pool {
        &self.pool
    }

    pub fn store(&self) -> &MinioClient {
        &self.store
    }

    pub fn app(&self) -> axum::Router {
        self.app.clone()
    }

    pub fn mock_base_url(&self) -> &str {
        self.mock.base_url()
    }

    pub async fn cleanup_qdrant(&self) {
        self.qdrant_admin
            .delete_collection(&self.qdrant_collection)
            .await
            .expect("qdrant collection cleanup");
    }

    /// Seed org/user, create collection via HTTP, upload + convert + index one file.
    pub async fn produce_indexed(
        &self,
        label: &str,
        filename: &str,
        content_type: &str,
        source: &[u8],
        permissions: &[&str],
    ) -> WorkerProducedDoc {
        let org = Uuid::new_v4();
        let user = Uuid::new_v4();
        let email = format!("{user}@worker.test");
        seed_user_with_permissions(
            &self.pool,
            org,
            user,
            &email,
            "correct-password-1",
            permissions,
        )
        .await;
        let token = login_access_token(&self.pool, &email, "correct-password-1").await;
        let (status, created) = json_post(
            self.app.clone(),
            "/api/v1/collections",
            &token,
            serde_json::json!({
                "name": format!("Worker {label}"),
                "slug": format!("worker-{}-{}", label, Uuid::new_v4().simple()),
                "visibility": "org"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        let collection_id = Uuid::parse_str(created["id"].as_str().unwrap()).unwrap();
        let worker_ctx = worker_org_context(org, user, collection_id, permissions.iter().copied());

        self.upload_convert_index(
            &token,
            &worker_ctx,
            collection_id,
            None,
            filename,
            content_type,
            source,
            label,
        )
        .await
    }

    /// Upload a revision onto an existing org document and run convert/index workers.
    ///
    /// Preserves the caller's exact permission set — no superset injection.
    pub async fn index_existing_document_revision(
        &self,
        revision: ExistingDocumentRevision<'_>,
    ) -> WorkerProducedDoc {
        let worker_ctx = worker_org_context(
            revision.org_id,
            revision.user_id,
            revision.collection_id,
            revision.permissions.iter().copied(),
        );
        self.upload_convert_index(
            revision.access_token,
            &worker_ctx,
            revision.collection_id,
            Some(revision.document_id),
            revision.filename,
            revision.content_type,
            revision.source,
            revision.label,
        )
        .await
    }

    /// Upload a revision onto an existing worker-produced document and re-index.
    pub async fn produce_revision(
        &self,
        doc: &WorkerProducedDoc,
        filename: &str,
        content_type: &str,
        source: &[u8],
        label: &str,
    ) -> WorkerProducedDoc {
        let email = format!("{}@worker.test", doc.user);
        let token = login_access_token(&self.pool, &email, "correct-password-1").await;
        let worker_ctx = worker_org_context(
            doc.org,
            doc.user,
            doc.collection_id,
            doc.permissions.iter().cloned(),
        );
        self.upload_convert_index(
            &token,
            &worker_ctx,
            doc.collection_id,
            Some(doc.document_id),
            filename,
            content_type,
            source,
            label,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn upload_convert_index(
        &self,
        token: &str,
        worker_ctx: &OrgContext,
        collection_id: Uuid,
        document_id: Option<Uuid>,
        filename: &str,
        content_type: &str,
        source: &[u8],
        label: &str,
    ) -> WorkerProducedDoc {
        let upload_response = self
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/uploads")
                    .header("authorization", format!("Bearer {token}"))
                    .header(
                        "idempotency-key",
                        format!("worker-pipeline-{label}-{}", Uuid::new_v4().simple()),
                    )
                    .header(
                        "content-type",
                        format!("multipart/form-data; boundary={BOUNDARY}"),
                    )
                    .body(Body::from(multipart(
                        filename,
                        content_type,
                        source,
                        collection_id,
                        document_id,
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            upload_response.status(),
            StatusCode::CREATED,
            "{label} upload status"
        );
        let upload_bytes = upload_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let upload: serde_json::Value = serde_json::from_slice(&upload_bytes).unwrap();
        assert_eq!(upload["disposition"], "accepted", "{label} disposition");
        let document_id = Uuid::parse_str(upload["documentId"].as_str().unwrap()).unwrap();
        let convert_job_id = Uuid::parse_str(upload["jobId"].as_str().unwrap()).unwrap();

        let mut convert_config = ConvertWorkerConfig::new(
            format!("pipeline-convert-{label}-{}", Uuid::new_v4()),
            SandboxConfig {
                argv_template: vec![
                    self.fileconv.display().to_string(),
                    "one".into(),
                    "{input}".into(),
                ],
                limits: ResourceLimits {
                    wall_timeout: Duration::from_secs(30),
                    ..ResourceLimits::default()
                },
            },
        );
        convert_config.heartbeat_interval = Duration::from_secs(5);
        convert_config.lease_ttl = Duration::from_secs(60);
        let convert_run = run_convert_worker_until_completed(
            &self.pool,
            &self.store,
            worker_ctx,
            convert_job_id,
            convert_config,
        )
        .await;
        assert!(
            matches!(
                convert_run,
                ConvertWorkerRun::Completed { job_id, .. } if job_id == convert_job_id
            ),
            "{label} unexpected convert outcome: {convert_run:?}"
        );

        let (published_version_id, markdown_sha, source_sha) =
            load_published_version(&self.pool, worker_ctx, document_id).await;
        assert_ne!(markdown_sha, source_sha, "{label} dual-hash identity");

        jobs::relay_outbox_with_sink(&self.pool, worker_ctx, 32, &self.sink)
            .await
            .unwrap_or_else(|error| panic!("{label} relay: {error}"));
        let index_runs = drain_index_jobs(&self.index_worker, worker_ctx).await;
        assert!(
            index_runs > 0,
            "{label} must complete at least one index/lifecycle job"
        );
        let embedding_runs = drain_embedding_jobs(&self.embedding_worker, worker_ctx).await;
        assert!(
            embedding_runs > 0,
            "{label} must complete at least one embedding batch"
        );

        let chunk =
            load_first_chunk(&self.pool, worker_ctx, document_id, published_version_id).await;
        WorkerProducedDoc {
            org: worker_ctx.org_id(),
            user: worker_ctx.user_id(),
            collection_id,
            document_id,
            version_id: published_version_id,
            source_sha,
            markdown_sha,
            chunk_id: chunk.id,
            span_start: chunk.span_start.unwrap_or(0) as usize,
            span_end: chunk.span_end.unwrap_or(chunk.body.len() as i32) as usize,
            quote: chunk.body,
            permissions: worker_ctx.permissions().clone(),
        }
    }
}

async fn run_convert_worker_until_completed(
    pool: &Pool,
    storage: &MinioClient,
    ctx: &OrgContext,
    convert_job_id: Uuid,
    config: ConvertWorkerConfig,
) -> ConvertWorkerRun {
    let worker = ConvertWorker::new(pool.clone(), storage.clone(), config).expect("convert worker");
    for round in 0..12 {
        let outcome = worker.run_once(ctx).await.expect("convert worker run");
        if !matches!(outcome, ConvertWorkerRun::Completed { .. })
            && job_status(pool, ctx, convert_job_id).await == JobStatus::Succeeded
        {
            return ConvertWorkerRun::Completed {
                job_id: convert_job_id,
                markdown_bytes: 0,
            };
        }
        match outcome {
            ConvertWorkerRun::Completed { job_id, .. } if job_id == convert_job_id => {
                return outcome;
            }
            ConvertWorkerRun::Failed {
                job_id,
                terminal: true,
                ..
            } if job_id == convert_job_id => {
                panic!("convert job {convert_job_id} dead-lettered on round {round}");
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
    unreachable!("convert worker run budget exhausted");
}

async fn job_status(pool: &Pool, ctx: &OrgContext, job_id: Uuid) -> JobStatus {
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT status FROM jobs WHERE org_id = $1 AND id = $2",
                        &[&ctx.org_id(), &job_id],
                    )
                    .await?;
                let status: String = row.get(0);
                Ok(JobStatus::parse(&status).unwrap_or(JobStatus::Pending))
            })
        }
    })
    .await
    .expect("job status")
}

pub async fn drain_index_jobs(worker: &IndexWorker, ctx: &OrgContext) -> usize {
    let mut completed = 0;
    for _ in 0..32 {
        match worker.run_once(ctx).await.expect("index/lifecycle run") {
            IndexWorkerRun::Completed { .. } => completed += 1,
            IndexWorkerRun::NoJob => return completed,
            outcome => panic!("unexpected index/lifecycle outcome: {outcome:?}"),
        }
    }
    panic!("index/lifecycle queue did not drain within 32 jobs");
}

pub async fn drain_embedding_jobs(worker: &EmbeddingWorker, ctx: &OrgContext) -> usize {
    let mut completed = 0;
    for _ in 0..32 {
        match worker.run_once(ctx).await.expect("embedding run") {
            EmbeddingWorkerRun::Completed { .. } => completed += 1,
            EmbeddingWorkerRun::NoJob => return completed,
            outcome => panic!("unexpected embedding outcome: {outcome:?}"),
        }
    }
    panic!("embedding queue did not drain within 32 jobs");
}

pub async fn load_published_version(
    pool: &Pool,
    ctx: &OrgContext,
    document_id: Uuid,
) -> (Uuid, String, String) {
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT dv.id, da.content_sha256 AS markdown_sha, dv.content_sha256 AS source_sha
                         FROM documents d
                         JOIN document_versions dv
                           ON dv.org_id = d.org_id AND dv.id = d.current_version_id
                         JOIN derived_artifacts da
                           ON da.org_id = dv.org_id
                          AND da.version_id = dv.id
                          AND da.artifact_kind = 'markdown'
                         WHERE d.org_id = $1 AND d.id = $2
                           AND dv.publication_state = 'published'
                           AND dv.is_current",
                        &[&ctx.org_id(), &document_id],
                    )
                    .await?;
                Ok((row.get(0), row.get(1), row.get(2)))
            })
        }
    })
    .await
    .expect("published version from convert worker")
}

pub struct ChunkRow {
    pub id: Uuid,
    pub body: String,
    pub span_start: Option<i32>,
    pub span_end: Option<i32>,
}

pub async fn load_first_chunk(
    pool: &Pool,
    ctx: &OrgContext,
    document_id: Uuid,
    version_id: Uuid,
) -> ChunkRow {
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT id, body, span_start, span_end
                         FROM chunks
                         WHERE org_id = $1 AND document_id = $2 AND version_id = $3
                         ORDER BY ordinal
                         LIMIT 1",
                        &[&ctx.org_id(), &document_id, &version_id],
                    )
                    .await?;
                Ok(ChunkRow {
                    id: row.get(0),
                    body: row.get(1),
                    span_start: row.get(2),
                    span_end: row.get(3),
                })
            })
        }
    })
    .await
    .expect("chunk produced by index worker")
}
