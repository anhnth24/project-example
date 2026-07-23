//! Upload intake integration tests (P1B-I01).
//!
//! Adversarial / unit-style validation runs without MinIO. Persistence paths are
//! gated on `MARKHAND_TEST_MINIO_*` and skip cleanly when unset. Auth-backed
//! HTTP tests also need `MARKHAND_TEST_DATABASE_URL`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use bytes::Bytes;
use deadpool_postgres::Pool;
use fileconv_server::auth::context::OrgContext;
use fileconv_server::auth::jwt::JwtKeys;
use fileconv_server::auth::provider::{AuthProvider, AuthRequestMeta, PasswordAuthProvider};
use fileconv_server::auth::session;
use fileconv_server::config::{
    Argon2Config, AuthConfig, JwtAlgorithm, MinioConfig, RuntimeEndpoints, SecretString,
    ServerConfig,
};
use fileconv_server::database::apply_migrations;
use fileconv_server::db::orgs;
use fileconv_server::db::pool::{create_pool, with_org_txn};
use fileconv_server::http::{router, AppState};
#[cfg(feature = "test-hooks")]
use fileconv_server::services::upload::{
    acquire_hook_test_guard, arm_pause_after_reconcile_claim, arm_pause_before_approve_commit,
    arm_pause_before_commit, arm_saga_fault, reconcile_stale_uploads, resume_after_reconcile_claim,
    resume_before_approve_commit, resume_before_commit, SagaFaultBarrier,
};
use fileconv_server::services::upload::{
    approve_quarantined_upload, assert_disposition_is_typed, detect_magic, quota_reserve_hook,
    reject_dangerous_entry_name, resolve_canonical_format, run_upload_saga, stream_to_tempfile,
    validate_and_quarantine, validate_streamed_bytes, validate_zip_archive, ApproveIntakeRequest,
    CanonicalFormat, Disposition, LimitsConfig, QuarantineIdentity, ReasonCode, SagaInput,
    ThreatClass, UploadError, PERMISSION_QUARANTINE_REVIEW,
};
use fileconv_server::state::RuntimeState;
use fileconv_server::storage::keys::{parse_key_for_org, quarantine_key, trusted_key};
use fileconv_server::storage::minio::{MinioClient, ObjectIdentityMeta, ObjectPutVerification};
use futures::stream;
use http_body_util::BodyExt;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tower::ServiceExt;
use uuid::Uuid;
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;

struct TrackingAllocator;

#[global_allocator]
static GLOBAL_ALLOCATOR: TrackingAllocator = TrackingAllocator;

static TRACK_ALLOCATIONS: AtomicBool = AtomicBool::new(false);
static CURRENT_ALLOCATED: AtomicUsize = AtomicUsize::new(0);
static PEAK_ALLOCATED: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            let current =
                CURRENT_ALLOCATED.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            record_peak(current);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        CURRENT_ALLOCATED.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) };
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let ptr = unsafe { System.realloc(ptr, layout, new_size) };
        if !ptr.is_null() {
            let old_size = layout.size();
            let current = if new_size >= old_size {
                CURRENT_ALLOCATED.fetch_add(new_size - old_size, Ordering::Relaxed)
                    + (new_size - old_size)
            } else {
                CURRENT_ALLOCATED.fetch_sub(old_size - new_size, Ordering::Relaxed)
                    - (old_size - new_size)
            };
            record_peak(current);
        }
        ptr
    }
}

fn record_peak(current: usize) {
    if !TRACK_ALLOCATIONS.load(Ordering::Relaxed) {
        return;
    }
    let mut peak = PEAK_ALLOCATED.load(Ordering::Relaxed);
    while current > peak {
        match PEAK_ALLOCATED.compare_exchange_weak(
            peak,
            current,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(next) => peak = next,
        }
    }
}

const DOCX_CONTENT_TYPES_XML: &[u8] = br#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#;

fn adversarial_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../bench/markhand_web/adversarial/files")
}

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../bench/markhand_web/golden/documents")
}

fn test_minio_client() -> Option<(MinioClient, String)> {
    let endpoint = match std::env::var("MARKHAND_TEST_MINIO_ENDPOINT") {
        Ok(url) if !url.trim().is_empty() => url,
        _ => {
            eprintln!("skipped: MARKHAND_TEST_MINIO_ENDPOINT unset");
            return None;
        }
    };
    let access_key = std::env::var("MARKHAND_TEST_MINIO_ACCESS_KEY").ok()?;
    let secret_key = std::env::var("MARKHAND_TEST_MINIO_SECRET_KEY").ok()?;
    if access_key.is_empty() || secret_key.is_empty() {
        eprintln!("skipped: MinIO test credentials empty");
        return None;
    }
    let region = std::env::var("MARKHAND_TEST_MINIO_REGION").unwrap_or_else(|_| "us-east-1".into());
    let bucket = format!("markhand-upload-{}", Uuid::new_v4().simple());
    std::env::set_var("RUST_S3_SKIP_LOCATION_CONSTRAINT", "true");
    let config = MinioConfig::new(
        endpoint,
        SecretString::new(access_key),
        SecretString::new(secret_key),
        bucket.clone(),
        region,
        true,
    )
    .expect("minio config");
    let client = MinioClient::from_config(&config).expect("client");
    Some((client, bucket))
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
    let (client, connection) = tokio_postgres::connect(database_url, tokio_postgres::NoTls)
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
        let db_name = format!("markhand_upload_{}", Uuid::new_v4().simple());
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

fn test_auth_config() -> AuthConfig {
    AuthConfig {
        issuer: Some("https://issuer.markhand.test".into()),
        audience: Some("markhand-api".into()),
        signing_key: Some(SecretString::new("integration-test-signing-key-32b!")),
        alg: JwtAlgorithm::Hs256,
        kid: Some("test-kid-1".into()),
        access_token_ttl_secs: 900,
        refresh_token_ttl_secs: 3_600,
        argon2: Argon2Config {
            memory_kib: 8_192,
            time_cost: 1,
            parallelism: 1,
        },
    }
}

async fn seed_uploader(pool: &Pool, org: Uuid, user: Uuid, email: &str, password: &str) -> Uuid {
    let collection_id = Uuid::new_v4();
    let ctx = OrgContext::try_new(org, user, ["doc.upload"], [collection_id]).unwrap();
    let email = email.to_string();
    with_org_txn(pool, &ctx, {
        let owned = ctx.clone();
        move |txn| {
            Box::pin(async move {
                orgs::ensure_exists(
                    txn,
                    &owned,
                    &format!("uploadorg-{}", owned.org_id().simple()),
                    "Upload Org",
                )
                .await?;
                orgs::ensure_user(txn, &owned, user, &email, "Uploader").await?;
                orgs::ensure_membership(txn, &owned).await?;
                txn.execute(
                    "INSERT INTO org_quotas (
                        org_id, max_storage_bytes, max_documents,
                        max_concurrent_jobs, max_monthly_tokens
                     )
                     VALUES ($1, 1073741824, 1000, 10, 1000000)
                     ON CONFLICT (org_id) DO NOTHING",
                    &[&org],
                )
                .await?;
                txn.execute(
                    "INSERT INTO permissions (id, code, description)
                     VALUES ($1, 'doc.upload', 'Upload')
                     ON CONFLICT (code) DO NOTHING",
                    &[&Uuid::new_v4()],
                )
                .await?;
                let role_id = Uuid::new_v4();
                txn.execute(
                    "INSERT INTO roles (id, org_id, code, name, is_system)
                     VALUES ($1, $2, 'owner', 'Owner', true)
                     ON CONFLICT (org_id, code) DO NOTHING",
                    &[&role_id, &org],
                )
                .await?;
                let role_id: Uuid = txn
                    .query_one(
                        "SELECT id FROM roles WHERE org_id = $1 AND code = 'owner'",
                        &[&org],
                    )
                    .await?
                    .get(0);
                let perm_id: Uuid = txn
                    .query_one("SELECT id FROM permissions WHERE code = 'doc.upload'", &[])
                    .await?
                    .get(0);
                txn.execute(
                    "INSERT INTO role_permissions (org_id, role_id, permission_id)
                     VALUES ($1, $2, $3)
                     ON CONFLICT DO NOTHING",
                    &[&org, &role_id, &perm_id],
                )
                .await?;
                txn.execute(
                    "INSERT INTO collections (
                        id, org_id, name, slug, visibility, owner_user_id
                     ) VALUES ($1, $2, 'Upload Collection', $3, 'org', $4)",
                    &[
                        &collection_id,
                        &org,
                        &format!("upload-{}", collection_id.simple()),
                        &user,
                    ],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("seed org");

    session::set_password_hash(pool, user, password, &test_auth_config().argon2)
        .await
        .expect("set password");
    collection_id
}

async fn stream_file(path: &Path) -> fileconv_server::services::upload::StreamedUpload {
    let data = std::fs::read(path).expect("read fixture");
    let limits = LimitsConfig::policy_defaults();
    stream_to_tempfile(
        stream::iter(vec![Ok::<_, std::io::Error>(Bytes::from(data))]),
        &limits,
    )
    .await
    .expect("stream")
}

fn upload_operation_key(org: Uuid, user: Uuid, idempotency_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(org.as_bytes());
    hasher.update(user.as_bytes());
    hasher.update(format!("client:{idempotency_key}").as_bytes());
    format!("op.{}", hex::encode(hasher.finalize()))
}

async fn quota_reservation_status(
    pool: &Pool,
    ctx: &OrgContext,
    reservation_key: &str,
) -> Option<String> {
    let reservation_key = reservation_key.to_string();
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_opt(
                        "SELECT status FROM quota_reservations
                         WHERE org_id = $1 AND reservation_key = $2",
                        &[&ctx.org_id(), &reservation_key],
                    )
                    .await?;
                Ok(row.map(|row| row.get::<_, String>(0)))
            })
        }
    })
    .await
    .expect("quota status")
}

async fn quota_counter_value(pool: &Pool, ctx: &OrgContext, counter_key: &str) -> i64 {
    let counter_key = counter_key.to_string();
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT COALESCE(SUM(value), 0)::bigint
                         FROM usage_counters
                         WHERE org_id = $1 AND counter_key = $2",
                        &[&ctx.org_id(), &counter_key],
                    )
                    .await?;
                Ok(row.get::<_, i64>(0))
            })
        }
    })
    .await
    .expect("quota counter")
}

