//! Live e2e cho luồng ADR 0016 (ignored — cần key OpenRouter thật):
//! HTTP upload PNG scan → ConvertWorker (deferred OCR trong sandbox →
//! vision OCR OpenRouter ở worker) → IndexWorker chunk → EmbeddingWorker
//! (OpenRouter `qwen/qwen3-embedding-8b`, MRL 1024, normalize client) →
//! Qdrant → hybrid `/api/v1/search` trả đúng document + citation anchor.
//!
//! Chạy:
//!   docker compose -f deploy/dev/compose.yml up -d postgres qdrant minio
//!   export MARKHAND_TEST_DATABASE_URL=... (xem .github/workflows/ci.yml)
//!   export MARKHAND_TEST_OPENROUTER_API_KEY=<openrouter key>
//!   cargo test -p fileconv-server --test live_openrouter_e2e -- --ignored
//!
//! Không có key/hạ tầng → soft-skip (trừ khi MARKHAND_TEST_REQUIRED=1).

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::worker_pipeline::{
    drain_embedding_jobs, drain_index_jobs, fileconv_binary, load_first_chunk,
    load_published_version, test_qdrant,
};
use common::{
    admin_database_url, app_database_url, boot_app_pool, build_app_state, login_access_token,
    seed_user_with_permissions, test_minio_client, tiny_png_ocr_bytes, MinioCleanupGuard,
};
use fileconv_knowledge::embedding::{EmbeddingPlan, ProviderDeployment, RUNTIME_PROVIDER_CLOUD};
use fileconv_server::auth::context::OrgContext;
use fileconv_server::config::Profile;
use fileconv_server::http::router;
use fileconv_server::jobs;
use fileconv_server::services::embedding::ApprovedEmbeddingRuntime;
use fileconv_server::services::indexing::IndexingOutboxSink;
use fileconv_server::services::vision_ocr::VisionOcrRuntime;
use fileconv_server::workers::convert::{ConvertWorker, ConvertWorkerConfig, ConvertWorkerRun};
use fileconv_server::workers::embedding::{EmbeddingWorker, EmbeddingWorkerConfig};
use fileconv_server::workers::index::{IndexWorker, IndexWorkerConfig};
use fileconv_server::workers::limits::ResourceLimits;
use fileconv_server::workers::sandbox::SandboxConfig;
use http_body_util::BodyExt;
use tower::ServiceExt;
use uuid::Uuid;

const BOUNDARY: &str = "----markhandOpenRouterE2eBoundary";
const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";
const EMBEDDING_MODEL: &str = "qwen/qwen3-embedding-8b";
const EMBEDDING_DIMENSIONS: usize = 1024;

/// Key OpenRouter là opt-in TÁCH khỏi `MARKHAND_TEST_REQUIRED` (CI chạy
/// `--include-ignored` không có key → test này soft-skip, không false-fail).
/// Muốn ép chạy: đặt `MARKHAND_TEST_OPENROUTER_REQUIRED=1`.
fn openrouter_key() -> Option<String> {
    let key = std::env::var("MARKHAND_TEST_OPENROUTER_API_KEY")
        .or_else(|_| std::env::var("MARKHAND_OCR_API_KEY"))
        .ok()
        .filter(|value| !value.trim().is_empty());
    if key.is_none() && std::env::var("MARKHAND_TEST_OPENROUTER_REQUIRED").is_ok_and(|v| v == "1") {
        panic!("MARKHAND_TEST_OPENROUTER_REQUIRED=1 nhưng thiếu MARKHAND_TEST_OPENROUTER_API_KEY");
    }
    key
}

fn multipart(filename: &str, content_type: &str, bytes: &[u8], collection_id: Uuid) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"collectionId\"\r\n\r\n{collection_id}\r\n"
        )
        .as_bytes(),
    );
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

/// PNG fixture → JPEG bytes (production artifact từ sandbox là JPEG).
fn fixture_jpeg(marker: &str) -> Vec<u8> {
    let png = tiny_png_ocr_bytes(marker);
    let decoded = image::load_from_memory(&png).expect("decode fixture png");
    let mut jpeg = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 90);
    decoded
        .to_rgb8()
        .write_with_encoder(encoder)
        .expect("encode jpeg");
    jpeg
}

