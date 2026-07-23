//! P1B-R03 live ask grounding: fail-closed extractive + delete-during-stream.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use fileconv_knowledge::ask::AnswerMode;
use fileconv_knowledge::identity::BODY_TEXT_VERSION;
use fileconv_server::auth::context::OrgContext;
use fileconv_server::auth::jwt::{AccessClaims, JwtKeys};
use fileconv_server::db::collections::{self, NewCollection};
use fileconv_server::db::documents::{self, NewDocument};
use fileconv_server::db::models::{ArtifactKind, DocumentState};
use fileconv_server::db::pool::with_org_txn;
use fileconv_server::services::chunking::prepare_chunks;
use fileconv_server::services::qa::provider::{ChatProvider, StaticChatProvider};
use fileconv_server::services::qa::stream::{ask_response_events, auth_closed_envelope};
use fileconv_server::services::qa::{ask, structured_entailment_available, AskRequest};
use fileconv_server::services::retrieval::VersionMode;
use fileconv_server::services::stream_auth::revalidate_ask_stream;
use fileconv_server::storage::minio::ObjectIdentityMeta;
use http_body_util::BodyExt;
use tower::ServiceExt;
use uuid::Uuid;

use common::{
    admin_database_url, app_database_url, assert_markhand_app_role, boot_app_pool, build_router,
    login_access_token, put_bytes, seed_user_with_permissions, sha256_hex, test_auth_config,
    test_minio_client, trusted_key,
};

#[test]
fn production_ask_path_is_extractive_while_entailment_unavailable() {
    assert!(
        !structured_entailment_available(),
        "must not claim GLM grounded answers without verified entailment"
    );
}

#[tokio::test]
async fn injectable_failing_and_timeout_providers_surface_provider_errors() {
    let messages = fileconv_server::services::qa::prompt::build_grounded_messages(
        "Kinh phí?",
        &[],
        &VersionMode::Current,
    );
    assert!(matches!(
        ChatProvider::Failing.complete(&messages).await,
        Err(fileconv_server::services::qa::provider::ProviderError::Transport)
    ));
    assert!(matches!(
        ChatProvider::Timeout.complete(&messages).await,
        Err(fileconv_server::services::qa::provider::ProviderError::Timeout)
    ));
    let provider = ChatProvider::Static(StaticChatProvider::new(
        "Fabricated [CITE-9999]",
        AnswerMode::LocalLlm,
    ));
    assert_eq!(
        provider.complete(&messages).await.unwrap(),
        "Fabricated [CITE-9999]"
    );
}