fn write_docx_zip(path: &Path, entries: &[(&str, &[u8])], compression: CompressionMethod) {
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(compression);
    zip.start_file("[Content_Types].xml", options).unwrap();
    zip.write_all(DOCX_CONTENT_TYPES_XML).unwrap();
    zip.start_file("word/document.xml", options).unwrap();
    zip.write_all(br#"<?xml version="1.0"?><w:document></w:document>"#)
        .unwrap();
    for (name, data) in entries {
        zip.start_file(*name, options).unwrap();
        zip.write_all(data).unwrap();
    }
    zip.finish().unwrap();
}

fn write_xlsb_zip(path: &Path) {
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    zip.start_file("[Content_Types].xml", options).unwrap();
    zip.write_all(br#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/xl/workbook.bin" ContentType="application/vnd.ms-excel.sheet.binary.macroEnabled.main"/></Types>"#)
        .unwrap();
    zip.start_file("xl/workbook.bin", options).unwrap();
    zip.write_all(b"binary workbook placeholder").unwrap();
    zip.finish().unwrap();
}

fn write_manual_stored_zip(path: &Path, entries: &[(&str, &[u8])]) {
    let mut bytes = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        let offset = bytes.len() as u32;
        let crc = crc32fast::hash(data);
        let name_bytes = name.as_bytes();
        bytes.extend_from_slice(b"PK\x03\x04");
        bytes.extend_from_slice(&20_u16.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.extend_from_slice(&crc.to_le_bytes());
        bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.extend_from_slice(name_bytes);
        bytes.extend_from_slice(data);

        central.extend_from_slice(b"PK\x01\x02");
        central.extend_from_slice(&20_u16.to_le_bytes());
        central.extend_from_slice(&20_u16.to_le_bytes());
        central.extend_from_slice(&0_u16.to_le_bytes());
        central.extend_from_slice(&0_u16.to_le_bytes());
        central.extend_from_slice(&0_u16.to_le_bytes());
        central.extend_from_slice(&0_u16.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        central.extend_from_slice(&0_u16.to_le_bytes());
        central.extend_from_slice(&0_u16.to_le_bytes());
        central.extend_from_slice(&0_u16.to_le_bytes());
        central.extend_from_slice(&0_u16.to_le_bytes());
        central.extend_from_slice(&0_u32.to_le_bytes());
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name_bytes);
    }
    let central_offset = bytes.len() as u32;
    let central_size = central.len() as u32;
    bytes.extend_from_slice(&central);
    bytes.extend_from_slice(b"PK\x05\x06");
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    bytes.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    bytes.extend_from_slice(&central_size.to_le_bytes());
    bytes.extend_from_slice(&central_offset.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    std::fs::write(path, bytes).unwrap();
}

#[tokio::test]
async fn spoof_pdf_and_html_pdf_reject() {
    let limits = LimitsConfig::policy_defaults();
    for name in ["plain-text.pdf", "actually-html.pdf"] {
        let path = adversarial_dir().join(name);
        let streamed = stream_file(&path).await;
        let err = validate_streamed_bytes(
            &streamed,
            Some(name),
            Some("application/octet-stream"),
            &limits,
        )
        .unwrap_err();
        assert_eq!(err.threat_class(), Some(ThreatClass::ExtensionSpoof));
        assert_eq!(err.reason_code(), ReasonCode::ExtensionMagicMismatch);
    }
}

#[tokio::test]
async fn malformed_and_traversal_docx_reject() {
    let limits = LimitsConfig::policy_defaults();
    let malformed = stream_file(&adversarial_dir().join("malformed.docx")).await;
    let err = validate_streamed_bytes(
        &malformed,
        Some("malformed.docx"),
        Some("application/octet-stream"),
        &limits,
    )
    .unwrap_err();
    assert!(matches!(
        err.threat_class(),
        Some(ThreatClass::MalformedOoxml)
    ));

    let traversal = stream_file(&adversarial_dir().join("traversal.docx")).await;
    let err = validate_streamed_bytes(
        &traversal,
        Some("traversal.docx"),
        Some("application/octet-stream"),
        &limits,
    )
    .unwrap_err();
    assert_eq!(err.threat_class(), Some(ThreatClass::ArchiveTraversal));
}

#[tokio::test]
async fn zip_bomb_rejects_without_unbounded_decompress() {
    let limits = LimitsConfig::policy_defaults();
    let streamed = stream_file(&adversarial_dir().join("compressed-bomb.docx")).await;
    // Memory bound: we only hold the small on-disk fixture + CD metadata.
    assert!(streamed.size_bytes < 64 * 1024);
    let err = validate_streamed_bytes(
        &streamed,
        Some("compressed-bomb.docx"),
        Some("application/octet-stream"),
        &limits,
    )
    .unwrap_err();
    assert_eq!(err.threat_class(), Some(ThreatClass::ArchiveBomb));
    assert_eq!(err.reason_code(), ReasonCode::ArchiveCompressionRatio);
}

#[tokio::test]
async fn large_lazy_stream_keeps_memory_bounded() {
    let limits = LimitsConfig::policy_defaults();
    let base = CURRENT_ALLOCATED.load(Ordering::Relaxed);
    PEAK_ALLOCATED.store(base, Ordering::Relaxed);
    TRACK_ALLOCATIONS.store(true, Ordering::Relaxed);
    let chunks = (0..1024).map(|_| Ok::<_, std::io::Error>(Bytes::from(vec![b'a'; 64 * 1024])));
    let streamed = stream_to_tempfile(stream::iter(chunks), &limits)
        .await
        .expect("stream large lazy body");
    TRACK_ALLOCATIONS.store(false, Ordering::Relaxed);
    let peak_delta = PEAK_ALLOCATED.load(Ordering::Relaxed).saturating_sub(base);
    assert_eq!(streamed.size_bytes, 64 * 1024 * 1024);
    assert!(streamed.head.len() <= 512);
    assert!(
        peak_delta < 32 * 1024 * 1024,
        "peak allocation grew by {peak_delta} bytes"
    );
}

#[tokio::test]
async fn hidden_nested_polyglot_duplicate_and_symlink_reject() {
    let limits = LimitsConfig::policy_defaults();
    let dir = tempfile::tempdir().unwrap();

    let nested = dir.path().join("nested.docx");
    write_docx_zip(
        &nested,
        &[("word/media/blob.bin", b"PK\x03\x04nested")],
        CompressionMethod::Stored,
    );
    let err = validate_zip_archive(&nested, CanonicalFormat::Docx, &limits).unwrap_err();
    assert_eq!(err.threat_class(), Some(ThreatClass::NestedArchive));

    let polyglot = dir.path().join("polyglot.docx");
    let file = std::fs::File::create(&polyglot).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    zip.start_file("[Content_Types].xml", options).unwrap();
    zip.write_all(DOCX_CONTENT_TYPES_XML).unwrap();
    zip.start_file("word/document.xml", options).unwrap();
    zip.write_all(br#"<?xml version="1.0"?><w:document></w:document>"#)
        .unwrap();
    zip.start_file("ppt/presentation.xml", options).unwrap();
    zip.write_all(br#"<?xml version="1.0"?><p:presentation></p:presentation>"#)
        .unwrap();
    zip.finish().unwrap();
    let err = validate_zip_archive(&polyglot, CanonicalFormat::Docx, &limits).unwrap_err();
    assert_eq!(err.threat_class(), Some(ThreatClass::MalformedOoxml));

    let ods_polyglot = dir.path().join("ods-polyglot.ods");
    let file = std::fs::File::create(&ods_polyglot).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file("mimetype", options).unwrap();
    zip.write_all(b"application/vnd.oasis.opendocument.spreadsheet")
        .unwrap();
    zip.start_file("[Content_Types].xml", options).unwrap();
    zip.write_all(DOCX_CONTENT_TYPES_XML).unwrap();
    zip.start_file("word/document.xml", options).unwrap();
    zip.write_all(br#"<?xml version="1.0"?><w:document></w:document>"#)
        .unwrap();
    zip.finish().unwrap();
    let err = validate_zip_archive(&ods_polyglot, CanonicalFormat::Ods, &limits).unwrap_err();
    assert_eq!(err.threat_class(), Some(ThreatClass::MalformedOoxml));

    let duplicate = dir.path().join("duplicate.docx");
    write_manual_stored_zip(
        &duplicate,
        &[
            ("[Content_Types].xml", DOCX_CONTENT_TYPES_XML),
            (
                "word/document.xml",
                br#"<?xml version="1.0"?><w:document></w:document>"#,
            ),
            (
                "word/document.xml",
                br#"<?xml version="1.0"?><w:document></w:document>"#,
            ),
        ],
    );
    let err = validate_zip_archive(&duplicate, CanonicalFormat::Docx, &limits).unwrap_err();
    assert_eq!(err.threat_class(), Some(ThreatClass::MalformedOoxml));

    let symlink = dir.path().join("symlink.docx");
    let file = std::fs::File::create(&symlink).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file("[Content_Types].xml", options).unwrap();
    zip.write_all(DOCX_CONTENT_TYPES_XML).unwrap();
    zip.start_file("word/document.xml", options).unwrap();
    zip.write_all(br#"<?xml version="1.0"?><w:document></w:document>"#)
        .unwrap();
    let symlink_options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .unix_permissions(0o120777);
    zip.start_file("word/media/link", symlink_options).unwrap();
    zip.write_all(b"target").unwrap();
    zip.finish().unwrap();
    let mut bytes = std::fs::read(&symlink).unwrap();
    let mut pos = 0;
    while let Some(offset) = bytes[pos..].windows(4).position(|w| w == b"PK\x01\x02") {
        let start = pos + offset;
        let name_len =
            u16::from_le_bytes(bytes[start + 28..start + 30].try_into().unwrap()) as usize;
        let name = &bytes[start + 46..start + 46 + name_len];
        if name == b"word/media/link" {
            bytes[start + 5] = 3;
            bytes[start + 38..start + 42].copy_from_slice(&((0o120777_u32) << 16).to_le_bytes());
            break;
        }
        pos = start + 4;
    }
    std::fs::write(&symlink, bytes).unwrap();
    let err = validate_zip_archive(&symlink, CanonicalFormat::Docx, &limits).unwrap_err();
    assert_eq!(err.threat_class(), Some(ThreatClass::ArchiveTraversal));
}

#[tokio::test]
async fn valid_zip_based_xlsb_is_accepted() {
    let limits = LimitsConfig::policy_defaults();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("book.xlsb");
    write_xlsb_zip(&path);
    let streamed = stream_file(&path).await;
    let (format, disposition, _, _) = validate_streamed_bytes(
        &streamed,
        Some("book.xlsb"),
        Some("application/vnd.ms-excel.sheet.binary.macroEnabled.12"),
        &limits,
    )
    .unwrap();
    assert_eq!(format, CanonicalFormat::Xlsb);
    assert_eq!(disposition, Disposition::Accepted);
}

#[tokio::test]
async fn forged_central_directory_size_rejects_during_inflation() {
    let limits = LimitsConfig::policy_defaults();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("forged.docx");
    let payload = [b'x'; 128];
    write_docx_zip(
        &path,
        &[("word/media/payload.bin", payload.as_slice())],
        CompressionMethod::Stored,
    );
    let mut bytes = std::fs::read(&path).unwrap();
    let mut pos = 0;
    while let Some(offset) = bytes[pos..].windows(4).position(|w| w == b"PK\x01\x02") {
        let start = pos + offset;
        let name_len =
            u16::from_le_bytes(bytes[start + 28..start + 30].try_into().unwrap()) as usize;
        let name = &bytes[start + 46..start + 46 + name_len];
        if name == b"word/media/payload.bin" {
            bytes[start + 24..start + 28].copy_from_slice(&1_u32.to_le_bytes());
            break;
        }
        pos = start + 4;
    }
    std::fs::write(&path, bytes).unwrap();
    let err = validate_zip_archive(&path, CanonicalFormat::Docx, &limits).unwrap_err();
    assert!(matches!(
        err.threat_class(),
        Some(ThreatClass::MalformedOoxml) | Some(ThreatClass::ArchiveBomb)
    ));
}

#[tokio::test]
async fn unparseable_compressed_span_rejects_closed() {
    let limits = LimitsConfig::policy_defaults();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad-span.docx");
    write_docx_zip(&path, &[], CompressionMethod::Stored);
    let mut bytes = std::fs::read(&path).unwrap();
    let first_local = bytes
        .windows(4)
        .position(|window| window == b"PK\x03\x04")
        .expect("local header");
    bytes[first_local..first_local + 4].copy_from_slice(b"PX\x03\x04");
    std::fs::write(&path, bytes).unwrap();

    let err = validate_zip_archive(&path, CanonicalFormat::Docx, &limits).unwrap_err();
    assert_eq!(err.threat_class(), Some(ThreatClass::MalformedOoxml));
    assert_eq!(err.reason_code(), ReasonCode::MalformedArchive);
}

#[tokio::test]
async fn declared_entry_count_rejects_before_name_allocation() {
    let limits = LimitsConfig {
        max_archive_entries: 4,
        ..LimitsConfig::policy_defaults()
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("declared-too-many.docx");
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"PK\x05\x06");
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&5_u16.to_le_bytes());
    bytes.extend_from_slice(&5_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    std::fs::write(&path, bytes).unwrap();

    let base = CURRENT_ALLOCATED.load(Ordering::Relaxed);
    PEAK_ALLOCATED.store(base, Ordering::Relaxed);
    TRACK_ALLOCATIONS.store(true, Ordering::Relaxed);
    let err = validate_zip_archive(&path, CanonicalFormat::Docx, &limits).unwrap_err();
    TRACK_ALLOCATIONS.store(false, Ordering::Relaxed);
    let peak_delta = PEAK_ALLOCATED.load(Ordering::Relaxed).saturating_sub(base);
    assert_eq!(err.threat_class(), Some(ThreatClass::ArchiveBomb));
    assert_eq!(err.reason_code(), ReasonCode::ArchiveEntryLimit);
    assert!(
        peak_delta < 8 * 1024 * 1024,
        "declared entry count rejection allocated {peak_delta} bytes"
    );
}

#[tokio::test]
async fn entry_count_bomb_rejects() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("many.docx");
    let file = std::fs::File::create(&path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    zip.start_file("[Content_Types].xml", options).unwrap();
    zip.write_all(DOCX_CONTENT_TYPES_XML).unwrap();
    zip.start_file("word/document.xml", options).unwrap();
    zip.write_all(br#"<?xml version="1.0"?><w:document></w:document>"#)
        .unwrap();
    for i in 0..5_000 {
        zip.start_file(format!("pad/{i}.bin"), options).unwrap();
        zip.write_all(b"x").unwrap();
    }
    zip.finish().unwrap();

    let err = validate_zip_archive(
        &path,
        CanonicalFormat::Docx,
        &LimitsConfig::policy_defaults(),
    )
    .unwrap_err();
    assert_eq!(err.reason_code(), ReasonCode::ArchiveEntryLimit);
}

#[tokio::test]
async fn oversize_stream_rejects_early() {
    let limits = LimitsConfig {
        max_upload_bytes: 2_048,
        ..LimitsConfig::policy_defaults()
    };
    let chunks = (0..40).map(|_| Ok::<_, std::io::Error>(Bytes::from(vec![0_u8; 128])));
    let err = stream_to_tempfile(stream::iter(chunks), &limits)
        .await
        .unwrap_err();
    assert_eq!(err.threat_class(), Some(ThreatClass::Oversize));
    assert_eq!(err.status_code(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn mime_mismatch_and_malformed_audio_reject() {
    let limits = LimitsConfig::policy_defaults();
    let pdf = stream_to_tempfile(
        stream::iter(vec![Ok::<_, std::io::Error>(Bytes::from_static(
            b"%PDF-1.4\n1 0 obj<</Type/Catalog>>endobj\nxref\n0 1\n0000000000 65535 f \ntrailer<</Root 1 0 R>>\nstartxref\n42\n%%EOF\n",
        ))]),
        &limits,
    )
    .await
    .unwrap();
    let err =
        validate_streamed_bytes(&pdf, Some("ok.pdf"), Some("text/plain"), &limits).unwrap_err();
    assert_eq!(err.threat_class(), Some(ThreatClass::MimeMismatch));

    let bad_mp3 = stream_to_tempfile(
        stream::iter(vec![Ok::<_, std::io::Error>(Bytes::from_static(b"ID3bad"))]),
        &limits,
    )
    .await
    .unwrap();
    let err = validate_streamed_bytes(&bad_mp3, Some("bad.mp3"), Some("audio/mpeg"), &limits)
        .unwrap_err();
    assert_eq!(err.threat_class(), Some(ThreatClass::ParserCorruption));

    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&36_u32.to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&16_000_u32.to_le_bytes());
    wav.extend_from_slice(&0_u32.to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&4_u32.to_le_bytes());
    wav.extend_from_slice(&[0_u8; 4]);
    let zero_rate = stream_to_tempfile(
        stream::iter(vec![Ok::<_, std::io::Error>(Bytes::from(wav))]),
        &limits,
    )
    .await
    .unwrap();
    let err = validate_streamed_bytes(&zero_rate, Some("zero.wav"), Some("audio/wav"), &limits)
        .unwrap_err();
    assert_eq!(err.threat_class(), Some(ThreatClass::ParserCorruption));

    let mut truncated_wav = Vec::new();
    truncated_wav.extend_from_slice(b"RIFF");
    truncated_wav.extend_from_slice(&140_u32.to_le_bytes());
    truncated_wav.extend_from_slice(b"WAVEfmt ");
    truncated_wav.extend_from_slice(&16_u32.to_le_bytes());
    truncated_wav.extend_from_slice(&1_u16.to_le_bytes());
    truncated_wav.extend_from_slice(&1_u16.to_le_bytes());
    truncated_wav.extend_from_slice(&16_000_u32.to_le_bytes());
    truncated_wav.extend_from_slice(&32_000_u32.to_le_bytes());
    truncated_wav.extend_from_slice(&2_u16.to_le_bytes());
    truncated_wav.extend_from_slice(&16_u16.to_le_bytes());
    truncated_wav.extend_from_slice(b"data");
    truncated_wav.extend_from_slice(&100_u32.to_le_bytes());
    truncated_wav.extend_from_slice(&[0_u8; 4]);
    let truncated = stream_to_tempfile(
        stream::iter(vec![Ok::<_, std::io::Error>(Bytes::from(truncated_wav))]),
        &limits,
    )
    .await
    .unwrap();
    let err = validate_streamed_bytes(
        &truncated,
        Some("truncated.wav"),
        Some("audio/wav"),
        &limits,
    )
    .unwrap_err();
    assert_eq!(err.threat_class(), Some(ThreatClass::ParserCorruption));
}

#[tokio::test]
async fn truncated_stream_is_not_accepted() {
    let limits = LimitsConfig::policy_defaults();
    // Empty body after interruption-style empty stream → magic unrecognized / reject.
    let streamed = stream_to_tempfile(
        stream::iter(Vec::<Result<Bytes, std::io::Error>>::new()),
        &limits,
    )
    .await
    .unwrap();
    assert_eq!(streamed.size_bytes, 0);
    let err = validate_streamed_bytes(
        &streamed,
        Some("empty.pdf"),
        Some("application/octet-stream"),
        &limits,
    )
    .unwrap_err();
    assert!(matches!(
        err.threat_class(),
        Some(ThreatClass::UnsupportedFormat) | Some(ThreatClass::ExtensionSpoof)
    ));
}

#[tokio::test]
async fn corrupt_and_page_bomb_pdf_reject() {
    let limits = LimitsConfig::policy_defaults();
    let corrupt = stream_file(&adversarial_dir().join("corrupt.pdf")).await;
    let err = validate_streamed_bytes(
        &corrupt,
        Some("corrupt.pdf"),
        Some("application/octet-stream"),
        &limits,
    )
    .unwrap_err();
    assert!(matches!(
        err.threat_class(),
        Some(ThreatClass::ParserCorruption) | Some(ThreatClass::PdfPageBomb)
    ));

    let page_bomb = stream_file(&adversarial_dir().join("page-bomb.pdf")).await;
    let err = validate_streamed_bytes(
        &page_bomb,
        Some("page-bomb.pdf"),
        Some("application/octet-stream"),
        &limits,
    )
    .unwrap_err();
    assert_eq!(err.threat_class(), Some(ThreatClass::PdfPageBomb));
}

#[tokio::test]
async fn formula_csv_and_prompt_html_quarantine() {
    let limits = LimitsConfig::policy_defaults();
    let csv = stream_file(&adversarial_dir().join("formula.csv")).await;
    let (format, disposition, threat, _) = validate_streamed_bytes(
        &csv,
        Some("formula.csv"),
        Some("application/octet-stream"),
        &limits,
    )
    .unwrap();
    assert_eq!(format, CanonicalFormat::Csv);
    assert_eq!(disposition, Disposition::Quarantined);
    assert_eq!(threat, Some(ThreatClass::CsvFormula));

    let html = stream_file(&adversarial_dir().join("prompt-injection.html")).await;
    let (format, disposition, threat, _) = validate_streamed_bytes(
        &html,
        Some("prompt-injection.html"),
        Some("application/octet-stream"),
        &limits,
    )
    .unwrap();
    assert_eq!(format, CanonicalFormat::Html);
    assert_eq!(disposition, Disposition::Quarantined);
    assert_eq!(threat, Some(ThreatClass::PromptInjection));
}

#[tokio::test]
async fn happy_path_small_fixtures_accepted() {
    let limits = LimitsConfig::policy_defaults();
    let cases = [
        ("gold-004.pdf", CanonicalFormat::Pdf),
        ("gold-006.docx", CanonicalFormat::Docx),
        ("gold-014.csv", CanonicalFormat::Csv),
        ("gold-020.png", CanonicalFormat::Png),
    ];
    for (name, expected) in cases {
        let streamed = stream_file(&golden_dir().join(name)).await;
        let (format, disposition, _, _) = validate_streamed_bytes(
            &streamed,
            Some(name),
            Some("application/octet-stream"),
            &limits,
        )
        .unwrap();
        assert_eq!(format, expected, "{name}");
        assert_eq!(disposition, Disposition::Accepted, "{name}");
        assert_disposition_is_typed(disposition);
    }
}

#[tokio::test]
async fn malicious_filename_never_enters_object_key() {
    let org = Uuid::new_v4();
    let object = Uuid::new_v4();
    for name in ["../../etc/passwd", "/abs/evil.pdf", "file\nname.docx"] {
        let key = quarantine_key(org, object, Some(name)).unwrap();
        let key_str = key.as_str();
        assert!(!key_str.contains("passwd"));
        assert!(!key_str.contains("evil"));
        assert!(!key_str.contains('\n'));
        assert!(key_str.starts_with("quarantine/"));
    }
    assert!(reject_dangerous_entry_name("../../etc/passwd").is_err());
}

#[tokio::test]
async fn property_filename_and_magic_never_panic() {
    let limits = LimitsConfig::policy_defaults();
    let names = [
        "",
        ".",
        "..",
        "a.pdf",
        "a.docx",
        "../../x.pdf",
        "file\0.pdf",
        "x.html",
        "x.csv",
        "noext",
    ];
    let payloads: &[&[u8]] = &[
        b"%PDF-1.4\n%%EOF\n",
        b"PK\x03\x04",
        b"not a pdf",
        b"<html>hi</html>",
        b"a,b\n1,2\n",
        &[0xff, 0xd8, 0xff, 0xe0],
        &[],
    ];
    for name in names {
        for payload in payloads {
            let streamed = stream_to_tempfile(
                stream::iter(vec![Ok::<_, std::io::Error>(Bytes::copy_from_slice(
                    payload,
                ))]),
                &limits,
            )
            .await
            .unwrap();
            let result = validate_streamed_bytes(
                &streamed,
                Some(name),
                Some("application/octet-stream"),
                &limits,
            );
            match result {
                Ok((_, disposition, _, _)) => assert_disposition_is_typed(disposition),
                Err(error) => {
                    assert!(error.threat_class().is_some());
                    let _ = error.reason_code();
                }
            }
            let _ = detect_magic(payload);
            let _ = resolve_canonical_format(payload, Some(name));
        }
    }
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn quota_hook_is_callable() {
    let Some(db_url) = test_database_url() else {
        return;
    };
    let ephemeral = EphemeralDb::create(&db_url).await;
    apply_migrations(&ephemeral.url).await.expect("migrations");
    let pool = create_pool(&ephemeral.url).expect("pool");
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let _collection_id = seed_uploader(
        &pool,
        org,
        user,
        "quota-hook@example.test",
        "correct-password-1",
    )
    .await;
    let ctx = OrgContext::try_new(org, user, ["doc.upload"], []).unwrap();
    let reservation = quota_reserve_hook(&pool, &ctx, "test-idem", 12)
        .await
        .expect("reserve quota");
    assert_eq!(reservation.storage.reservation.amount, 12);
    ephemeral.drop().await;
}

#[cfg(feature = "test-hooks")]
#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL and MARKHAND_TEST_MINIO_* + test-hooks"]
async fn registration_fault_refunds_quota_and_deletes_quarantine_object() {
    let Some(db_url) = test_database_url() else {
        return;
    };
    let _hook_guard = acquire_hook_test_guard();
    let Some((client, bucket)) = test_minio_client() else {
        return;
    };
    client.ensure_bucket().await.expect("bucket");

    let ephemeral = EphemeralDb::create(&db_url).await;
    apply_migrations(&ephemeral.url).await.expect("migrations");
    let pool = create_pool(&ephemeral.url).expect("pool");
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let collection_id = seed_uploader(
        &pool,
        org,
        user,
        "finalize-cleanup@example.test",
        "correct-password-1",
    )
    .await;
    let ctx = OrgContext::try_new(org, user, ["doc.upload"], [collection_id]).unwrap();
    let streamed = stream_file(&golden_dir().join("gold-004.pdf")).await;
    let reservation_key = "op.registration-fault";
    let identity = QuarantineIdentity {
        object_id: Uuid::new_v4(),
        collection_id,
        document_id: Uuid::new_v4(),
        version_id: Uuid::new_v4(),
    };
    arm_saga_fault(SagaFaultBarrier::RegistrationFail);
    let err = run_upload_saga(
        &pool,
        &client,
        &ctx,
        &LimitsConfig::policy_defaults(),
        SagaInput {
            collection_id,
            idempotency_key: "client:registration-fault".into(),
            reservation_key: reservation_key.into(),
            streamed,
            declared_filename: Some("report.pdf".into()),
            declared_content_type: Some("application/pdf".into()),
            identity,
        },
    )
    .await
    .expect_err("registration fault must fail closed");
    let _ = err;

    assert_eq!(
        quota_reservation_status(&pool, &ctx, &format!("upload.storage.{reservation_key}")).await,
        Some("refunded".into())
    );
    assert_eq!(
        quota_reservation_status(&pool, &ctx, &format!("upload.documents.{reservation_key}")).await,
        Some("refunded".into())
    );
    assert_eq!(quota_counter_value(&pool, &ctx, "storage_bytes").await, 0);
    assert_eq!(quota_counter_value(&pool, &ctx, "documents").await, 0);
    let docs: i64 = with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT COUNT(*)::bigint FROM documents WHERE org_id = $1",
                        &[&ctx.org_id()],
                    )
                    .await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .unwrap();
    assert_eq!(docs, 0, "registration fault must leave zero document rows");
    let jobs: i64 = with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT COUNT(*)::bigint FROM jobs WHERE org_id = $1",
                        &[&ctx.org_id()],
                    )
                    .await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .unwrap();
    assert_eq!(jobs, 0);
    let _ = bucket;
    client
        .cleanup_bucket_and_assert_gone()
        .await
        .expect("minio bucket cleanup must succeed and assert gone");

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_MINIO_*"]
async fn happy_path_persists_to_quarantine_with_metadata() {
    let Some((client, _bucket)) = test_minio_client() else {
        return;
    };
    client.ensure_bucket().await.expect("bucket");
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let ctx = OrgContext::try_new(org, user, ["doc.upload"], []).unwrap();
    let limits = LimitsConfig::policy_defaults();
    let path = golden_dir().join("gold-004.pdf");
    let streamed = stream_file(&path).await;
    let expected_sha = streamed.sha256_hex.clone();
    let outcome = validate_and_quarantine(
        &ctx,
        &client,
        &limits,
        streamed,
        Some("report.pdf"),
        Some("application/pdf"),
    )
    .await
    .expect("accepted");
    assert_eq!(outcome.disposition, Disposition::Accepted);
    assert_eq!(outcome.canonical_format, CanonicalFormat::Pdf);
    assert_eq!(outcome.sha256_hex, expected_sha);
    assert!(!outcome.object_key.as_str().contains("report.pdf"));
    assert!(outcome.object_key.as_str().starts_with("quarantine/"));

    let meta = client
        .head_metadata(org, &outcome.object_key)
        .await
        .expect("head");
    assert_eq!(
        meta.get("original-filename").map(String::as_str),
        Some("report.pdf")
    );
    assert_eq!(
        meta.get("canonical-format").map(String::as_str),
        Some("pdf")
    );
    assert_eq!(
        meta.get("content-sha256").map(String::as_str),
        Some(expected_sha.as_str())
    );
    client
        .delete_object(org, &outcome.object_key)
        .await
        .expect("cleanup");
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_MINIO_*"]
async fn rejected_upload_is_not_stored() {
    let Some((client, _bucket)) = test_minio_client() else {
        return;
    };
    client.ensure_bucket().await.expect("bucket");
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let ctx = OrgContext::try_new(org, user, ["doc.upload"], []).unwrap();
    let limits = LimitsConfig::policy_defaults();
    let streamed = stream_file(&adversarial_dir().join("plain-text.pdf")).await;
    let err = validate_and_quarantine(
        &ctx,
        &client,
        &limits,
        streamed,
        Some("plain-text.pdf"),
        Some("application/pdf"),
    )
    .await
    .unwrap_err();
    assert_eq!(err.threat_class(), Some(ThreatClass::ExtensionSpoof));
    // No object should have been created; a random key lookup stays NotFound-ish after auth.
    let probe = quarantine_key(org, Uuid::new_v4(), Some("plain-text.pdf")).unwrap();
    assert!(!client.object_exists(org, &probe).await.expect("exists"));
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_MINIO_*"]
async fn verification_failed_upload_is_cleaned_by_generated_key() {
    let Some((client, _bucket)) = test_minio_client() else {
        return;
    };
    client.ensure_bucket().await.expect("bucket");
    let org = Uuid::new_v4();
    let object_id = Uuid::new_v4();
    let key = quarantine_key(org, object_id, Some("bad.bin")).unwrap();
    let payload: &'static [u8] = b"actual object bytes";
    let wrong_sha = hex::encode(Sha256::digest(b"different bytes"));
    let meta = ObjectIdentityMeta {
        org_id: org,
        collection_id: None,
        document_id: None,
        version_id: None,
        original_filename: Some("bad.bin".into()),
        canonical_format: Some("txt".into()),
        content_sha256: Some(wrong_sha.clone()),
        content_length: Some(payload.len() as u64),
        disposition: Some("accepted".into()),
    };
    let err = client
        .put_object_stream(
            org,
            &key,
            payload,
            &meta,
            "text/plain",
            ObjectPutVerification {
                expected_len: payload.len() as u64,
                expected_sha256: &wrong_sha,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        fileconv_server::storage::error::StorageError::Backend
            | fileconv_server::storage::error::StorageError::Transport
    ));
    assert!(!client.object_exists(org, &key).await.expect("exists"));
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_MINIO_*"]
async fn cancelled_stream_upload_finishes_verify_or_cleanup() {
    let Some((client, _bucket)) = test_minio_client() else {
        return;
    };
    client.ensure_bucket().await.expect("bucket");
    let org = Uuid::new_v4();
    let object_id = Uuid::new_v4();
    let version_id = Uuid::new_v4();
    let key = quarantine_key(org, object_id, Some("cancelled.txt")).unwrap();
    let trusted_probe =
        trusted_key(org, version_id, Uuid::new_v4(), Some("cancelled.txt")).unwrap();
    let payload = b"cancelled upload still finishes verification and cleanup".to_vec();
    let wrong_sha = hex::encode(Sha256::digest(b"not the uploaded payload"));
    let meta = ObjectIdentityMeta {
        org_id: org,
        collection_id: None,
        document_id: None,
        version_id: None,
        original_filename: Some("cancelled.txt".into()),
        canonical_format: Some("txt".into()),
        content_sha256: Some(wrong_sha.clone()),
        content_length: Some(payload.len() as u64),
        disposition: Some("accepted".into()),
    };
    let (mut writer, reader) = tokio::io::duplex(8);
    {
        let upload = client.put_object_stream(
            org,
            &key,
            reader,
            &meta,
            "text/plain",
            ObjectPutVerification {
                expected_len: payload.len() as u64,
                expected_sha256: &wrong_sha,
            },
        );
        tokio::pin!(upload);
        tokio::select! {
            result = &mut upload => panic!("upload unexpectedly finished before cancellation: {result:?}"),
            _ = tokio::time::sleep(std::time::Duration::from_millis(20)) => {}
        }
        writer
            .write_all(&payload[..8])
            .await
            .expect("write first chunk");
    }
    writer
        .write_all(&payload[8..])
        .await
        .expect("write remaining chunks");
    writer.shutdown().await.expect("finish writer");

    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            assert!(!client
                .object_exists(org, &trusted_probe)
                .await
                .expect("trusted exists"));
            if !client.object_exists(org, &key).await.expect("exists") {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("owned upload task cleaned generated quarantine object");
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL and MARKHAND_TEST_MINIO_*"]
async fn http_upload_happy_and_spoof() {
    let Some(db_url) = test_database_url() else {
        return;
    };
    let Some((store, _bucket)) = test_minio_client() else {
        return;
    };
    store.ensure_bucket().await.expect("bucket");
    let store_for_assert = store.clone();

    let ephemeral = EphemeralDb::create(&db_url).await;
    apply_migrations(&ephemeral.url).await.expect("migrations");
    let pool = create_pool(&ephemeral.url).expect("pool");
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let collection_id = seed_uploader(
        &pool,
        org,
        user,
        "uploader@example.test",
        "correct-password-1",
    )
    .await;

    let runtime = RuntimeState::from_config(ServerConfig::test_with_endpoints(RuntimeEndpoints {
        database_url: SecretString::new(&ephemeral.url),
        qdrant_url: "http://127.0.0.1:1".into(),
        minio_url: "http://127.0.0.1:9000".into(),
    }))
    .expect("runtime");
    let auth = PasswordAuthProvider::new(
        pool.clone(),
        test_auth_config(),
        JwtKeys::from_auth(&test_auth_config()).unwrap(),
    );
    let state_auth = PasswordAuthProvider::new(
        pool.clone(),
        test_auth_config(),
        JwtKeys::from_auth(&test_auth_config()).unwrap(),
    );
    let state = AppState::from_parts_with_store(runtime, pool, Some(state_auth), Some(store))
        .expect("state");
    let app = router(state);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header("content-type", "application/json")
                .body(Body::from(vec![b'a'; 3 * 1024 * 1024]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let login = auth
        .login_password(
            "uploader@example.test",
            "correct-password-1",
            &AuthRequestMeta {
                request_id: "req-upload-test".into(),
            },
        )
        .await
        .expect("login");
    let token = login.tokens.access_token.expose().to_string();

    let pdf = std::fs::read(golden_dir().join("gold-004.pdf")).unwrap();
    let body = multipart_body("report.pdf", &pdf, collection_id);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/uploads")
                .header("authorization", format!("Bearer {token}"))
                .header("idempotency-key", "http-upload-retry-key")
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={BOUNDARY}"),
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["disposition"], "accepted");
    assert_eq!(json["canonicalFormat"], "pdf");
    let key = json["objectKey"].as_str().unwrap();
    assert!(!key.contains("report.pdf"));
    assert!(key.starts_with("quarantine/"));
    let parsed_key = parse_key_for_org(key, org).expect("parse quarantine key");
    let stored_meta = store_for_assert
        .head_metadata(org, &parsed_key)
        .await
        .expect("head stored upload");
    assert_eq!(
        stored_meta.get("original-filename").map(String::as_str),
        Some("report.pdf")
    );
    assert_eq!(
        stored_meta.get("content-length-bytes"),
        Some(&pdf.len().to_string())
    );
    let stored = store_for_assert
        .get_object(org, &parsed_key)
        .await
        .expect("get stored upload");
    assert_eq!(stored.len(), pdf.len());
    assert_eq!(hex::encode(Sha256::digest(&stored)), json["sha256"]);

    let first_document_id = json["documentId"].as_str().unwrap().to_string();
    let first_job_id = json["jobId"].as_str().unwrap().to_string();
    let retry = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/uploads")
                .header("authorization", format!("Bearer {token}"))
                .header("idempotency-key", "http-upload-retry-key")
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={BOUNDARY}"),
                )
                .body(Body::from(multipart_body(
                    "report.pdf",
                    &pdf,
                    collection_id,
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        retry.status(),
        StatusCode::CREATED,
        "same digest replay must return 201"
    );
    let retry_bytes = retry.into_body().collect().await.unwrap().to_bytes();
    let retry_json: serde_json::Value = serde_json::from_slice(&retry_bytes).unwrap();
    assert_eq!(retry_json["documentId"], first_document_id);
    assert_eq!(retry_json["jobId"], first_job_id);

    let conflict = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/uploads")
                .header("authorization", format!("Bearer {token}"))
                .header("idempotency-key", "http-upload-retry-key")
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={BOUNDARY}"),
                )
                .body(Body::from(multipart_body(
                    "note.txt",
                    b"different digest payload\n",
                    collection_id,
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        conflict.status(),
        StatusCode::CONFLICT,
        "different digest with same Idempotency-Key must 409"
    );

    let spoof = std::fs::read(adversarial_dir().join("plain-text.pdf")).unwrap();
    let body = multipart_body("plain-text.pdf", &spoof, collection_id);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/uploads")
                .header("authorization", format!("Bearer {token}"))
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={BOUNDARY}"),
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["code"], "upload_rejected");
    assert!(!bytes.windows(b"not a pdf".len()).any(|w| w == b"not a pdf"));

    let truncated = multipart_body_without_close("late.pdf", &pdf, collection_id);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/uploads")
                .header("authorization", format!("Bearer {token}"))
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={BOUNDARY}"),
                )
                .body(Body::from(truncated))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_ne!(json["disposition"], "accepted");
    assert!(json.get("objectKey").is_none());

    let too_many_parts = many_part_body(10);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/uploads")
                .header("authorization", format!("Bearer {token}"))
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={BOUNDARY}"),
                )
                .body(Body::from(too_many_parts))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        json["details"]["reasonCode"],
        ReasonCode::MultipartTooManyParts.as_str()
    );

    let huge_name = format!("{}.pdf", "a".repeat(9 * 1024));
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/uploads")
                .header("authorization", format!("Bearer {token}"))
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={BOUNDARY}"),
                )
                .body(Body::from(multipart_body(&huge_name, &pdf, collection_id)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL and MARKHAND_TEST_MINIO_*"]
async fn cancelled_http_upload_settles_quota_consistently() {
    let Some(db_url) = test_database_url() else {
        return;
    };
    let Some((store, _bucket)) = test_minio_client() else {
        return;
    };
    store.ensure_bucket().await.expect("bucket");

    let ephemeral = EphemeralDb::create(&db_url).await;
    apply_migrations(&ephemeral.url).await.expect("migrations");
    let pool = create_pool(&ephemeral.url).expect("pool");
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let collection_id = seed_uploader(
        &pool,
        org,
        user,
        "cancel-quota@example.test",
        "correct-password-1",
    )
    .await;
    let ctx = OrgContext::try_new(org, user, ["doc.upload"], []).unwrap();

    let runtime = RuntimeState::from_config(ServerConfig::test_with_endpoints(RuntimeEndpoints {
        database_url: SecretString::new(&ephemeral.url),
        qdrant_url: "http://127.0.0.1:1".into(),
        minio_url: "http://127.0.0.1:9000".into(),
    }))
    .expect("runtime");
    let auth = PasswordAuthProvider::new(
        pool.clone(),
        test_auth_config(),
        JwtKeys::from_auth(&test_auth_config()).unwrap(),
    );
    let state_auth = PasswordAuthProvider::new(
        pool.clone(),
        test_auth_config(),
        JwtKeys::from_auth(&test_auth_config()).unwrap(),
    );
    let state =
        AppState::from_parts_with_store(runtime, pool.clone(), Some(state_auth), Some(store))
            .expect("state");
    let app = router(state);
    let login = auth
        .login_password(
            "cancel-quota@example.test",
            "correct-password-1",
            &AuthRequestMeta {
                request_id: "req-cancel-quota".into(),
            },
        )
        .await
        .expect("login");
    let token = login.tokens.access_token.expose().to_string();

    let idempotency_key = "cancel-quota-key-1";
    let operation_key = upload_operation_key(org, user, idempotency_key);
    let storage_reservation_key = format!("upload.storage.{operation_key}");
    let payload = vec![b'a'; 8 * 1024 * 1024];
    let expected_len = payload.len() as i64;
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/uploads")
        .header("authorization", format!("Bearer {token}"))
        .header("idempotency-key", idempotency_key)
        .header(
            "content-type",
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(Body::from(multipart_body(
            "cancelled.txt",
            &payload,
            collection_id,
        )))
        .unwrap();
    let handle = tokio::spawn(app.oneshot(request));

    let mut saw_reservation = false;
    for _ in 0..100 {
        if quota_reservation_status(&pool, &ctx, &storage_reservation_key)
            .await
            .is_some()
        {
            saw_reservation = true;
            break;
        }
        if handle.is_finished() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(saw_reservation, "request should reach quota reservation");
    handle.abort();
    let _ = handle.await;

    let mut terminal = None;
    for _ in 0..200 {
        let status = quota_reservation_status(&pool, &ctx, &storage_reservation_key).await;
        if matches!(
            status.as_deref(),
            Some("finalized" | "refunded" | "expired")
        ) {
            terminal = status;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let terminal = terminal.expect("quota reservation reaches terminal state");
    let storage_counter = quota_counter_value(&pool, &ctx, "storage_bytes").await;
    match terminal.as_str() {
        "finalized" => assert_eq!(storage_counter, expected_len),
        "refunded" | "expired" => assert_eq!(storage_counter, 0),
        other => panic!("unexpected terminal status {other}"),
    }

    ephemeral.drop().await;
}

const BOUNDARY: &str = "----markhandUploadBoundary7MA4YWxk";

fn multipart_body(filename: &str, content: &[u8], collection_id: Uuid) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"collectionId\"\r\n\r\n{collection_id}\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(
        format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n").as_bytes(),
    );
    body.extend_from_slice(content);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    body
}

fn multipart_body_without_close(filename: &str, content: &[u8], collection_id: Uuid) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"collectionId\"\r\n\r\n{collection_id}\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(
        format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: application/pdf\r\n\r\n").as_bytes(),
    );
    body.extend_from_slice(content);
    body
}

fn many_part_body(parts: usize) -> Vec<u8> {
    let mut body = Vec::new();
    for i in 0..parts {
        body.extend_from_slice(
            format!(
                "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"field{i}\"\r\n\r\nx\r\n"
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    body
}

#[test]
fn upload_error_redacts_and_maps_status() {
    let err = UploadError::rejected(ThreatClass::Oversize, ReasonCode::UploadTooLarge);
    assert_eq!(err.status_code(), StatusCode::PAYLOAD_TOO_LARGE);
    let debug = format!("{err:?}");
    assert!(!debug.contains("passwd"));
    assert_eq!(
        hex::encode(Sha256::digest(b"hello")),
        hex::encode(Sha256::digest(b"hello"))
    );
}

async fn count_org_rows(pool: &Pool, ctx: &OrgContext, table: &str) -> i64 {
    let sql = format!("SELECT COUNT(*)::bigint FROM {table} WHERE org_id = $1");
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn.query_one(&sql, &[&ctx.org_id()]).await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .expect("count")
}

async fn grant_permission(pool: &Pool, ctx: &OrgContext, code: &str) {
    let code = code.to_string();
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                fileconv_server::services::authz_lock::lock_principal_authz(
                    txn,
                    ctx.org_id(),
                    ctx.user_id(),
                )
                .await?;
                txn.execute(
                    "INSERT INTO permissions (id, code, description)
                     VALUES ($1, $2, $2)
                     ON CONFLICT (code) DO NOTHING",
                    &[&Uuid::new_v4(), &code],
                )
                .await?;
                let role_id: Uuid = txn
                    .query_one(
                        "SELECT id FROM roles WHERE org_id = $1 AND code = 'owner'",
                        &[&ctx.org_id()],
                    )
                    .await?
                    .get(0);
                let perm_id: Uuid = txn
                    .query_one("SELECT id FROM permissions WHERE code = $1", &[&code])
                    .await?
                    .get(0);
                txn.execute(
                    "INSERT INTO role_permissions (org_id, role_id, permission_id)
                     VALUES ($1, $2, $3)
                     ON CONFLICT DO NOTHING",
                    &[&ctx.org_id(), &role_id, &perm_id],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("grant permission");
}

#[cfg(feature = "test-hooks")]
#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL and MARKHAND_TEST_MINIO_* + test-hooks"]
async fn saga_fault_barriers_leave_terminal_cleanup() {
    let Some(db_url) = test_database_url() else {
        return;
    };
    let _hook_guard = acquire_hook_test_guard();
    let Some((client, _bucket)) = test_minio_client() else {
        return;
    };
    client.ensure_bucket().await.expect("bucket");
    let ephemeral = EphemeralDb::create(&db_url).await;
    apply_migrations(&ephemeral.url).await.expect("migrations");
    let pool = create_pool(&ephemeral.url).expect("pool");
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let collection_id = seed_uploader(
        &pool,
        org,
        user,
        "saga-barriers@example.test",
        "correct-password-1",
    )
    .await;
    let ctx = OrgContext::try_new(org, user, ["doc.upload"], [collection_id]).unwrap();
    let limits = LimitsConfig::policy_defaults();

    for (idx, barrier) in [
        SagaFaultBarrier::AfterReserve,
        SagaFaultBarrier::AfterObjectPut,
        SagaFaultBarrier::BeforeCommit,
        SagaFaultBarrier::RegistrationFail,
    ]
    .into_iter()
    .enumerate()
    {
        let streamed = stream_file(&golden_dir().join("gold-004.pdf")).await;
        let reservation_key = format!("op.barrier-{idx}");
        arm_saga_fault(barrier);
        let err = run_upload_saga(
            &pool,
            &client,
            &ctx,
            &limits,
            SagaInput {
                collection_id,
                idempotency_key: format!("client:barrier-{idx}"),
                reservation_key: reservation_key.clone(),
                streamed,
                declared_filename: Some("report.pdf".into()),
                declared_content_type: Some("application/pdf".into()),
                identity: QuarantineIdentity {
                    object_id: Uuid::new_v4(),
                    collection_id,
                    document_id: Uuid::new_v4(),
                    version_id: Uuid::new_v4(),
                },
            },
        )
        .await
        .expect_err("armed barrier must fail");
        let _ = err;
        assert_eq!(count_org_rows(&pool, &ctx, "documents").await, 0);
        assert_eq!(count_org_rows(&pool, &ctx, "jobs").await, 0);
        assert_eq!(quota_counter_value(&pool, &ctx, "storage_bytes").await, 0);
        assert_eq!(quota_counter_value(&pool, &ctx, "documents").await, 0);
        assert_eq!(
            quota_reservation_status(&pool, &ctx, &format!("upload.storage.{reservation_key}"))
                .await,
            Some("refunded".into())
        );
    }
    client
        .cleanup_bucket_and_assert_gone()
        .await
        .expect("cleanup");
    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL and MARKHAND_TEST_MINIO_* + test-hooks"]
async fn quarantined_review_requires_approval_for_single_job() {
    let Some(db_url) = test_database_url() else {
        return;
    };
    let Some((client, _bucket)) = test_minio_client() else {
        return;
    };
    client.ensure_bucket().await.expect("bucket");
    let ephemeral = EphemeralDb::create(&db_url).await;
    apply_migrations(&ephemeral.url).await.expect("migrations");
    let pool = create_pool(&ephemeral.url).expect("pool");
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let collection_id = seed_uploader(
        &pool,
        org,
        user,
        "review-upload@example.test",
        "correct-password-1",
    )
    .await;
    let ctx = OrgContext::try_new(org, user, ["doc.upload"], [collection_id]).unwrap();
    let streamed = stream_file(&adversarial_dir().join("formula.csv")).await;
    let success = run_upload_saga(
        &pool,
        &client,
        &ctx,
        &LimitsConfig::policy_defaults(),
        SagaInput {
            collection_id,
            idempotency_key: "client:review-csv".into(),
            reservation_key: "op.review-csv".into(),
            streamed,
            declared_filename: Some("formula.csv".into()),
            declared_content_type: Some("text/csv".into()),
            identity: QuarantineIdentity {
                object_id: Uuid::new_v4(),
                collection_id,
                document_id: Uuid::new_v4(),
                version_id: Uuid::new_v4(),
            },
        },
    )
    .await
    .expect("quarantined upload registers");
    assert_eq!(success.outcome.disposition, Disposition::Quarantined);
    assert!(success.registered.job_id.is_none());
    assert_eq!(count_org_rows(&pool, &ctx, "jobs").await, 0);
    assert_eq!(count_org_rows(&pool, &ctx, "documents").await, 1);
    // No current published version / chunks before approval.
    let currents: i64 = with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        let document_id = success.registered.document_id;
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT COUNT(*)::bigint FROM document_versions
                         WHERE org_id = $1 AND document_id = $2 AND is_current",
                        &[&ctx.org_id(), &document_id],
                    )
                    .await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .unwrap();
    assert_eq!(currents, 0);
    let chunks: i64 = with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        let document_id = success.registered.document_id;
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT COUNT(*)::bigint FROM chunks
                         WHERE org_id = $1 AND document_id = $2",
                        &[&ctx.org_id(), &document_id],
                    )
                    .await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .unwrap();
    assert_eq!(chunks, 0);

    // Uploader without review permission cannot self-approve.
    let denied = approve_quarantined_upload(
        &pool,
        &ctx,
        ApproveIntakeRequest {
            collection_id,
            document_id: success.registered.document_id,
            reason: Some("self"),
            request_id: "req-self-approve",
        },
    )
    .await;
    assert!(
        denied.is_err(),
        "uploader without review permission must fail"
    );

    // Grant reviewer permission and approve twice (idempotent).
    grant_permission(&pool, &ctx, PERMISSION_QUARANTINE_REVIEW).await;
    let reviewer = OrgContext::try_new(
        org,
        user,
        ["doc.upload", PERMISSION_QUARANTINE_REVIEW],
        [collection_id],
    )
    .unwrap();
    let first = approve_quarantined_upload(
        &pool,
        &reviewer,
        ApproveIntakeRequest {
            collection_id,
            document_id: success.registered.document_id,
            reason: Some("looks safe"),
            request_id: "req-approve-1",
        },
    )
    .await
    .expect("approve");
    let second = approve_quarantined_upload(
        &pool,
        &reviewer,
        ApproveIntakeRequest {
            collection_id,
            document_id: success.registered.document_id,
            reason: Some("looks safe"),
            request_id: "req-approve-2",
        },
    )
    .await
    .expect("approve replay");
    assert_eq!(first.job_id, second.job_id);
    assert!(first.created_job);
    assert!(!second.created_job);
    assert_eq!(count_org_rows(&pool, &ctx, "jobs").await, 1);
    // Cross-collection IDOR → not found.
    let idor = approve_quarantined_upload(
        &pool,
        &reviewer,
        ApproveIntakeRequest {
            collection_id: Uuid::new_v4(),
            document_id: success.registered.document_id,
            reason: None,
            request_id: "req-idor",
        },
    )
    .await;
    assert!(matches!(
        idor,
        Err(fileconv_server::services::upload::SagaError::NotFound)
            | Err(fileconv_server::services::upload::SagaError::PermissionDenied)
    ));

    client
        .cleanup_bucket_and_assert_gone()
        .await
        .expect("cleanup");
    ephemeral.drop().await;
}

#[cfg(feature = "test-hooks")]
async fn run_fresh_auth_barrier_case(
    pool: &Pool,
    client: &MinioClient,
    label: &str,
    mutate: impl FnOnce(
        Pool,
        Uuid,
        Uuid,
        Uuid,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>,
) {
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let collection_id = seed_uploader(
        pool,
        org,
        user,
        &format!("{label}@example.test"),
        "correct-password-1",
    )
    .await;
    let ctx = OrgContext::try_new(org, user, ["doc.upload"], [collection_id]).unwrap();
    let limits = LimitsConfig::policy_defaults();
    let streamed = stream_file(&golden_dir().join("gold-004.pdf")).await;
    arm_pause_before_commit();
    let pool_bg = pool.clone();
    let client_bg = client.clone();
    let ctx_bg = ctx.clone();
    let idem = format!("client:auth-{label}");
    let reservation_key = format!("op.auth-{label}");
    let handle = tokio::spawn(async move {
        run_upload_saga(
            &pool_bg,
            &client_bg,
            &ctx_bg,
            &limits,
            SagaInput {
                collection_id,
                idempotency_key: idem,
                reservation_key,
                streamed,
                declared_filename: Some("report.pdf".into()),
                declared_content_type: Some("application/pdf".into()),
                identity: QuarantineIdentity {
                    object_id: Uuid::new_v4(),
                    collection_id,
                    document_id: Uuid::new_v4(),
                    version_id: Uuid::new_v4(),
                },
            },
        )
        .await
    });
    for _ in 0..50 {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        // Wait until durable object_stored intent exists.
        let ready = with_org_txn(pool, &ctx, {
            let ctx = ctx.clone();
            let key = format!("client:auth-{label}");
            move |txn| {
                Box::pin(async move {
                    let row = txn
                        .query_opt(
                            "SELECT state FROM upload_operations
                             WHERE org_id = $1 AND user_id = $2 AND idempotency_key = $3",
                            &[&ctx.org_id(), &ctx.user_id(), &key],
                        )
                        .await?;
                    Ok(row.map(|r| r.get::<_, String>(0)))
                })
            }
        })
        .await
        .unwrap();
        if ready.as_deref() == Some("object_stored") {
            break;
        }
    }
    mutate(pool.clone(), org, user, collection_id).await;
    resume_before_commit();
    let result = handle.await.expect("join");
    assert!(
        result.is_err(),
        "{label}: fresh auth barrier must deny registration: {result:?}"
    );
}

#[cfg(feature = "test-hooks")]
#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL and MARKHAND_TEST_MINIO_* + test-hooks"]
async fn fresh_auth_barriers_deny_cleanup_and_zero_rows() {
    let Some(db_url) = test_database_url() else {
        return;
    };
    let _hook_guard = acquire_hook_test_guard();
    let Some((client, _bucket)) = test_minio_client() else {
        return;
    };
    client.ensure_bucket().await.expect("bucket");
    let ephemeral = EphemeralDb::create(&db_url).await;
    apply_migrations(&ephemeral.url).await.expect("migrations");
    let pool = create_pool(&ephemeral.url).expect("pool");

    run_fresh_auth_barrier_case(
        &pool,
        &client,
        "disable",
        |pool, _org, user, _collection| {
            Box::pin(async move {
                let client = pool.get().await.expect("client");
                client
                    .execute(
                        "UPDATE users SET disabled_at = now() WHERE id = $1",
                        &[&user],
                    )
                    .await
                    .expect("disable");
            })
        },
    )
    .await;

    run_fresh_auth_barrier_case(
        &pool,
        &client,
        "remove-membership",
        |pool, org, user, _collection| {
            Box::pin(async move {
                let client = pool.get().await.expect("client");
                client
                    .execute(
                        "DELETE FROM org_memberships WHERE org_id = $1 AND user_id = $2",
                        &[&org, &user],
                    )
                    .await
                    .expect("remove");
            })
        },
    )
    .await;

    run_fresh_auth_barrier_case(
        &pool,
        &client,
        "delete-collection",
        |pool, org, _user, collection| {
            Box::pin(async move {
                let client = pool.get().await.expect("client");
                client
                    .execute(
                        "UPDATE collections SET deleted_at = now()
                         WHERE org_id = $1 AND id = $2",
                        &[&org, &collection],
                    )
                    .await
                    .expect("soft-delete");
            })
        },
    )
    .await;

    run_fresh_auth_barrier_case(
        &pool,
        &client,
        "revoke-upload-perm",
        |pool, org, user, _collection| {
            Box::pin(async move {
                let ctx = OrgContext::try_new(org, user, ["doc.upload"], []).unwrap();
                with_org_txn(&pool, &ctx, {
                    let ctx = ctx.clone();
                    move |txn| {
                        Box::pin(async move {
                            fileconv_server::services::acl_mutate::revoke_role_permission_for_principal(
                                txn,
                                ctx.org_id(),
                                ctx.user_id(),
                                "doc.upload",
                            )
                            .await?;
                            Ok(())
                        })
                    }
                })
                .await
                .expect("revoke upload perm");
            })
        },
    )
    .await;

    run_fresh_auth_barrier_case(
        &pool,
        &client,
        "revoke-collection-acl",
        |pool, org, user, collection| {
            Box::pin(async move {
                // Alternate owner so target principal loses private-collection access.
                let alt = Uuid::new_v4();
                let ctx = OrgContext::try_new(org, user, ["doc.upload"], [collection]).unwrap();
                with_org_txn(&pool, &ctx, {
                    let ctx = ctx.clone();
                    move |txn| {
                        Box::pin(async move {
                            orgs::ensure_user(
                                txn,
                                &ctx,
                                alt,
                                &format!("alt-{}@example.test", alt.simple()),
                                "Alt Owner",
                            )
                            .await?;
                            txn.execute(
                                "INSERT INTO org_memberships (org_id, user_id, role)
                                 VALUES ($1, $2, 'owner')
                                 ON CONFLICT (org_id, user_id) DO NOTHING",
                                &[&org, &alt],
                            )
                            .await?;
                            fileconv_server::services::acl_mutate::revoke_collection_access_for_principal(
                                txn,
                                org,
                                user,
                                collection,
                                alt,
                            )
                            .await?;
                            Ok(())
                        })
                    }
                })
                .await
                .expect("revoke collection acl");
            })
        },
    )
    .await;

    client
        .cleanup_bucket_and_assert_gone()
        .await
        .expect("cleanup");
    ephemeral.drop().await;
}

#[cfg(feature = "test-hooks")]
#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL and MARKHAND_TEST_MINIO_* + test-hooks"]
async fn concurrent_idempotent_upload_one_side_effect() {
    let Some(db_url) = test_database_url() else {
        return;
    };
    let _hook_guard = acquire_hook_test_guard();
    let Some((client, _bucket)) = test_minio_client() else {
        return;
    };
    client.ensure_bucket().await.expect("bucket");
    let ephemeral = EphemeralDb::create(&db_url).await;
    apply_migrations(&ephemeral.url).await.expect("migrations");
    let pool = create_pool(&ephemeral.url).expect("pool");
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let collection_id = seed_uploader(
        &pool,
        org,
        user,
        "concurrent-idem@example.test",
        "correct-password-1",
    )
    .await;
    let ctx = OrgContext::try_new(org, user, ["doc.upload"], [collection_id]).unwrap();
    let limits = LimitsConfig::policy_defaults();
    let pool_a = pool.clone();
    let pool_b = pool.clone();
    let client_a = client.clone();
    let client_b = client.clone();
    let ctx_a = ctx.clone();
    let ctx_b = ctx.clone();
    let a = tokio::spawn(async move {
        let streamed = stream_file(&golden_dir().join("gold-004.pdf")).await;
        run_upload_saga(
            &pool_a,
            &client_a,
            &ctx_a,
            &limits,
            SagaInput {
                collection_id,
                idempotency_key: "client:concurrent-same".into(),
                reservation_key: upload_operation_key(org, user, "concurrent-same"),
                streamed,
                declared_filename: Some("report.pdf".into()),
                declared_content_type: Some("application/pdf".into()),
                identity: QuarantineIdentity {
                    object_id: Uuid::new_v4(),
                    collection_id,
                    document_id: Uuid::new_v4(),
                    version_id: Uuid::new_v4(),
                },
            },
        )
        .await
    });
    let b = tokio::spawn(async move {
        let streamed = stream_file(&golden_dir().join("gold-004.pdf")).await;
        run_upload_saga(
            &pool_b,
            &client_b,
            &ctx_b,
            &limits,
            SagaInput {
                collection_id,
                idempotency_key: "client:concurrent-same".into(),
                reservation_key: upload_operation_key(org, user, "concurrent-same"),
                streamed,
                declared_filename: Some("report.pdf".into()),
                declared_content_type: Some("application/pdf".into()),
                identity: QuarantineIdentity {
                    object_id: Uuid::new_v4(),
                    collection_id,
                    document_id: Uuid::new_v4(),
                    version_id: Uuid::new_v4(),
                },
            },
        )
        .await
    });
    let ra = a.await.unwrap();
    let rb = b.await.unwrap();
    let successes = [&ra, &rb].iter().filter(|r| r.is_ok()).count();
    let conflicts = [&ra, &rb]
        .iter()
        .filter(|r| {
            matches!(
                r,
                Err(fileconv_server::services::upload::SagaError::IdempotencyInProgress)
                    | Err(fileconv_server::services::upload::SagaError::IdempotencyConflict)
            )
        })
        .count();
    assert!(
        successes >= 1,
        "at least one concurrent upload must complete: {ra:?} {rb:?}"
    );
    assert_eq!(
        successes + conflicts,
        2,
        "other side must be in-progress/conflict or success replay"
    );
    // Retry the loser until durable replay 201-equivalent.
    let mut final_ok = ra.ok().or_else(|| rb.ok());
    for _ in 0..20 {
        if final_ok.is_some()
            && final_ok
                .as_ref()
                .is_some_and(|s| s.registered.job_id.is_some())
        {
            break;
        }
        let streamed = stream_file(&golden_dir().join("gold-004.pdf")).await;
        match run_upload_saga(
            &pool,
            &client,
            &ctx,
            &limits,
            SagaInput {
                collection_id,
                idempotency_key: "client:concurrent-same".into(),
                reservation_key: upload_operation_key(org, user, "concurrent-same"),
                streamed,
                declared_filename: Some("report.pdf".into()),
                declared_content_type: Some("application/pdf".into()),
                identity: QuarantineIdentity {
                    object_id: Uuid::new_v4(),
                    collection_id,
                    document_id: Uuid::new_v4(),
                    version_id: Uuid::new_v4(),
                },
            },
        )
        .await
        {
            Ok(success) => final_ok = Some(success),
            Err(fileconv_server::services::upload::SagaError::IdempotencyInProgress) => {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            Err(other) => panic!("unexpected concurrent retry error: {other:?}"),
        }
    }
    let success = final_ok.expect("eventually one completed upload");
    assert_eq!(count_org_rows(&pool, &ctx, "documents").await, 1);
    assert_eq!(count_org_rows(&pool, &ctx, "jobs").await, 1);
    assert!(success.registered.job_id.is_some());
    assert_eq!(
        quota_counter_value(&pool, &ctx, "documents").await,
        1,
        "quota documents committed exactly once"
    );

    client
        .cleanup_bucket_and_assert_gone()
        .await
        .expect("cleanup");
    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL and MARKHAND_TEST_MINIO_*"]
async fn envelope_binds_collection_and_stable_replay_deep_equality() {
    let Some(db_url) = test_database_url() else {
        return;
    };
    let Some((client, _bucket)) = test_minio_client() else {
        return;
    };
    client.ensure_bucket().await.expect("bucket");
    let ephemeral = EphemeralDb::create(&db_url).await;
    apply_migrations(&ephemeral.url).await.expect("migrations");
    let pool = create_pool(&ephemeral.url).expect("pool");
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let collection_a = seed_uploader(
        &pool,
        org,
        user,
        "envelope-a@example.test",
        "correct-password-1",
    )
    .await;
    // Second collection in same org.
    let collection_b = Uuid::new_v4();
    let ctx = OrgContext::try_new(org, user, ["doc.upload"], [collection_a, collection_b]).unwrap();
    with_org_txn(&pool, &ctx, {
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "INSERT INTO collections (
                        id, org_id, name, slug, visibility, owner_user_id
                     ) VALUES ($1, $2, 'B', $3, 'org', $4)",
                    &[
                        &collection_b,
                        &org,
                        &format!("upload-b-{}", collection_b.simple()),
                        &user,
                    ],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("seed collection b");

    let limits = LimitsConfig::policy_defaults();
    let streamed = stream_file(&golden_dir().join("gold-004.pdf")).await;
    let first = run_upload_saga(
        &pool,
        &client,
        &ctx,
        &limits,
        SagaInput {
            collection_id: collection_a,
            idempotency_key: "client:envelope-stable".into(),
            reservation_key: upload_operation_key(org, user, "envelope-stable"),
            streamed,
            declared_filename: Some("report.pdf".into()),
            declared_content_type: Some("application/pdf".into()),
            identity: QuarantineIdentity {
                object_id: Uuid::new_v4(),
                collection_id: collection_a,
                document_id: Uuid::new_v4(),
                version_id: Uuid::new_v4(),
            },
        },
    )
    .await
    .expect("first upload");
    assert!(!first.replayed);

    // Same key + bytes but different collection → conflict.
    let streamed = stream_file(&golden_dir().join("gold-004.pdf")).await;
    let conflict = run_upload_saga(
        &pool,
        &client,
        &ctx,
        &limits,
        SagaInput {
            collection_id: collection_b,
            idempotency_key: "client:envelope-stable".into(),
            reservation_key: upload_operation_key(org, user, "envelope-stable"),
            streamed,
            declared_filename: Some("report.pdf".into()),
            declared_content_type: Some("application/pdf".into()),
            identity: QuarantineIdentity {
                object_id: Uuid::new_v4(),
                collection_id: collection_b,
                document_id: Uuid::new_v4(),
                version_id: Uuid::new_v4(),
            },
        },
    )
    .await;
    assert!(matches!(
        conflict,
        Err(fileconv_server::services::upload::SagaError::IdempotencyConflict)
    ));

    // Same envelope → deep-equal stable fields (requestId is outside StableUploadResponse).
    let streamed = stream_file(&golden_dir().join("gold-004.pdf")).await;
    let replay = run_upload_saga(
        &pool,
        &client,
        &ctx,
        &limits,
        SagaInput {
            collection_id: collection_a,
            idempotency_key: "client:envelope-stable".into(),
            reservation_key: upload_operation_key(org, user, "envelope-stable"),
            streamed,
            declared_filename: Some("report.pdf".into()),
            declared_content_type: Some("application/pdf".into()),
            identity: QuarantineIdentity {
                object_id: Uuid::new_v4(),
                collection_id: collection_a,
                document_id: Uuid::new_v4(),
                version_id: Uuid::new_v4(),
            },
        },
    )
    .await
    .expect("replay");
    assert!(replay.replayed);
    assert_eq!(first.stable, replay.stable);

    // Revoke ACL on original collection → replay fail-closed.
    with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                fileconv_server::services::acl_mutate::revoke_role_permission_for_principal(
                    txn,
                    ctx.org_id(),
                    ctx.user_id(),
                    "doc.upload",
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("revoke");
    let streamed = stream_file(&golden_dir().join("gold-004.pdf")).await;
    let denied = run_upload_saga(
        &pool,
        &client,
        &OrgContext::try_new(org, user, ["doc.upload"], [collection_a]).unwrap(),
        &limits,
        SagaInput {
            collection_id: collection_a,
            idempotency_key: "client:envelope-stable".into(),
            reservation_key: upload_operation_key(org, user, "envelope-stable"),
            streamed,
            declared_filename: Some("report.pdf".into()),
            declared_content_type: Some("application/pdf".into()),
            identity: QuarantineIdentity {
                object_id: Uuid::new_v4(),
                collection_id: collection_a,
                document_id: Uuid::new_v4(),
                version_id: Uuid::new_v4(),
            },
        },
    )
    .await;
    assert!(matches!(
        denied,
        Err(fileconv_server::services::upload::SagaError::PermissionDenied)
    ));

    client
        .cleanup_bucket_and_assert_gone()
        .await
        .expect("cleanup");
    ephemeral.drop().await;
}

#[cfg(feature = "test-hooks")]
#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL and MARKHAND_TEST_MINIO_* + test-hooks"]
async fn reconcile_vs_commit_interleavings_no_deadlock_object_retained_when_completed() {
    let Some(db_url) = test_database_url() else {
        return;
    };
    let _hook_guard = acquire_hook_test_guard();
    let Some((client, _bucket)) = test_minio_client() else {
        return;
    };
    client.ensure_bucket().await.expect("bucket");
    let ephemeral = EphemeralDb::create(&db_url).await;
    apply_migrations(&ephemeral.url).await.expect("migrations");
    let pool = create_pool(&ephemeral.url).expect("pool");
    let limits = LimitsConfig::policy_defaults();

    // Interleaving A: reconcile claims first → commit loses; no documents.
    {
        let org = Uuid::new_v4();
        let user = Uuid::new_v4();
        let collection_id = seed_uploader(
            &pool,
            org,
            user,
            "recon-a@example.test",
            "correct-password-1",
        )
        .await;
        let ctx = OrgContext::try_new(org, user, ["doc.upload"], [collection_id]).unwrap();
        arm_pause_before_commit();
        let pool_bg = pool.clone();
        let client_bg = client.clone();
        let ctx_bg = ctx.clone();
        let handle = tokio::spawn(async move {
            let streamed = stream_file(&golden_dir().join("gold-004.pdf")).await;
            run_upload_saga(
                &pool_bg,
                &client_bg,
                &ctx_bg,
                &limits,
                SagaInput {
                    collection_id,
                    idempotency_key: "client:recon-a".into(),
                    reservation_key: upload_operation_key(org, user, "recon-a"),
                    streamed,
                    declared_filename: Some("report.pdf".into()),
                    declared_content_type: Some("application/pdf".into()),
                    identity: QuarantineIdentity {
                        object_id: Uuid::new_v4(),
                        collection_id,
                        document_id: Uuid::new_v4(),
                        version_id: Uuid::new_v4(),
                    },
                },
            )
            .await
        });
        wait_for_op_state(&pool, &ctx, "client:recon-a", "object_stored").await;
        with_org_txn(&pool, &ctx, {
            let ctx = ctx.clone();
            move |txn| {
                Box::pin(async move {
                    txn.execute(
                        "UPDATE upload_operations SET updated_at = now() - interval '2 hours'
                         WHERE org_id = $1 AND user_id = $2 AND idempotency_key = $3",
                        &[&ctx.org_id(), &ctx.user_id(), &"client:recon-a"],
                    )
                    .await?;
                    Ok(())
                })
            }
        })
        .await
        .expect("age");
        arm_pause_after_reconcile_claim();
        let pool_r = pool.clone();
        let client_r = client.clone();
        let ctx_r = ctx.clone();
        let recon = tokio::spawn(async move {
            reconcile_stale_uploads(
                &pool_r,
                &client_r,
                &ctx_r,
                chrono::Utc::now() - chrono::Duration::hours(1),
                10,
            )
            .await
        });
        // Wait until reconciling claimed.
        for _ in 0..50 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            if op_state(&pool, &ctx, "client:recon-a").await.as_deref() == Some("reconciling") {
                break;
            }
        }
        resume_before_commit();
        let commit_result = handle.await.expect("join");
        resume_after_reconcile_claim();
        let _ = recon.await.expect("recon join");
        assert!(
            commit_result.is_err(),
            "commit must lose to reconcile claim: {commit_result:?}"
        );
        assert_eq!(count_org_rows(&pool, &ctx, "documents").await, 0);
    }

    // Interleaving B: commit completes first → reconcile must retain object.
    {
        let org = Uuid::new_v4();
        let user = Uuid::new_v4();
        let collection_id = seed_uploader(
            &pool,
            org,
            user,
            "recon-b@example.test",
            "correct-password-1",
        )
        .await;
        let ctx = OrgContext::try_new(org, user, ["doc.upload"], [collection_id]).unwrap();
        arm_pause_before_commit();
        let pool_bg = pool.clone();
        let client_bg = client.clone();
        let ctx_bg = ctx.clone();
        let handle = tokio::spawn(async move {
            let streamed = stream_file(&golden_dir().join("gold-004.pdf")).await;
            run_upload_saga(
                &pool_bg,
                &client_bg,
                &ctx_bg,
                &limits,
                SagaInput {
                    collection_id,
                    idempotency_key: "client:recon-b".into(),
                    reservation_key: upload_operation_key(org, user, "recon-b"),
                    streamed,
                    declared_filename: Some("report.pdf".into()),
                    declared_content_type: Some("application/pdf".into()),
                    identity: QuarantineIdentity {
                        object_id: Uuid::new_v4(),
                        collection_id,
                        document_id: Uuid::new_v4(),
                        version_id: Uuid::new_v4(),
                    },
                },
            )
            .await
        });
        wait_for_op_state(&pool, &ctx, "client:recon-b", "object_stored").await;
        resume_before_commit();
        let success = handle.await.expect("join").expect("commit wins");
        assert!(!success.replayed);
        assert_eq!(
            op_state(&pool, &ctx, "client:recon-b").await.as_deref(),
            Some("completed")
        );
        with_org_txn(&pool, &ctx, {
            let ctx = ctx.clone();
            move |txn| {
                Box::pin(async move {
                    txn.execute(
                        "UPDATE upload_operations SET updated_at = now() - interval '2 hours'
                         WHERE org_id = $1 AND user_id = $2 AND idempotency_key = $3",
                        &[&ctx.org_id(), &ctx.user_id(), &"client:recon-b"],
                    )
                    .await?;
                    Ok(())
                })
            }
        })
        .await
        .expect("age");
        let cleaned = reconcile_stale_uploads(
            &pool,
            &client,
            &ctx,
            chrono::Utc::now() - chrono::Duration::hours(1),
            10,
        )
        .await
        .expect("reconcile");
        assert_eq!(cleaned, 0, "completed ops must not be claimed");
        assert_eq!(
            op_state(&pool, &ctx, "client:recon-b").await.as_deref(),
            Some("completed")
        );
        assert!(
            client
                .object_exists(org, &success.outcome.object_key)
                .await
                .expect("exists"),
            "completed object must be retained"
        );
        assert_eq!(count_org_rows(&pool, &ctx, "documents").await, 1);
    }

    client
        .cleanup_bucket_and_assert_gone()
        .await
        .expect("cleanup");
    ephemeral.drop().await;
}

#[cfg(feature = "test-hooks")]
#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL and MARKHAND_TEST_MINIO_* + test-hooks"]
async fn stale_started_putting_reconcile_and_retry_with_new_attempt() {
    let Some(db_url) = test_database_url() else {
        return;
    };
    let _hook_guard = acquire_hook_test_guard();
    let Some((client, _bucket)) = test_minio_client() else {
        return;
    };
    client.ensure_bucket().await.expect("bucket");
    let ephemeral = EphemeralDb::create(&db_url).await;
    apply_migrations(&ephemeral.url).await.expect("migrations");
    let pool = create_pool(&ephemeral.url).expect("pool");
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let collection_id = seed_uploader(
        &pool,
        org,
        user,
        "retry-stale@example.test",
        "correct-password-1",
    )
    .await;
    let ctx = OrgContext::try_new(org, user, ["doc.upload"], [collection_id]).unwrap();
    let limits = LimitsConfig::policy_defaults();

    // Fault after reserve → refunded; retry with same idempotency key succeeds.
    arm_saga_fault(SagaFaultBarrier::AfterReserve);
    let streamed = stream_file(&golden_dir().join("gold-004.pdf")).await;
    let err = run_upload_saga(
        &pool,
        &client,
        &ctx,
        &limits,
        SagaInput {
            collection_id,
            idempotency_key: "client:retry-stale".into(),
            reservation_key: upload_operation_key(org, user, "retry-stale"),
            streamed,
            declared_filename: Some("report.pdf".into()),
            declared_content_type: Some("application/pdf".into()),
            identity: QuarantineIdentity {
                object_id: Uuid::new_v4(),
                collection_id,
                document_id: Uuid::new_v4(),
                version_id: Uuid::new_v4(),
            },
        },
    )
    .await
    .expect_err("fault");
    let _ = err;
    assert_eq!(
        op_state(&pool, &ctx, "client:retry-stale").await.as_deref(),
        Some("refunded")
    );

    let streamed = stream_file(&golden_dir().join("gold-004.pdf")).await;
    let success = run_upload_saga(
        &pool,
        &client,
        &ctx,
        &limits,
        SagaInput {
            collection_id,
            idempotency_key: "client:retry-stale".into(),
            reservation_key: upload_operation_key(org, user, "retry-stale"),
            streamed,
            declared_filename: Some("report.pdf".into()),
            declared_content_type: Some("application/pdf".into()),
            identity: QuarantineIdentity {
                object_id: Uuid::new_v4(),
                collection_id,
                document_id: Uuid::new_v4(),
                version_id: Uuid::new_v4(),
            },
        },
    )
    .await
    .expect("retry after refunded");
    assert!(!success.replayed);
    assert_eq!(
        op_state(&pool, &ctx, "client:retry-stale").await.as_deref(),
        Some("completed")
    );
    let attempt: i32 = with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT attempt FROM upload_operations
                         WHERE org_id = $1 AND user_id = $2 AND idempotency_key = $3",
                        &[&ctx.org_id(), &ctx.user_id(), &"client:retry-stale"],
                    )
                    .await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .expect("attempt");
    assert!(attempt >= 2, "retry must bump attempt, got {attempt}");

    // Stale started/putting reconcile window: age a reserved-only op.
    arm_saga_fault(SagaFaultBarrier::AfterReserve);
    let streamed = stream_file(&golden_dir().join("gold-004.pdf")).await;
    let _ = run_upload_saga(
        &pool,
        &client,
        &ctx,
        &limits,
        SagaInput {
            collection_id,
            idempotency_key: "client:stale-putting".into(),
            reservation_key: upload_operation_key(org, user, "stale-putting"),
            streamed,
            declared_filename: Some("report.pdf".into()),
            declared_content_type: Some("application/pdf".into()),
            identity: QuarantineIdentity {
                object_id: Uuid::new_v4(),
                collection_id,
                document_id: Uuid::new_v4(),
                version_id: Uuid::new_v4(),
            },
        },
    )
    .await;
    with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                // Force putting + age for reconciler coverage.
                txn.execute(
                    "UPDATE upload_operations
                     SET state = 'putting', updated_at = now() - interval '3 hours'
                     WHERE org_id = $1 AND user_id = $2 AND idempotency_key = $3",
                    &[&ctx.org_id(), &ctx.user_id(), &"client:stale-putting"],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("force putting");
    let cleaned = reconcile_stale_uploads(
        &pool,
        &client,
        &ctx,
        chrono::Utc::now() - chrono::Duration::hours(1),
        10,
    )
    .await
    .expect("reconcile stale putting");
    assert!(cleaned >= 1);
    assert_eq!(
        op_state(&pool, &ctx, "client:stale-putting")
            .await
            .as_deref(),
        Some("refunded")
    );

    client
        .cleanup_bucket_and_assert_gone()
        .await
        .expect("cleanup");
    ephemeral.drop().await;
}

#[cfg(feature = "test-hooks")]
#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL and MARKHAND_TEST_MINIO_* + test-hooks"]
async fn quarantine_reviewer_concurrent_and_suspend_mid_approve() {
    let Some(db_url) = test_database_url() else {
        return;
    };
    let _hook_guard = acquire_hook_test_guard();
    let Some((client, _bucket)) = test_minio_client() else {
        return;
    };
    client.ensure_bucket().await.expect("bucket");
    let ephemeral = EphemeralDb::create(&db_url).await;
    apply_migrations(&ephemeral.url).await.expect("migrations");
    let pool = create_pool(&ephemeral.url).expect("pool");
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let collection_id = seed_uploader(
        &pool,
        org,
        user,
        "review-conc@example.test",
        "correct-password-1",
    )
    .await;
    let ctx = OrgContext::try_new(org, user, ["doc.upload"], [collection_id]).unwrap();
    let streamed = stream_file(&adversarial_dir().join("formula.csv")).await;
    let success = run_upload_saga(
        &pool,
        &client,
        &ctx,
        &LimitsConfig::policy_defaults(),
        SagaInput {
            collection_id,
            idempotency_key: "client:review-conc".into(),
            reservation_key: "op.review-conc".into(),
            streamed,
            declared_filename: Some("formula.csv".into()),
            declared_content_type: Some("text/csv".into()),
            identity: QuarantineIdentity {
                object_id: Uuid::new_v4(),
                collection_id,
                document_id: Uuid::new_v4(),
                version_id: Uuid::new_v4(),
            },
        },
    )
    .await
    .expect("quarantined");
    grant_permission(&pool, &ctx, PERMISSION_QUARANTINE_REVIEW).await;
    let reviewer = OrgContext::try_new(
        org,
        user,
        ["doc.upload", PERMISSION_QUARANTINE_REVIEW],
        [collection_id],
    )
    .unwrap();

    let pool_a = pool.clone();
    let pool_b = pool.clone();
    let rev_a = reviewer.clone();
    let rev_b = reviewer.clone();
    let doc = success.registered.document_id;
    let a = tokio::spawn(async move {
        approve_quarantined_upload(
            &pool_a,
            &rev_a,
            ApproveIntakeRequest {
                collection_id,
                document_id: doc,
                reason: Some("a"),
                request_id: "req-a",
            },
        )
        .await
    });
    let b = tokio::spawn(async move {
        approve_quarantined_upload(
            &pool_b,
            &rev_b,
            ApproveIntakeRequest {
                collection_id,
                document_id: doc,
                reason: Some("b"),
                request_id: "req-b",
            },
        )
        .await
    });
    let ra = a.await.unwrap();
    let rb = b.await.unwrap();
    let oks: Vec<_> = [&ra, &rb].iter().filter_map(|r| r.as_ref().ok()).collect();
    assert_eq!(
        oks.len(),
        2,
        "both approves must succeed idempotently: {ra:?} {rb:?}"
    );
    assert_eq!(oks[0].job_id, oks[1].job_id);
    assert_eq!(
        oks.iter().filter(|r| r.created_job).count(),
        1,
        "exactly one created job"
    );
    assert_eq!(count_org_rows(&pool, &ctx, "jobs").await, 1);

    // Second quarantined doc: suspend mid-approve fail-closed.
    let streamed = stream_file(&adversarial_dir().join("formula.csv")).await;
    let second = run_upload_saga(
        &pool,
        &client,
        &ctx,
        &LimitsConfig::policy_defaults(),
        SagaInput {
            collection_id,
            idempotency_key: "client:review-suspend".into(),
            reservation_key: "op.review-suspend".into(),
            streamed,
            declared_filename: Some("formula.csv".into()),
            declared_content_type: Some("text/csv".into()),
            identity: QuarantineIdentity {
                object_id: Uuid::new_v4(),
                collection_id,
                document_id: Uuid::new_v4(),
                version_id: Uuid::new_v4(),
            },
        },
    )
    .await
    .expect("second quarantined");
    arm_pause_before_approve_commit();
    let pool_bg = pool.clone();
    let rev_bg = reviewer.clone();
    let doc2 = second.registered.document_id;
    let handle = tokio::spawn(async move {
        approve_quarantined_upload(
            &pool_bg,
            &rev_bg,
            ApproveIntakeRequest {
                collection_id,
                document_id: doc2,
                reason: Some("suspend-me"),
                request_id: "req-suspend",
            },
        )
        .await
    });
    // Wait until approve has entered the txn (best-effort short delay).
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let admin = pool.get().await.expect("client");
    admin
        .execute(
            "UPDATE users SET disabled_at = now() WHERE id = $1",
            &[&user],
        )
        .await
        .expect("suspend");
    resume_before_approve_commit();
    let denied = handle.await.expect("join");
    assert!(
        matches!(
            denied,
            Err(fileconv_server::services::upload::SagaError::PermissionDenied)
        ),
        "suspend mid-approve must deny: {denied:?}"
    );

    client
        .cleanup_bucket_and_assert_gone()
        .await
        .expect("cleanup");
    ephemeral.drop().await;
}

#[cfg(feature = "test-hooks")]
async fn op_state(pool: &Pool, ctx: &OrgContext, idempotency_key: &str) -> Option<String> {
    let key = idempotency_key.to_string();
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_opt(
                        "SELECT state FROM upload_operations
                         WHERE org_id = $1 AND user_id = $2 AND idempotency_key = $3",
                        &[&ctx.org_id(), &ctx.user_id(), &key],
                    )
                    .await?;
                Ok(row.map(|r| r.get::<_, String>(0)))
            })
        }
    })
    .await
    .ok()
    .flatten()
}

#[cfg(feature = "test-hooks")]
async fn wait_for_op_state(pool: &Pool, ctx: &OrgContext, idempotency_key: &str, want: &str) {
    for _ in 0..100 {
        if op_state(pool, ctx, idempotency_key).await.as_deref() == Some(want) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for upload_operations.state={want}");
}