/// Live check cho batch OCR nhiều trang/request (bench product owner):
/// 3 ảnh marker khác nhau trong MỘT request; model phải trả đủ 3 khối đúng
/// thứ tự theo contract `<!-- markhand:page k -->`.
#[tokio::test]
#[ignore = "requires real OpenRouter API key"]
async fn live_openrouter_batch_ocr_returns_pages_in_order() {
    let Some(api_key) = openrouter_key() else {
        eprintln!("skipped: MARKHAND_TEST_OPENROUTER_API_KEY not set");
        return;
    };
    let runtime = VisionOcrRuntime::new(
        "https://openrouter.ai/api".into(),
        api_key,
        fileconv_core::image_ocr::DEFAULT_VISION_OCR_MODEL.into(),
        fileconv_core::image_ocr::default_vision_ocr_system_prompt("vie+eng"),
        300,
    )
    .expect("vision runtime")
    .with_batch_pages(5);
    let markers = ["SOAK15", "OCR22", "TRANG33"];
    let jpegs: Vec<Vec<u8>> = markers.iter().map(|marker| fixture_jpeg(marker)).collect();
    let refs: Vec<&[u8]> = jpegs.iter().map(|bytes| bytes.as_slice()).collect();
    let started = std::time::Instant::now();
    let pages = runtime.ocr_jpeg_batch(&refs).await.expect("batch OCR");
    assert_eq!(pages.len(), markers.len());
    for (page, marker) in pages.iter().zip(markers) {
        assert!(
            page.to_ascii_uppercase().contains(marker),
            "trang phải chứa marker {marker}: {page:?}"
        );
    }
    eprintln!(
        "LIVE BATCH OCR OK: {} trang / 1 request trong {:?}",
        markers.len(),
        started.elapsed()
    );
}

