//! Upload intake integration tests (P1B-I01).
//!
//! Adversarial / unit-style validation runs without MinIO. Persistence paths are
//! gated on `MARKHAND_TEST_MINIO_*` and skip cleanly when unset. Auth-backed
//! HTTP tests also need `MARKHAND_TEST_DATABASE_URL`.

use std::io::Write;
use std::path::{Path, PathBuf};

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
use fileconv_server::services::upload::{
    assert_disposition_is_typed, detect_magic, quota_reserve_hook, reject_dangerous_entry_name,
    resolve_canonical_format, stream_to_tempfile, validate_and_quarantine, validate_streamed_bytes,
    validate_zip_archive, CanonicalFormat, Disposition, LimitsConfig, ReasonCode, ThreatClass,
    UploadError,
};
use fileconv_server::state::RuntimeState;
use fileconv_server::storage::keys::{parse_key_for_org, quarantine_key};
use fileconv_server::storage::minio::MinioClient;
use futures::stream;
use http_body_util::BodyExt;
use sha2::{Digest, Sha256};
use tower::ServiceExt;
use uuid::Uuid;
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;

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

async fn seed_uploader(pool: &Pool, org: Uuid, user: Uuid, email: &str, password: &str) {
    let ctx = OrgContext::try_new(org, user, ["doc.upload"], []).unwrap();
    let email = email.to_string();
    with_org_txn(pool, &ctx, {
        let owned = ctx.clone();
        move |txn| {
            Box::pin(async move {
                orgs::ensure_exists(txn, &owned, "uploadorg", "Upload Org").await?;
                orgs::ensure_user(txn, &owned, user, &email, "Uploader").await?;
                orgs::ensure_membership(txn, &owned).await?;
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
                Ok(())
            })
        }
    })
    .await
    .expect("seed org");

    session::set_password_hash(pool, user, password, &test_auth_config().argon2)
        .await
        .expect("set password");
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
    let before = current_rss_kib();
    let chunks = (0..1024).map(|_| Ok::<_, std::io::Error>(Bytes::from(vec![b'a'; 64 * 1024])));
    let streamed = stream_to_tempfile(stream::iter(chunks), &limits)
        .await
        .expect("stream large lazy body");
    let after = current_rss_kib();
    assert_eq!(streamed.size_bytes, 64 * 1024 * 1024);
    assert!(streamed.head.len() <= 512);
    if let (Some(before), Some(after)) = (before, after) {
        assert!(
            after.saturating_sub(before) < 32 * 1024,
            "RSS grew from {before} KiB to {after} KiB"
        );
    }
}

fn current_rss_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| {
        let value = line.strip_prefix("VmRSS:")?.trim();
        value.split_whitespace().next()?.parse().ok()
    })
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
async fn quota_hook_is_callable() {
    let org = OrgContext::try_new(Uuid::new_v4(), Uuid::new_v4(), ["doc.upload"], []).unwrap();
    quota_reserve_hook(&org, "test-idem", Some(12));
}

#[tokio::test]
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
    seed_uploader(
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
    let body = multipart_body("report.pdf", &pdf);
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

    let spoof = std::fs::read(adversarial_dir().join("plain-text.pdf")).unwrap();
    let body = multipart_body("plain-text.pdf", &spoof);
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

    let truncated = multipart_body_without_close("late.pdf", &pdf);
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
                .body(Body::from(multipart_body(&huge_name, &pdf)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    ephemeral.drop().await;
}

const BOUNDARY: &str = "----markhandUploadBoundary7MA4YWxk";

fn multipart_body(filename: &str, content: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(
        format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n").as_bytes(),
    );
    body.extend_from_slice(content);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    body
}

fn multipart_body_without_close(filename: &str, content: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
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
