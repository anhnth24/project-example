//! Hermetic + live PostgreSQL acceptance for grounded Q&A (P1B-R03).

use std::collections::BTreeSet;
use std::time::Duration;

use chrono::{TimeZone, Utc};
use fileconv_server::auth::context::OrgContext;
use fileconv_server::config::Profile;
use fileconv_server::database::apply_migrations;
use fileconv_server::db::authz_lock::LockPool;
use fileconv_server::db::pool::{create_pool, create_pool_with_max_size, with_org_txn};
use fileconv_server::db::search;
use fileconv_server::services::authz_mutation::PERMISSION_MEMBER_MANAGE;
use fileconv_server::services::qa::provider::{
    canonicalize_base_url, ChatCompletionRequest, GlmCompatibleProvider, ProviderError,
    QaChatProvider, QaProviderConfig,
};
use fileconv_server::services::qa::{
    answer_question_hermetic, answer_question_live, collect_sse_token_text, retrieval_fixture,
    stream_answer_live, AnswerMode, AuthzCloseKind, GroundingPassage, HermeticAskInput, QaError,
    QaRequest, ScriptedProvider, StreamBounds, StreamCancel, StreamCloseReason, StreamLiveInput,
};
use fileconv_server::services::retrieval::{
    RetrievalHit, RetrievalProvenance, RetrievalResponse, VersionMode, PERMISSION_QA_HISTORY,
    PERMISSION_QA_QUERY,
};
use fileconv_server::storage::keys::{quarantine_key, trusted_key};
use fileconv_server::storage::MemoryBlobStore;
use rust_decimal::Decimal;
use sha2::{Digest, Sha256};
use tokio_postgres::NoTls;
use uuid::Uuid;

