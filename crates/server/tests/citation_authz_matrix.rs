//! P1B-R02 live citation / preview / download authorization matrix.
//!
//! Requires dual-role Postgres + MinIO. Admin role only creates/migrates the
//! ephemeral DB; application assertions run as `markhand_app`.

mod common;

use std::collections::BTreeSet;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::Utc;
use fileconv_knowledge::identity::BODY_TEXT_VERSION;
use fileconv_server::auth::context::OrgContext;
use fileconv_server::db::collections::{self, NewCollection};
use fileconv_server::db::documents::{self, NewDocument};
use fileconv_server::db::models::{ArtifactKind, DocumentState};
use fileconv_server::db::pool::with_org_txn;
use fileconv_server::services::chunking::prepare_chunks;
use fileconv_server::services::citation::{resolve_citation, ResolveCitationRequest};
use fileconv_server::services::download::{
    issue_capability, redeem_capability, CapabilityKeys, DownloadError, DownloadPurpose,
};
use fileconv_server::services::preview::{preview_markdown, PreviewError};
use fileconv_server::storage::minio::ObjectIdentityMeta;
use http_body_util::BodyExt;
use tower::ServiceExt;
use uuid::Uuid;

use common::{
    admin_database_url, app_database_url, assert_markhand_app_role, boot_app_pool, build_router,
    convert_to_markdown, login_access_token, put_bytes, quarantine_key, seed_user_with_permissions,
    sha256_hex, take_live, test_auth_config, test_minio_client, tiny_pdf_bytes, tiny_pptx_bytes,
    tiny_xlsx_bytes, trusted_key,
};

struct IndexedDoc {
    org: Uuid,
    user: Uuid,
    collection_id: Uuid,
    document_id: Uuid,
    version_id: Uuid,
    source_sha: String,
    markdown_sha: String,
    markdown: String,
    chunk_id: Uuid,
    span_start: usize,
    span_end: usize,
    quote: String,
    original_key: String,
    markdown_key: String,
    page: Option<i32>,
    slide: Option<i32>,
    sheet: Option<String>,
}

