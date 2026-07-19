//! Live tests for citation, preview, and original-download authorization.
//!
//! These tests skip cleanly unless PostgreSQL, MinIO, and Qdrant test endpoints
//! are provided in the environment. They are intentionally not run by the normal
//! library test gate.

use bytes::Bytes;
use chrono::{Duration as ChronoDuration, Utc};
use deadpool_postgres::Pool;
use fileconv_server::auth::context::OrgContext;
use fileconv_server::config::{MinioConfig, SecretString};
use fileconv_server::database::apply_migrations;
use fileconv_server::db::collections::{self, NewCollection};
use fileconv_server::db::models::{ArtifactKind, CollectionVisibility};
use fileconv_server::db::orgs;
use fileconv_server::db::pool::{create_pool, with_org_txn};
use fileconv_server::services::citation::{resolve_citation, CitationError, CitationPin};
use fileconv_server::services::deletion;
use fileconv_server::services::download::{
    authorize_download, redeem_download, CapabilityKey, ConsumedDownloadNonces, DownloadError,
};
use fileconv_server::services::preview::{fetch_markdown_preview, PreviewError};
use fileconv_server::storage::minio::{MinioClient, ObjectIdentityMeta};
use fileconv_server::storage::{quarantine_key, trusted_key};
use sha2::{Digest, Sha256};
use tokio_postgres::NoTls;
use uuid::Uuid;

const POC_ORG_ID: &str = "11111111-1111-1111-1111-111111111111";
const POC_USER_ID: &str = "22222222-2222-2222-2222-222222222201";

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
    let access_key = match std::env::var("MARKHAND_TEST_MINIO_ACCESS_KEY") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => {
            eprintln!("skipped: MARKHAND_TEST_MINIO_ACCESS_KEY unset");
            return None;
        }
    };
    let secret_key = match std::env::var("MARKHAND_TEST_MINIO_SECRET_KEY") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => {
            eprintln!("skipped: MARKHAND_TEST_MINIO_SECRET_KEY unset");
            return None;
        }
    };
    let region = std::env::var("MARKHAND_TEST_MINIO_REGION").unwrap_or_else(|_| "us-east-1".into());
    let bucket = format!("markhand-r02-{}", Uuid::new_v4().simple());
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