#[tokio::test]
#[ignore = "requires live DB/MinIO/Qdrant + real OpenRouter API key + built fileconv"]
async fn live_openrouter_upload_ocr_embed_search_e2e() {
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
        return;
    };
    let Some(api_key) = openrouter_key() else {
        eprintln!("skipped: MARKHAND_TEST_OPENROUTER_API_KEY not set");
        return;
    };
    let fileconv = fileconv_binary()
        .expect("target/debug/fileconv missing — build fileconv-cli for the OCR path");
    let cleanup = MinioCleanupGuard::new(store.clone());
    store.ensure_bucket().await.expect("bucket");

    let (ephemeral, pool) = boot_app_pool(&admin, &app_url).await;
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    seed_user_with_permissions(
        &pool,
        org,
        user,
        &format!("{user}@openrouter-e2e.test"),
        "correct-password-1",
        &["qa.query", "doc.upload", "doc.publish", "jobs.system"],
    )
    .await;
    let token = login_access_token(
        &pool,
        &format!("{user}@openrouter-e2e.test"),
        "correct-password-1",
    )
    .await;

    // Embedding runtime: OpenRouter provider-cloud (egress opt-in, ADR 0016);
    // MRL 1024 + normalize client — đúng cấu hình worker.env.example.
    let embedder = ApprovedEmbeddingRuntime::new(
        OPENROUTER_BASE_URL.into(),
        api_key.clone(),
        "openrouter".into(),
        EMBEDDING_MODEL.into(),
        "e2e-live".into(),
        EMBEDDING_DIMENSIONS,
        RUNTIME_PROVIDER_CLOUD.into(),
        Profile::Dev,
        true,
        None,
    )
    .expect("openrouter embedding runtime")
    .with_client_normalization(true)
    .with_request_dimensions(true);
    let embedding_plan = EmbeddingPlan::provider(
        "openrouter",
        EMBEDDING_MODEL,
        "e2e-live",
        ProviderDeployment::from_base_url(Some(OPENROUTER_BASE_URL)).expect("deployment"),
        Some(EMBEDDING_DIMENSIONS),
        RUNTIME_PROVIDER_CLOUD,
    )
    .expect("plan");

    let app = router(
        build_app_state(pool.clone(), &ephemeral.app_url, Some(store.clone()))
            .with_retrieval_backends(qdrant.clone(), Some(embedder.clone())),
    );

    let (status, created) = json_post(
        app.clone(),
        "/api/v1/collections",
        &token,
        serde_json::json!({
            "name": "OpenRouter E2E",
            "slug": format!("openrouter-e2e-{}", Uuid::new_v4().simple()),
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

    // 1) Upload ảnh scan qua HTTP API thật.
    let source = tiny_png_ocr_bytes("SOAK15");
    let upload_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/uploads")
                .header("authorization", format!("Bearer {token}"))
                .header(
                    "idempotency-key",
                    format!("openrouter-e2e-{}", Uuid::new_v4().simple()),
                )
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={BOUNDARY}"),
                )
                .body(Body::from(multipart(
                    "scan.png",
                    "image/png",
                    &source,
                    collection_id,
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(upload_response.status(), StatusCode::CREATED, "upload");
    let upload: serde_json::Value = serde_json::from_slice(
        &upload_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes(),
    )
    .unwrap();
    assert_eq!(upload["disposition"], "accepted");
    let document_id = Uuid::parse_str(upload["documentId"].as_str().unwrap()).unwrap();
    let convert_job_id = Uuid::parse_str(upload["jobId"].as_str().unwrap()).unwrap();

    // 2) ConvertWorker production: sandbox render JPEG (deferred) → worker gọi
    //    OpenRouter Qwen3.7 Flash thay placeholder.
    let mut convert_config = ConvertWorkerConfig::new(
        format!("openrouter-e2e-convert-{}", Uuid::new_v4()),
        SandboxConfig {
            argv_template: vec![
                fileconv.display().to_string(),
                "one".into(),
                "{input}".into(),
                "--ocr-defer-dir".into(),
                ".".into(),
            ],
            limits: ResourceLimits {
                wall_timeout: Duration::from_secs(60),
                ..ResourceLimits::default()
            },
        },
    );
    convert_config.heartbeat_interval = Duration::from_secs(5);
    convert_config.lease_ttl = Duration::from_secs(120);
    convert_config.max_job_duration = Duration::from_secs(300);
    convert_config.vision_ocr = Some(Arc::new(
        VisionOcrRuntime::new(
            "https://openrouter.ai/api".into(),
            api_key.clone(),
            fileconv_core::image_ocr::DEFAULT_VISION_OCR_MODEL.into(),
            fileconv_core::image_ocr::default_vision_ocr_system_prompt("vie+eng"),
            180,
        )
        .expect("vision ocr runtime"),
    ));
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
        "OCR qua OpenRouter phải hoàn tất convert: {convert_run:?}"
    );

    // 3) Index + embedding thật qua OpenRouter.
    let sink = Arc::new(IndexingOutboxSink::new(&embedding_plan).expect("sink"));
    let mut index_config =
        IndexWorkerConfig::new(format!("openrouter-e2e-index-{}", Uuid::new_v4()));
    index_config.lease_ttl = Duration::from_secs(60);
    index_config.heartbeat_interval = Duration::from_secs(5);
    index_config.max_job_duration = Duration::from_secs(120);
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
        EmbeddingWorkerConfig::new(format!("openrouter-e2e-embed-{}", Uuid::new_v4()));
    embedding_config.lease_ttl = Duration::from_secs(60);
    embedding_config.heartbeat_interval = Duration::from_secs(5);
    embedding_config.max_job_duration = Duration::from_secs(120);
    let embedding_worker = EmbeddingWorker::new(
        pool.clone(),
        qdrant.clone(),
        embedding_config,
        embedder.clone(),
    )
    .expect("embedding worker");

    jobs::relay_outbox_with_sink(&pool, &worker_ctx, 32, &sink)
        .await
        .expect("relay outbox");
    assert!(drain_index_jobs(&index_worker, &worker_ctx).await > 0);
    assert!(drain_embedding_jobs(&embedding_worker, &worker_ctx).await > 0);

    // 4) Nội dung OCR phải nằm trong chunk đã publish.
    let (published_version_id, _, _) =
        load_published_version(&pool, &worker_ctx, document_id).await;
    let chunk = load_first_chunk(&pool, &worker_ctx, document_id, published_version_id).await;
    assert!(
        chunk.body.to_ascii_uppercase().contains("SOAK15"),
        "OCR text phải vào chunk: {:?}",
        chunk.body
    );

    // 5) Hybrid search (FTS + vector OpenRouter cho query) trả đúng document.
    let (status, search) = json_post(
        app.clone(),
        "/api/v1/search",
        &token,
        serde_json::json!({
            "query": "SOAK15",
            "mode": "current",
            "limit": 5
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "search: {search}");
    let hits = search["hits"].as_array().expect("hits array");
    assert!(!hits.is_empty(), "search phải có kết quả: {search}");
    let hit = &hits[0];
    assert_eq!(
        hit["documentId"].as_str().unwrap(),
        document_id.to_string(),
        "top hit phải là tài liệu vừa upload: {search}"
    );
    assert!(
        hit["vectorScore"].is_number(),
        "dense leg (OpenRouter query embedding) phải tham gia: {hit}"
    );
    assert!(
        hit["isCurrent"].as_bool().unwrap_or(false),
        "citation phải trỏ version hiện hành: {hit}"
    );
    assert_eq!(
        search["embeddingMode"].as_str().unwrap_or_default(),
        RUNTIME_PROVIDER_CLOUD,
        "dense search phải chạy bằng provider-cloud embedding: {search}"
    );

    // 6) Q&A: không có chat provider → extractive fail-closed, vẫn phải trả
    //    answer + citation trỏ đúng tài liệu (grounded, không bịa).
    let (status, ask) = json_post(
        app.clone(),
        "/api/v1/ask",
        &token,
        serde_json::json!({
            "question": "SOAK15",
            "mode": "current",
            "limit": 5
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "ask: {ask}");
    let citations = ask["citations"].as_array().expect("citations array");
    assert!(!citations.is_empty(), "ask phải có citation: {ask}");
    let cited_document = citations[0]["logicalDocumentId"]
        .as_str()
        .or_else(|| citations[0]["logical_document_id"].as_str())
        .unwrap_or_default();
    assert_eq!(
        cited_document,
        document_id.to_string(),
        "citation phải trỏ tài liệu OCR: {ask}"
    );
    assert!(
        ask["answer"]
            .as_str()
            .unwrap_or_default()
            .to_ascii_uppercase()
            .contains("SOAK15"),
        "extractive answer phải chứa nội dung OCR: {ask}"
    );

    drop(cleanup);
    eprintln!(
        "LIVE E2E OK: upload → deferred OCR (OpenRouter) → chunk → embed ({EMBEDDING_MODEL}, \
         {EMBEDDING_DIMENSIONS}-d MRL) → Qdrant → hybrid search + ask/citation đúng document"
    );
}
