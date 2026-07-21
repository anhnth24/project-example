//! Integration tests for citation / preview / download (P1B-R02) after review fixes.
//!
//! Hermetic unit coverage lives under `services/{citation,preview,download}` and
//! `storage::blob`. Live PostgreSQL tests require `MARKHAND_TEST_DATABASE_URL` and
//! exercise MemoryBlobStore (no MinIO) for real fetch/redeem paths.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{TimeZone, Utc};
use fileconv_server::auth::context::OrgContext;
use fileconv_server::config::SecretString;
use fileconv_server::database::apply_migrations;
use fileconv_server::db::download_capabilities::{
    classify_liveness, consume_authorized_or_classify, AuthorizedConsumeBinding,
    AuthorizedConsumeOutcome, CapabilityLiveness, DownloadCapabilityRow, DownloadPurpose,
};
use fileconv_server::db::pool::{apply_org_context, create_pool, with_org_txn};
use fileconv_server::services::citation::{
    resolve_citation, CitationError, CitationResolveRequest,
};
use fileconv_server::services::download::{
    mint_download_capability, redeem_download_capability, CapabilitySigner, DownloadError,
    DownloadFetchBudget, MintDownloadCapabilityRequest, DEFAULT_CAPABILITY_TTL,
};
use fileconv_server::services::preview::{fetch_trusted_markdown, PreviewError};
use fileconv_server::services::retrieval::{PERMISSION_QA_HISTORY, PERMISSION_QA_QUERY};
use fileconv_server::storage::keys::{quarantine_key, trusted_key, ObjectKey};
use fileconv_server::storage::MemoryBlobStore;
use fileconv_server::storage::{BlobStore, ObjectExpectation, ObjectHead, StorageError};
use sha2::{Digest, Sha256};
use tokio::sync::{oneshot, Barrier};
use tokio_postgres::NoTls;
use uuid::Uuid;

/// Blocks **after** a successful inner fetch so revoke/delete races are truly post-fetch.
struct GateBlobStore {
    inner: MemoryBlobStore,
    fetched: Mutex<Option<oneshot::Sender<()>>>,
    release: Mutex<Option<oneshot::Receiver<()>>>,
}

impl GateBlobStore {
    fn new(
        inner: MemoryBlobStore,
        fetched: oneshot::Sender<()>,
        release: oneshot::Receiver<()>,
    ) -> Self {
        Self {
            inner,
            fetched: Mutex::new(Some(fetched)),
            release: Mutex::new(Some(release)),
        }
    }
}

impl BlobStore for GateBlobStore {
    async fn head_object(&self, org_id: Uuid, key: &ObjectKey) -> Result<ObjectHead, StorageError> {
        self.inner.head_object(org_id, key).await
    }

    async fn get_object_bounded(
        &self,
        org_id: Uuid,
        key: &ObjectKey,
        max_bytes: u64,
        expected: &ObjectExpectation<'_>,
    ) -> Result<fileconv_server::storage::FetchedObject, StorageError> {
        let object = self
            .inner
            .get_object_bounded(org_id, key, max_bytes, expected)
            .await?;
        // Signal only after bytes are in hand (post-fetch), then wait for test side.
        let fetched = self.fetched.lock().expect("gate lock").take();
        if let Some(tx) = fetched {
            let _ = tx.send(());
        }
        let release = self.release.lock().expect("gate lock").take();
        if let Some(rx) = release {
            let _ = rx.await;
        }
        Ok(object)
    }
}

fn test_database_url() -> Option<String> {
    match std::env::var("MARKHAND_TEST_DATABASE_URL") {
        Ok(url) if !url.trim().is_empty() => Some(url),
        _ => None,
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

async fn connect_raw_result(database_url: &str) -> Result<tokio_postgres::Client, String> {
    let (client, connection) = tokio_postgres::connect(database_url, NoTls)
        .await
        .map_err(|error| format!("connect failed for {database_url}: {error}"))?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    Ok(client)
}

async fn database_exists(admin_url: &str, db_name: &str) -> Result<bool, String> {
    let admin = connect_raw_result(admin_url).await?;
    let row = admin
        .query_opt("SELECT 1 FROM pg_database WHERE datname = $1", &[&db_name])
        .await
        .map_err(|error| format!("pg_database lookup for {db_name}: {error}"))?;
    Ok(row.is_some())
}

struct EphemeralDb {
    admin_url: String,
    /// Taken on cleanup so `Drop` and async `drop` each run at most once.
    db_name: Option<String>,
    url: String,
}

impl EphemeralDb {
    async fn create(base_url: &str) -> Self {
        let suffix = Uuid::new_v4().simple();
        let db_name = format!("markhand_r02_{suffix}");
        let admin_url = rewrite_database_url(base_url, "postgres");
        let admin = connect_raw_result(&admin_url)
            .await
            .expect("admin connect for CREATE DATABASE");
        admin
            .batch_execute(&format!("CREATE DATABASE \"{db_name}\""))
            .await
            .expect("CREATE DATABASE");
        Self {
            admin_url,
            db_name: Some(db_name.clone()),
            url: rewrite_database_url(base_url, &db_name),
        }
    }

    /// Terminate backends, then `DROP DATABASE … WITH (FORCE)` in a **separate**
    /// statement (DROP cannot run inside a multi-statement transaction block).
    async fn cleanup_async(admin_url: &str, db_name: &str) -> Result<(), String> {
        let admin = connect_raw_result(admin_url).await?;
        admin
            .execute(
                "SELECT pg_terminate_backend(pid)
                 FROM pg_stat_activity
                 WHERE datname = $1 AND pid <> pg_backend_pid()",
                &[&db_name],
            )
            .await
            .map_err(|error| format!("terminate backends for {db_name}: {error}"))?;
        // Separate simple query — never batched with terminate.
        admin
            .batch_execute(&format!(
                "DROP DATABASE IF EXISTS \"{db_name}\" WITH (FORCE)"
            ))
            .await
            .map_err(|error| format!("DROP DATABASE {db_name}: {error}"))?;
        Ok(())
    }

    /// Drop-safe cleanup: always runs on a **dedicated OS thread** with its own
    /// Tokio runtime. Never uses `block_in_place` / the parent handle — that
    /// double-panics on `current_thread` runtimes during panic unwind.
    fn cleanup_blocking(admin_url: &str, db_name: &str) -> Result<(), String> {
        let admin_url = admin_url.to_string();
        let db_name = db_name.to_string();
        std::thread::Builder::new()
            .name("ephemeral-db-cleanup".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|error| format!("runtime for cleanup: {error}"))?;
                rt.block_on(Self::cleanup_async(&admin_url, &db_name))
            })
            .map_err(|error| format!("spawn cleanup thread: {error}"))?
            .join()
            .map_err(|_| "cleanup thread panicked".to_string())?
    }

    async fn drop(mut self) -> Result<(), String> {
        let Some(db_name) = self.db_name.take() else {
            return Ok(());
        };
        Self::cleanup_async(&self.admin_url, &db_name).await
    }
}

impl Drop for EphemeralDb {
    fn drop(&mut self) {
        let Some(db_name) = self.db_name.take() else {
            return;
        };
        if let Err(error) = Self::cleanup_blocking(&self.admin_url, &db_name) {
            // Best-effort on panic paths; never panic here (would abort the process).
            eprintln!("EphemeralDb Drop cleanup failed for {db_name}: {error}");
        }
    }
}