async fn seed_ask_doc(
    pool: &deadpool_postgres::Pool,
    store: &fileconv_server::storage::minio::MinioClient,
    markdown: &str,
) -> (OrgContext, Uuid, Uuid, String) {
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let perms = ["qa.query", "qa.history", "doc.upload", "doc.delete"];
    seed_user_with_permissions(
        pool,
        org,
        user,
        &format!("{user}@ask.test"),
        "correct-password-1",
        &perms,
    )
    .await;
    let collection_id = Uuid::new_v4();
    let document_id = Uuid::new_v4();
    let version_id = Uuid::new_v4();
    let artifact_id = Uuid::new_v4();
    let index_meta_id = Uuid::new_v4();
    let markdown_sha = sha256_hex(markdown.as_bytes());
    let key = trusted_key(org, version_id, Uuid::new_v4(), None).unwrap();
    let ctx = OrgContext::try_new(org, user, perms, [collection_id]).unwrap();
    put_bytes(
        store,
        org,
        &key,
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
    let chunks = prepare_chunks(document_id, version_id, markdown, "md");
    let signature = format!("{:0>64}", index_meta_id.as_u128());
    let key_str = key.as_str();
    let md_len = markdown.len() as i64;
    with_org_txn(pool, &ctx, {
        let ctx = ctx.clone();
        let markdown_sha = markdown_sha.clone();
        let chunks = chunks.clone();
        let signature = signature.clone();
        move |txn| {
            Box::pin(async move {
                collections::insert(
                    txn,
                    &ctx,
                    NewCollection {
                        id: collection_id,
                        name: "Ask collection",
                        slug: &format!("ask-{}", collection_id.simple()),
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
                        title: "Ask doc",
                    },
                )
                .await?;
                txn.execute(
                    "INSERT INTO document_versions (
                        id, org_id, document_id, version_number, publication_state,
                        is_current, content_sha256, original_object_key, markdown_object_key,
                        source_content_type, byte_size, created_by_user_id
                     ) VALUES ($1,$2,$3,1,'published',true,$4,$5,$5,'text/markdown',$6,$7)",
                    &[
                        &version_id,
                        &ctx.org_id(),
                        &document_id,
                        &markdown_sha,
                        &key_str,
                        &md_len,
                        &ctx.user_id(),
                    ],
                )
                .await?;
                let kind = ArtifactKind::Markdown.as_str();
                txn.execute(
                    "INSERT INTO derived_artifacts (
                        id, org_id, document_id, version_id, artifact_kind,
                        object_key, content_sha256, content_type, byte_size
                     ) VALUES ($1,$2,$3,$4,$5,$6,$7,'text/markdown; charset=utf-8',$8)",
                    &[
                        &artifact_id,
                        &ctx.org_id(),
                        &document_id,
                        &version_id,
                        &kind,
                        &key_str,
                        &markdown_sha,
                        &md_len,
                    ],
                )
                .await?;
                let indexed = DocumentState::Indexed.as_str();
                txn.execute(
                    "UPDATE documents SET state=$3, current_version_id=$4 WHERE org_id=$1 AND id=$2",
                    &[&ctx.org_id(), &document_id, &indexed, &version_id],
                )
                .await?;
                txn.execute(
                    "INSERT INTO index_metadata (
                        id, org_id, collection_id, index_signature_sha256, embedding_family,
                        embedding_revision, dimensions, runtime_path, generation, is_active, state
                     ) VALUES ($1,$2,$3,$4,'test','r1',8,'local-hash',1,true,'active')",
                    &[&index_meta_id, &ctx.org_id(), &collection_id, &signature],
                )
                .await?;
                for chunk in chunks {
                    fileconv_server::db::chunks::insert(
                        txn,
                        &ctx,
                        fileconv_server::db::chunks::NewChunk {
                            id: Uuid::new_v4(),
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
    .expect("seed ask doc");
    let token = login_access_token(pool, &format!("{user}@ask.test"), "correct-password-1").await;
    (ctx, document_id, version_id, token)
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL/APP + MARKHAND_TEST_MINIO_* + MARKHAND_TEST_QDRANT_URL"]
async fn live_ask_is_extractive_and_delete_during_stream_closes() {
    let Some(admin) = admin_database_url() else {
        return;
    };
    let Some(app) = app_database_url() else {
        return;
    };
    let Some(store) = test_minio_client() else {
        return;
    };
    let qdrant_url = match std::env::var("MARKHAND_TEST_QDRANT_URL") {
        Ok(url) if !url.trim().is_empty() => url,
        _ => {
            eprintln!("skipped: MARKHAND_TEST_QDRANT_URL unset");
            return;
        }
    };
    let (ephemeral, pool) = boot_app_pool(&admin, &app).await;
    assert_markhand_app_role(&pool).await;

    let markdown = "# BA\n\nKinh phí được phê duyệt là 15 triệu đồng.\n";
    let (ctx, document_id, _version_id, token) = seed_ask_doc(&pool, &store, markdown).await;

    let qdrant = fileconv_server::storage::QdrantClient::new(&qdrant_url).expect("qdrant");
    let response = ask(
        &pool,
        &qdrant,
        None,
        None,
        &ctx,
        AskRequest {
            question: "Kinh phí được phê duyệt là bao nhiêu?".into(),
            collection_ids: Some(
                [ctx.allowed_collection_ids().iter().copied().next().unwrap()].into(),
            ),
            mode: VersionMode::Current,
            limit: 5,
            conflict_ids: vec![],
        },
    )
    .await
    .expect("ask");
    assert_eq!(response.mode, AnswerMode::OfflineExtractive);
    assert!(response
        .warnings
        .iter()
        .any(|w| w.contains("fail-closed") || w.contains("extractive")));
    assert!(!response
        .answer
        .to_ascii_lowercase()
        .contains("glm grounded"));

    // Stream auth closes when cited document is deleted mid-stream.
    let keys = JwtKeys::from_auth(&test_auth_config()).unwrap();
    // Reconstruct claims from a fresh login token decode path via AuthenticatedOrg is heavy;
    // instead exercise revalidate_ask_stream with minted claims matching the seeded user.
    let claims: AccessClaims = keys.verify_access_token(&token).expect("verify access");
    revalidate_ask_stream(&pool, &claims, &[document_id])
        .await
        .expect("stream auth before delete");

    with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let tombstoned = DocumentState::Tombstoned.as_str();
                txn.execute(
                    "UPDATE documents
                     SET state = $3, deleted_at = clock_timestamp()
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

    let denied = revalidate_ask_stream(&pool, &claims, &[document_id])
        .await
        .expect_err("delete during stream must deny");
    assert_eq!(denied.close_reason(), "citation_revoked");
    let closed = auth_closed_envelope(9, "req", denied.close_reason());
    assert_eq!(closed.event, "stream.closed");

    // HTTP ask route also stays extractive-only.
    let router = build_router(pool.clone(), &ephemeral.app_url, Some(store));
    // Document is tombstoned; ask may return empty extractive but must not 500 claiming grounded GLM.
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/ask")
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "question": "Kinh phí?",
                        "mode": "current",
                        "limit": 3
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    // May be 200 with empty hits or an auth/not-found style response; never claim grounded GLM.
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&body);
    assert!(
        status == StatusCode::OK
            || status == StatusCode::BAD_REQUEST
            || status == StatusCode::NOT_FOUND
            || status == StatusCode::FORBIDDEN
            || status == StatusCode::UNAUTHORIZED
            || status == StatusCode::SERVICE_UNAVAILABLE,
        "unexpected status {status}: {text}"
    );
    assert!(!text.to_ascii_lowercase().contains("\"mode\":\"local_llm\""));
    assert!(!text.to_ascii_lowercase().contains("\"mode\":\"cloud_llm\""));

    let events = ask_response_events(
        "req",
        &fileconv_server::services::qa::AskResponse {
            answer: "extractive".into(),
            mode: AnswerMode::OfflineExtractive,
            citations: vec![],
            warnings: vec!["fail-closed".into()],
            version_context: fileconv_server::services::qa::grounding::VersionContext {
                mode: "current".into(),
                current_version_ids: vec![],
                cited_version_ids: vec![],
                change_note: None,
            },
            embedding_mode: "fts_only".into(),
        },
    );
    assert!(events.iter().any(|e| e.event == "ask.warning"));

    ephemeral.drop().await;
}