fn require_qdrant_url() -> Option<()> {
    match std::env::var("MARKHAND_TEST_QDRANT_URL") {
        Ok(url) if !url.trim().is_empty() => Some(()),
        _ => {
            eprintln!("skipped: MARKHAND_TEST_QDRANT_URL unset");
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
        let db_name = format!("markhand_r02_{}", Uuid::new_v4().simple());
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
    base_ctx: OrgContext,
    capability_key: CapabilityKey,
}

impl LiveEnv {
    async fn boot() -> Option<Self> {
        let base_url = test_database_url()?;
        let storage = test_minio_client()?;
        require_qdrant_url()?;
        storage.ensure_bucket().await.expect("ensure bucket");
        let db = EphemeralDb::create(&base_url).await;
        apply_migrations(&db.url).await.expect("apply migrations");
        let pool = create_pool(&db.url).expect("pool");
        let org_id = Uuid::parse_str(POC_ORG_ID).expect("org id");
        let user_id = Uuid::parse_str(POC_USER_ID).expect("user id");
        let base_ctx = OrgContext::try_new(org_id, user_id, ["qa.query", "doc.delete"], [])
            .expect("org context");
        let capability_key = CapabilityKey::derive_from_auth_signing_key(&SecretString::new(
            "r02-live-test-signing-key-at-least-32-bytes",
        ));
        Some(Self {
            db,
            pool,
            storage,
            base_ctx,
            capability_key,
        })
    }

    fn ctx_for(&self, collection_ids: impl IntoIterator<Item = Uuid>) -> OrgContext {
        OrgContext::try_new(
            self.base_ctx.org_id(),
            self.base_ctx.user_id(),
            self.base_ctx.permissions().iter().cloned(),
            collection_ids,
        )
        .expect("scoped context")
    }

    async fn drop(self) {
        self.db.drop().await;
    }
}

struct SeededDocument {
    collection_id: Uuid,
    document_id: Uuid,
    version_id: Uuid,
    chunk_id: Uuid,
    markdown_sha256: String,
    original_sha256: String,
    markdown: Vec<u8>,
    original: Vec<u8>,
}

async fn seed_indexed_document(env: &LiveEnv, title: &str) -> SeededDocument {
    let collection_id = Uuid::new_v4();
    let document_id = Uuid::new_v4();
    let version_id = Uuid::new_v4();
    let chunk_id = Uuid::new_v4();
    let artifact_id = Uuid::new_v4();
    let index_metadata_id = Uuid::new_v4();
    let markdown =
        format!("# R02\n\nNội dung đáng tin R02-HOATDONG-2026 trong tài liệu {title}.\n")
            .into_bytes();
    let original = format!("%PDF-1.7 original bytes for {title}").into_bytes();
    let markdown_sha256 = hex::encode(Sha256::digest(&markdown));
    let original_sha256 = hex::encode(Sha256::digest(&original));
    let markdown_len = markdown.len() as i64;
    let trusted =
        trusted_key(env.base_ctx.org_id(), version_id, Uuid::new_v4(), None).expect("trusted key");
    let quarantine = quarantine_key(
        env.base_ctx.org_id(),
        Uuid::new_v4(),
        Some("../Báo cáo\nR02.pdf"),
    )
    .expect("quarantine key");
    env.storage
        .put_object(
            env.base_ctx.org_id(),
            &trusted,
            Bytes::copy_from_slice(&markdown),
            &ObjectIdentityMeta {
                org_id: env.base_ctx.org_id(),
                collection_id: Some(collection_id),
                document_id: Some(document_id),
                version_id: Some(version_id),
                original_filename: Some("preview.md".into()),
                canonical_format: Some("md".into()),
                content_sha256: Some(markdown_sha256.clone()),
                content_length: Some(markdown.len() as u64),
                disposition: Some("trusted".into()),
            },
            "text/markdown; charset=utf-8",
        )
        .await
        .expect("put markdown");
    env.storage
        .put_object(
            env.base_ctx.org_id(),
            &quarantine,
            Bytes::copy_from_slice(&original),
            &ObjectIdentityMeta {
                org_id: env.base_ctx.org_id(),
                collection_id: Some(collection_id),
                document_id: Some(document_id),
                version_id: Some(version_id),
                // Stored object metadata is always sanitized at upload (control
                // chars/newlines are invalid S3 header values). The adversarial
                // name lives in PG source_filename below, which the download
                // service must sanitize for the Content-Disposition header.
                original_filename: Some("R02.pdf".into()),
                canonical_format: Some("pdf".into()),
                content_sha256: Some(original_sha256.clone()),
                content_length: Some(original.len() as u64),
                disposition: Some("accepted".into()),
            },
            "application/pdf",
        )
        .await
        .expect("put original");

    let trusted_key = trusted.as_str();
    let quarantine_key = quarantine.as_str();
    let title = title.to_string();
    let body = String::from_utf8(markdown.clone()).expect("markdown utf8");
    let index_signature = "a".repeat(64);
    with_org_txn(&env.pool, &env.base_ctx, {
        let ctx = env.base_ctx.clone();
        let markdown_sha256 = markdown_sha256.clone();
        move |txn| {
            Box::pin(async move {
                orgs::ensure_user(txn, &ctx, ctx.user_id(), "admin@poc.example", "POC Admin")
                    .await?;
                let collection_name = format!("R02 {collection_id}");
                let collection_slug = format!("r02-{collection_id}");
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
                txn.execute(
                    "INSERT INTO documents (
                        id, org_id, collection_id, title, state, created_by_user_id
                     )
                     VALUES ($1, $2, $3, $4, 'indexed', $5)",
                    &[
                        &document_id,
                        &ctx.org_id(),
                        &collection_id,
                        &title,
                        &ctx.user_id(),
                    ],
                )
                .await?;
                txn.execute(
                    "INSERT INTO document_versions (
                        id, org_id, document_id, version_number, publication_state,
                        is_current, content_sha256, original_object_key, markdown_object_key,
                        source_filename, source_content_type, byte_size, created_by_user_id
                     )
                     VALUES ($1, $2, $3, 1, 'published', true, $4, $5, $6,
                             $7, 'application/pdf', $8, $9)",
                    &[
                        &version_id,
                        &ctx.org_id(),
                        &document_id,
                        &markdown_sha256,
                        &quarantine_key,
                        &trusted_key,
                        &Some("../Báo cáo\nR02.pdf"),
                        &markdown_len,
                        &ctx.user_id(),
                    ],
                )
                .await?;
                let artifact_kind = ArtifactKind::Markdown.as_str();
                txn.execute(
                    "UPDATE documents
                     SET current_version_id = $3, updated_at = clock_timestamp()
                     WHERE org_id = $1 AND id = $2",
                    &[&ctx.org_id(), &document_id, &version_id],
                )
                .await?;
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
                        &artifact_kind,
                        &trusted_key,
                        &markdown_sha256,
                        &markdown_len,
                    ],
                )
                .await?;
                txn.execute(
                    "INSERT INTO index_metadata (
                        id, org_id, collection_id, index_signature_sha256,
                        embedding_family, embedding_revision, dimensions,
                        normalized, runtime_path
                     )
                     VALUES ($1, $2, $3, $4, 'local-hash', 'r02-test', 16,
                             true, 'local-hash')",
                    &[
                        &index_metadata_id,
                        &ctx.org_id(),
                        &collection_id,
                        &index_signature,
                    ],
                )
                .await?;
                let chunk_identity = hex::encode(Sha256::digest(format!("{version_id}:{body}")));
                let heading_path = vec!["R02".to_string()];
                txn.execute(
                    "INSERT INTO chunks (
                        id, org_id, document_id, version_id, ordinal,
                        heading_path, body, body_text_version, chunk_identity_sha256,
                        index_metadata_id, index_signature, page, span_start, span_end
                     )
                     VALUES ($1, $2, $3, $4, 0, $5, $6, 'nfc-v1', $7,
                             $8, $9, 1, 0, $10)",
                    &[
                        &chunk_id,
                        &ctx.org_id(),
                        &document_id,
                        &version_id,
                        &heading_path,
                        &body,
                        &chunk_identity,
                        &index_metadata_id,
                        &index_signature,
                        &(body.len() as i32),
                    ],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("seed indexed document");

    SeededDocument {
        collection_id,
        document_id,
        version_id,
        chunk_id,
        markdown_sha256,
        original_sha256,
        markdown,
        original,
    }
}

fn pin_for(doc: &SeededDocument) -> CitationPin {
    CitationPin {
        document_id: doc.document_id,
        version_id: doc.version_id,
        version_number: 1,
        content_sha256: doc.markdown_sha256.clone(),
        chunk_id: doc.chunk_id,
        span_start: Some(0),
        span_end: None,
        quote: Some("R02-HOATDONG-2026".into()),
    }
}

#[tokio::test]
async fn live_citation_preview_download_authorization() {
    let Some(env) = LiveEnv::boot().await else {
        return;
    };
    let doc = seed_indexed_document(&env, "authorized").await;
    let ctx = env.ctx_for([doc.collection_id]);
    let resolved = resolve_citation(&env.pool, &ctx, pin_for(&doc))
        .await
        .expect("resolve citation");
    assert_eq!(resolved.document_id, doc.document_id);
    assert_eq!(resolved.version_id, doc.version_id);
    assert_eq!(resolved.content_sha256, doc.markdown_sha256);
    assert_eq!(resolved.chunk_id, doc.chunk_id);
    assert!(resolved.snippet.contains("R02-HOATDONG-2026"));

    let mut wrong_hash = pin_for(&doc);
    wrong_hash.content_sha256 = "b".repeat(64);
    assert!(matches!(
        resolve_citation(&env.pool, &ctx, wrong_hash).await,
        Err(CitationError::NotFound)
    ));
    let mut wrong_version = pin_for(&doc);
    wrong_version.version_number = 2;
    assert!(matches!(
        resolve_citation(&env.pool, &ctx, wrong_version).await,
        Err(CitationError::NotFound)
    ));
    let mut wrong_chunk = pin_for(&doc);
    wrong_chunk.chunk_id = Uuid::new_v4();
    assert!(matches!(
        resolve_citation(&env.pool, &ctx, wrong_chunk).await,
        Err(CitationError::NotFound)
    ));
    let mut wrong_quote = pin_for(&doc);
    wrong_quote.quote = Some("not in the immutable chunk".into());
    assert!(matches!(
        resolve_citation(&env.pool, &ctx, wrong_quote).await,
        Err(CitationError::NotFound)
    ));

    let preview = fetch_markdown_preview(
        &env.pool,
        &env.storage,
        &ctx,
        doc.document_id,
        doc.version_id,
    )
    .await
    .expect("preview");
    assert_eq!(preview.bytes.as_ref(), doc.markdown.as_slice());
    assert_ne!(preview.bytes.as_ref(), doc.original.as_slice());
    assert_eq!(preview.content_sha256, doc.markdown_sha256);

    let capability = authorize_download(
        &env.pool,
        &env.storage,
        &ctx,
        doc.document_id,
        doc.version_id,
        &env.capability_key,
        Utc::now(),
    )
    .await
    .expect("download capability");
    assert_eq!(capability.content_sha256, doc.original_sha256);
    assert_eq!(capability.byte_size, doc.original.len() as u64);
    assert!(!capability.token.contains('/'));
    assert!(capability
        .download_path
        .starts_with("/api/v1/documents/download/"));
    assert!(!capability.filename.contains('/'));
    assert!(!capability.filename.contains('\n'));
    let consumed = ConsumedDownloadNonces::new();
    let stream = redeem_download(
        &env.pool,
        &env.storage,
        &env.capability_key,
        &consumed,
        &capability.token,
        Utc::now(),
    )
    .await
    .expect("redeem");
    assert_eq!(stream.bytes.as_ref(), doc.original.as_slice());
    assert_eq!(stream.content_sha256, doc.original_sha256);
    assert!(matches!(
        redeem_download(
            &env.pool,
            &env.storage,
            &env.capability_key,
            &consumed,
            &capability.token,
            Utc::now(),
        )
        .await,
        Err(DownloadError::Replay)
    ));

    let expired = authorize_download(
        &env.pool,
        &env.storage,
        &ctx,
        doc.document_id,
        doc.version_id,
        &env.capability_key,
        Utc::now() - ChronoDuration::seconds(120),
    )
    .await
    .expect("expired capability");
    assert!(matches!(
        redeem_download(
            &env.pool,
            &env.storage,
            &env.capability_key,
            &ConsumedDownloadNonces::new(),
            &expired.token,
            Utc::now(),
        )
        .await,
        Err(DownloadError::Expired)
    ));
    // Flip the last base64 char (part of the HMAC tag) to a guaranteed-different
    // value so the tag verification fails deterministically. (The first char is
    // always 'A' because the token's version byte is 1, so tampering byte 0 is a
    // no-op.)
    let mut tampered = capability.token.clone();
    let last = tampered.pop().expect("non-empty token");
    tampered.push(if last == 'A' { 'B' } else { 'A' });
    assert!(matches!(
        redeem_download(
            &env.pool,
            &env.storage,
            &env.capability_key,
            &ConsumedDownloadNonces::new(),
            &tampered,
            Utc::now(),
        )
        .await,
        Err(DownloadError::InvalidToken)
    ));

    env.drop().await;
}

#[tokio::test]
async fn live_cross_collection_and_tombstone_are_non_disclosing() {
    let Some(env) = LiveEnv::boot().await else {
        return;
    };
    let doc = seed_indexed_document(&env, "denied").await;
    let denied_ctx = env.ctx_for([Uuid::new_v4()]);
    assert!(matches!(
        resolve_citation(&env.pool, &denied_ctx, pin_for(&doc)).await,
        Err(CitationError::NotFound)
    ));
    assert!(matches!(
        fetch_markdown_preview(
            &env.pool,
            &env.storage,
            &denied_ctx,
            doc.document_id,
            doc.version_id,
        )
        .await,
        Err(PreviewError::NotFound)
    ));
    assert!(matches!(
        authorize_download(
            &env.pool,
            &env.storage,
            &denied_ctx,
            doc.document_id,
            doc.version_id,
            &env.capability_key,
            Utc::now(),
        )
        .await,
        Err(DownloadError::NotFound)
    ));

    let ctx = env.ctx_for([doc.collection_id]);
    deletion::request_delete(&env.pool, &ctx, doc.document_id)
        .await
        .expect("tombstone");
    assert!(matches!(
        resolve_citation(&env.pool, &ctx, pin_for(&doc)).await,
        Err(CitationError::NotFound)
    ));
    assert!(matches!(
        fetch_markdown_preview(
            &env.pool,
            &env.storage,
            &ctx,
            doc.document_id,
            doc.version_id,
        )
        .await,
        Err(PreviewError::NotFound)
    ));
    assert!(matches!(
        authorize_download(
            &env.pool,
            &env.storage,
            &ctx,
            doc.document_id,
            doc.version_id,
            &env.capability_key,
            Utc::now(),
        )
        .await,
        Err(DownloadError::NotFound)
    ));
    env.drop().await;
}