async fn seed_indexed_format(
    pool: &deadpool_postgres::Pool,
    store: &fileconv_server::storage::minio::MinioClient,
    ext: &'static str,
    source: &[u8],
    permissions: &[&str],
) -> IndexedDoc {
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    seed_user_with_permissions(
        pool,
        org,
        user,
        &format!("{user}@cite.test"),
        "correct-password-1",
        permissions,
    )
    .await;
    let markdown = convert_to_markdown(ext, source);
    let source_sha = sha256_hex(source);
    let markdown_sha = sha256_hex(markdown.as_bytes());
    assert_ne!(
        source_sha, markdown_sha,
        "{ext}: source SHA must differ from canonical Markdown SHA"
    );

    let collection_id = Uuid::new_v4();
    let document_id = Uuid::new_v4();
    let version_id = Uuid::new_v4();
    let artifact_id = Uuid::new_v4();
    let index_meta_id = Uuid::new_v4();
    let original = quarantine_key(org, Uuid::new_v4(), None).expect("quarantine");
    let markdown_obj = trusted_key(org, version_id, Uuid::new_v4(), None).expect("trusted");
    let ctx = OrgContext::try_new(org, user, permissions.iter().copied(), [collection_id]).unwrap();

    put_bytes(
        store,
        org,
        &original,
        source,
        "application/octet-stream",
        ObjectIdentityMeta {
            org_id: org,
            collection_id: Some(collection_id),
            document_id: Some(document_id),
            version_id: Some(version_id),
            original_filename: Some(format!("fixture.{ext}")),
            canonical_format: Some(ext.into()),
            content_sha256: Some(source_sha.clone()),
            content_length: Some(source.len() as u64),
            disposition: Some("accepted".into()),
        },
    )
    .await;
    put_bytes(
        store,
        org,
        &markdown_obj,
        markdown.as_bytes(),
        "text/markdown; charset=utf-8",
        ObjectIdentityMeta {
            org_id: org,
            collection_id: Some(collection_id),
            document_id: Some(document_id),
            version_id: Some(version_id),
            original_filename: None,
            canonical_format: Some("md".into()),
            content_sha256: Some(markdown_sha.clone()),
            content_length: Some(markdown.len() as u64),
            disposition: Some("trusted".into()),
        },
    )
    .await;

    let chunks = prepare_chunks(document_id, version_id, &markdown, ext);
    assert!(!chunks.is_empty(), "{ext}: expected at least one chunk");
    let primary = chunks[0].clone();
    let quote = primary.body.clone();
    let span_start = usize::try_from(primary.span_start).unwrap();
    let span_end = usize::try_from(primary.span_end).unwrap();
    let chunk_ids: Vec<Uuid> = chunks.iter().map(|_| Uuid::new_v4()).collect();
    let primary_chunk_id = chunk_ids[0];
    let signature = format!("{:0>64}", index_meta_id.as_u128());

    let original_key = original.as_str().to_string();
    let markdown_key = markdown_obj.as_str().to_string();
    let markdown_len = markdown.len() as i64;
    let source_len = source.len() as i64;
    let ext_owned = ext.to_string();
    let collection_name = format!("Cite {ext_owned}");
    let collection_slug = format!("cite-{ext_owned}-{}", collection_id.simple());
    let document_title = format!("Doc {ext_owned}");
    let content_type = format!("application/{ext_owned}");
    with_org_txn(pool, &ctx, {
        let ctx = ctx.clone();
        let source_sha = source_sha.clone();
        let markdown_sha = markdown_sha.clone();
        let chunks = chunks.clone();
        let chunk_ids = chunk_ids.clone();
        let signature = signature.clone();
        let original_key = original_key.clone();
        let markdown_key = markdown_key.clone();
        move |txn| {
            Box::pin(async move {
                collections::insert(
                    txn,
                    &ctx,
                    NewCollection {
                        id: collection_id,
                        name: &collection_name,
                        slug: &collection_slug,
                        description: None,
                        visibility: fileconv_server::db::models::CollectionVisibility::Org,
                    },
                )
                .await?;
                documents::insert(
                    txn,
                    &ctx,
                    NewDocument {
                        id: document_id,
                        collection_id,
                        title: &document_title,
                    },
                )
                .await?;
                txn.execute(
                    "INSERT INTO document_versions (
                        id, org_id, document_id, version_number, publication_state,
                        is_current, content_sha256, original_object_key, markdown_object_key,
                        source_content_type, byte_size, created_by_user_id
                     )
                     VALUES ($1, $2, $3, 1, 'published', true, $4, $5, $6, $7, $8, $9)",
                    &[
                        &version_id,
                        &ctx.org_id(),
                        &document_id,
                        &source_sha,
                        &original_key,
                        &markdown_key,
                        &content_type,
                        &source_len,
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
                     VALUES ($1, $2, $3, $4, $5, $6, $7, 'text/markdown; charset=utf-8', $8)",
                    &[
                        &artifact_id,
                        &ctx.org_id(),
                        &document_id,
                        &version_id,
                        &kind,
                        &markdown_key,
                        &markdown_sha,
                        &markdown_len,
                    ],
                )
                .await?;
                let indexed = DocumentState::Indexed.as_str();
                txn.execute(
                    "UPDATE documents
                     SET state = $3, current_version_id = $4, updated_at = clock_timestamp()
                     WHERE org_id = $1 AND id = $2",
                    &[&ctx.org_id(), &document_id, &indexed, &version_id],
                )
                .await?;
                txn.execute(
                    "INSERT INTO index_metadata (
                        id, org_id, collection_id, index_signature_sha256, embedding_family,
                        embedding_revision, dimensions, runtime_path, generation, is_active, state
                     ) VALUES (
                        $1, $2, $3, $4, 'test', 'r1', 8, 'local-hash', 1, true, 'active'
                     )",
                    &[&index_meta_id, &ctx.org_id(), &collection_id, &signature],
                )
                .await?;
                for (chunk, chunk_id) in chunks.iter().zip(chunk_ids.iter()) {
                    fileconv_server::db::chunks::insert(
                        txn,
                        &ctx,
                        fileconv_server::db::chunks::NewChunk {
                            id: *chunk_id,
                            document_id,
                            version_id,
                            ordinal: chunk.ordinal,
                            heading_path: &chunk.heading_path,
                            body: &chunk.body,
                            body_text_version: BODY_TEXT_VERSION,
                            chunk_identity_sha256: &chunk.chunk_identity,
                            index_metadata_id: index_meta_id,
                            index_signature: &signature,
                            page: chunk.page,
                            slide: chunk.slide,
                            sheet: chunk.sheet.as_deref(),
                            span_start: Some(chunk.span_start),
                            span_end: Some(chunk.span_end),
                        },
                    )
                    .await?;
                }
                Ok(())
            })
        }
    })
    .await
    .expect("seed indexed document");

    IndexedDoc {
        org,
        user,
        collection_id,
        document_id,
        version_id,
        source_sha,
        markdown_sha,
        markdown,
        chunk_id: primary_chunk_id,
        span_start,
        span_end,
        quote,
        original_key,
        markdown_key,
        page: primary.page,
        slide: primary.slide,
        sheet: primary.sheet.clone(),
    }
}

