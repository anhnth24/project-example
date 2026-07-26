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
    login_access_token, seed_user_with_permissions, take_live, test_minio_client, tiny_docx_bytes,
    tiny_pdf_bytes, tiny_png_ocr_bytes, tiny_pptx_bytes, tiny_xlsx_bytes, MinioCleanupGuard,
};
use deadpool_postgres::Pool;
use fileconv_knowledge::embedding::{EmbeddingPlan, ProviderDeployment, RUNTIME_VLLM_LOCAL};
use fileconv_server::auth::context::OrgContext;
use fileconv_server::config::{Profile, SecretString};
use fileconv_server::db::pool::with_org_txn;
use fileconv_server::jobs::{self};
use fileconv_server::services::citation::{
    resolve_citation, CitationError, ResolveCitationRequest,
};
use fileconv_server::services::embedding::ApprovedEmbeddingRuntime;
use fileconv_server::services::index_signature::collection_name_for_signature;
use fileconv_server::services::indexing::IndexingOutboxSink;
use fileconv_server::storage::parse_key_for_org;
use fileconv_server::storage::qdrant::{QdrantAdminApiKey, QdrantAdminClient, QdrantClient};
use fileconv_server::workers::convert::{ConvertWorker, ConvertWorkerConfig, ConvertWorkerRun};
use fileconv_server::workers::embedding::{
    EmbeddingWorker, EmbeddingWorkerConfig, EmbeddingWorkerRun,
};
use fileconv_server::workers::index::{IndexWorker, IndexWorkerConfig, IndexWorkerRun};
use fileconv_server::workers::limits::ResourceLimits;
use fileconv_server::workers::sandbox::{self, SandboxCancel, SandboxConfig, SandboxInput};
use http_body_util::BodyExt;
use tower::ServiceExt;
use uuid::Uuid;

const BOUNDARY: &str = "----markhandVerticalSliceBoundary";

fn fileconv_binary() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("MARKHAND_TEST_FILECONV_BIN") {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path);
        }
    }
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

fn test_qdrant_admin() -> Option<QdrantAdminClient> {
    let url = std::env::var("MARKHAND_TEST_QDRANT_URL").ok()?;
    let key = std::env::var("MARKHAND_TEST_QDRANT_ADMIN_API_KEY").ok()?;
    QdrantAdminClient::new(
        url,
        QdrantAdminApiKey::new(SecretString::new(key)).expect("admin key"),
    )
    .ok()
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

/// Source of truth for expected formats: `bench/markhand_web/workloads/phase1b-mixed.yaml`.
fn expected_formats_from_workload() -> Vec<String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../bench/markhand_web/workloads/phase1b-mixed.yaml");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read workload {}: {error}", path.display()));
    let line = text
        .lines()
        .find(|line| line.contains("formats:"))
        .unwrap_or_else(|| panic!("formats: missing in {}", path.display()));
    let start = line
        .find('[')
        .and_then(|i| line[i + 1..].find(']').map(|j| (i + 1, i + 1 + j)))
        .unwrap_or_else(|| panic!("formats list missing in {}", path.display()));
    let mut formats: Vec<String> = line[start.0..start.1]
        .split(',')
        .map(|part| part.trim().to_ascii_lowercase())
        .filter(|part| !part.is_empty())
        .collect();
    formats.sort();
    formats.dedup();
    assert!(
        !formats.is_empty(),
        "empty formats list in {}",
        path.display()
    );
    formats
}

