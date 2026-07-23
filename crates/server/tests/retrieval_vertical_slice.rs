//! P1B-R02 vertical slice evidence (Sol round1):
//! HTTP upload → ConvertWorker → IndexWorker → citation resolve.
//! Does **not** SQL-seed `document_versions` / `derived_artifacts` / `chunks`.

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{
    admin_database_url, app_database_url, assert_markhand_app_role, boot_app_pool, build_router,
    login_access_token, seed_user_with_permissions, test_minio_client, MinioCleanupGuard,
};
use deadpool_postgres::Pool;
use fileconv_knowledge::embedding::{EmbeddingPlan, ProviderDeployment, RUNTIME_VLLM_LOCAL};
use fileconv_server::auth::context::OrgContext;
use fileconv_server::db::pool::with_org_txn;
use fileconv_server::jobs::{self};
use fileconv_server::services::citation::{resolve_citation, ResolveCitationRequest};
use fileconv_server::services::indexing::IndexingOutboxSink;
use fileconv_server::storage::qdrant::QdrantClient;
use fileconv_server::workers::convert::{ConvertWorker, ConvertWorkerConfig, ConvertWorkerRun};
use fileconv_server::workers::index::{IndexWorker, IndexWorkerConfig, IndexWorkerRun};
use fileconv_server::workers::limits::ResourceLimits;
use fileconv_server::workers::sandbox::SandboxConfig;
use http_body_util::BodyExt;
use tower::ServiceExt;
use uuid::Uuid;

const BOUNDARY: &str = "----markhandVerticalSliceBoundary";

fn fileconv_binary() -> Option<PathBuf> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/fileconv");
    path.exists().then_some(path)
}

fn test_qdrant() -> Option<QdrantClient> {
    let url = std::env::var("MARKHAND_TEST_QDRANT_URL").ok()?;
    if url.trim().is_empty() {
        return None;
    }
    QdrantClient::with_api_key(url, None).ok()
}