fn ctx_for(doc: &IndexedDoc, permissions: &[&str]) -> OrgContext {
    OrgContext::try_new(
        doc.org,
        doc.user,
        permissions.iter().copied(),
        [doc.collection_id],
    )
    .unwrap()
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL/APP + MARKHAND_TEST_MINIO_*"]
async fn live_pdf_pptx_xlsx_citation_preview_download_matrix() {
    let Some(admin) = admin_database_url() else {
        return;
    };
    let Some(app) = app_database_url() else {
        return;
    };
    let Some(store) = test_minio_client() else {
        return;
    };
    let (ephemeral, pool) = boot_app_pool(&admin, &app).await;
    assert_markhand_app_role(&pool).await;
    let keys =
        CapabilityKeys::from_auth_signing_key(test_auth_config().signing_key.as_ref().unwrap())
            .unwrap();

    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("pdf", tiny_pdf_bytes("Kinh phi PDF 10 trieu")),
        ("pptx", tiny_pptx_bytes("Kinh phi PPTX 15 trieu")),
        ("xlsx", tiny_xlsx_bytes("Kinh phi XLSX 20 trieu")),
    ];

    for (ext, source) in cases {
        let doc = seed_indexed_format(
            &pool,
            &store,
            ext,
            &source,
            &["qa.query", "qa.history", "doc.upload", "doc.delete"],
        )
        .await;
        let ctx = ctx_for(
            &doc,
            &["qa.query", "qa.history", "doc.upload", "doc.delete"],
        );

        let preview = preview_markdown(&pool, &ctx, &store, doc.document_id, None)
            .await
            .expect("preview");
        assert_eq!(preview.source_content_sha256, doc.source_sha);
        assert_eq!(preview.canonical_markdown_sha256, doc.markdown_sha);
        assert!(preview.markdown.contains("Kinh phi") || preview.markdown.contains(&doc.quote));

        let pin = resolve_citation(
            &pool,
            &ctx,
            &store,
            ResolveCitationRequest {
                logical_document_id: doc.document_id,
                version_id: doc.version_id,
                source_content_sha256: doc.source_sha.clone(),
                canonical_markdown_sha256: doc.markdown_sha.clone(),
                chunk_id: doc.chunk_id,
                source_span_start: doc.span_start,
                source_span_end: doc.span_end,
                quote_local_start: 0,
                quote_local_end: doc.quote.len(),
                quote: doc.quote.clone(),
                require_current: true,
            },
        )
        .await
        .unwrap_or_else(|error| panic!("{ext} resolve: {}", error.code()));
        assert_eq!(pin.version_id, doc.version_id);
        assert_eq!(pin.source_content_sha256, doc.source_sha);
        assert_eq!(pin.canonical_markdown_sha256, doc.markdown_sha);
        assert!(pin.anchor.starts_with("mhcite1."));
        assert_eq!(pin.page, doc.page.map(|v| v as u32));
        assert_eq!(pin.slide, doc.slide.map(|v| v as u32));
        assert_eq!(pin.sheet, doc.sheet);

        let issued = issue_capability(
            &pool,
            &ctx,
            &keys,
            doc.document_id,
            doc.version_id,
            DownloadPurpose::Original,
            Some(60),
        )
        .await
        .expect("issue original");
        let bytes = redeem_capability(&pool, &ctx, &keys, &store, issued.token.expose())
            .await
            .expect("redeem original");
        assert_eq!(bytes.content_sha256, doc.source_sha);

        let issued_md = issue_capability(
            &pool,
            &ctx,
            &keys,
            doc.document_id,
            doc.version_id,
            DownloadPurpose::Markdown,
            Some(60),
        )
        .await
        .expect("issue markdown");
        let md_bytes = redeem_capability(&pool, &ctx, &keys, &store, issued_md.token.expose())
            .await
            .expect("redeem markdown");
        assert_eq!(md_bytes.content_sha256, doc.markdown_sha);

        // Live MinIO tamper must fail for original + markdown + citation/preview.
        let markdown_key =
            fileconv_server::storage::keys::parse_key_for_org(&doc.markdown_key, doc.org).unwrap();
        put_bytes(
            &store,
            doc.org,
            &markdown_key,
            b"TAMPERED MARKDOWN BYTES",
            "text/markdown; charset=utf-8",
            ObjectIdentityMeta {
                org_id: doc.org,
                collection_id: Some(doc.collection_id),
                document_id: Some(doc.document_id),
                version_id: Some(doc.version_id),
                original_filename: None,
                canonical_format: Some("md".into()),
                content_sha256: Some(sha256_hex(b"TAMPERED MARKDOWN BYTES")),
                content_length: Some(24),
                disposition: Some("trusted".into()),
            },
        )
        .await;
        assert!(matches!(
            preview_markdown(&pool, &ctx, &store, doc.document_id, None).await,
            Err(PreviewError::ArtifactUnavailable)
        ));
        assert!(resolve_citation(
            &pool,
            &ctx,
            &store,
            ResolveCitationRequest {
                logical_document_id: doc.document_id,
                version_id: doc.version_id,
                source_content_sha256: doc.source_sha.clone(),
                canonical_markdown_sha256: doc.markdown_sha.clone(),
                chunk_id: doc.chunk_id,
                source_span_start: doc.span_start,
                source_span_end: doc.span_end,
                quote_local_start: 0,
                quote_local_end: doc.quote.len(),
                quote: doc.quote.clone(),
                require_current: true,
            },
        )
        .await
        .is_err());
        let issued_md2 = issue_capability(
            &pool,
            &ctx,
            &keys,
            doc.document_id,
            doc.version_id,
            DownloadPurpose::Markdown,
            Some(60),
        )
        .await
        .expect("re-issue markdown");
        assert!(matches!(
            redeem_capability(&pool, &ctx, &keys, &store, issued_md2.token.expose()).await,
            Err(DownloadError::ObjectUnavailable)
        ));

        // Restore markdown, tamper original.
        put_bytes(
            &store,
            doc.org,
            &markdown_key,
            doc.markdown.as_bytes(),
            "text/markdown; charset=utf-8",
            ObjectIdentityMeta {
                org_id: doc.org,
                collection_id: Some(doc.collection_id),
                document_id: Some(doc.document_id),
                version_id: Some(doc.version_id),
                original_filename: None,
                canonical_format: Some("md".into()),
                content_sha256: Some(doc.markdown_sha.clone()),
                content_length: Some(doc.markdown.len() as u64),
                disposition: Some("trusted".into()),
            },
        )
        .await;
        let original_key =
            fileconv_server::storage::keys::parse_key_for_org(&doc.original_key, doc.org).unwrap();
        put_bytes(
            &store,
            doc.org,
            &original_key,
            b"TAMPERED ORIGINAL",
            "application/octet-stream",
            ObjectIdentityMeta {
                org_id: doc.org,
                collection_id: Some(doc.collection_id),
                document_id: Some(doc.document_id),
                version_id: Some(doc.version_id),
                original_filename: Some(format!("fixture.{ext}")),
                canonical_format: Some(ext.into()),
                content_sha256: Some(sha256_hex(b"TAMPERED ORIGINAL")),
                content_length: Some(17),
                disposition: Some("accepted".into()),
            },
        )
        .await;
        let issued_orig2 = issue_capability(
            &pool,
            &ctx,
            &keys,
            doc.document_id,
            doc.version_id,
            DownloadPurpose::Original,
            Some(60),
        )
        .await
        .expect("re-issue original");
        assert!(matches!(
            redeem_capability(&pool, &ctx, &keys, &store, issued_orig2.token.expose()).await,
            Err(DownloadError::ObjectUnavailable)
        ));
    }

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL/APP + MARKHAND_TEST_MINIO_*"]
async fn live_citation_authz_expiry_replay_idor_and_immediate_deny() {
    let Some(admin) = take_live(admin_database_url(), "MARKHAND_TEST_DATABASE_URL") else {
        return;
    };
    let Some(app) = take_live(app_database_url(), "MARKHAND_TEST_APP_DATABASE_URL") else {
        return;
    };
    let Some(store) = take_live(test_minio_client(), "MARKHAND_TEST_MINIO_*") else {
        return;
    };
    let (ephemeral, pool) = boot_app_pool(&admin, &app).await;
    assert_markhand_app_role(&pool).await;
    let keys =
        CapabilityKeys::from_auth_signing_key(test_auth_config().signing_key.as_ref().unwrap())
            .unwrap();

    let doc = seed_indexed_format(
        &pool,
        &store,
        "pdf",
        &tiny_pdf_bytes("Authz matrix PDF"),
        &["qa.query", "qa.history", "doc.upload", "doc.delete"],
    )
    .await;
    let ctx = ctx_for(
        &doc,
        &["qa.query", "qa.history", "doc.upload", "doc.delete"],
    );

    // Concurrent redemption barrier: exactly one success, one replay.
    let issued = issue_capability(
        &pool,
        &ctx,
        &keys,
        doc.document_id,
        doc.version_id,
        DownloadPurpose::Markdown,
        Some(60),
    )
    .await
    .unwrap();
    let token = issued.token.expose().to_string();
    let (a, b) = tokio::join!(
        redeem_capability(&pool, &ctx, &keys, &store, &token),
        redeem_capability(&pool, &ctx, &keys, &store, &token),
    );
    let outcomes = [a.is_ok(), b.is_ok()];
    assert_eq!(
        outcomes.iter().filter(|ok| **ok).count(),
        1,
        "exactly one concurrent redemption must succeed, got {outcomes:?}"
    );
    assert!(
        matches!(a, Err(DownloadError::Replay)) || matches!(b, Err(DownloadError::Replay)),
        "loser must be Replay"
    );

    // Sequential replay after winner.
    assert!(matches!(
        redeem_capability(&pool, &ctx, &keys, &store, &token).await,
        Err(DownloadError::Replay)
    ));

    // Capability expiry (exp <= now after sleep).
    let expired = issue_capability(
        &pool,
        &ctx,
        &keys,
        doc.document_id,
        doc.version_id,
        DownloadPurpose::Markdown,
        Some(1),
    )
    .await
    .unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    assert!(matches!(
        redeem_capability(&pool, &ctx, &keys, &store, expired.token.expose()).await,
        Err(DownloadError::InvalidCapability)
    ));

    // History permission required for non-current versions: promote a synthetic
    // current sibling so the published document invariant still holds.
    let historical = doc.version_id;
    let current_v2 = Uuid::new_v4();
    with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        let document_id = doc.document_id;
        let markdown_key = doc.markdown_key.clone();
        let source_sha = doc.source_sha.clone();
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "UPDATE document_versions
                     SET is_current = false, effective_to = clock_timestamp()
                     WHERE org_id = $1 AND document_id = $2 AND id = $3",
                    &[&ctx.org_id(), &document_id, &historical],
                )
                .await?;
                txn.execute(
                    "INSERT INTO document_versions (
                        id, org_id, document_id, version_number, publication_state,
                        is_current, content_sha256, original_object_key, markdown_object_key,
                        source_content_type, byte_size, created_by_user_id
                     ) VALUES ($1,$2,$3,2,'published',true,$4,$5,$5,'application/pdf',1,$6)",
                    &[
                        &current_v2,
                        &ctx.org_id(),
                        &document_id,
                        &source_sha,
                        &markdown_key,
                        &ctx.user_id(),
                    ],
                )
                .await?;
                txn.execute(
                    "UPDATE documents SET current_version_id = $3 WHERE org_id = $1 AND id = $2",
                    &[&ctx.org_id(), &document_id, &current_v2],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .unwrap();
    let no_history = ctx_for(&doc, &["qa.query"]);
    assert!(matches!(
        preview_markdown(
            &pool,
            &no_history,
            &store,
            doc.document_id,
            Some(historical)
        )
        .await,
        Err(PreviewError::HistoryRequired)
    ));
    // With qa.history, historical preview is allowed against the demoted version.
    let with_history = ctx_for(&doc, &["qa.query", "qa.history"]);
    let historical_preview = preview_markdown(
        &pool,
        &with_history,
        &store,
        doc.document_id,
        Some(historical),
    )
    .await;
    assert!(
        historical_preview.is_ok()
            || matches!(historical_preview, Err(PreviewError::ArtifactUnavailable)),
        "history permission should authorize historical resolve path"
    );
    let _ = current_v2;

    // Multi-document / multi-version IDOR → not found.
    let other = seed_indexed_format(
        &pool,
        &store,
        "xlsx",
        &tiny_xlsx_bytes("Other tenant sheet"),
        &["qa.query", "qa.history"],
    )
    .await;
    let attacker = ctx_for(&doc, &["qa.query", "qa.history"]);
    assert!(matches!(
        preview_markdown(&pool, &attacker, &store, other.document_id, None).await,
        Err(PreviewError::NotFound)
    ));
    assert!(resolve_citation(
        &pool,
        &attacker,
        &store,
        ResolveCitationRequest {
            logical_document_id: other.document_id,
            version_id: other.version_id,
            source_content_sha256: other.source_sha,
            canonical_markdown_sha256: other.markdown_sha,
            chunk_id: other.chunk_id,
            source_span_start: other.span_start,
            source_span_end: other.span_end,
            quote_local_start: 0,
            quote_local_end: other.quote.len(),
            quote: other.quote,
            require_current: true,
        },
    )
    .await
    .is_err());

    // Empty collection allow-list fails closed.
    let empty = OrgContext::try_new(doc.org, doc.user, ["qa.query"], BTreeSet::new()).unwrap();
    assert!(matches!(
        preview_markdown(&pool, &empty, &store, doc.document_id, None).await,
        Err(PreviewError::NotFound)
    ));

    // Immediate deny after delete/tombstone.
    with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        let document_id = doc.document_id;
        move |txn| {
            Box::pin(async move {
                let tombstoned = DocumentState::Tombstoned.as_str();
                txn.execute(
                    "UPDATE documents
                     SET state = $3, deleted_at = clock_timestamp(), updated_at = clock_timestamp()
                     WHERE org_id = $1 AND id = $2",
                    &[&ctx.org_id(), &document_id, &tombstoned],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .unwrap();
    assert!(matches!(
        preview_markdown(&pool, &ctx, &store, doc.document_id, None).await,
        Err(PreviewError::NotFound)
    ));

    // Membership removal → HTTP auth fails closed (immediate deny).
    let live = seed_indexed_format(
        &pool,
        &store,
        "pptx",
        &tiny_pptx_bytes("Membership deny"),
        &["qa.query", "doc.upload"],
    )
    .await;
    let token = login_access_token(
        &pool,
        &format!("{}@cite.test", live.user),
        "correct-password-1",
    )
    .await;
    let app_router = build_router(pool.clone(), &ephemeral.app_url, Some(store.clone()));
    let ok = app_router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/v1/documents/{}/preview", live.document_id))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK);

    with_org_txn(&pool, &ctx_for(&live, &["qa.query"]), {
        let org = live.org;
        let user = live.user;
        move |txn| {
            Box::pin(async move {
                // Revoke session family first so membership DELETE is not FK-blocked.
                txn.execute(
                    "UPDATE refresh_tokens
                     SET revoked_at = clock_timestamp()
                     WHERE org_id = $1 AND user_id = $2 AND revoked_at IS NULL",
                    &[&org, &user],
                )
                .await?;
                txn.execute(
                    "DELETE FROM refresh_tokens WHERE org_id = $1 AND user_id = $2",
                    &[&org, &user],
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
    .unwrap();
    let denied = app_router
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/v1/documents/{}/preview", live.document_id))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        denied.status() == StatusCode::UNAUTHORIZED
            || denied.status() == StatusCode::FORBIDDEN
            || denied.status() == StatusCode::NOT_FOUND,
        "membership removal must deny, got {}",
        denied.status()
    );
    let body = denied.into_body().collect().await.unwrap().to_bytes();
    assert!(!body.is_empty());

    // Suspend/disable user.
    let suspended = seed_indexed_format(
        &pool,
        &store,
        "pdf",
        &tiny_pdf_bytes("Suspend deny"),
        &["qa.query"],
    )
    .await;
    let token = login_access_token(
        &pool,
        &format!("{}@cite.test", suspended.user),
        "correct-password-1",
    )
    .await;
    with_org_txn(&pool, &ctx_for(&suspended, &["qa.query"]), {
        let user = suspended.user;
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "UPDATE users SET disabled_at = $2 WHERE id = $1",
                    &[&user, &Utc::now()],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .unwrap();
    let app_router = build_router(pool.clone(), &ephemeral.app_url, Some(store));
    let denied = app_router
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/api/v1/documents/{}/preview",
                    suspended.document_id
                ))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(denied.status(), StatusCode::OK);

    ephemeral.drop().await;
}