/// Fixture matrix keyed by workload formats (must cover every expected format).
fn vertical_format_cases() -> Vec<(
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    Vec<u8>,
)> {
    vec![
        (
            "csv",
            "budget.csv",
            "text/csv",
            "O04CSV15",
            b"item,amount\nKinh phi CSV O04CSV15 la 15000000\n".to_vec(),
        ),
        (
            "docx",
            "budget.docx",
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            "O04DOCX15",
            tiny_docx_bytes("Kinh phi DOCX O04DOCX15"),
        ),
        (
            "html",
            "budget.html",
            "text/html",
            "O04HTML15",
            b"<html><body><p>Kinh phi HTML O04HTML15</p></body></html>".to_vec(),
        ),
        (
            "pdf",
            "budget.pdf",
            "application/pdf",
            "O04PDF15",
            tiny_pdf_bytes("Kinh phi PDF O04PDF15"),
        ),
        (
            "png",
            "budget.png",
            "image/png",
            "SOAK15",
            // Shared real OCR fixture; missing OCR runtime must fail the live suite.
            tiny_png_ocr_bytes("SOAK15"),
        ),
        (
            "pptx",
            "budget.pptx",
            "application/vnd.openxmlformats-officedocument.presentationml.presentation",
            "O04PPTX15",
            tiny_pptx_bytes("Kinh phi PPTX O04PPTX15"),
        ),
        (
            "txt",
            "budget.txt",
            "text/plain",
            "O04TXT15",
            b"Kinh phi du an O04TXT15 la 15 trieu dong.\n".to_vec(),
        ),
        (
            "xlsx",
            "budget.xlsx",
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            "O04XLSX15",
            tiny_xlsx_bytes("Kinh phi XLSX O04XLSX15"),
        ),
    ]
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
    let Some(admin) = take_live(admin_database_url(), "MARKHAND_TEST_DATABASE_URL") else {
        return;
    };
    let Some(app_url) = take_live(app_database_url(), "MARKHAND_TEST_APP_DATABASE_URL") else {
        return;
    };
    let Some(store) = take_live(test_minio_client(), "MARKHAND_TEST_MINIO_*") else {
        return;
    };
    let Some(qdrant) = take_live(test_qdrant(), "MARKHAND_TEST_QDRANT_URL") else {
        eprintln!("skipped: MARKHAND_TEST_QDRANT_URL unset");
        return;
    };
    let Some(qdrant_admin) = take_live(test_qdrant_admin(), "MARKHAND_TEST_QDRANT_ADMIN_API_KEY")
    else {
        eprintln!("skipped: MARKHAND_TEST_QDRANT_ADMIN_API_KEY unset");
        return;
    };
    let Some(fileconv) = take_live(fileconv_binary(), "target/debug/fileconv") else {
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
    let worker_ctx = OrgContext::try_new(
        org,
        user,
        ["doc.upload", "jobs.system", "qa.query"],
        [collection_id],
    )
    .unwrap();
    // One embedding plan/signature for the whole matrix — swapping mock URLs
    // mid-collection produces index signature mismatch against the active generation.
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
    let mut index_config = IndexWorkerConfig::new(format!("vertical-index-{}", Uuid::new_v4()));
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
        EmbeddingWorkerConfig::new(format!("vertical-embedding-{}", Uuid::new_v4()));
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

    let expected_formats = expected_formats_from_workload();
    let cases = vertical_format_cases();
    let case_exts: Vec<String> = cases.iter().map(|(ext, ..)| (*ext).to_string()).collect();
    assert_eq!(
        case_exts, expected_formats,
        "vertical_format_cases must match phase1b-mixed.yaml ingest formats exactly"
    );
    let mut observed_formats: Vec<String> = Vec::new();

    for (ext, filename, content_type, source_marker, source) in cases {
        let upload_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/uploads")
                    .header("authorization", format!("Bearer {token}"))
                    .header(
                        "idempotency-key",
                        format!("vertical-slice-upload-{ext}-{}", Uuid::new_v4().simple()),
                    )
                    .header(
                        "content-type",
                        format!("multipart/form-data; boundary={BOUNDARY}"),
                    )
                    .body(Body::from(multipart(
                        filename,
                        content_type,
                        &source,
                        collection_id,
                        None,
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            upload_response.status(),
            StatusCode::CREATED,
            "{ext} upload status"
        );
        let upload_bytes = upload_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let upload: serde_json::Value = serde_json::from_slice(&upload_bytes).unwrap();
        assert_eq!(upload["disposition"], "accepted", "{ext} disposition");
        let document_id = Uuid::parse_str(upload["documentId"].as_str().unwrap()).unwrap();
        let source_version_id = Uuid::parse_str(upload["versionId"].as_str().unwrap()).unwrap();
        let convert_job_id = Uuid::parse_str(upload["jobId"].as_str().unwrap()).unwrap();

        let mut convert_config = ConvertWorkerConfig::new(
            format!("vertical-convert-{ext}-{}", Uuid::new_v4()),
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
        // This matrix validates format/history behavior, not lease expiry. Keep
        // the lease above the sandbox's 30-second wall timeout so a valid but
        // cold converter cannot be misclassified as reconciliation-needed.
        convert_config.heartbeat_interval = Duration::from_secs(5);
        convert_config.lease_ttl = Duration::from_secs(60);
        let convert_worker = ConvertWorker::new(pool.clone(), store.clone(), convert_config)
            .expect("convert worker");
        let convert_run = convert_worker
            .run_once(&worker_ctx)
            .await
            .unwrap_or_else(|error| panic!("{ext} convert run: {error}"));
        let worker_org_id = worker_ctx.org_id();
        let convert_last_error = with_org_txn(&pool, &worker_ctx, move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT last_error FROM jobs WHERE org_id = $1 AND id = $2",
                        &[&worker_org_id, &convert_job_id],
                    )
                    .await?;
                Ok::<_, fileconv_server::db::error::DbError>(
                    row.get::<_, Option<String>>("last_error"),
                )
            })
        })
        .await
        .unwrap_or_else(|error| panic!("{ext} load convert job: {error}"));
        assert!(
            matches!(
                convert_run,
                ConvertWorkerRun::Completed { job_id, .. } if job_id == convert_job_id
            ),
            "{ext} unexpected convert outcome: {convert_run:?}; last_error={convert_last_error:?}"
        );

        let (published_version_id, markdown_sha, source_sha) =
            load_published_version(&pool, &worker_ctx, document_id).await;
        assert_ne!(
            published_version_id, source_version_id,
            "{ext} published version must differ from upload draft"
        );
        assert_ne!(markdown_sha, source_sha, "{ext} dual-hash identity");

        jobs::relay_outbox_with_sink(&pool, &worker_ctx, 32, &sink)
            .await
            .unwrap_or_else(|error| panic!("{ext} relay: {error}"));
        let index_runs = drain_index_jobs(&index_worker, &worker_ctx).await;
        assert!(
            index_runs > 0,
            "{ext} must complete at least one index/lifecycle job"
        );
        let embedding_runs = drain_embedding_jobs(&embedding_worker, &worker_ctx).await;
        assert!(
            embedding_runs > 0,
            "{ext} must complete at least one embedding batch"
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
                source_content_sha256: source_sha.clone(),
                canonical_markdown_sha256: markdown_sha.clone(),
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
        .unwrap_or_else(|error| panic!("{ext} citation resolve: {error:?}"));
        assert_eq!(resolved.logical_document_id, document_id);
        assert_eq!(resolved.version_id, published_version_id);
        assert_eq!(resolved.chunk_id, chunk.id);
        assert!(resolved.is_current, "{ext} citation must be current");
        let marker = source_marker.to_ascii_uppercase();
        assert!(
            resolved.quote.to_ascii_uppercase().contains(&marker)
                || chunk.body.to_ascii_uppercase().contains(&marker),
            "{ext} conversion/index/citation path must recover source marker {source_marker}"
        );

        if ext == "txt" {
            let revision_source = b"Kinh phi du an O04TXT20 la 20 trieu dong.\n".to_vec();
            let revision_response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/api/v1/uploads")
                        .header("authorization", format!("Bearer {token}"))
                        .header(
                            "idempotency-key",
                            format!("vertical-slice-revision-{}", Uuid::new_v4().simple()),
                        )
                        .header(
                            "content-type",
                            format!("multipart/form-data; boundary={BOUNDARY}"),
                        )
                        .body(Body::from(multipart(
                            "budget-v2.txt",
                            "text/plain",
                            &revision_source,
                            collection_id,
                            Some(document_id),
                        )))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(revision_response.status(), StatusCode::CREATED);
            let revision: serde_json::Value = serde_json::from_slice(
                &revision_response
                    .into_body()
                    .collect()
                    .await
                    .unwrap()
                    .to_bytes(),
            )
            .unwrap();
            assert_eq!(revision["documentId"], document_id.to_string());
            let revision_source_version_id =
                Uuid::parse_str(revision["versionId"].as_str().unwrap()).unwrap();
            let revision_convert_job_id =
                Uuid::parse_str(revision["jobId"].as_str().unwrap()).unwrap();
            let revision_key =
                parse_key_for_org(revision["objectKey"].as_str().unwrap(), org).unwrap();
            let stored_revision = store
                .get_object(org, &revision_key)
                .await
                .expect("read stored revision");
            assert_eq!(
                stored_revision.as_ref(),
                revision_source.as_slice(),
                "revision upload must preserve exact source bytes"
            );

            let revision_convert = convert_worker
                .run_once(&worker_ctx)
                .await
                .expect("convert revision");
            let revision_worker_org_id = worker_ctx.org_id();
            let revision_last_error = with_org_txn(&pool, &worker_ctx, move |txn| {
                Box::pin(async move {
                    let row = txn
                        .query_one(
                            "SELECT last_error FROM jobs WHERE org_id = $1 AND id = $2",
                            &[&revision_worker_org_id, &revision_convert_job_id],
                        )
                        .await?;
                    Ok::<_, fileconv_server::db::error::DbError>(
                        row.get::<_, Option<String>>("last_error"),
                    )
                })
            })
            .await
            .expect("load revision convert job");
            if !matches!(
                revision_convert,
                ConvertWorkerRun::Completed { job_id, .. }
                    if job_id == revision_convert_job_id
            ) {
                let diagnostic_fileconv = fileconv.clone();
                let diagnostic_input = stored_revision.to_vec();
                let diagnostic = tokio::task::spawn_blocking(move || {
                    sandbox::run(
                        &SandboxConfig {
                            argv_template: vec![
                                diagnostic_fileconv.display().to_string(),
                                "one".into(),
                                "{input}".into(),
                            ],
                            limits: ResourceLimits {
                                wall_timeout: Duration::from_secs(30),
                                ..ResourceLimits::default()
                            },
                        },
                        SandboxInput {
                            bytes: diagnostic_input,
                            canonical_extension: "txt".into(),
                        },
                        &SandboxCancel::default(),
                    )
                })
                .await
                .expect("join revision diagnostic")
                .expect("run revision diagnostic");
                panic!(
                    "revision must run through ConvertWorker: {revision_convert:?}; \
                     last_error={revision_last_error:?}; direct_exit={:?}; direct_stderr={}",
                    diagnostic.exit,
                    String::from_utf8_lossy(&diagnostic.stderr)
                );
            }
            let (revision_version_id, revision_markdown_sha, revision_source_sha) =
                load_published_version(&pool, &worker_ctx, document_id).await;
            assert_ne!(revision_version_id, published_version_id);

            jobs::relay_outbox_with_sink(&pool, &worker_ctx, 32, &sink)
                .await
                .expect("relay revision index");
            let revision_index_runs = drain_index_jobs(&index_worker, &worker_ctx).await;
            assert!(
                revision_index_runs >= 2,
                "revision must process lifecycle refresh and new index; completed {revision_index_runs}"
            );
            let revision_embedding_runs =
                drain_embedding_jobs(&embedding_worker, &worker_ctx).await;
            assert!(
                revision_embedding_runs > 0,
                "revision must complete at least one embedding batch"
            );
            let revision_chunk =
                load_first_chunk(&pool, &worker_ctx, document_id, revision_version_id).await;
            assert!(revision_chunk
                .body
                .to_ascii_uppercase()
                .contains("O04TXT20"));

            let history_ctx =
                OrgContext::try_new(org, user, ["qa.query", "qa.history"], [collection_id])
                    .unwrap();
            let no_history_ctx =
                OrgContext::try_new(org, user, ["qa.query"], [collection_id]).unwrap();
            let historical = resolve_citation(
                &pool,
                &history_ctx,
                &store,
                ResolveCitationRequest {
                    logical_document_id: document_id,
                    version_id: published_version_id,
                    source_content_sha256: source_sha.clone(),
                    canonical_markdown_sha256: markdown_sha.clone(),
                    chunk_id: chunk.id,
                    source_span_start: chunk.span_start.unwrap_or(0) as usize,
                    source_span_end: chunk.span_end.unwrap_or(quote.len() as i32) as usize,
                    quote_local_start: 0,
                    quote_local_end: quote.len(),
                    quote: quote.clone(),
                    require_current: false,
                },
            )
            .await
            .expect("historical citation with qa.history");
            assert_eq!(historical.version_id, published_version_id);
            assert!(!historical.is_current);
            let denied = resolve_citation(
                &pool,
                &no_history_ctx,
                &store,
                ResolveCitationRequest {
                    logical_document_id: document_id,
                    version_id: published_version_id,
                    source_content_sha256: source_sha.clone(),
                    canonical_markdown_sha256: markdown_sha.clone(),
                    chunk_id: chunk.id,
                    source_span_start: chunk.span_start.unwrap_or(0) as usize,
                    source_span_end: chunk.span_end.unwrap_or(quote.len() as i32) as usize,
                    quote_local_start: 0,
                    quote_local_end: quote.len(),
                    quote: quote.clone(),
                    require_current: false,
                },
            )
            .await
            .expect_err("historical citation without qa.history must fail");
            assert!(matches!(denied, CitationError::HistoryDenied));

            let revision_quote = revision_chunk.body.clone();
            let current = resolve_citation(
                &pool,
                &history_ctx,
                &store,
                ResolveCitationRequest {
                    logical_document_id: document_id,
                    version_id: revision_version_id,
                    source_content_sha256: revision_source_sha,
                    canonical_markdown_sha256: revision_markdown_sha,
                    chunk_id: revision_chunk.id,
                    source_span_start: revision_chunk.span_start.unwrap_or(0) as usize,
                    source_span_end: revision_chunk
                        .span_end
                        .unwrap_or(revision_quote.len() as i32)
                        as usize,
                    quote_local_start: 0,
                    quote_local_end: revision_quote.len(),
                    quote: revision_quote,
                    require_current: true,
                },
            )
            .await
            .expect("current revision citation");
            assert!(current.is_current);
            assert_eq!(current.version_id, revision_version_id);

            let (old_current, old_effective_to, source_parent, new_current, new_parent): (
                bool,
                Option<chrono::DateTime<chrono::Utc>>,
                Option<Uuid>,
                bool,
                Option<Uuid>,
            ) = with_org_txn(&pool, &history_ctx, {
                let ctx = history_ctx.clone();
                move |txn| {
                    Box::pin(async move {
                        let old = txn
                            .query_one(
                                "SELECT is_current, effective_to
                                 FROM document_versions
                                 WHERE org_id=$1 AND id=$2",
                                &[&ctx.org_id(), &published_version_id],
                            )
                            .await?;
                        let source = txn
                            .query_one(
                                "SELECT parent_version_id
                                 FROM document_versions
                                 WHERE org_id=$1 AND id=$2",
                                &[&ctx.org_id(), &revision_source_version_id],
                            )
                            .await?;
                        let new = txn
                            .query_one(
                                "SELECT is_current, parent_version_id
                                 FROM document_versions
                                 WHERE org_id=$1 AND id=$2",
                                &[&ctx.org_id(), &revision_version_id],
                            )
                            .await?;
                        Ok((
                            old.get(0),
                            old.get(1),
                            source.get(0),
                            new.get(0),
                            new.get(1),
                        ))
                    })
                }
            })
            .await
            .expect("load revision lineage");
            assert!(!old_current);
            assert!(old_effective_to.is_some());
            assert_eq!(source_parent, Some(published_version_id));
            assert!(new_current);
            assert_eq!(new_parent, Some(revision_source_version_id));

            let diff_response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(format!(
                            "/api/v1/documents/{document_id}/versions/{published_version_id}/diff?against={revision_version_id}"
                        ))
                        .header("authorization", format!("Bearer {token}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(diff_response.status(), StatusCode::OK);
            let diff: serde_json::Value = serde_json::from_slice(
                &diff_response
                    .into_body()
                    .collect()
                    .await
                    .unwrap()
                    .to_bytes(),
            )
            .unwrap();
            assert_eq!(diff["left"]["id"], published_version_id.to_string());
            assert_eq!(diff["right"]["id"], revision_version_id.to_string());
        }
        observed_formats.push(ext.to_string());
    }

    observed_formats.sort();
    assert_eq!(
        observed_formats, expected_formats,
        "vertical slice must cover every expected format"
    );
    // Machine-readable coverage line consumed by O04 release harness.
    eprintln!(
        "O04_FORMAT_COVERAGE\t{}",
        serde_json::to_string(&observed_formats).expect("format json")
    );

    qdrant_admin
        .delete_collection(&qdrant_collection)
        .await
        .expect("qdrant collection cleanup");
    cleanup.cleanup().await.expect("minio bucket cleanup");
    ephemeral.drop().await;
}

async fn drain_index_jobs(worker: &IndexWorker, ctx: &OrgContext) -> usize {
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

async fn drain_embedding_jobs(worker: &EmbeddingWorker, ctx: &OrgContext) -> usize {
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

    fn base_url(&self) -> &str {
        &self.base_url
    }
}

#[tokio::test]
async fn mock_embedding_reads_complete_batched_requests() {
    let mock = MockEmbedding::start();
    let runtime = ApprovedEmbeddingRuntime::new(
        mock.base_url().to_string(),
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
    .expect("embedding runtime");

    let vectors = runtime
        .embed(&["first".into(), "second".into()])
        .await
        .expect("batched mock response");
    assert_eq!(vectors.len(), 2);
    assert!(vectors.iter().all(|vector| vector.len() == 8));
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