fn multipart(filename: &str, bytes: &[u8], collection_id: Uuid) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"collectionId\"\r\n\r\n{collection_id}\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(
        format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: text/plain\r\n\r\n"
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

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL/APP + MINIO + QDRANT + built fileconv"]
async fn live_upload_convert_index_citation_vertical_slice() {
    let Some(admin) = admin_database_url() else {
        return;
    };
    let Some(app_url) = app_database_url() else {
        return;
    };
    let Some(store) = test_minio_client() else {
        return;
    };
    let Some(qdrant) = test_qdrant() else {
        eprintln!("skipped: MARKHAND_TEST_QDRANT_URL unset");
        return;
    };
    let Some(fileconv) = fileconv_binary() else {
        panic!("target/debug/fileconv missing — build fileconv-cli for vertical slice evidence");
    };
    let cleanup = MinioCleanupGuard::new(store.clone());
    store.ensure_bucket().await.expect("bucket");

    let (ephemeral, pool) = boot_app_pool(&admin, &app_url).await;
    assert_markhand_app_role(&pool).await;
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    seed_user_with_permissions(
        &pool,
        org,
        user,
        &format!("{user}@vertical.test"),
        "correct-password-1",
        &[
            "qa.query",
            "qa.history",
            "doc.upload",
            "doc.delete",
            "doc.publish",
            "jobs.system",
        ],
    )
    .await;
    let token = login_access_token(
        &pool,
        &format!("{user}@vertical.test"),
        "correct-password-1",
    )
    .await;
    let app = build_router(pool.clone(), &ephemeral.app_url, Some(store.clone()));

    let (status, created) = json_post(
        app.clone(),
        "/api/v1/collections",
        &token,
        serde_json::json!({
            "name": "Vertical",
            "slug": format!("vertical-{}", Uuid::new_v4().simple()),
            "visibility": "org"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let collection_id = Uuid::parse_str(created["id"].as_str().unwrap()).unwrap();

    let source = b"Kinh phi du an la 15 trieu dong.\n";
    let upload_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/uploads")
                .header("authorization", format!("Bearer {token}"))
                .header("idempotency-key", "vertical-slice-upload-1")
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={BOUNDARY}"),
                )
                .body(Body::from(multipart("budget.txt", source, collection_id)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(upload_response.status(), StatusCode::CREATED);
    let upload_bytes = upload_response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes();
    let upload: serde_json::Value = serde_json::from_slice(&upload_bytes).unwrap();
    assert_eq!(upload["disposition"], "accepted");
    let document_id = Uuid::parse_str(upload["documentId"].as_str().unwrap()).unwrap();
    let source_version_id = Uuid::parse_str(upload["versionId"].as_str().unwrap()).unwrap();
    let convert_job_id = Uuid::parse_str(upload["jobId"].as_str().unwrap()).unwrap();

    let worker_ctx = OrgContext::try_new(
        org,
        user,
        ["doc.upload", "jobs.system", "qa.query"],
        [collection_id],
    )
    .unwrap();
    let mut convert_config = ConvertWorkerConfig::new(
        format!("vertical-convert-{}", Uuid::new_v4()),
        SandboxConfig {
            argv_template: vec![
                fileconv.display().to_string(),
                "one".into(),
                "{input}".into(),
            ],
            limits: ResourceLimits {
                wall_timeout: Duration::from_secs(30),
                ..ResourceLimits::default()
            },
        },
    );
    convert_config.heartbeat_interval = Duration::from_millis(50);
    convert_config.lease_ttl = Duration::from_secs(5);
    let convert_worker =
        ConvertWorker::new(pool.clone(), store.clone(), convert_config).expect("convert worker");
    let convert_run = convert_worker
        .run_once(&worker_ctx)
        .await
        .expect("convert run");
    assert!(
        matches!(
            convert_run,
            ConvertWorkerRun::Completed { job_id, .. } if job_id == convert_job_id
        ),
        "unexpected convert outcome: {convert_run:?}"
    );

    let (published_version_id, markdown_sha, source_sha) =
        load_published_version(&pool, &worker_ctx, document_id).await;
    assert_ne!(published_version_id, source_version_id);
    assert_ne!(markdown_sha, source_sha);

    // Relay convert outbox → index job, then index with mock embeddings.
    let mock = MockEmbedding::start();
    let embedding_plan = EmbeddingPlan::provider(
        "test",
        "test-embedding",
        "r1",
        ProviderDeployment::from_base_url(Some(mock.base_url())).expect("deployment"),
        Some(8),
        RUNTIME_VLLM_LOCAL,
    )
    .expect("plan");
    let sink = Arc::new(IndexingOutboxSink::new(&embedding_plan).expect("sink"));
    jobs::relay_outbox_with_sink(&pool, &worker_ctx, 32, &sink)
        .await
        .expect("relay");

    let mut index_config = IndexWorkerConfig::new(format!("vertical-index-{}", Uuid::new_v4()));
    index_config.lease_ttl = Duration::from_secs(30);
    index_config.heartbeat_interval = Duration::from_secs(5);
    index_config.max_job_duration = Duration::from_secs(60);
    index_config.embedding_batch_size = 8;
    let index_worker = IndexWorker::new_with_plan(
        pool.clone(),
        store.clone(),
        qdrant,
        index_config,
        None,
        embedding_plan,
    )
    .expect("index worker");
    let index_run = index_worker.run_once(&worker_ctx).await.expect("index run");
    assert!(
        matches!(index_run, IndexWorkerRun::Completed { .. }),
        "unexpected index outcome: {index_run:?}"
    );

    let chunk = load_first_chunk(&pool, &worker_ctx, document_id, published_version_id).await;
    let quote = chunk.body.clone();
    let resolved = resolve_citation(
        &pool,
        &worker_ctx,
        &store,
        ResolveCitationRequest {
            logical_document_id: document_id,
            version_id: published_version_id,
            source_content_sha256: source_sha,
            canonical_markdown_sha256: markdown_sha,
            chunk_id: chunk.id,
            source_span_start: chunk.span_start.unwrap_or(0) as usize,
            source_span_end: chunk.span_end.unwrap_or(quote.len() as i32) as usize,
            quote_local_start: 0,
            quote_local_end: quote.len(),
            quote: quote.clone(),
            require_current: true,
        },
    )
    .await
    .expect("citation resolve from worker-produced IDs");
    assert_eq!(resolved.logical_document_id, document_id);
    assert_eq!(resolved.version_id, published_version_id);
    assert_eq!(resolved.chunk_id, chunk.id);
    assert!(resolved.is_current);

    cleanup.cleanup().await.expect("minio bucket cleanup");
    ephemeral.drop().await;
}

async fn load_published_version(
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

struct ChunkRow {
    id: Uuid,
    body: String,
    span_start: Option<i32>,
    span_end: Option<i32>,
}

async fn load_first_chunk(
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

struct MockEmbedding {
    base_url: String,
    stopping: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl MockEmbedding {
    fn start() -> Self {
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
                        let mut buf = Vec::new();
                        let mut tmp = [0u8; 1024];
                        loop {
                            match stream.read(&mut tmp) {
                                Ok(0) => break,
                                Ok(n) => {
                                    buf.extend_from_slice(&tmp[..n]);
                                    if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                                        break;
                                    }
                                }
                                Err(_) => break,
                            }
                        }
                        let body = br#"{"data":[{"index":0,"embedding":[1,0,0,0,0,0,0,0]}]}"#;
                        let headers = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = stream.write_all(headers.as_bytes());
                        let _ = stream.write_all(body);
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

    fn base_url(&self) -> &str {
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