async fn poll_sse_frame(
    body: &mut fileconv_server::services::qa::GuardedSseBody,
) -> Option<Result<bytes::Bytes, fileconv_server::services::qa::StreamError>> {
    use http_body::Body;
    use std::pin::Pin;
    let frame = std::future::poll_fn(|cx| Pin::new(&mut *body).poll_frame(cx)).await?;
    Some(frame.map(|f| f.into_data().unwrap_or_else(|_| bytes::Bytes::new())))
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

async fn connect_raw(database_url: &str) -> tokio_postgres::Client {
    let (client, connection) = tokio_postgres::connect(database_url, NoTls)
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
        let suffix = Uuid::new_v4().simple();
        let db_name = format!("markhand_r03_{suffix}");
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

fn hit(collection_id: Uuid, snippet: &str) -> RetrievalHit {
    RetrievalHit {
        chunk_id: Uuid::new_v4(),
        chunk_identity_sha256: "e".repeat(64),
        collection_id,
        document_id: Uuid::new_v4(),
        version_id: Uuid::new_v4(),
        version_number: 1,
        content_sha256: "f".repeat(64),
        heading: "Mục".into(),
        snippet: snippet.into(),
        body: snippet.into(),
        lexical_score: 1.0,
        vector_score: 0.5,
        rerank_score: 1.5,
        is_current: true,
        effective_from: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        effective_to: None,
        page: Some(1),
        slide: None,
        sheet: None,
        span_start: 0,
        span_end: snippet.len(),
    }
}

#[tokio::test]
async fn public_ask_surface_falls_back_when_provider_missing() {
    let collection_id = Uuid::new_v4();
    let ctx = OrgContext::try_new(
        Uuid::new_v4(),
        Uuid::new_v4(),
        [PERMISSION_QA_QUERY],
        [collection_id],
    )
    .unwrap();
    let quote = "Kinh phí phê duyệt là 15 triệu đồng.";
    let hits = vec![hit(collection_id, quote)];
    let retrieval = retrieval_fixture(&ctx, VersionMode::Current, hits.clone(), vec![]);
    let mut passages = GroundingPassage::from_hits(&hits);
    for passage in &mut passages {
        passage.authoritative_quote = Some(quote.into());
    }
    let answer = answer_question_hermetic::<ScriptedProvider>(HermeticAskInput {
        ctx: &ctx,
        request: QaRequest {
            question: "Kinh phí hiện tại là bao nhiêu theo tài liệu?".into(),
            mode: VersionMode::Current,
            use_llm: true,
            collection_ids: None,
        },
        retrieval,
        passages,
        conflicts: vec![],
        timeline: vec![],
        provider: None,
        provider_config: None,
    })
    .await
    .expect("extractive fallback");
    assert_eq!(answer.mode, AnswerMode::FallbackExtractive);
    assert!(answer.answer.contains("[CITE-0001]"));
    assert!(answer.grounded);
    assert_eq!(answer.audit.fallback_reason, Some("provider_unavailable"));
}

#[test]
fn provider_url_policy_rejects_ssrf_shapes() {
    assert!(matches!(
        canonicalize_base_url(
            "https://169.254.169.254/latest",
            &["169.254.169.254".into()],
            false
        ),
        Err(ProviderError::UrlPolicy)
    ));
    assert!(matches!(
        canonicalize_base_url("http://10.0.0.1/v1", &[], true),
        Err(ProviderError::UrlPolicy)
    ));
    assert!(matches!(
        canonicalize_base_url(
            "https://this-host-should-not-resolve.invalid/v1",
            &["this-host-should-not-resolve.invalid".into()],
            false
        ),
        Err(ProviderError::UrlPolicy)
    ));
}

#[tokio::test]
async fn mock_provider_bounds_oversized_response() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 8192];
        let _ = stream.read(&mut buf);
        let huge = "x".repeat(70_000);
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\n\r\n{}",
            huge.len(),
            huge
        );
        let _ = stream.write_all(resp.as_bytes());
    });
    let config = QaProviderConfig::with_api_key(
        format!("http://127.0.0.1:{}", addr.port()),
        "key-not-fake",
        "configured-model",
        "glm",
        Duration::from_secs(2),
        [] as [&str; 0],
        true,
        Profile::Dev,
    )
    .unwrap();
    let provider = GlmCompatibleProvider::new(config).unwrap();
    let err = provider
        .complete_grounded(&ChatCompletionRequest {
            system: "s".into(),
            user: "u".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(err, ProviderError::Truncated);
}

struct LiveFixture {
    ctx: OrgContext,
    collection_id: Uuid,
    document_id: Uuid,
    document_b_id: Uuid,
    version_current: Uuid,
    version_old: Uuid,
    version_b: Uuid,
    chunk_id: Uuid,
    chunk_b_id: Uuid,
    markdown_sha: String,
    markdown_old_sha: String,
    markdown_b_sha: String,
    chunk_old_id: Uuid,
    quote: String,
    quote_b: String,
    quote_start: usize,
    quote_end: usize,
    quote_b_start: usize,
    quote_b_end: usize,
    conflict_id: Uuid,
}

async fn seed_live_fixture(pool: &deadpool_postgres::Pool, store: &MemoryBlobStore) -> LiveFixture {
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let owner_user = Uuid::new_v4();
    let role = Uuid::new_v4();
    let collection_id = Uuid::new_v4();
    let document_id = Uuid::new_v4();
    let document_b_id = Uuid::new_v4();
    let source_version = Uuid::new_v4();
    let source_version_b = Uuid::new_v4();
    let version_current = Uuid::new_v4();
    let version_old = Uuid::new_v4();
    let version_b = Uuid::new_v4();
    let chunk_id = Uuid::new_v4();
    let chunk_old_id = Uuid::new_v4();
    let chunk_b_id = Uuid::new_v4();
    let index_meta = Uuid::new_v4();
    let original_object = Uuid::new_v4();
    let original_object_b = Uuid::new_v4();
    let markdown_object = Uuid::new_v4();
    let markdown_object_old = Uuid::new_v4();
    let markdown_object_b = Uuid::new_v4();
    let conflict_id = Uuid::new_v4();
    let claim_raw_a = Uuid::new_v4();
    let claim_raw_b = Uuid::new_v4();
    let (claim_a_id, claim_b_id) = if claim_raw_a < claim_raw_b {
        (claim_raw_a, claim_raw_b)
    } else {
        (claim_raw_b, claim_raw_a)
    };

    let markdown =
        "# Mục\n\nMở đầu.\n\nKinh phí phê duyệt là 15 triệu đồng.\n\nKết thúc phiên bản.\n"
            .to_string();
    let quote = "Kinh phí phê duyệt là 15 triệu đồng.".to_string();
    let quote_start = markdown.find(&quote).unwrap();
    let quote_end = quote_start + quote.len();
    let markdown_sha = hex::encode(Sha256::digest(markdown.as_bytes()));
    let markdown_old = "# Mục\n\nKinh phí phê duyệt là 10 triệu đồng.\n".to_string();
    let markdown_old_sha = hex::encode(Sha256::digest(markdown_old.as_bytes()));
    let markdown_b = "# Phụ lục\n\nKinh phí phê duyệt là 20 triệu đồng.\n".to_string();
    let quote_b = "Kinh phí phê duyệt là 20 triệu đồng.".to_string();
    let quote_b_start = markdown_b.find(&quote_b).unwrap();
    let quote_b_end = quote_b_start + quote_b.len();
    let markdown_b_sha = hex::encode(Sha256::digest(markdown_b.as_bytes()));
    let original_bytes = b"%PDF-1.4 original-upload-bytes".to_vec();
    let original_b_bytes = b"%PDF-1.4 original-upload-bytes-b".to_vec();
    let original_sha = hex::encode(Sha256::digest(&original_bytes));
    let original_b_sha = hex::encode(Sha256::digest(&original_b_bytes));
    let original_key = quarantine_key(org, original_object, Some("doc.pdf")).unwrap();
    let original_key_b = quarantine_key(org, original_object_b, Some("doc-b.pdf")).unwrap();
    let markdown_key = trusted_key(org, version_current, markdown_object, None).unwrap();
    let markdown_key_old = trusted_key(org, version_old, markdown_object_old, None).unwrap();
    let markdown_key_b = trusted_key(org, version_b, markdown_object_b, None).unwrap();
    store
        .put(
            org,
            &original_key,
            original_bytes.clone(),
            Some("application/pdf"),
        )
        .unwrap();
    store
        .put(
            org,
            &original_key_b,
            original_b_bytes.clone(),
            Some("application/pdf"),
        )
        .unwrap();
    store
        .put(
            org,
            &markdown_key,
            markdown.as_bytes().to_vec(),
            Some("text/markdown; charset=utf-8"),
        )
        .unwrap();
    store
        .put(
            org,
            &markdown_key_old,
            markdown_old.as_bytes().to_vec(),
            Some("text/markdown; charset=utf-8"),
        )
        .unwrap();
    store
        .put(
            org,
            &markdown_key_b,
            markdown_b.as_bytes().to_vec(),
            Some("text/markdown; charset=utf-8"),
        )
        .unwrap();

    let ctx = OrgContext::try_new(
        org,
        user,
        [
            PERMISSION_QA_QUERY,
            PERMISSION_QA_HISTORY,
            PERMISSION_MEMBER_MANAGE,
        ],
        [collection_id],
    )
    .unwrap();
    let markdown_len = markdown.len() as i64;
    let markdown_old_len = markdown_old.len() as i64;
    let markdown_b_len = markdown_b.len() as i64;
    let original_len = original_bytes.len() as i64;
    let original_b_len = original_b_bytes.len() as i64;

    with_org_txn(pool, &ctx, {
        let markdown_sha = markdown_sha.clone();
        let markdown_old_sha = markdown_old_sha.clone();
        let markdown_b_sha = markdown_b_sha.clone();
        let original_sha = original_sha.clone();
        let original_b_sha = original_b_sha.clone();
        let original_key_str = original_key.as_str().to_string();
        let original_key_b_str = original_key_b.as_str().to_string();
        let markdown_key_str = markdown_key.as_str().to_string();
        let markdown_key_old_str = markdown_key_old.as_str().to_string();
        let markdown_key_b_str = markdown_key_b.as_str().to_string();
        let quote = quote.clone();
        let quote_b = quote_b.clone();
        let markdown_old = markdown_old.clone();
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "INSERT INTO orgs (id, slug, name) VALUES ($1, $2, $3)",
                    &[&org, &format!("org-{org}"), &"org"],
                )
                .await?;
                txn.execute(
                    "INSERT INTO users (id, email, display_name, password_hash)
                     VALUES ($1, $2, 'u', 'test-hash'), ($3, $4, 'owner', 'test-hash')",
                    &[
                        &user,
                        &format!("{user}@example.test"),
                        &owner_user,
                        &format!("{owner_user}@example.test"),
                    ],
                )
                .await?;
                txn.execute(
                    "INSERT INTO org_memberships (org_id, user_id, role)
                     VALUES ($1, $2, 'owner'), ($1, $3, 'owner')",
                    &[&org, &user, &owner_user],
                )
                .await?;
                txn.execute(
                    "INSERT INTO roles (id, org_id, code, name, is_system)
                     VALUES ($1, $2, 'owner', 'Owner', true)",
                    &[&role, &org],
                )
                .await?;
                txn.execute(
                    "INSERT INTO role_permissions (org_id, role_id, permission_id)
                     SELECT $1, $2, id FROM permissions
                     WHERE code IN ('qa.query', 'qa.history', 'member.manage')",
                    &[&org, &role],
                )
                .await?;
                txn.execute(
                    "INSERT INTO collections (
                        id, org_id, name, slug, owner_user_id, visibility
                     ) VALUES ($1, $2, 'c', $3, $4, 'org')",
                    &[
                        &collection_id,
                        &org,
                        &format!("c-{collection_id}"),
                        &owner_user,
                    ],
                )
                .await?;
                txn.execute(
                    "INSERT INTO documents (
                        id, org_id, collection_id, title, state, created_by_user_id
                     ) VALUES ($1, $2, $3, 'doc', 'indexed', $4)",
                    &[&document_id, &org, &collection_id, &user],
                )
                .await?;
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
                        &source_version,
                        &org,
                        &document_id,
                        &original_sha,
                        &original_key_str,
                        &original_len,
                        &user,
                    ],
                )
                .await?;
                // Published lineage: draft source=1, superseded=2, current=3.
                txn.execute(
                    "INSERT INTO document_versions (
                        id, org_id, document_id, version_number, parent_version_id,
                        publication_state, is_current, content_sha256, original_object_key,
                        markdown_object_key, source_filename, source_content_type, byte_size,
                        effective_from, effective_to, created_by_user_id
                     ) VALUES (
                        $1,$2,$3,2,$4,'published',false,$5,$6,$7,'doc.pdf',
                        'text/markdown; charset=utf-8',$8,'2024-01-01Z','2024-05-31Z',$9
                     )",
                    &[
                        &version_old,
                        &org,
                        &document_id,
                        &source_version,
                        &markdown_old_sha,
                        &original_key_str,
                        &markdown_key_old_str,
                        &markdown_old_len,
                        &user,
                    ],
                )
                .await?;
                txn.execute(
                    "INSERT INTO document_versions (
                        id, org_id, document_id, version_number, parent_version_id,
                        publication_state, is_current, content_sha256, original_object_key,
                        markdown_object_key, source_filename, source_content_type, byte_size,
                        effective_from, created_by_user_id
                     ) VALUES (
                        $1,$2,$3,3,$4,'published',true,$5,$6,$7,'doc.pdf',
                        'text/markdown; charset=utf-8',$8,'2024-06-01Z',$9
                     )",
                    &[
                        &version_current,
                        &org,
                        &document_id,
                        &version_old,
                        &markdown_sha,
                        &original_key_str,
                        &markdown_key_str,
                        &markdown_len,
                        &user,
                    ],
                )
                .await?;
                txn.execute(
                    "UPDATE documents SET current_version_id = $1 WHERE id = $2",
                    &[&version_current, &document_id],
                )
                .await?;
                txn.execute(
                    "INSERT INTO derived_artifacts (
                        id, org_id, document_id, version_id, artifact_kind, object_key,
                        content_sha256, content_type, byte_size
                     ) VALUES
                     ($1,$2,$3,$4,'markdown',$5,$6,'text/markdown; charset=utf-8',$7),
                     ($8,$2,$3,$9,'markdown',$10,$11,'text/markdown; charset=utf-8',$12)",
                    &[
                        &Uuid::new_v4(),
                        &org,
                        &document_id,
                        &version_current,
                        &markdown_key_str,
                        &markdown_sha,
                        &markdown_len,
                        &Uuid::new_v4(),
                        &version_old,
                        &markdown_key_old_str,
                        &markdown_old_sha,
                        &markdown_old_len,
                    ],
                )
                .await?;
                txn.execute(
                    "INSERT INTO index_metadata (
                        id, org_id, collection_id, index_signature_sha256,
                        embedding_family, embedding_revision, dimensions,
                        runtime_path, generation, is_active, state
                     ) VALUES (
                        $1,$2,$3,$4,'f','r',8,'local-hash',1,true,'active'
                     )",
                    &[&index_meta, &org, &collection_id, &"c".repeat(64)],
                )
                .await?;
                txn.execute(
                    "INSERT INTO chunks (
                        id, org_id, document_id, version_id, ordinal, heading_path, body,
                        body_text_version, chunk_identity_sha256, index_metadata_id,
                        index_signature, page, slide, sheet, span_start, span_end, tsv
                     ) VALUES (
                        $1,$2,$3,$4,0,ARRAY['Mục'],$5,'v1',$6,$7,$8,NULL,NULL,NULL,$9,$10,
                        to_tsvector('simple',$5)
                     )",
                    &[
                        &chunk_id,
                        &org,
                        &document_id,
                        &version_current,
                        &quote,
                        &("a".repeat(64)),
                        &index_meta,
                        &"c".repeat(64),
                        &(quote_start as i32),
                        &(quote_end as i32),
                    ],
                )
                .await?;
                // Representative chunk for superseded version (compare/history evidence).
                let quote_old = "Kinh phí phê duyệt là 10 triệu đồng.".to_string();
                let quote_old_start = markdown_old.find(&quote_old).unwrap_or(0) as i32;
                let quote_old_end = quote_old_start + quote_old.len() as i32;
                txn.execute(
                    "INSERT INTO chunks (
                        id, org_id, document_id, version_id, ordinal, heading_path, body,
                        body_text_version, chunk_identity_sha256, index_metadata_id,
                        index_signature, page, slide, sheet, span_start, span_end, tsv
                     ) VALUES (
                        $1,$2,$3,$4,0,ARRAY['Mục'],$5,'v1',$6,$7,$8,NULL,NULL,NULL,$9,$10,
                        to_tsvector('simple',$5)
                     )",
                    &[
                        &chunk_old_id,
                        &org,
                        &document_id,
                        &version_old,
                        &quote_old,
                        &("d".repeat(64)),
                        &index_meta,
                        &"c".repeat(64),
                        &quote_old_start,
                        &quote_old_end,
                    ],
                )
                .await?;
                // Second current document for open numeric conflict (both sides current).
                txn.execute(
                    "INSERT INTO documents (
                        id, org_id, collection_id, title, state, created_by_user_id
                     ) VALUES ($1, $2, $3, 'doc-b', 'indexed', $4)",
                    &[&document_b_id, &org, &collection_id, &user],
                )
                .await?;
                txn.execute(
                    "INSERT INTO document_versions (
                        id, org_id, document_id, version_number, publication_state, is_current,
                        content_sha256, original_object_key, source_filename, source_content_type,
                        byte_size, effective_from, created_by_user_id
                     ) VALUES (
                        $1,$2,$3,1,'draft',false,$4,$5,'doc-b.pdf','application/pdf',$6,
                        '2024-06-01Z',$7
                     )",
                    &[
                        &source_version_b,
                        &org,
                        &document_b_id,
                        &original_b_sha,
                        &original_key_b_str,
                        &original_b_len,
                        &user,
                    ],
                )
                .await?;
                txn.execute(
                    "INSERT INTO document_versions (
                        id, org_id, document_id, version_number, parent_version_id,
                        publication_state, is_current, content_sha256, original_object_key,
                        markdown_object_key, source_filename, source_content_type, byte_size,
                        effective_from, created_by_user_id
                     ) VALUES (
                        $1,$2,$3,2,$4,'published',true,$5,$6,$7,'doc-b.pdf',
                        'text/markdown; charset=utf-8',$8,'2024-06-01Z',$9
                     )",
                    &[
                        &version_b,
                        &org,
                        &document_b_id,
                        &source_version_b,
                        &markdown_b_sha,
                        &original_key_b_str,
                        &markdown_key_b_str,
                        &markdown_b_len,
                        &user,
                    ],
                )
                .await?;
                txn.execute(
                    "UPDATE documents SET current_version_id = $1 WHERE id = $2",
                    &[&version_b, &document_b_id],
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
                        &org,
                        &document_b_id,
                        &version_b,
                        &markdown_key_b_str,
                        &markdown_b_sha,
                        &markdown_b_len,
                    ],
                )
                .await?;
                txn.execute(
                    "INSERT INTO chunks (
                        id, org_id, document_id, version_id, ordinal, heading_path, body,
                        body_text_version, chunk_identity_sha256, index_metadata_id,
                        index_signature, page, slide, sheet, span_start, span_end, tsv
                     ) VALUES (
                        $1,$2,$3,$4,0,ARRAY['Phụ lục'],$5,'v1',$6,$7,$8,NULL,NULL,NULL,$9,$10,
                        to_tsvector('simple',$5)
                     )",
                    &[
                        &chunk_b_id,
                        &org,
                        &document_b_id,
                        &version_b,
                        &quote_b,
                        &("b".repeat(64)),
                        &index_meta,
                        &"c".repeat(64),
                        &(quote_b_start as i32),
                        &(quote_b_end as i32),
                    ],
                )
                .await?;
                // Bind claims so claim_a_id always maps to document_id / version_current.
                let (claim_doc_a, claim_ver_a, claim_chunk_a, claim_quote_a, claim_money_a) =
                    (document_id, version_current, chunk_id, quote.clone(), 15i64);
                let (claim_doc_b, claim_ver_b, claim_chunk_b, claim_quote_b, claim_money_b) =
                    (document_b_id, version_b, chunk_b_id, quote_b.clone(), 20i64);
                txn.execute(
                    "INSERT INTO claims (
                        id, org_id, document_id, version_id, chunk_id, claim_key, subject,
                        predicate, value_type, value_money, unit, scope, effective_from,
                        citation_quote, citation_span_start, citation_span_end
                     ) VALUES (
                        $1,$2,$3,$4,$5,'budget','kinh phí','phê duyệt','money',$6,'VND','org',
                        '2024-06-01Z',$7,$8,$9
                     )",
                    &[
                        &claim_a_id,
                        &org,
                        &claim_doc_a,
                        &claim_ver_a,
                        &claim_chunk_a,
                        &Decimal::from(claim_money_a),
                        &claim_quote_a,
                        &(quote_start as i32),
                        &(quote_end as i32),
                    ],
                )
                .await?;
                txn.execute(
                    "INSERT INTO claims (
                        id, org_id, document_id, version_id, chunk_id, claim_key, subject,
                        predicate, value_type, value_money, unit, scope, effective_from,
                        citation_quote, citation_span_start, citation_span_end
                     ) VALUES (
                        $1,$2,$3,$4,$5,'budget','kinh phí','phê duyệt','money',$6,'VND','org',
                        '2024-06-01Z',$7,$8,$9
                     )",
                    &[
                        &claim_b_id,
                        &org,
                        &claim_doc_b,
                        &claim_ver_b,
                        &claim_chunk_b,
                        &Decimal::from(claim_money_b),
                        &claim_quote_b,
                        &(quote_b_start as i32),
                        &(quote_b_end as i32),
                    ],
                )
                .await?;
                txn.execute(
                    "INSERT INTO conflicts (
                        id, org_id, status, severity, conflict_type, claim_a_id, claim_b_id,
                        first_detected_version_id
                     ) VALUES ($1,$2,'open','warning','numeric',$3,$4,$5)",
                    &[
                        &conflict_id,
                        &org,
                        &claim_a_id,
                        &claim_b_id,
                        &version_current,
                    ],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("seed live fixture");

    LiveFixture {
        ctx,
        collection_id,
        document_id,
        document_b_id,
        version_current,
        version_old,
        version_b,
        chunk_id,
        chunk_b_id,
        chunk_old_id,
        markdown_sha,
        markdown_old_sha,
        markdown_b_sha,
        quote,
        quote_b,
        quote_start,
        quote_end,
        quote_b_start,
        quote_b_end,
        conflict_id,
    }
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn live_pg_provenance_history_conflict_and_stream_races() {
    let base =
        test_database_url().expect("MARKHAND_TEST_DATABASE_URL required for ignored live test");
    let db = EphemeralDb::create(&base).await;
    apply_migrations(&db.url).await.expect("migrations");
    let pool = create_pool(&db.url).expect("pool");
    let lock_pool = LockPool::new(&db.url).expect("lock pool");
    let store = MemoryBlobStore::default();
    let fx = seed_live_fixture(&pool, &store).await;

    let timeline = with_org_txn(&pool, &fx.ctx, {
        let ctx = fx.ctx.clone();
        let document_id = fx.document_id;
        let collection_id = fx.collection_id;
        move |txn| {
            Box::pin(async move {
                search::list_published_version_timeline(txn, &ctx, document_id, &[collection_id])
                    .await
            })
        }
    })
    .await
    .expect("timeline");
    assert!(timeline.len() >= 2);

    // Compare mode with representative evidence (independent of top-K).
    let compare_mode = VersionMode::Compare {
        document_id: fx.document_id,
        version_a: fx.version_old,
        version_b: fx.version_current,
    };
    let compare_retrieval = RetrievalResponse {
        hits: vec![], // empty top-K — server must load representative evidence
        warnings: vec![],
        embedding_mode: "test".into(),
        conflict_evidence: vec![],
        vector_weight: 0.55,
        provenance: RetrievalProvenance {
            org_id: fx.ctx.org_id(),
            user_id: fx.ctx.user_id(),
            mode: compare_mode.clone(),
            collection_ids: BTreeSet::from([fx.collection_id]),
            retrieved_at: Utc::now(),
            document_id: Some(fx.document_id),
        },
    };
    let compare_answer = answer_question_live::<_, ScriptedProvider>(
        &pool,
        &lock_pool,
        &store,
        &fx.ctx,
        QaRequest {
            question: "So sánh kinh phí giữa hai phiên bản theo tài liệu?".into(),
            mode: compare_mode,
            use_llm: false,
            collection_ids: None,
        },
        compare_retrieval,
        None,
        None,
        None,
    )
    .await
    .expect("compare ask");
    assert!(compare_answer.grounded());
    assert!(compare_answer.version_context().change_note.is_some());
    let cited_versions: BTreeSet<_> = compare_answer
        .citations()
        .iter()
        .map(|c| c.version_id)
        .collect();
    assert!(cited_versions.contains(&fx.version_old));
    assert!(cited_versions.contains(&fx.version_current));
    compare_answer.finish().await;

    // History mode end-to-end (hits for two lineage versions + DB timeline metadata).
    let history_mode = VersionMode::History {
        document_id: fx.document_id,
        before_version_no: None,
    };
    let history_retrieval = RetrievalResponse {
        hits: vec![
            RetrievalHit {
                chunk_id: fx.chunk_id,
                chunk_identity_sha256: "a".repeat(64),
                collection_id: fx.collection_id,
                document_id: fx.document_id,
                version_id: fx.version_current,
                version_number: 3,
                content_sha256: fx.markdown_sha.clone(),
                heading: "Mục".into(),
                snippet: fx.quote.clone(),
                body: fx.quote.clone(),
                lexical_score: 1.0,
                vector_score: 0.5,
                rerank_score: 1.5,
                is_current: true,
                effective_from: Utc.with_ymd_and_hms(2024, 6, 1, 0, 0, 0).unwrap(),
                effective_to: None,
                page: None,
                slide: None,
                sheet: None,
                span_start: fx.quote_start,
                span_end: fx.quote_end,
            },
            RetrievalHit {
                chunk_id: fx.chunk_old_id,
                chunk_identity_sha256: "d".repeat(64),
                collection_id: fx.collection_id,
                document_id: fx.document_id,
                version_id: fx.version_old,
                version_number: 2,
                content_sha256: fx.markdown_old_sha.clone(),
                heading: "Mục".into(),
                snippet: "Kinh phí phê duyệt là 10 triệu đồng.".into(),
                body: "Kinh phí phê duyệt là 10 triệu đồng.".into(),
                lexical_score: 0.8,
                vector_score: 0.3,
                rerank_score: 1.1,
                is_current: false,
                effective_from: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
                effective_to: Some(Utc.with_ymd_and_hms(2024, 5, 31, 0, 0, 0).unwrap()),
                page: None,
                slide: None,
                sheet: None,
                span_start: 0,
                span_end: "Kinh phí phê duyệt là 10 triệu đồng.".len(),
            },
        ],
        warnings: vec![],
        embedding_mode: "test".into(),
        conflict_evidence: vec![],
        vector_weight: 0.55,
        provenance: RetrievalProvenance {
            org_id: fx.ctx.org_id(),
            user_id: fx.ctx.user_id(),
            mode: history_mode.clone(),
            collection_ids: BTreeSet::from([fx.collection_id]),
            retrieved_at: Utc::now(),
            document_id: Some(fx.document_id),
        },
    };
    let history_answer = answer_question_live::<_, ScriptedProvider>(
        &pool,
        &lock_pool,
        &store,
        &fx.ctx,
        QaRequest {
            question: "Lịch sử kinh phí của tài liệu này theo các phiên bản?".into(),
            mode: history_mode,
            use_llm: false,
            collection_ids: None,
        },
        history_retrieval,
        None,
        None,
        None,
    )
    .await
    .expect("history ask");
    assert!(history_answer.version_context().history.len() >= 2);
    assert!(history_answer.grounded());
    history_answer.finish().await;

    let hits = vec![
        RetrievalHit {
            chunk_id: fx.chunk_id,
            chunk_identity_sha256: "a".repeat(64),
            collection_id: fx.collection_id,
            document_id: fx.document_id,
            version_id: fx.version_current,
            version_number: 3,
            content_sha256: fx.markdown_sha.clone(),
            heading: "Mục".into(),
            snippet: fx.quote.clone(),
            body: fx.quote.clone(),
            lexical_score: 1.0,
            vector_score: 0.5,
            rerank_score: 1.5,
            is_current: true,
            effective_from: Utc.with_ymd_and_hms(2024, 6, 1, 0, 0, 0).unwrap(),
            effective_to: None,
            page: None,
            slide: None,
            sheet: None,
            span_start: fx.quote_start,
            span_end: fx.quote_end,
        },
        RetrievalHit {
            chunk_id: fx.chunk_b_id,
            chunk_identity_sha256: "b".repeat(64),
            collection_id: fx.collection_id,
            document_id: fx.document_b_id,
            version_id: fx.version_b,
            version_number: 2,
            content_sha256: fx.markdown_b_sha.clone(),
            heading: "Phụ lục".into(),
            snippet: fx.quote_b.clone(),
            body: fx.quote_b.clone(),
            lexical_score: 0.9,
            vector_score: 0.4,
            rerank_score: 1.3,
            is_current: true,
            effective_from: Utc.with_ymd_and_hms(2024, 6, 1, 0, 0, 0).unwrap(),
            effective_to: None,
            page: None,
            slide: None,
            sheet: None,
            span_start: fx.quote_b_start,
            span_end: fx.quote_b_end,
        },
    ];
    let retrieval = RetrievalResponse {
        hits: hits.clone(),
        warnings: vec![],
        embedding_mode: "test".into(),
        conflict_evidence: vec![],
        vector_weight: 0.55,
        provenance: RetrievalProvenance {
            org_id: fx.ctx.org_id(),
            user_id: fx.ctx.user_id(),
            mode: VersionMode::Current,
            collection_ids: BTreeSet::from([fx.collection_id]),
            retrieved_at: Utc::now(),
            document_id: None,
        },
    };

    let answer = answer_question_live::<_, ScriptedProvider>(
        &pool,
        &lock_pool,
        &store,
        &fx.ctx,
        QaRequest {
            question: "Kinh phí phê duyệt hiện tại là bao nhiêu theo tài liệu?".into(),
            mode: VersionMode::Current,
            use_llm: false,
            collection_ids: None,
        },
        retrieval.clone(),
        None,
        None,
        None,
    )
    .await
    .expect("live ask");
    assert!(answer.grounded());
    assert_eq!(answer.conflict_warnings().len(), 1);
    assert_eq!(answer.conflict_warnings()[0].conflict_id, fx.conflict_id);
    assert!(!answer.conflict_warnings()[0].message.contains("CITE-?"));
    answer.finish().await;

    // Live stream with DB authz probe — membership revoke mid-stream.
    // Pad retrieval so the protected replay has many tokens still buffered.
    let mut long_hits = hits.clone();
    long_hits[0].snippet = format!("{} {}", fx.quote, "token ".repeat(80));
    long_hits[0].body = long_hits[0].snippet.clone();
    let long_retrieval = RetrievalResponse {
        hits: long_hits,
        ..retrieval.clone()
    };
    let cancel = StreamCancel::new();
    let rx = stream_answer_live(StreamLiveInput::<_, ScriptedProvider> {
        pool: &pool,
        lock_pool: &lock_pool,
        storage: &store,
        ctx: &fx.ctx,
        request: QaRequest {
            question: "Kinh phí phê duyệt hiện tại là bao nhiêu theo tài liệu dài để stream?"
                .into(),
            mode: VersionMode::Current,
            use_llm: false,
            collection_ids: None,
        },
        retrieval: long_retrieval,
        provider: None,
        provider_config: None,
        cancel: cancel.clone(),
        bounds: StreamBounds {
            max_tokens: 1000,
            max_bytes: 64 * 1024,
            buffer: 64,
            backpressure_wait: Duration::from_secs(1),
            overall_timeout: Some(Duration::from_secs(5)),
            source_wait: Duration::from_secs(1),
        },
    })
    .await
    .expect("live stream");
    // Actively consume tokens under guarded body, then revoke (zero later tokens).
    let fence = rx.fence().cloned().expect("stream fence");
    assert!(!rx.conflict_warnings.is_empty());
    for warning in &rx.conflict_warnings {
        assert!(!warning.pin_cite_ids.is_empty());
        assert!(
            rx.citations.len() >= warning.pin_cite_ids.len(),
            "all current pins must be returned; pins={:?} cites={}",
            warning.pin_cite_ids,
            rx.citations.len()
        );
    }
    let mut body = rx.into_sse_body();
    let mut consumed = 0usize;
    for _ in 0..2 {
        let frame = poll_sse_frame(&mut body)
            .await
            .expect("token frame")
            .expect("ok frame");
        let chunk = String::from_utf8_lossy(&frame);
        assert!(
            !chunk.contains("event: close"),
            "expected token before revoke: {chunk}"
        );
        consumed += 1;
    }
    // Stop app emission, end the HTTP body (releases session guard), then mutate.
    // App guarantee: no new frames enqueued after revoke (bounded transport tail
    // may still exist at Hyper/kernel — not claimed as zero network bytes).
    fence
        .revoke_and_drain(fileconv_server::services::qa::authz_fence::CloseKind::Revoked)
        .await;
    let (later, reason) = collect_sse_token_text(body).await;
    assert_eq!(reason, StreamCloseReason::AuthzRevoked);
    assert!(
        later.is_empty(),
        "no new app tokens after revoke; consumed={consumed} later={later:?}"
    );
    fileconv_server::services::qa::revoke_membership_with_fence(
        &lock_pool,
        &fx.ctx,
        &fence,
        fx.ctx.org_id(),
        fx.ctx.user_id(),
    )
    .await
    .expect("revoke after body end");

    db.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn live_pg_current_pointer_race_and_delete_during_stream() {
    let base =
        test_database_url().expect("MARKHAND_TEST_DATABASE_URL required for ignored live test");
    let db = EphemeralDb::create(&base).await;
    apply_migrations(&db.url).await.expect("migrations");
    let pool = create_pool(&db.url).expect("pool");
    let lock_pool = LockPool::new(&db.url).expect("lock pool");
    let store = MemoryBlobStore::default();
    let fx = seed_live_fixture(&pool, &store).await;

    let hits = vec![RetrievalHit {
        chunk_id: fx.chunk_id,
        chunk_identity_sha256: "a".repeat(64),
        collection_id: fx.collection_id,
        document_id: fx.document_id,
        version_id: fx.version_current,
        version_number: 3,
        content_sha256: fx.markdown_sha.clone(),
        heading: "Mục".into(),
        snippet: fx.quote.clone(),
        body: fx.quote.clone(),
        lexical_score: 1.0,
        vector_score: 0.5,
        rerank_score: 1.5,
        is_current: true,
        effective_from: Utc.with_ymd_and_hms(2024, 6, 1, 0, 0, 0).unwrap(),
        effective_to: None,
        page: None,
        slide: None,
        sheet: None,
        span_start: fx.quote_start,
        span_end: fx.quote_end,
    }];
    let retrieval = RetrievalResponse {
        hits,
        warnings: vec![],
        embedding_mode: "test".into(),
        conflict_evidence: vec![],
        vector_weight: 0.55,
        provenance: RetrievalProvenance {
            org_id: fx.ctx.org_id(),
            user_id: fx.ctx.user_id(),
            mode: VersionMode::Current,
            collection_ids: BTreeSet::from([fx.collection_id]),
            retrieved_at: Utc::now(),
            document_id: None,
        },
    };

    // Publish a new current version so the cited version is no longer current.
    let version_new = Uuid::new_v4();
    with_org_txn(&pool, &fx.ctx, {
        let ctx = fx.ctx.clone();
        let version_current = fx.version_current;
        let document_id = fx.document_id;
        let markdown_sha = fx.markdown_sha.clone();
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "UPDATE document_versions SET is_current = false
                     WHERE org_id = $1 AND id = $2",
                    &[&ctx.org_id(), &version_current],
                )
                .await?;
                txn.execute(
                    "INSERT INTO document_versions (
                        id, org_id, document_id, version_number, parent_version_id,
                        publication_state, is_current, content_sha256, original_object_key,
                        markdown_object_key, source_filename, source_content_type, byte_size,
                        effective_from, created_by_user_id
                     ) SELECT
                        $1,$2,$3,4,$4,'published',true,$5, original_object_key,
                        markdown_object_key, source_filename, source_content_type, byte_size,
                        now(), created_by_user_id
                     FROM document_versions WHERE id = $4 AND org_id = $2",
                    &[
                        &version_new,
                        &ctx.org_id(),
                        &document_id,
                        &version_current,
                        &markdown_sha,
                    ],
                )
                .await?;
                txn.execute(
                    "UPDATE documents SET current_version_id = $1 WHERE id = $2 AND org_id = $3",
                    &[&version_new, &document_id, &ctx.org_id()],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .unwrap();

    let raced = answer_question_live::<_, ScriptedProvider>(
        &pool,
        &lock_pool,
        &store,
        &fx.ctx,
        QaRequest {
            question: "Kinh phí phê duyệt hiện tại là bao nhiêu theo tài liệu?".into(),
            mode: VersionMode::Current,
            use_llm: false,
            collection_ids: None,
        },
        retrieval.clone(),
        None,
        None,
        None,
    )
    .await;
    assert!(
        matches!(raced, Err(QaError::StaleRetrieval)),
        "pointer race must fail closed for re-retrieve, got {raced:?}"
    );

    // Fresh fixture stream + delete mid-stream.
    let db2 = EphemeralDb::create(&base).await;
    apply_migrations(&db2.url).await.expect("migrations");
    let pool2 = create_pool(&db2.url).expect("pool");
    let lock_pool2 = LockPool::new(&db2.url).expect("lock pool");
    let store2 = MemoryBlobStore::default();
    let fx2 = seed_live_fixture(&pool2, &store2).await;
    let hits2 = vec![RetrievalHit {
        chunk_id: fx2.chunk_id,
        chunk_identity_sha256: "a".repeat(64),
        collection_id: fx2.collection_id,
        document_id: fx2.document_id,
        version_id: fx2.version_current,
        version_number: 3,
        content_sha256: fx2.markdown_sha.clone(),
        heading: "Mục".into(),
        snippet: format!("{} {}", fx2.quote, "pad ".repeat(60)),
        body: format!("{} {}", fx2.quote, "pad ".repeat(60)),
        lexical_score: 1.0,
        vector_score: 0.5,
        rerank_score: 1.5,
        is_current: true,
        effective_from: Utc.with_ymd_and_hms(2024, 6, 1, 0, 0, 0).unwrap(),
        effective_to: None,
        page: None,
        slide: None,
        sheet: None,
        span_start: fx2.quote_start,
        span_end: fx2.quote_end,
    }];
    let retrieval2 = RetrievalResponse {
        hits: hits2,
        warnings: vec![],
        embedding_mode: "test".into(),
        conflict_evidence: vec![],
        vector_weight: 0.55,
        provenance: RetrievalProvenance {
            org_id: fx2.ctx.org_id(),
            user_id: fx2.ctx.user_id(),
            mode: VersionMode::Current,
            collection_ids: BTreeSet::from([fx2.collection_id]),
            retrieved_at: Utc::now(),
            document_id: None,
        },
    };
    let rx = stream_answer_live(StreamLiveInput::<_, ScriptedProvider> {
        pool: &pool2,
        lock_pool: &lock_pool2,
        storage: &store2,
        ctx: &fx2.ctx,
        request: QaRequest {
            question: "Kinh phí phê duyệt hiện tại là bao nhiêu theo tài liệu để stream xóa?"
                .into(),
            mode: VersionMode::Current,
            use_llm: false,
            collection_ids: None,
        },
        retrieval: retrieval2,
        provider: None,
        provider_config: None,
        cancel: StreamCancel::new(),
        bounds: StreamBounds {
            max_tokens: 1000,
            max_bytes: 64 * 1024,
            buffer: 64,
            backpressure_wait: Duration::from_secs(1),
            overall_timeout: Some(Duration::from_secs(5)),
            source_wait: Duration::from_secs(1),
        },
    })
    .await
    .expect("stream before delete");
    let fence = rx.fence().cloned().expect("fence");
    let mut body = rx.into_sse_body();
    let frame = poll_sse_frame(&mut body)
        .await
        .expect("token frame")
        .expect("ok frame");
    let chunk = String::from_utf8_lossy(&frame);
    assert!(
        !chunk.contains("event: close"),
        "expected token before delete, got {chunk}"
    );
    // Stop app emission, end body (release session guard), then exclusive mutate.
    fence.revoke_and_drain(AuthzCloseKind::Deleted).await;
    let (later, reason) = collect_sse_token_text(body).await;
    assert_eq!(reason, StreamCloseReason::DocumentDeleted);
    assert!(
        later.is_empty(),
        "no new app tokens after delete signal: {later:?}"
    );
    fileconv_server::db::authz_lock::with_exclusive_mutation(
        &lock_pool2,
        &fx2.ctx,
        fx2.ctx.org_id(),
        fx2.ctx.user_id(),
        &[fx2.document_id],
        {
            let ctx = fx2.ctx.clone();
            let document_id = fx2.document_id;
            move |txn| {
                Box::pin(async move {
                    fileconv_server::db::authz_epoch::bump_document_epoch(
                        txn,
                        ctx.org_id(),
                        document_id,
                    )
                    .await?;
                    txn.execute(
                        "UPDATE documents SET state = 'tombstoned', deleted_at = now()
                         WHERE id = $1 AND org_id = $2",
                        &[&document_id, &ctx.org_id()],
                    )
                    .await?;
                    Ok(())
                })
            }
        },
    )
    .await
    .expect("delete after body end");

    db.drop().await;
    db2.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn live_pg_advisory_delivery_blocks_independent_pool_mutation() {
    use fileconv_server::db::authz_lock::{with_exclusive_mutation, DeliveryGuard};
    use std::sync::Arc;
    use tokio::sync::Notify;

    let base =
        test_database_url().expect("MARKHAND_TEST_DATABASE_URL required for ignored live test");
    let db = EphemeralDb::create(&base).await;
    apply_migrations(&db.url).await.expect("migrations");
    // Independent pools (separate connections) for delivery vs mutation.
    let delivery_pool = create_pool_with_max_size(&db.url, 2).expect("delivery pool");
    let delivery_locks = LockPool::with_capacity(
        &db.url,
        4,
        4,
        std::time::Duration::from_secs(5),
        std::time::Duration::from_secs(5),
    )
    .expect("delivery locks");
    let mutation_locks = LockPool::with_capacity(
        &db.url,
        4,
        4,
        std::time::Duration::from_secs(5),
        std::time::Duration::from_secs(5),
    )
    .expect("mutation locks");
    let store = MemoryBlobStore::default();
    let fx = seed_live_fixture(&delivery_pool, &store).await;

    let guard = DeliveryGuard::acquire_shared(
        &delivery_locks,
        fx.ctx.org_id(),
        fx.ctx.user_id(),
        &[fx.document_id],
        &[fx.collection_id],
    )
    .await
    .expect("shared delivery lock");

    let started = Arc::new(Notify::new());
    let finished = Arc::new(Notify::new());
    let started2 = started.clone();
    let finished2 = finished.clone();
    let ctx = fx.ctx.clone();
    let org_id = ctx.org_id();
    let user_id = ctx.user_id();
    let doc = fx.document_id;
    let mutation_locks2 = mutation_locks.clone();
    let join = tokio::spawn(async move {
        started2.notify_one();
        with_exclusive_mutation(
            &mutation_locks2,
            &ctx,
            org_id,
            user_id,
            &[doc],
            move |txn| {
                Box::pin(async move {
                    fileconv_server::db::authz_epoch::bump_document_epoch(txn, org_id, doc).await?;
                    Ok(())
                })
            },
        )
        .await
        .expect("exclusive mutation after delivery unlock");
        finished2.notify_one();
    });

    started.notified().await;
    // Mutation must not finish while shared delivery guard is held.
    tokio::select! {
        _ = finished.notified() => panic!("exclusive mutation raced ahead of delivery guard"),
        _ = tokio::time::sleep(Duration::from_millis(200)) => {}
    }
    drop(guard);
    tokio::time::timeout(Duration::from_secs(5), finished.notified())
        .await
        .expect("mutation proceeds after delivery unlock");
    join.await.unwrap();
    db.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn live_pg_authz_mutation_apis_require_member_manage_and_access_level() {
    use fileconv_server::db::models::AccessLevel;
    use fileconv_server::services::authz_mutation::{
        grant_collection_user_access, grant_membership, revoke_collection_user_access,
        revoke_membership, AuthzMutationError, PERMISSION_MEMBER_MANAGE,
    };

    let base =
        test_database_url().expect("MARKHAND_TEST_DATABASE_URL required for ignored live test");
    let db = EphemeralDb::create(&base).await;
    apply_migrations(&db.url).await.expect("migrations");
    let pool = create_pool(&db.url).expect("pool");
    let lock_pool = LockPool::new(&db.url).expect("lock pool");
    let store = MemoryBlobStore::default();
    let fx = seed_live_fixture(&pool, &store).await;
    let target = Uuid::new_v4();
    let viewer_actor = Uuid::new_v4();

    // Seed target (no membership) + viewer actor (membership without member.manage).
    with_org_txn(&pool, &fx.ctx, {
        let org = fx.ctx.org_id();
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "INSERT INTO users (id, email, display_name, password_hash)
                     VALUES ($1, $2, 'target', 'test-hash'), ($3, $4, 'viewer', 'test-hash')",
                    &[
                        &target,
                        &format!("{target}@example.test"),
                        &viewer_actor,
                        &format!("{viewer_actor}@example.test"),
                    ],
                )
                .await?;
                let viewer_role = Uuid::new_v4();
                txn.execute(
                    "INSERT INTO roles (id, org_id, code, name, is_system)
                     VALUES ($1, $2, 'viewer', 'Viewer', false)",
                    &[&viewer_role, &org],
                )
                .await?;
                txn.execute(
                    "INSERT INTO role_permissions (org_id, role_id, permission_id)
                     SELECT $1, $2, id FROM permissions WHERE code = 'qa.query'",
                    &[&org, &viewer_role],
                )
                .await?;
                txn.execute(
                    "INSERT INTO org_memberships (org_id, user_id, role)
                     VALUES ($1, $2, 'viewer')",
                    &[&org, &viewer_actor],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("seed target + viewer actor");

    let admin = OrgContext::try_new(
        fx.ctx.org_id(),
        fx.ctx.user_id(),
        [
            PERMISSION_QA_QUERY,
            PERMISSION_QA_HISTORY,
            PERMISSION_MEMBER_MANAGE,
        ],
        [fx.collection_id],
    )
    .unwrap();
    // Denial is DB-resolved member.manage inside the locked txn (not OrgContext alone).
    let no_admin = OrgContext::try_new(
        fx.ctx.org_id(),
        viewer_actor,
        [PERMISSION_QA_QUERY, PERMISSION_MEMBER_MANAGE],
        [fx.collection_id],
    )
    .unwrap();

    let denied = grant_membership(&lock_pool, &no_admin, fx.ctx.org_id(), target, "viewer").await;
    assert_eq!(denied, Err(AuthzMutationError::PermissionDenied));

    grant_membership(&lock_pool, &admin, fx.ctx.org_id(), target, "viewer")
        .await
        .expect("grant membership");
    grant_collection_user_access(
        &lock_pool,
        &admin,
        fx.ctx.org_id(),
        fx.collection_id,
        target,
        AccessLevel::Read,
    )
    .await
    .expect("grant ACL with required access_level");
    revoke_collection_user_access(
        &lock_pool,
        &admin,
        fx.ctx.org_id(),
        fx.collection_id,
        target,
    )
    .await
    .expect("revoke ACL");
    revoke_membership(&lock_pool, &admin, fx.ctx.org_id(), target)
        .await
        .expect("revoke membership");
    db.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn live_pg_advisory_lock_timeout_fails_closed_cross_process() {
    use fileconv_server::db::authz_lock::DeliveryGuard;
    use fileconv_server::db::error::DbError;

    let base =
        test_database_url().expect("MARKHAND_TEST_DATABASE_URL required for ignored live test");
    let db = EphemeralDb::create(&base).await;
    apply_migrations(&db.url).await.expect("migrations");
    let pool = create_pool_with_max_size(&db.url, 2).expect("pool");
    let store = MemoryBlobStore::default();
    let fx = seed_live_fixture(&pool, &store).await;

    // Process A: short deadline delivery pool holding shared lock.
    let holder = LockPool::with_capacity(
        &db.url,
        2,
        2,
        Duration::from_secs(5),
        Duration::from_secs(5),
    )
    .expect("holder locks");
    // Process B: tight acquire deadline — must LockTimeout, not starve.
    let waiter = LockPool::with_capacity(
        &db.url,
        2,
        2,
        Duration::from_millis(150),
        Duration::from_secs(5),
    )
    .expect("waiter locks");

    let guard = DeliveryGuard::acquire_shared(
        &holder,
        fx.ctx.org_id(),
        fx.ctx.user_id(),
        &[fx.document_id],
        &[fx.collection_id],
    )
    .await
    .expect("holder shared lock");

    let err = fileconv_server::db::authz_lock::with_exclusive_mutation(
        &waiter,
        &fx.ctx,
        fx.ctx.org_id(),
        fx.ctx.user_id(),
        &[fx.document_id],
        move |_txn| Box::pin(async move { Ok(()) }),
    )
    .await
    .expect_err("waiter must time out while holder keeps shared lock");
    assert!(
        matches!(err, DbError::LockTimeout),
        "expected LockTimeout, got {err:?}"
    );

    guard.release().await;
    // After release, mutation capacity succeeds (fail-closed did not starve slots).
    fileconv_server::db::authz_lock::with_exclusive_mutation(
        &waiter,
        &fx.ctx,
        fx.ctx.org_id(),
        fx.ctx.user_id(),
        &[fx.document_id],
        move |_txn| Box::pin(async move { Ok(()) }),
    )
    .await
    .expect("mutation after timeout+release");
    db.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn live_pg_group_and_role_acl_end_to_end() {
    use fileconv_server::db::models::AccessLevel;
    use fileconv_server::services::acl::{
        probe_collection_readable, service_grant_collection_group_access,
        service_grant_collection_role_access, service_revoke_collection_group_access,
        service_revoke_collection_role_access,
    };
    use fileconv_server::services::authz_mutation::{grant_membership, upsert_role};

    let base =
        test_database_url().expect("MARKHAND_TEST_DATABASE_URL required for ignored live test");
    let db = EphemeralDb::create(&base).await;
    apply_migrations(&db.url).await.expect("migrations");
    let pool = create_pool(&db.url).expect("pool");
    let lock_pool = LockPool::new(&db.url).expect("lock pool");
    let store = MemoryBlobStore::default();
    let fx = seed_live_fixture(&pool, &store).await;

    let outsider = Uuid::new_v4();
    let group_id = Uuid::new_v4();
    let role_id = Uuid::new_v4();
    with_org_txn(&pool, &fx.ctx, {
        let org = fx.ctx.org_id();
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "INSERT INTO users (id, email, display_name, password_hash)
                     VALUES ($1, $2, 'outsider', 'test-hash')",
                    &[&outsider, &format!("{outsider}@example.test")],
                )
                .await?;
                txn.execute(
                    "INSERT INTO groups (id, org_id, name) VALUES ($1, $2, 'readers')",
                    &[&group_id, &org],
                )
                .await?;
                txn.execute(
                    "INSERT INTO group_memberships (org_id, group_id, user_id)
                     VALUES ($1, $2, $3)",
                    &[&org, &group_id, &outsider],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("seed group");

    // Outsider not yet org member with role — grant membership + role ACL path.
    let admin = OrgContext::try_new(
        fx.ctx.org_id(),
        fx.ctx.user_id(),
        [
            PERMISSION_QA_QUERY,
            PERMISSION_QA_HISTORY,
            PERMISSION_MEMBER_MANAGE,
        ],
        [fx.collection_id],
    )
    .unwrap();

    // Make collection private so org visibility does not grant access.
    with_org_txn(&pool, &fx.ctx, {
        let org = fx.ctx.org_id();
        let cid = fx.collection_id;
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "UPDATE collections SET visibility = 'private' WHERE org_id = $1 AND id = $2",
                    &[&org, &cid],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .unwrap();

    grant_membership(&lock_pool, &admin, fx.ctx.org_id(), outsider, "viewer")
        .await
        .expect("grant membership");

    assert!(
        !probe_collection_readable(&pool, &admin, fx.collection_id, outsider)
            .await
            .unwrap()
    );

    service_grant_collection_group_access(
        &lock_pool,
        &pool,
        &admin,
        fx.ctx.org_id(),
        fx.collection_id,
        group_id,
        AccessLevel::Read,
    )
    .await
    .expect("grant group ACL");
    assert!(
        probe_collection_readable(&pool, &admin, fx.collection_id, outsider)
            .await
            .unwrap()
    );
    service_revoke_collection_group_access(
        &lock_pool,
        &pool,
        &admin,
        fx.ctx.org_id(),
        fx.collection_id,
        group_id,
    )
    .await
    .expect("revoke group ACL");
    assert!(
        !probe_collection_readable(&pool, &admin, fx.collection_id, outsider)
            .await
            .unwrap()
    );

    // Role ACL joins roles.code = org_memberships.role (owner/admin/editor/viewer).
    upsert_role(
        &lock_pool,
        &admin,
        fx.ctx.org_id(),
        role_id,
        "viewer",
        "Viewer",
    )
    .await
    .expect("upsert role");
    with_org_txn(&pool, &fx.ctx, {
        let org = fx.ctx.org_id();
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "INSERT INTO role_permissions (org_id, role_id, permission_id)
                     SELECT $1, $2, id FROM permissions WHERE code = 'qa.query'
                     ON CONFLICT DO NOTHING",
                    &[&org, &role_id],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("grant qa.query on viewer role");

    service_grant_collection_role_access(
        &lock_pool,
        &pool,
        &admin,
        fx.ctx.org_id(),
        fx.collection_id,
        role_id,
        AccessLevel::Read,
    )
    .await
    .expect("grant role ACL");
    assert!(
        probe_collection_readable(&pool, &admin, fx.collection_id, outsider)
            .await
            .unwrap()
    );
    service_revoke_collection_role_access(
        &lock_pool,
        &pool,
        &admin,
        fx.ctx.org_id(),
        fx.collection_id,
        role_id,
    )
    .await
    .expect("revoke role ACL");
    assert!(
        !probe_collection_readable(&pool, &admin, fx.collection_id, outsider)
            .await
            .unwrap()
    );

    // R9.6: group/role QA end-to-end — outsider can ask only while ACL grants.
    service_grant_collection_group_access(
        &lock_pool,
        &pool,
        &admin,
        fx.ctx.org_id(),
        fx.collection_id,
        group_id,
        AccessLevel::Read,
    )
    .await
    .expect("re-grant group for QA");
    let outsider_ctx = OrgContext::try_new(
        fx.ctx.org_id(),
        outsider,
        [PERMISSION_QA_QUERY],
        [fx.collection_id],
    )
    .unwrap();
    let hits = vec![RetrievalHit {
        chunk_id: fx.chunk_id,
        chunk_identity_sha256: "a".repeat(64),
        collection_id: fx.collection_id,
        document_id: fx.document_id,
        version_id: fx.version_current,
        version_number: 3,
        content_sha256: fx.markdown_sha.clone(),
        heading: "Mục".into(),
        snippet: fx.quote.clone(),
        body: fx.quote.clone(),
        lexical_score: 1.0,
        vector_score: 0.5,
        rerank_score: 1.5,
        is_current: true,
        effective_from: Utc.with_ymd_and_hms(2024, 6, 1, 0, 0, 0).unwrap(),
        effective_to: None,
        page: None,
        slide: None,
        sheet: None,
        span_start: fx.quote_start,
        span_end: fx.quote_end,
    }];
    let retrieval = RetrievalResponse {
        hits,
        warnings: vec![],
        embedding_mode: "test".into(),
        conflict_evidence: vec![],
        vector_weight: 0.55,
        provenance: RetrievalProvenance {
            org_id: outsider_ctx.org_id(),
            user_id: outsider_ctx.user_id(),
            mode: VersionMode::Current,
            collection_ids: BTreeSet::from([fx.collection_id]),
            retrieved_at: Utc::now(),
            document_id: None,
        },
    };
    let qa_ok = answer_question_live::<_, ScriptedProvider>(
        &pool,
        &lock_pool,
        &store,
        &outsider_ctx,
        QaRequest {
            question: "Kinh phí phê duyệt hiện tại là bao nhiêu theo tài liệu?".into(),
            mode: VersionMode::Current,
            use_llm: false,
            collection_ids: None,
        },
        retrieval.clone(),
        None,
        None,
        None,
    )
    .await
    .expect("outsider QA under group ACL");
    assert!(qa_ok.grounded());
    qa_ok.finish().await;

    service_revoke_collection_group_access(
        &lock_pool,
        &pool,
        &admin,
        fx.ctx.org_id(),
        fx.collection_id,
        group_id,
    )
    .await
    .expect("revoke group before denied QA");
    let qa_denied = answer_question_live::<_, ScriptedProvider>(
        &pool,
        &lock_pool,
        &store,
        &outsider_ctx,
        QaRequest {
            question: "Kinh phí phê duyệt hiện tại là bao nhiêu theo tài liệu?".into(),
            mode: VersionMode::Current,
            use_llm: false,
            collection_ids: None,
        },
        retrieval,
        None,
        None,
        None,
    )
    .await;
    assert!(
        matches!(
            qa_denied,
            Err(QaError::PermissionDenied) | Err(QaError::StaleRetrieval)
        ) || qa_denied.as_ref().is_ok_and(|a| !a.grounded()),
        "outsider QA must fail closed after group revoke: {qa_denied:?}"
    );
    if let Ok(a) = qa_denied {
        a.finish().await;
    }

    db.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn live_tower_hyper_body_drop_eof_stall_watchdog() {
    use axum::body::Body as AxumBody;
    use fileconv_server::db::authz_lock::{with_exclusive_mutation, DeliveryGuard};
    use fileconv_server::services::qa::stream::{GuardedSseBody, DEFAULT_STALL_WATCHDOG};
    use fileconv_server::services::qa::{StreamCancel, StreamCloseReason, StreamEvent};
    use http_body_util::BodyExt;
    use std::time::Duration;
    use tokio::sync::mpsc;

    let base =
        test_database_url().expect("MARKHAND_TEST_DATABASE_URL required for ignored live test");
    let db = EphemeralDb::create(&base).await;
    apply_migrations(&db.url).await.expect("migrations");
    let pool = create_pool_with_max_size(&db.url, 4).expect("pool");
    let lock_pool = LockPool::new(&db.url).expect("lock pool");
    let store = MemoryBlobStore::default();
    let fx = seed_live_fixture(&pool, &store).await;

    // Drop path: acquire shared delivery, wrap Axum body, drop without EOF → locks free.
    {
        let guard = DeliveryGuard::acquire_shared(
            &lock_pool,
            fx.ctx.org_id(),
            fx.ctx.user_id(),
            &[fx.document_id],
            &[fx.collection_id],
        )
        .await
        .expect("shared");
        let (tx, rx) = mpsc::channel::<StreamEvent>(2);
        drop(tx);
        let sse = GuardedSseBody::new(
            rx,
            Some(guard),
            None,
            Some(StreamCancel::new()),
            None,
            Duration::from_secs(30),
            DEFAULT_STALL_WATCHDOG,
        );
        let body = AxumBody::new(sse);
        drop(body);
        with_exclusive_mutation(
            &lock_pool,
            &fx.ctx,
            fx.ctx.org_id(),
            fx.ctx.user_id(),
            &[fx.document_id],
            |_txn| Box::pin(async move { Ok(()) }),
        )
        .await
        .expect("mutation after body drop");
    }

    // EOF path: collect Axum body to end, finish releases guard.
    {
        let guard = DeliveryGuard::acquire_shared(
            &lock_pool,
            fx.ctx.org_id(),
            fx.ctx.user_id(),
            &[fx.document_id],
            &[fx.collection_id],
        )
        .await
        .expect("shared eof");
        let (tx, rx) = mpsc::channel::<StreamEvent>(4);
        tx.send(StreamEvent::Token("hi".into())).await.unwrap();
        tx.send(StreamEvent::Closed {
            reason: StreamCloseReason::Completed,
        })
        .await
        .unwrap();
        drop(tx);
        let sse = GuardedSseBody::new(
            rx,
            Some(guard),
            None,
            Some(StreamCancel::new()),
            Some(bytes::Bytes::from(r#"{"grounded":true}"#)),
            Duration::from_secs(30),
            DEFAULT_STALL_WATCHDOG,
        );
        let body = AxumBody::new(sse);
        let collected = body.collect().await.expect("collect").to_bytes();
        let text = String::from_utf8_lossy(&collected);
        assert!(text.contains("event: metadata"));
        assert!(text.contains("event: close"));
        with_exclusive_mutation(
            &lock_pool,
            &fx.ctx,
            fx.ctx.org_id(),
            fx.ctx.user_id(),
            &[fx.document_id],
            |_txn| Box::pin(async move { Ok(()) }),
        )
        .await
        .expect("mutation after EOF collect");
    }

    // Stall watchdog: no polls → independent cancel + guard release.
    {
        let guard = DeliveryGuard::acquire_shared(
            &lock_pool,
            fx.ctx.org_id(),
            fx.ctx.user_id(),
            &[fx.document_id],
            &[fx.collection_id],
        )
        .await
        .expect("shared stall");
        let (_tx, rx) = mpsc::channel::<StreamEvent>(2);
        let sse = GuardedSseBody::new(
            rx,
            Some(guard),
            None,
            Some(StreamCancel::new()),
            None,
            Duration::from_secs(30),
            Duration::from_millis(100),
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
        let (later, reason) = collect_sse_token_text(sse).await;
        assert_eq!(reason, StreamCloseReason::StallWatchdog);
        assert!(later.is_empty());
        with_exclusive_mutation(
            &lock_pool,
            &fx.ctx,
            fx.ctx.org_id(),
            fx.ctx.user_id(),
            &[fx.document_id],
            |_txn| Box::pin(async move { Ok(()) }),
        )
        .await
        .expect("mutation after stall watchdog");
    }

    db.drop().await;
}