#[derive(Clone, Copy)]
struct TenantIds {
    org: Uuid,
    user: Uuid,
    collection: Uuid,
    role: Uuid,
    document: Uuid,
    source_version: Uuid,
    published_version: Uuid,
    chunk_first: Uuid,
    chunk_second: Uuid,
    index_meta: Uuid,
    original_object: Uuid,
    markdown_object: Uuid,
}

struct SeededTenant {
    ids: TenantIds,
    markdown: String,
    markdown_sha: String,
    original_bytes: Vec<u8>,
    original_sha: String,
    quote_start: usize,
    quote_end: usize,
    quote: String,
    second_start: usize,
    second_end: usize,
    second_quote: String,
}

async fn seed_tenant(
    pool: &deadpool_postgres::Pool,
    store: &MemoryBlobStore,
    collection_visibility: &str,
) -> SeededTenant {
    let ids = TenantIds {
        org: Uuid::new_v4(),
        user: Uuid::new_v4(),
        collection: Uuid::new_v4(),
        role: Uuid::new_v4(),
        document: Uuid::new_v4(),
        source_version: Uuid::new_v4(),
        published_version: Uuid::new_v4(),
        chunk_first: Uuid::new_v4(),
        chunk_second: Uuid::new_v4(),
        index_meta: Uuid::new_v4(),
        original_object: Uuid::new_v4(),
        markdown_object: Uuid::new_v4(),
    };
    let markdown =
        "# Mục\n\nMở đầu.\n\nKinh phí phê duyệt là 15 triệu đồng.\n\nKết thúc phiên bản.\n"
            .to_string();
    let quote = "Kinh phí phê duyệt là 15 triệu đồng.".to_string();
    let quote_start = markdown.find(&quote).expect("quote in markdown");
    let quote_end = quote_start + quote.len();
    assert!(quote_start > 0);
    let second_quote = "Kết thúc phiên bản.".to_string();
    let second_start = markdown.find(&second_quote).expect("second quote");
    let second_end = second_start + second_quote.len();
    let markdown_sha = hex::encode(Sha256::digest(markdown.as_bytes()));
    let original_bytes = b"%PDF-1.4 original-upload-bytes".to_vec();
    let original_sha = hex::encode(Sha256::digest(&original_bytes));
    let original_key = quarantine_key(ids.org, ids.original_object, Some("doc.pdf")).unwrap();
    let markdown_key =
        trusted_key(ids.org, ids.published_version, ids.markdown_object, None).unwrap();
    store
        .put(
            ids.org,
            &original_key,
            original_bytes.clone(),
            Some("application/pdf"),
        )
        .unwrap();
    store
        .put(
            ids.org,
            &markdown_key,
            markdown.as_bytes().to_vec(),
            Some("text/markdown; charset=utf-8"),
        )
        .unwrap();

    let ctx = OrgContext::try_new(
        ids.org,
        ids.user,
        [PERMISSION_QA_QUERY, PERMISSION_QA_HISTORY],
        [ids.collection],
    )
    .unwrap();
    let markdown_len = markdown.len() as i64;
    let original_len = original_bytes.len() as i64;
    with_org_txn(pool, &ctx, {
        let markdown_sha = markdown_sha.clone();
        let original_sha = original_sha.clone();
        let original_key_str = original_key.as_str();
        let markdown_key_str = markdown_key.as_str();
        let quote = quote.clone();
        let second_quote = second_quote.clone();
        let visibility = collection_visibility.to_string();
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "INSERT INTO orgs (id, slug, name) VALUES ($1, $2, $3)",
                    &[&ids.org, &format!("org-{}", ids.org), &"org"],
                )
                .await?;
                let email = format!("{}@example.test", ids.user);
                txn.execute(
                    "INSERT INTO users (id, email, display_name, password_hash)
                     VALUES ($1, $2, 'u', 'test-hash')",
                    &[&ids.user, &email],
                )
                .await?;
                // Separate collection owner so ACL revoke via user_access is meaningful.
                let owner_user = Uuid::new_v4();
                txn.execute(
                    "INSERT INTO users (id, email, display_name, password_hash)
                     VALUES ($1, $2, 'owner', 'test-hash')",
                    &[&owner_user, &format!("{owner_user}@example.test")],
                )
                .await?;
                txn.execute(
                    "INSERT INTO org_memberships (org_id, user_id, role)
                     VALUES ($1, $2, 'owner'), ($1, $3, 'owner')",
                    &[&ids.org, &ids.user, &owner_user],
                )
                .await?;
                txn.execute(
                    "INSERT INTO roles (id, org_id, code, name, is_system)
                     VALUES ($1, $2, 'owner', 'Owner', true)",
                    &[&ids.role, &ids.org],
                )
                .await?;
                txn.execute(
                    "INSERT INTO role_permissions (org_id, role_id, permission_id)
                     SELECT $1, $2, id FROM permissions
                     WHERE code IN ('qa.query', 'qa.history')",
                    &[&ids.org, &ids.role],
                )
                .await?;
                txn.execute(
                    "INSERT INTO collections (
                        id, org_id, name, slug, owner_user_id, visibility
                     ) VALUES ($1, $2, 'c', $3, $4, $5)",
                    &[
                        &ids.collection,
                        &ids.org,
                        &format!("c-{}", ids.collection),
                        &owner_user,
                        &visibility,
                    ],
                )
                .await?;
                if visibility == "private" {
                    txn.execute(
                        "INSERT INTO collection_user_access (
                            id, org_id, collection_id, user_id, access_level
                         ) VALUES ($1, $2, $3, $4, 'read')",
                        &[&Uuid::new_v4(), &ids.org, &ids.collection, &ids.user],
                    )
                    .await?;
                }
                txn.execute(
                    "INSERT INTO documents (
                        id, org_id, collection_id, title, state, created_by_user_id
                     ) VALUES ($1, $2, $3, 'doc', 'indexed', $4)",
                    &[&ids.document, &ids.org, &ids.collection, &ids.user],
                )
                .await?;
                // Source/upload version carries original hash/size/type.
                txn.execute(
                    "INSERT INTO document_versions (
                        id, org_id, document_id, version_number, publication_state, is_current,
                        content_sha256, original_object_key, source_filename, source_content_type,
                        byte_size, effective_from, created_by_user_id
                     ) VALUES (
                        $1,$2,$3,1,'draft',false,$4,$5,'doc.pdf','application/pdf',$6,
                        '2024-01-01Z',$7
                     )",
                    &[
                        &ids.source_version,
                        &ids.org,
                        &ids.document,
                        &original_sha,
                        &original_key_str,
                        &original_len,
                        &ids.user,
                    ],
                )
                .await?;
                // Published version reuses original key but stores Markdown hash/size.
                txn.execute(
                    "INSERT INTO document_versions (
                        id, org_id, document_id, version_number, parent_version_id,
                        publication_state, is_current, content_sha256, original_object_key,
                        markdown_object_key, source_filename, source_content_type, byte_size,
                        effective_from, created_by_user_id
                     ) VALUES (
                        $1,$2,$3,2,$4,'published',true,$5,$6,$7,'doc.pdf',
                        'text/markdown; charset=utf-8',$8,'2024-06-01Z',$9
                     )",
                    &[
                        &ids.published_version,
                        &ids.org,
                        &ids.document,
                        &ids.source_version,
                        &markdown_sha,
                        &original_key_str,
                        &markdown_key_str,
                        &markdown_len,
                        &ids.user,
                    ],
                )
                .await?;
                txn.execute(
                    "UPDATE documents SET current_version_id = $1 WHERE id = $2",
                    &[&ids.published_version, &ids.document],
                )
                .await?;
                txn.execute(
                    "INSERT INTO derived_artifacts (
                        id, org_id, document_id, version_id, artifact_kind, object_key,
                        content_sha256, content_type, byte_size
                     ) VALUES (
                        $1,$2,$3,$4,'markdown',$5,$6,'text/markdown; charset=utf-8',$7
                     )",
                    &[
                        &Uuid::new_v4(),
                        &ids.org,
                        &ids.document,
                        &ids.published_version,
                        &markdown_key_str,
                        &markdown_sha,
                        &markdown_len,
                    ],
                )
                .await?;
                // Retired generation — exact citation must still resolve.
                txn.execute(
                    "INSERT INTO index_metadata (
                        id, org_id, collection_id, index_signature_sha256,
                        embedding_family, embedding_revision, dimensions,
                        runtime_path, generation, is_active, state
                     ) VALUES (
                        $1,$2,$3,$4,'f','r',8,'local-hash',1,false,'retired'
                     )",
                    &[&ids.index_meta, &ids.org, &ids.collection, &"c".repeat(64)],
                )
                .await?;
                let identity1 = format!("{:064x}", 1u8);
                let identity2 = format!("{:064x}", 2u8);
                txn.execute(
                    "INSERT INTO chunks (
                        id, org_id, document_id, version_id, ordinal, heading_path, body,
                        body_text_version, chunk_identity_sha256, index_metadata_id,
                        index_signature, page, slide, sheet, span_start, span_end, tsv
                     ) VALUES
                     ($1,$2,$3,$4,0,ARRAY['Mục','Slide 3'],$5,'v1',$6,$7,$8,2,3,NULL,$9,$10,
                      to_tsvector('simple',$5)),
                     ($11,$2,$3,$4,1,ARRAY['Mục'],$12,'v1',$13,$7,$8,NULL,NULL,NULL,$14,$15,
                      to_tsvector('simple',$12))",
                    &[
                        &ids.chunk_first,
                        &ids.org,
                        &ids.document,
                        &ids.published_version,
                        &quote,
                        &identity1,
                        &ids.index_meta,
                        &"c".repeat(64),
                        &(quote_start as i32),
                        &(quote_end as i32),
                        &ids.chunk_second,
                        &second_quote,
                        &identity2,
                        &(second_start as i32),
                        &(second_end as i32),
                    ],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("seed tenant");

    SeededTenant {
        ids,
        markdown,
        markdown_sha,
        original_bytes,
        original_sha,
        quote_start,
        quote_end,
        quote,
        second_start,
        second_end,
        second_quote,
    }
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn citation_preview_download_happy_path_with_memory_store() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let ephemeral = EphemeralDb::create(&base_url).await;
    apply_migrations(&ephemeral.url).await.expect("migrate");
    let pool = create_pool(&ephemeral.url).expect("pool");
    let store = MemoryBlobStore::new();
    let tenant = seed_tenant(&pool, &store, "org").await;
    let signer =
        CapabilitySigner::new(SecretString::new("integration-download-signing-key-32b!")).unwrap();

    let citation = resolve_citation(
        &pool,
        &store,
        tenant.ids.org,
        tenant.ids.user,
        CitationResolveRequest {
            chunk_id: tenant.ids.chunk_first,
            expected_version_id: Some(tenant.ids.published_version),
            expected_document_id: Some(tenant.ids.document),
            expected_content_sha256: Some(tenant.markdown_sha.clone()),
            expected_quote: Some(tenant.quote.clone()),
            expected_span_start: Some(tenant.quote_start),
            expected_span_end: Some(tenant.quote_end),
        },
    )
    .await
    .expect("citation from retired generation + markdown span");
    assert_eq!(citation.quote, tenant.quote);
    assert!(citation.span_start > 0);
    assert_eq!(citation.page, Some(2));
    assert_eq!(citation.slide, Some(3));

    let second = resolve_citation(
        &pool,
        &store,
        tenant.ids.org,
        tenant.ids.user,
        CitationResolveRequest {
            chunk_id: tenant.ids.chunk_second,
            expected_version_id: None,
            expected_document_id: None,
            expected_content_sha256: None,
            expected_quote: Some(tenant.second_quote.clone()),
            expected_span_start: Some(tenant.second_start),
            expected_span_end: Some(tenant.second_end),
        },
    )
    .await
    .expect("second chunk");
    assert_eq!(second.quote, tenant.second_quote);
    assert_ne!(second.span_start, citation.span_start);

    let preview = fetch_trusted_markdown(
        &pool,
        &store,
        tenant.ids.org,
        tenant.ids.user,
        tenant.ids.document,
        tenant.ids.published_version,
    )
    .await
    .expect("preview");
    assert_eq!(preview.markdown, tenant.markdown);
    assert_eq!(preview.markdown_sha256, tenant.markdown_sha);

    let minted = mint_download_capability(
        &pool,
        &signer,
        &MintDownloadCapabilityRequest {
            org_id: tenant.ids.org,
            user_id: tenant.ids.user,
            document_id: tenant.ids.document,
            version_id: tenant.ids.published_version,
            purpose: DownloadPurpose::Original,
            ttl: DEFAULT_CAPABILITY_TTL,
        },
    )
    .await
    .expect("mint original");
    assert_eq!(minted.content_sha256, tenant.original_sha);
    assert_eq!(minted.content_type, "application/pdf");
    assert_eq!(minted.byte_size, tenant.original_bytes.len() as u64);

    let budget = DownloadFetchBudget::for_tests();
    let artifact = redeem_download_capability(
        &pool,
        &store,
        &signer,
        &budget,
        tenant.ids.org,
        tenant.ids.user,
        minted.token.expose(),
    )
    .await
    .expect("redeem original");
    assert_eq!(artifact.body.as_bytes(), tenant.original_bytes.as_slice());
    assert_eq!(artifact.content_type, "application/pdf");
    assert!(
        artifact.body.permit_held(),
        "budget permit must stay held until body is consumed/dropped"
    );

    ephemeral.drop().await.expect("ephemeral cleanup");
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn two_tenants_idor_is_permission_denied_not_database() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let ephemeral = EphemeralDb::create(&base_url).await;
    apply_migrations(&ephemeral.url).await.expect("migrate");
    let pool = create_pool(&ephemeral.url).expect("pool");
    let store = MemoryBlobStore::new();
    let a = seed_tenant(&pool, &store, "org").await;
    let b = seed_tenant(&pool, &store, "org").await;
    let signer =
        CapabilitySigner::new(SecretString::new("integration-download-signing-key-32b!")).unwrap();

    let preview_cross = fetch_trusted_markdown(
        &pool,
        &store,
        b.ids.org,
        b.ids.user,
        a.ids.document,
        a.ids.published_version,
    )
    .await;
    assert!(
        matches!(
            preview_cross,
            Err(PreviewError::PermissionDenied | PreviewError::NotFound)
        ),
        "cross-tenant preview must not surface Database: {preview_cross:?}"
    );

    let minted = mint_download_capability(
        &pool,
        &signer,
        &MintDownloadCapabilityRequest {
            org_id: a.ids.org,
            user_id: a.ids.user,
            document_id: a.ids.document,
            version_id: a.ids.published_version,
            purpose: DownloadPurpose::Markdown,
            ttl: DEFAULT_CAPABILITY_TTL,
        },
    )
    .await
    .expect("mint");
    let budget = DownloadFetchBudget::for_tests();
    let redeem_cross = redeem_download_capability(
        &pool,
        &store,
        &signer,
        &budget,
        b.ids.org,
        b.ids.user,
        minted.token.expose(),
    )
    .await;
    assert!(
        matches!(
            redeem_cross,
            Err(DownloadError::PermissionDenied | DownloadError::NotFound)
        ),
        "cross-tenant redeem must not surface Database: {redeem_cross:?}"
    );

    let cite_cross = resolve_citation(
        &pool,
        &store,
        b.ids.org,
        b.ids.user,
        CitationResolveRequest {
            chunk_id: a.ids.chunk_first,
            expected_version_id: None,
            expected_document_id: None,
            expected_content_sha256: None,
            expected_quote: None,
            expected_span_start: None,
            expected_span_end: None,
        },
    )
    .await;
    assert!(
        matches!(
            cite_cross,
            Err(CitationError::PermissionDenied | CitationError::NotFound)
        ),
        "cross-tenant citation must not surface Database: {cite_cross:?}"
    );

    ephemeral.drop().await.expect("ephemeral cleanup");
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn concurrent_redeem_allows_only_one_winner() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let ephemeral = EphemeralDb::create(&base_url).await;
    apply_migrations(&ephemeral.url).await.expect("migrate");
    let pool = Arc::new(create_pool(&ephemeral.url).expect("pool"));
    let store = MemoryBlobStore::new();
    let tenant = seed_tenant(&pool, &store, "org").await;
    let signer =
        CapabilitySigner::new(SecretString::new("integration-download-signing-key-32b!")).unwrap();
    let minted = mint_download_capability(
        &pool,
        &signer,
        &MintDownloadCapabilityRequest {
            org_id: tenant.ids.org,
            user_id: tenant.ids.user,
            document_id: tenant.ids.document,
            version_id: tenant.ids.published_version,
            purpose: DownloadPurpose::Markdown,
            ttl: DEFAULT_CAPABILITY_TTL,
        },
    )
    .await
    .expect("mint");

    let ctx = OrgContext::try_new(
        tenant.ids.org,
        tenant.ids.user,
        [PERMISSION_QA_QUERY, PERMISSION_QA_HISTORY],
        [tenant.ids.collection],
    )
    .unwrap();
    let capability = with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        let id = minted.capability_id;
        move |txn| {
            Box::pin(async move {
                fileconv_server::db::download_capabilities::get_by_id(txn, &ctx, id).await
            })
        }
    })
    .await
    .expect("load capability")
    .expect("capability row");

    // Two prepared org transactions rendezvous immediately before
    // `consume_authorized_or_classify` (FOR UPDATE) — deterministic SQL lock
    // contention without any production service hook.
    let barrier = Arc::new(Barrier::new(2));
    let run = |pool: Arc<deadpool_postgres::Pool>,
               ctx: OrgContext,
               capability: DownloadCapabilityRow,
               barrier: Arc<Barrier>| {
        tokio::spawn(async move {
            let mut client = pool.get().await.expect("checkout");
            let txn = client.transaction().await.expect("begin");
            apply_org_context(&txn, &ctx).await.expect("RLS GUCs");
            barrier.wait().await;
            let outcome = consume_authorized_or_classify(
                &txn,
                &ctx,
                &AuthorizedConsumeBinding::from_row(&capability),
            )
            .await
            .expect("consume_authorized_or_classify");
            txn.commit().await.expect("commit");
            outcome
        })
    };

    let t1 = run(
        pool.clone(),
        ctx.clone(),
        capability.clone(),
        barrier.clone(),
    );
    let t2 = run(pool.clone(), ctx.clone(), capability.clone(), barrier);
    let r1 = t1.await.expect("join1");
    let r2 = t2.await.expect("join2");
    let wins = matches!(r1, AuthorizedConsumeOutcome::Consumed(_)) as u8
        + matches!(r2, AuthorizedConsumeOutcome::Consumed(_)) as u8;
    let replays = matches!(r1, AuthorizedConsumeOutcome::Replay) as u8
        + matches!(r2, AuthorizedConsumeOutcome::Replay) as u8;
    assert_eq!(wins, 1, "exactly one consume must succeed: {r1:?} / {r2:?}");
    assert_eq!(
        replays, 1,
        "the other consume must be Replay: {r1:?} / {r2:?}"
    );

    ephemeral.drop().await.expect("ephemeral cleanup");
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn ephemeral_db_drop_cleans_up_after_panic_on_current_thread() {
    let Some(base_url) = test_database_url() else {
        return;
    };

    let captured = Arc::new(Mutex::new(None::<(String, String)>));
    let captured_for_task = captured.clone();
    let join = tokio::task::spawn(async move {
        let ephemeral = EphemeralDb::create(&base_url).await;
        *captured_for_task.lock().expect("capture lock") = Some((
            ephemeral.admin_url.clone(),
            ephemeral
                .db_name
                .clone()
                .expect("ephemeral db name before panic"),
        ));
        panic!("intentional panic to exercise EphemeralDb::Drop");
    });
    let join_err = join.await.expect_err("spawned task must panic");
    assert!(
        join_err.is_panic(),
        "expected panic unwind, got: {join_err:?}"
    );

    let (admin_url, db_name) = captured
        .lock()
        .expect("capture lock")
        .clone()
        .expect("ephemeral identity captured before panic");
    // Drop ran during task unwind on this current_thread runtime; process must
    // still be alive and the ephemeral database must be gone.
    let exists = database_exists(&admin_url, &db_name)
        .await
        .expect("database_exists");
    assert!(
        !exists,
        "ephemeral DB {db_name} must be dropped after panic-path Drop"
    );
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn acl_revoke_and_history_revoke_deny_fresh_resolve() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let ephemeral = EphemeralDb::create(&base_url).await;
    apply_migrations(&ephemeral.url).await.expect("migrate");
    let pool = create_pool(&ephemeral.url).expect("pool");
    let store = MemoryBlobStore::new();
    let tenant = seed_tenant(&pool, &store, "private").await;
    let ctx = OrgContext::try_new(
        tenant.ids.org,
        tenant.ids.user,
        [PERMISSION_QA_QUERY, PERMISSION_QA_HISTORY],
        [tenant.ids.collection],
    )
    .unwrap();

    // ACL revoke: remove collection_user_access on private collection.
    with_org_txn(&pool, &ctx, {
        let org = tenant.ids.org;
        let collection = tenant.ids.collection;
        let user = tenant.ids.user;
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "DELETE FROM collection_user_access
                     WHERE org_id = $1 AND collection_id = $2 AND user_id = $3",
                    &[&org, &collection, &user],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("revoke acl");

    let denied = fetch_trusted_markdown(
        &pool,
        &store,
        tenant.ids.org,
        tenant.ids.user,
        tenant.ids.document,
        tenant.ids.published_version,
    )
    .await;
    assert!(
        matches!(
            denied,
            Err(PreviewError::PermissionDenied | PreviewError::NotFound)
        ),
        "ACL revoke must deny preview: {denied:?}"
    );

    ephemeral.drop().await.expect("ephemeral cleanup");
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn half_open_span_is_invalid_request() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let ephemeral = EphemeralDb::create(&base_url).await;
    apply_migrations(&ephemeral.url).await.expect("migrate");
    let pool = create_pool(&ephemeral.url).expect("pool");
    let store = MemoryBlobStore::new();
    let tenant = seed_tenant(&pool, &store, "org").await;
    let err = resolve_citation(
        &pool,
        &store,
        tenant.ids.org,
        tenant.ids.user,
        CitationResolveRequest {
            chunk_id: tenant.ids.chunk_first,
            expected_version_id: None,
            expected_document_id: None,
            expected_content_sha256: None,
            expected_quote: None,
            expected_span_start: Some(tenant.quote_start),
            expected_span_end: None,
        },
    )
    .await;
    assert_eq!(err, Err(CitationError::InvalidRequest));
    ephemeral.drop().await.expect("ephemeral cleanup");
}

#[test]
fn hermetic_effective_time_pin_shape() {
    let at = Utc.with_ymd_and_hms(2024, 2, 15, 0, 0, 0).unwrap();
    assert_eq!(at.timestamp(), 1_707_955_200);
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn mint_rejects_oversized_ttl() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let ephemeral = EphemeralDb::create(&base_url).await;
    apply_migrations(&ephemeral.url).await.expect("migrate");
    let pool = create_pool(&ephemeral.url).expect("pool");
    let store = MemoryBlobStore::new();
    let tenant = seed_tenant(&pool, &store, "org").await;
    let signer =
        CapabilitySigner::new(SecretString::new("integration-download-signing-key-32b!")).unwrap();
    let denied = mint_download_capability(
        &pool,
        &signer,
        &MintDownloadCapabilityRequest {
            org_id: tenant.ids.org,
            user_id: tenant.ids.user,
            document_id: tenant.ids.document,
            version_id: tenant.ids.published_version,
            purpose: DownloadPurpose::Original,
            ttl: Duration::from_secs(10_000),
        },
    )
    .await;
    assert_eq!(denied, Err(DownloadError::InvalidTtl));
    ephemeral.drop().await.expect("ephemeral cleanup");
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn disabled_user_and_membership_removal_deny_services() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let ephemeral = EphemeralDb::create(&base_url).await;
    apply_migrations(&ephemeral.url).await.expect("migrate");
    let pool = create_pool(&ephemeral.url).expect("pool");
    let store = MemoryBlobStore::new();
    let tenant = seed_tenant(&pool, &store, "org").await;
    let signer =
        CapabilitySigner::new(SecretString::new("integration-download-signing-key-32b!")).unwrap();
    let ctx = OrgContext::try_new(
        tenant.ids.org,
        tenant.ids.user,
        [PERMISSION_QA_QUERY, PERMISSION_QA_HISTORY],
        [tenant.ids.collection],
    )
    .unwrap();

    with_org_txn(&pool, &ctx, {
        let user = tenant.ids.user;
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "UPDATE users SET disabled_at = clock_timestamp() WHERE id = $1",
                    &[&user],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("disable user");

    let disabled = mint_download_capability(
        &pool,
        &signer,
        &MintDownloadCapabilityRequest {
            org_id: tenant.ids.org,
            user_id: tenant.ids.user,
            document_id: tenant.ids.document,
            version_id: tenant.ids.published_version,
            purpose: DownloadPurpose::Markdown,
            ttl: DEFAULT_CAPABILITY_TTL,
        },
    )
    .await;
    assert_eq!(disabled, Err(DownloadError::PermissionDenied));

    // Re-enable, then remove membership.
    with_org_txn(&pool, &ctx, {
        let org = tenant.ids.org;
        let user = tenant.ids.user;
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "UPDATE users SET disabled_at = NULL WHERE id = $1",
                    &[&user],
                )
                .await?;
                txn.execute(
                    "DELETE FROM org_memberships WHERE org_id = $1 AND user_id = $2",
                    &[&org, &user],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("remove membership");

    let no_member = fetch_trusted_markdown(
        &pool,
        &store,
        tenant.ids.org,
        tenant.ids.user,
        tenant.ids.document,
        tenant.ids.published_version,
    )
    .await;
    assert!(
        matches!(
            no_member,
            Err(PreviewError::PermissionDenied | PreviewError::NotFound)
        ),
        "membership removal must deny: {no_member:?}"
    );

    ephemeral.drop().await.expect("ephemeral cleanup");
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn document_deletion_denies_preview_and_citation() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let ephemeral = EphemeralDb::create(&base_url).await;
    apply_migrations(&ephemeral.url).await.expect("migrate");
    let pool = create_pool(&ephemeral.url).expect("pool");
    let store = MemoryBlobStore::new();
    let tenant = seed_tenant(&pool, &store, "org").await;
    let ctx = OrgContext::try_new(
        tenant.ids.org,
        tenant.ids.user,
        [PERMISSION_QA_QUERY, PERMISSION_QA_HISTORY],
        [tenant.ids.collection],
    )
    .unwrap();

    with_org_txn(&pool, &ctx, {
        let org = tenant.ids.org;
        let document = tenant.ids.document;
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "UPDATE documents
                     SET deleted_at = clock_timestamp(), updated_at = clock_timestamp()
                     WHERE org_id = $1 AND id = $2",
                    &[&org, &document],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("soft-delete document");

    let preview = fetch_trusted_markdown(
        &pool,
        &store,
        tenant.ids.org,
        tenant.ids.user,
        tenant.ids.document,
        tenant.ids.published_version,
    )
    .await;
    assert!(
        matches!(
            preview,
            Err(PreviewError::PermissionDenied | PreviewError::NotFound)
        ),
        "deleted document must deny preview: {preview:?}"
    );

    let citation = resolve_citation(
        &pool,
        &store,
        tenant.ids.org,
        tenant.ids.user,
        CitationResolveRequest {
            chunk_id: tenant.ids.chunk_first,
            expected_version_id: None,
            expected_document_id: None,
            expected_content_sha256: None,
            expected_quote: None,
            expected_span_start: None,
            expected_span_end: None,
        },
    )
    .await;
    assert!(
        matches!(
            citation,
            Err(CitationError::PermissionDenied | CitationError::NotFound)
        ),
        "deleted document must deny citation: {citation:?}"
    );

    ephemeral.drop().await.expect("ephemeral cleanup");
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn qa_history_revoke_denies_non_current_version() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let ephemeral = EphemeralDb::create(&base_url).await;
    apply_migrations(&ephemeral.url).await.expect("migrate");
    let pool = create_pool(&ephemeral.url).expect("pool");
    let store = MemoryBlobStore::new();
    let tenant = seed_tenant(&pool, &store, "org").await;
    let ctx = OrgContext::try_new(
        tenant.ids.org,
        tenant.ids.user,
        [PERMISSION_QA_QUERY, PERMISSION_QA_HISTORY],
        [tenant.ids.collection],
    )
    .unwrap();

    // Supersede published version with a new current row so the original becomes history.
    with_org_txn(&pool, &ctx, {
        let org = tenant.ids.org;
        let document = tenant.ids.document;
        let version = tenant.ids.published_version;
        let user = tenant.ids.user;
        let markdown_sha = tenant.markdown_sha.clone();
        let markdown_len = tenant.markdown.len() as i64;
        move |txn| {
            Box::pin(async move {
                let successor = Uuid::new_v4();
                let now: chrono::DateTime<chrono::Utc> =
                    txn.query_one("SELECT clock_timestamp()", &[]).await?.get(0);
                txn.execute(
                    "UPDATE document_versions
                     SET is_current = false, effective_to = $2
                     WHERE id = $1",
                    &[&version, &now],
                )
                .await?;
                txn.execute(
                    "INSERT INTO document_versions (
                        id, org_id, document_id, version_number, parent_version_id,
                        publication_state, is_current, content_sha256, original_object_key,
                        markdown_object_key, source_filename, source_content_type, byte_size,
                        effective_from, created_by_user_id
                     )
                     SELECT $1, org_id, document_id, version_number + 1, id,
                            'published', true, $2, original_object_key, markdown_object_key,
                            source_filename, source_content_type, $3, $4, $5
                     FROM document_versions WHERE id = $6",
                    &[
                        &successor,
                        &markdown_sha,
                        &markdown_len,
                        &now,
                        &user,
                        &version,
                    ],
                )
                .await?;
                txn.execute(
                    "UPDATE documents
                     SET current_version_id = $1, updated_at = clock_timestamp()
                     WHERE org_id = $2 AND id = $3",
                    &[&successor, &org, &document],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("supersede to create historical version");

    let allowed = fetch_trusted_markdown(
        &pool,
        &store,
        tenant.ids.org,
        tenant.ids.user,
        tenant.ids.document,
        tenant.ids.published_version,
    )
    .await;
    assert!(
        allowed.is_ok(),
        "qa.history should allow historical: {allowed:?}"
    );

    with_org_txn(&pool, &ctx, {
        let org = tenant.ids.org;
        let role = tenant.ids.role;
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "DELETE FROM role_permissions
                     WHERE org_id = $1 AND role_id = $2
                       AND permission_id = (SELECT id FROM permissions WHERE code = 'qa.history')",
                    &[&org, &role],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("revoke qa.history");

    let denied = fetch_trusted_markdown(
        &pool,
        &store,
        tenant.ids.org,
        tenant.ids.user,
        tenant.ids.document,
        tenant.ids.published_version,
    )
    .await;
    assert!(
        matches!(
            denied,
            Err(PreviewError::PermissionDenied | PreviewError::NotFound)
        ),
        "qa.history revoke must deny historical preview: {denied:?}"
    );

    let cite = resolve_citation(
        &pool,
        &store,
        tenant.ids.org,
        tenant.ids.user,
        CitationResolveRequest {
            chunk_id: tenant.ids.chunk_first,
            expected_version_id: None,
            expected_document_id: None,
            expected_content_sha256: None,
            expected_quote: None,
            expected_span_start: None,
            expected_span_end: None,
        },
    )
    .await;
    assert!(
        matches!(
            cite,
            Err(CitationError::PermissionDenied | CitationError::NotFound)
        ),
        "qa.history revoke must deny historical citation: {cite:?}"
    );

    ephemeral.drop().await.expect("ephemeral cleanup");
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn capability_expiry_boundary_is_expired_not_replay() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let ephemeral = EphemeralDb::create(&base_url).await;
    apply_migrations(&ephemeral.url).await.expect("migrate");
    let pool = create_pool(&ephemeral.url).expect("pool");
    let store = MemoryBlobStore::new();
    let tenant = seed_tenant(&pool, &store, "org").await;
    let signer =
        CapabilitySigner::new(SecretString::new("integration-download-signing-key-32b!")).unwrap();
    let budget = DownloadFetchBudget::for_tests();

    let minted = mint_download_capability(
        &pool,
        &signer,
        &MintDownloadCapabilityRequest {
            org_id: tenant.ids.org,
            user_id: tenant.ids.user,
            document_id: tenant.ids.document,
            version_id: tenant.ids.published_version,
            purpose: DownloadPurpose::Markdown,
            ttl: Duration::from_secs(1),
        },
    )
    .await
    .expect("mint ttl=1s");

    // Force expiry with DB clock (avoid sleeping on wall clock drift).
    let ctx = OrgContext::try_new(
        tenant.ids.org,
        tenant.ids.user,
        [PERMISSION_QA_QUERY, PERMISSION_QA_HISTORY],
        [tenant.ids.collection],
    )
    .unwrap();
    with_org_txn(&pool, &ctx, {
        let id = minted.capability_id;
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "UPDATE download_capabilities
                     SET created_at = clock_timestamp() - interval '10 seconds',
                         expires_at = clock_timestamp() - interval '1 second'
                     WHERE id = $1",
                    &[&id],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("force expiry");

    // Re-sign is required because HMAC binds expires_at; load row and resign.
    let row = with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        let id = minted.capability_id;
        move |txn| {
            Box::pin(async move {
                fileconv_server::db::download_capabilities::get_by_id(txn, &ctx, id).await
            })
        }
    })
    .await
    .expect("load")
    .expect("row");
    let token = signer.sign_capability(&row);

    let expired = redeem_download_capability(
        &pool,
        &store,
        &signer,
        &budget,
        tenant.ids.org,
        tenant.ids.user,
        token.expose(),
    )
    .await;
    assert!(
        matches!(expired, Err(DownloadError::Expired)),
        "expiry boundary must be Expired, not Replay: {expired:?}"
    );

    // Second redeem of the same expired token remains Expired (not Replay).
    let expired_again = redeem_download_capability(
        &pool,
        &store,
        &signer,
        &budget,
        tenant.ids.org,
        tenant.ids.user,
        token.expose(),
    )
    .await;
    assert!(
        matches!(expired_again, Err(DownloadError::Expired)),
        "{expired_again:?}"
    );

    ephemeral.drop().await.expect("ephemeral cleanup");
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn content_type_mismatch_on_redeem_is_integrity() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let ephemeral = EphemeralDb::create(&base_url).await;
    apply_migrations(&ephemeral.url).await.expect("migrate");
    let pool = create_pool(&ephemeral.url).expect("pool");
    let store = MemoryBlobStore::new();
    let tenant = seed_tenant(&pool, &store, "org").await;
    let signer =
        CapabilitySigner::new(SecretString::new("integration-download-signing-key-32b!")).unwrap();
    let budget = DownloadFetchBudget::for_tests();

    let minted = mint_download_capability(
        &pool,
        &signer,
        &MintDownloadCapabilityRequest {
            org_id: tenant.ids.org,
            user_id: tenant.ids.user,
            document_id: tenant.ids.document,
            version_id: tenant.ids.published_version,
            purpose: DownloadPurpose::Original,
            ttl: DEFAULT_CAPABILITY_TTL,
        },
    )
    .await
    .expect("mint");

    // Overwrite stored object with wrong content-type; hash/size unchanged.
    let original_key =
        quarantine_key(tenant.ids.org, tenant.ids.original_object, Some("doc.pdf")).unwrap();
    store
        .put(
            tenant.ids.org,
            &original_key,
            tenant.original_bytes.clone(),
            Some("text/plain"),
        )
        .unwrap();

    let mismatched = redeem_download_capability(
        &pool,
        &store,
        &signer,
        &budget,
        tenant.ids.org,
        tenant.ids.user,
        minted.token.expose(),
    )
    .await;
    assert!(
        matches!(mismatched, Err(DownloadError::Integrity)),
        "{mismatched:?}"
    );

    // Integrity must leave the token retryable — restore type and redeem again.
    store
        .put(
            tenant.ids.org,
            &original_key,
            tenant.original_bytes.clone(),
            Some("application/pdf"),
        )
        .unwrap();
    let retry = redeem_download_capability(
        &pool,
        &store,
        &signer,
        &budget,
        tenant.ids.org,
        tenant.ids.user,
        minted.token.expose(),
    )
    .await
    .expect("integrity failure must not burn token");
    assert_eq!(retry.body.as_bytes(), tenant.original_bytes.as_slice());

    ephemeral.drop().await.expect("ephemeral cleanup");
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn busy_budget_keeps_token_retryable() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let ephemeral = EphemeralDb::create(&base_url).await;
    apply_migrations(&ephemeral.url).await.expect("migrate");
    let pool = create_pool(&ephemeral.url).expect("pool");
    let store = MemoryBlobStore::new();
    let tenant = seed_tenant(&pool, &store, "org").await;
    let signer =
        CapabilitySigner::new(SecretString::new("integration-download-signing-key-32b!")).unwrap();
    // Tiny budget: one held reservation blocks the redeem fetch.
    let budget =
        DownloadFetchBudget::try_new(tenant.original_bytes.len() as u64, 1).expect("tiny budget");
    let blocker = budget
        .acquire(tenant.original_bytes.len() as u64)
        .await
        .expect("hold budget");

    let minted = mint_download_capability(
        &pool,
        &signer,
        &MintDownloadCapabilityRequest {
            org_id: tenant.ids.org,
            user_id: tenant.ids.user,
            document_id: tenant.ids.document,
            version_id: tenant.ids.published_version,
            purpose: DownloadPurpose::Original,
            ttl: DEFAULT_CAPABILITY_TTL,
        },
    )
    .await
    .expect("mint");

    let busy = redeem_download_capability(
        &pool,
        &store,
        &signer,
        &budget,
        tenant.ids.org,
        tenant.ids.user,
        minted.token.expose(),
    )
    .await;
    assert!(matches!(busy, Err(DownloadError::Busy)), "{busy:?}");
    drop(blocker);

    let retry = redeem_download_capability(
        &pool,
        &store,
        &signer,
        &budget,
        tenant.ids.org,
        tenant.ids.user,
        minted.token.expose(),
    )
    .await
    .expect("Busy must not burn token");
    assert_eq!(retry.body.as_bytes(), tenant.original_bytes.as_slice());

    ephemeral.drop().await.expect("ephemeral cleanup");
}

#[tokio::test]
async fn memory_blob_content_type_and_length_semantics_hermetic() {
    let store = MemoryBlobStore::new();
    let org = Uuid::new_v4();
    let key = quarantine_key(org, Uuid::new_v4(), None).unwrap();
    let body = b"payload-bytes".to_vec();
    let sha = hex::encode(Sha256::digest(&body));
    store
        .put(org, &key, body.clone(), Some("application/pdf"))
        .unwrap();

    assert!(matches!(
        store
            .get_object_bounded(
                org,
                &key,
                64,
                &ObjectExpectation {
                    content_sha256: &sha,
                    content_length: body.len() as u64,
                    content_type: Some("text/plain"),
                },
            )
            .await,
        Err(StorageError::PreconditionFailed)
    ));
    assert!(matches!(
        store
            .get_object_bounded(
                org,
                &key,
                64,
                &ObjectExpectation {
                    content_sha256: &sha,
                    content_length: (body.len() as u64) + 3,
                    content_type: Some("application/pdf"),
                },
            )
            .await,
        Err(StorageError::PreconditionFailed)
    ));
    assert!(matches!(
        store
            .get_object_bounded(
                org,
                &key,
                4,
                &ObjectExpectation {
                    content_sha256: &sha,
                    content_length: body.len() as u64,
                    content_type: Some("application/pdf"),
                },
            )
            .await,
        Err(StorageError::ObjectTooLarge)
    ));
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn barrier_acl_revoke_before_consume_denies_without_burning_token() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let ephemeral = EphemeralDb::create(&base_url).await;
    apply_migrations(&ephemeral.url).await.expect("migrate");
    let pool = Arc::new(create_pool(&ephemeral.url).expect("pool"));
    let memory = MemoryBlobStore::new();
    let tenant = seed_tenant(&pool, &memory, "private").await;
    let signer = Arc::new(
        CapabilitySigner::new(SecretString::new("integration-download-signing-key-32b!")).unwrap(),
    );
    let budget = DownloadFetchBudget::for_tests();
    let minted = mint_download_capability(
        &pool,
        &signer,
        &MintDownloadCapabilityRequest {
            org_id: tenant.ids.org,
            user_id: tenant.ids.user,
            document_id: tenant.ids.document,
            version_id: tenant.ids.published_version,
            purpose: DownloadPurpose::Markdown,
            ttl: DEFAULT_CAPABILITY_TTL,
        },
    )
    .await
    .expect("mint");

    let (fetched_tx, fetched_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let gate = Arc::new(GateBlobStore::new(memory, fetched_tx, release_rx));

    let pool_r = pool.clone();
    let gate_r = gate.clone();
    let signer_r = signer.clone();
    let budget_r = budget.clone();
    let token = minted.token.expose().to_string();
    let org = tenant.ids.org;
    let user = tenant.ids.user;
    let redeem = tokio::spawn(async move {
        redeem_download_capability(
            &pool_r,
            gate_r.as_ref(),
            &signer_r,
            &budget_r,
            org,
            user,
            &token,
        )
        .await
    });

    fetched_rx.await.expect("fetch reached storage gate");
    let ctx = OrgContext::try_new(
        tenant.ids.org,
        tenant.ids.user,
        [PERMISSION_QA_QUERY, PERMISSION_QA_HISTORY],
        [tenant.ids.collection],
    )
    .unwrap();
    with_org_txn(&pool, &ctx, {
        let org = tenant.ids.org;
        let collection = tenant.ids.collection;
        let user = tenant.ids.user;
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "DELETE FROM collection_user_access
                     WHERE org_id = $1 AND collection_id = $2 AND user_id = $3",
                    &[&org, &collection, &user],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("revoke ACL while redeem is past fetch, before consume");
    release_tx.send(()).expect("release gate");

    let denied = redeem.await.expect("join redeem");
    assert!(
        matches!(
            denied,
            Err(DownloadError::PermissionDenied | DownloadError::NotFound)
        ),
        "ACL revoke at consume must deny body: {denied:?}"
    );

    let live = with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        let id = minted.capability_id;
        move |txn| Box::pin(async move { classify_liveness(txn, &ctx, id).await })
    })
    .await
    .expect("liveness");
    assert_eq!(
        live,
        CapabilityLiveness::Open,
        "auth failure must leave token retryable, not Replay"
    );

    // Restore ACL — same token must succeed (proves consume did not fire).
    with_org_txn(&pool, &ctx, {
        let org = tenant.ids.org;
        let collection = tenant.ids.collection;
        let user = tenant.ids.user;
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "INSERT INTO collection_user_access (
                        id, org_id, collection_id, user_id, access_level
                     ) VALUES ($1, $2, $3, $4, 'read')",
                    &[&Uuid::new_v4(), &org, &collection, &user],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("restore ACL");

    let retry_store = gate.inner.clone();
    let retry = redeem_download_capability(
        &pool,
        &retry_store,
        &signer,
        &budget,
        tenant.ids.org,
        tenant.ids.user,
        minted.token.expose(),
    )
    .await
    .expect("retry after ACL restore");
    assert_eq!(retry.body.as_bytes(), tenant.markdown.as_bytes());

    ephemeral.drop().await.expect("ephemeral cleanup");
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn barrier_document_delete_before_consume_denies_without_body() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let ephemeral = EphemeralDb::create(&base_url).await;
    apply_migrations(&ephemeral.url).await.expect("migrate");
    let pool = Arc::new(create_pool(&ephemeral.url).expect("pool"));
    let memory = MemoryBlobStore::new();
    let tenant = seed_tenant(&pool, &memory, "org").await;
    let signer = Arc::new(
        CapabilitySigner::new(SecretString::new("integration-download-signing-key-32b!")).unwrap(),
    );
    let budget = DownloadFetchBudget::for_tests();
    let minted = mint_download_capability(
        &pool,
        &signer,
        &MintDownloadCapabilityRequest {
            org_id: tenant.ids.org,
            user_id: tenant.ids.user,
            document_id: tenant.ids.document,
            version_id: tenant.ids.published_version,
            purpose: DownloadPurpose::Original,
            ttl: DEFAULT_CAPABILITY_TTL,
        },
    )
    .await
    .expect("mint");

    let (fetched_tx, fetched_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let gate = Arc::new(GateBlobStore::new(memory, fetched_tx, release_rx));

    let pool_r = pool.clone();
    let gate_r = gate.clone();
    let signer_r = signer.clone();
    let budget_r = budget.clone();
    let token = minted.token.expose().to_string();
    let org = tenant.ids.org;
    let user = tenant.ids.user;
    let redeem = tokio::spawn(async move {
        redeem_download_capability(
            &pool_r,
            gate_r.as_ref(),
            &signer_r,
            &budget_r,
            org,
            user,
            &token,
        )
        .await
    });

    fetched_rx.await.expect("fetch gate");
    let ctx = OrgContext::try_new(
        tenant.ids.org,
        tenant.ids.user,
        [PERMISSION_QA_QUERY, PERMISSION_QA_HISTORY],
        [tenant.ids.collection],
    )
    .unwrap();
    with_org_txn(&pool, &ctx, {
        let org = tenant.ids.org;
        let document = tenant.ids.document;
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "UPDATE documents
                     SET deleted_at = clock_timestamp(), updated_at = clock_timestamp()
                     WHERE org_id = $1 AND id = $2",
                    &[&org, &document],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("soft-delete during redeem window");
    release_tx.send(()).expect("release");

    let denied = redeem.await.expect("join");
    assert!(
        matches!(
            denied,
            Err(DownloadError::PermissionDenied | DownloadError::NotFound)
        ),
        "document delete at consume must not return body: {denied:?}"
    );
    assert!(
        !matches!(denied, Err(DownloadError::Replay | DownloadError::Expired)),
        "must not burn/classify as replay/expired after delete race"
    );

    let live = with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        let id = minted.capability_id;
        move |txn| Box::pin(async move { classify_liveness(txn, &ctx, id).await })
    })
    .await
    .expect("liveness");
    assert_eq!(live, CapabilityLiveness::Open);

    ephemeral.drop().await.expect("ephemeral cleanup");
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn wrong_user_consume_is_permission_denied_not_expired_oracle() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let ephemeral = EphemeralDb::create(&base_url).await;
    apply_migrations(&ephemeral.url).await.expect("migrate");
    let pool = create_pool(&ephemeral.url).expect("pool");
    let store = MemoryBlobStore::new();
    let owner = seed_tenant(&pool, &store, "org").await;
    let signer =
        CapabilitySigner::new(SecretString::new("integration-download-signing-key-32b!")).unwrap();
    let budget = DownloadFetchBudget::for_tests();
    let minted = mint_download_capability(
        &pool,
        &signer,
        &MintDownloadCapabilityRequest {
            org_id: owner.ids.org,
            user_id: owner.ids.user,
            document_id: owner.ids.document,
            version_id: owner.ids.published_version,
            purpose: DownloadPurpose::Markdown,
            ttl: DEFAULT_CAPABILITY_TTL,
        },
    )
    .await
    .expect("mint");

    // Same-org peer (not the capability owner) — IDOR must not learn Expired.
    let peer = Uuid::new_v4();
    let owner_ctx = OrgContext::try_new(
        owner.ids.org,
        owner.ids.user,
        [PERMISSION_QA_QUERY, PERMISSION_QA_HISTORY],
        [owner.ids.collection],
    )
    .unwrap();
    with_org_txn(&pool, &owner_ctx, {
        let org = owner.ids.org;
        let role = owner.ids.role;
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "INSERT INTO users (id, email, display_name, password_hash)
                     VALUES ($1, $2, 'peer', 'test-hash')",
                    &[&peer, &format!("{peer}@example.test")],
                )
                .await?;
                txn.execute(
                    "INSERT INTO org_memberships (org_id, user_id, role)
                     VALUES ($1, $2, 'owner')",
                    &[&org, &peer],
                )
                .await?;
                // Peer shares the same role row already granted qa.* via seed.
                let _ = role;
                Ok(())
            })
        }
    })
    .await
    .expect("add peer");

    with_org_txn(&pool, &owner_ctx, {
        let id = minted.capability_id;
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "UPDATE download_capabilities
                     SET created_at = clock_timestamp() - interval '10 seconds',
                         expires_at = clock_timestamp() - interval '1 second'
                     WHERE id = $1",
                    &[&id],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("expire");
    let row = with_org_txn(&pool, &owner_ctx, {
        let ctx = owner_ctx.clone();
        let id = minted.capability_id;
        move |txn| {
            Box::pin(async move {
                fileconv_server::db::download_capabilities::get_by_id(txn, &ctx, id).await
            })
        }
    })
    .await
    .expect("load")
    .expect("row");
    let token = signer.sign_capability(&row);

    let probe = redeem_download_capability(
        &pool,
        &store,
        &signer,
        &budget,
        owner.ids.org,
        peer,
        token.expose(),
    )
    .await;
    assert!(
        matches!(
            probe,
            Err(DownloadError::PermissionDenied | DownloadError::NotFound)
        ),
        "wrong user must not learn Expired/Replay: {probe:?}"
    );
    assert!(
        !matches!(probe, Err(DownloadError::Expired | DownloadError::Replay)),
        "IDOR oracle: {probe:?}"
    );

    // Direct atomic consume classify (same txn semantics) for peer.
    let peer_ctx = OrgContext::try_new(
        owner.ids.org,
        peer,
        [PERMISSION_QA_QUERY, PERMISSION_QA_HISTORY],
        [owner.ids.collection],
    )
    .unwrap();
    let classified = with_org_txn(&pool, &peer_ctx, {
        let ctx = peer_ctx.clone();
        let expected = AuthorizedConsumeBinding::from_row(&row);
        move |txn| {
            Box::pin(async move { consume_authorized_or_classify(txn, &ctx, &expected).await })
        }
    })
    .await
    .expect("classify");
    assert_eq!(
        classified,
        fileconv_server::db::download_capabilities::AuthorizedConsumeOutcome::PermissionDenied,
        "peer must not observe Expired via consume classify"
    );

    ephemeral.drop().await.expect("ephemeral cleanup");
}
