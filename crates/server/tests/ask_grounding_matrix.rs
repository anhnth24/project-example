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
use fileconv_server::services::qa::provider::{
    ChatProvider, StaticChatProvider, StreamingStaticProvider,
};
use fileconv_server::services::qa::stream::{ask_response_events, auth_closed_envelope};
use fileconv_server::services::qa::{ask, structured_entailment_available, AskRequest};
use fileconv_server::services::retrieval::VersionMode;
use fileconv_server::services::stream_auth::revalidate_ask_stream;
use fileconv_server::storage::minio::ObjectIdentityMeta;
use futures::StreamExt;
use http_body_util::BodyExt;
use tower::ServiceExt;
use uuid::Uuid;

use common::{
    admin_database_url, app_database_url, assert_markhand_app_role, boot_app_pool, build_app_state,
    login_access_token, put_bytes, seed_user_with_permissions, sha256_hex, test_auth_config,
    test_minio_client, trusted_key, MinioCleanupGuard,
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
    let cleanup = MinioCleanupGuard::new(store.clone());
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

    // Consume the production router SSE path. The first durable event must
    // reflect a naturally retrieved citation; no SQL pinning is allowed.
    let stream_provider = ChatProvider::StreamingStatic(StreamingStaticProvider::new(
        (0..40).map(|index| format!("token-{index} ")).collect(),
        AnswerMode::LocalLlm,
    ));
    let state = build_app_state(pool.clone(), &ephemeral.app_url, Some(store.clone()))
        .with_retrieval_backends(qdrant.clone(), None)
        .with_chat_provider(stream_provider);
    let router = fileconv_server::http::router(state);
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/ask/stream")
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "question": "Kinh phí được phê duyệt là bao nhiêu?",
                        "mode": "current",
                        "limit": 5,
                        "collectionIds": ctx.allowed_collection_ids()
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream_body = response.into_body().into_data_stream();
    let first_chunk = tokio::time::timeout(std::time::Duration::from_secs(5), stream_body.next())
        .await
        .expect("router SSE first event deadline")
        .expect("router SSE body")
        .expect("router SSE chunk");
    let first_text = String::from_utf8_lossy(&first_chunk);
    let citation_count = first_text
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .filter_map(|data| serde_json::from_str::<serde_json::Value>(data.trim()).ok())
        .find_map(|event| event["data"]["citationCount"].as_u64())
        .unwrap_or(0);
    assert!(
        citation_count > 0,
        "router ask stream must naturally retrieve and pin citations: {first_text}"
    );

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

    let mut post_delete = String::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(std::time::Duration::from_millis(500), stream_body.next()).await
        {
            Ok(Some(Ok(chunk))) => {
                post_delete.push_str(&String::from_utf8_lossy(&chunk));
                if post_delete.contains("stream.closed") {
                    break;
                }
            }
            _ => break,
        }
    }
    assert!(
        post_delete.contains("stream.closed") && post_delete.contains("citation_revoked"),
        "router SSE must close with citation_revoked after delete: {post_delete}"
    );
    assert!(
        !post_delete.contains("ask.completed"),
        "router SSE must not deliver completion after cited document delete: {post_delete}"
    );

    // HTTP ask route also stays extractive-only.
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

    cleanup.cleanup().await.expect("clean ask grounding bucket");
    ephemeral.drop().await;
}

/// Seeds an org/user + collection/document with one published+current version
/// (PG only; conflict warnings never require MinIO/Qdrant/chunks).
async fn seed_conflict_capable_doc(
    pool: &deadpool_postgres::Pool,
    label: &str,
) -> (OrgContext, Uuid, Uuid) {
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let perms = ["qa.query", "qa.history", "doc.upload", "doc.delete"];
    seed_user_with_permissions(
        pool,
        org,
        user,
        &format!("{user}@{label}.test"),
        "correct-password-1",
        &perms,
    )
    .await;
    let collection_id = Uuid::new_v4();
    let document_id = Uuid::new_v4();
    let version_id = Uuid::new_v4();
    let ctx = OrgContext::try_new(org, user, perms, [collection_id]).unwrap();
    with_org_txn(pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                collections::insert(
                    txn,
                    &ctx,
                    NewCollection {
                        id: collection_id,
                        name: "Conflict collection",
                        slug: &format!("conflict-{}", collection_id.simple()),
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
                        title: "Conflict doc",
                    },
                )
                .await?;
                let sha = format!("{:0>64}", version_id.as_u128());
                let key = format!("org/{}/doc/{document_id}/v/{version_id}/source", ctx.org_id());
                txn.execute(
                    "INSERT INTO document_versions (
                        id, org_id, document_id, version_number, publication_state,
                        is_current, content_sha256, original_object_key, markdown_object_key,
                        source_content_type, byte_size, created_by_user_id
                     ) VALUES ($1,$2,$3,1,'published',true,$4,$5,$5,'text/markdown',1,$6)",
                    &[
                        &version_id,
                        &ctx.org_id(),
                        &document_id,
                        &sha,
                        &key,
                        &ctx.user_id(),
                    ],
                )
                .await?;
                let indexed = DocumentState::Indexed.as_str();
                txn.execute(
                    "UPDATE documents SET state=$3, current_version_id=$4 WHERE org_id=$1 AND id=$2",
                    &[&ctx.org_id(), &document_id, &indexed, &version_id],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("seed conflict-capable doc");
    (ctx, document_id, version_id)
}

/// Inserts two claims on the same current+published version plus an `open`
/// numeric conflict between them (canonical `claim_a_id < claim_b_id`).
async fn seed_open_conflict(
    pool: &deadpool_postgres::Pool,
    ctx: &OrgContext,
    document_id: Uuid,
    version_id: Uuid,
    value_low: i64,
    value_high: i64,
) -> Uuid {
    let claim_x = Uuid::new_v4();
    let claim_y = Uuid::new_v4();
    let (claim_a, claim_b) = if claim_x < claim_y {
        (claim_x, claim_y)
    } else {
        (claim_y, claim_x)
    };
    let conflict_id = Uuid::new_v4();
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let value_low_decimal = rust_decimal::Decimal::from(value_low);
                let value_high_decimal = rust_decimal::Decimal::from(value_high);
                let quote_low = format!("Kinh phí là {value_low} triệu đồng.");
                let quote_high = format!("Kinh phí là {value_high} triệu đồng.");
                txn.execute(
                    "INSERT INTO claims (
                        id, org_id, document_id, version_id, claim_key, subject, predicate,
                        value_type, value_money, unit, scope, effective_from, citation_quote
                     ) VALUES ($1,$2,$3,$4,'budget','Kinh phí','is','money',$5,'triệu','', now(),$6)",
                    &[
                        &claim_a,
                        &ctx.org_id(),
                        &document_id,
                        &version_id,
                        &value_low_decimal,
                        &quote_low,
                    ],
                )
                .await?;
                txn.execute(
                    "INSERT INTO claims (
                        id, org_id, document_id, version_id, claim_key, subject, predicate,
                        value_type, value_money, unit, scope, effective_from, citation_quote
                     ) VALUES ($1,$2,$3,$4,'budget','Kinh phí','is','money',$5,'triệu','', now(),$6)",
                    &[
                        &claim_b,
                        &ctx.org_id(),
                        &document_id,
                        &version_id,
                        &value_high_decimal,
                        &quote_high,
                    ],
                )
                .await?;
                txn.execute(
                    "INSERT INTO conflicts (
                        id, org_id, status, severity, conflict_type, claim_a_id, claim_b_id,
                        first_detected_version_id
                     ) VALUES ($1,$2,'open','warning','numeric',$3,$4,$5)",
                    &[&conflict_id, &ctx.org_id(), &claim_a, &claim_b, &version_id],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("seed open conflict");
    conflict_id
}

/// P1B-R03 "Remaining for Done" (b): triage-then-current/history matrix on a
/// real DB. Current mode must warn only while a conflict is `open`; history
/// mode must surface the resolution note once terminal, across every terminal
/// status (`resolved`/`accepted_exception`/`false_positive`).
#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL/APP"]
async fn live_ask_conflict_triage_then_current_and_history_matrix() {
    use fileconv_server::services::access::triage_authorized_conflict;

    let Some(admin) = admin_database_url() else {
        return;
    };
    let Some(app) = app_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_app_pool(&admin, &app).await;
    assert_markhand_app_role(&pool).await;

    // No embedder is configured anywhere in this test, so the vector leg is
    // never dispatched (see `search_all_vector_legs`'s `query_vector` guard) —
    // this Qdrant client is constructed but never dialed over the network.
    let qdrant =
        fileconv_server::storage::QdrantClient::new("http://127.0.0.1:6333").expect("qdrant");

    for status in ["resolved", "accepted_exception", "false_positive"] {
        let (ctx, document_id, version_id) =
            seed_conflict_capable_doc(&pool, &format!("conflict-{status}")).await;
        let conflict_id = seed_open_conflict(&pool, &ctx, document_id, version_id, 10, 15).await;
        let collection_id = *ctx.allowed_collection_ids().iter().next().unwrap();

        let current_before = ask(
            &pool,
            &qdrant,
            None,
            None,
            &ctx,
            AskRequest {
                question: "Kinh phí là bao nhiêu?".into(),
                collection_ids: Some([collection_id].into_iter().collect()),
                mode: VersionMode::Current,
                limit: 5,
                conflict_ids: vec![conflict_id],
            },
        )
        .await
        .expect("ask current before triage");
        assert!(
            current_before
                .warnings
                .iter()
                .any(|w| w.contains("Unresolved conflict") && w.contains(&conflict_id.to_string())),
            "status={status} expected open-conflict warning before triage: {:?}",
            current_before.warnings
        );

        let history_before = ask(
            &pool,
            &qdrant,
            None,
            None,
            &ctx,
            AskRequest {
                question: "Lịch sử kinh phí?".into(),
                collection_ids: Some([collection_id].into_iter().collect()),
                mode: VersionMode::History { document_id },
                limit: 5,
                conflict_ids: vec![conflict_id],
            },
        )
        .await
        .expect("ask history before triage");
        assert!(
            !history_before
                .warnings
                .iter()
                .any(|w| w.starts_with("Conflict ") && w.contains("status=")),
            "status={status} unexpected resolution note before triage: {:?}",
            history_before.warnings
        );

        let note = format!("Đã thống nhất theo bản mới nhất ({status}).");
        triage_authorized_conflict(&pool, &ctx, conflict_id, status, Some(&note))
            .await
            .expect("triage conflict");

        let current_after = ask(
            &pool,
            &qdrant,
            None,
            None,
            &ctx,
            AskRequest {
                question: "Kinh phí là bao nhiêu?".into(),
                collection_ids: Some([collection_id].into_iter().collect()),
                mode: VersionMode::Current,
                limit: 5,
                conflict_ids: vec![conflict_id],
            },
        )
        .await
        .expect("ask current after triage");
        assert!(
            !current_after
                .warnings
                .iter()
                .any(|w| w.contains("Unresolved conflict")),
            "status={status} triaged conflict must stop warning current-mode: {:?}",
            current_after.warnings
        );

        let history_after = ask(
            &pool,
            &qdrant,
            None,
            None,
            &ctx,
            AskRequest {
                question: "Lịch sử kinh phí?".into(),
                collection_ids: Some([collection_id].into_iter().collect()),
                mode: VersionMode::History { document_id },
                limit: 5,
                conflict_ids: vec![conflict_id],
            },
        )
        .await
        .expect("ask history after triage");
        assert!(
            history_after.warnings.iter().any(|w| {
                w.contains(&conflict_id.to_string())
                    && w.contains(&format!("status={status}"))
                    && w.contains(&note)
            }),
            "status={status} expected resolution note in history mode: {:?}",
            history_after.warnings
        );
    }

    ephemeral.drop().await;
}

/// P1B-R03 "Remaining for Done" (c): wrong-delta/same-topic contradiction
/// soak through the production `ask()` path. `force_extractive_only()` is
/// hardcoded fail-closed while structured entailment is unavailable
/// (see `services::qa::STRUCTURED_ENTAILMENT_AVAILABLE`), so this asserts the
/// *practical* guarantee under concurrent/repeated load: no wrong-delta or
/// same-topic-contradiction LLM answer can ever leak through `ask()` as a
/// claimed-grounded response, and no citation/answer cross-contaminates
/// across concurrent callers sharing one pool.
#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL/APP"]
async fn live_ask_wrong_delta_and_contradiction_soak_stays_fail_closed() {
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
    let cleanup = MinioCleanupGuard::new(store.clone());

    const SOAK_ITERATIONS: usize = 20;
    // Two independent tenants, each with a distinct real budget value. Every
    // provider deliberately fabricates a wrong-delta/contradiction answer
    // that quotes the *other* tenant's value, so any cross-tenant leak or
    // wrong-delta pass-through is directly observable in the final text.
    let markdown_a = "# BA\n\nKinh phí được phê duyệt là 15 triệu đồng.\n";
    let markdown_b = "# BA\n\nKinh phí được phê duyệt là 76 triệu đồng.\n";
    let (ctx_a, _document_id_a, _version_id_a, _token_a) =
        seed_ask_doc(&pool, &store, markdown_a).await;
    let (ctx_b, _document_id_b, _version_id_b, _token_b) =
        seed_ask_doc(&pool, &store, markdown_b).await;
    let collection_a = *ctx_a.allowed_collection_ids().iter().next().unwrap();
    let collection_b = *ctx_b.allowed_collection_ids().iter().next().unwrap();

    // Tenant A's provider fabricates B's value (wrong-delta); vice versa.
    let provider_a = ChatProvider::Static(StaticChatProvider::new(
        "Kinh phí được phê duyệt là 76 triệu đồng, không phải 15 triệu [CITE-0001].",
        AnswerMode::LocalLlm,
    ));
    let provider_b = ChatProvider::Static(StaticChatProvider::new(
        "Kinh phí được phê duyệt là 15 triệu đồng, không phải 76 triệu [CITE-0001].",
        AnswerMode::LocalLlm,
    ));

    let mut handles = Vec::with_capacity(SOAK_ITERATIONS);
    for iteration in 0..SOAK_ITERATIONS {
        let pool = pool.clone();
        let is_a = iteration % 2 == 0;
        let ctx = if is_a { ctx_a.clone() } else { ctx_b.clone() };
        let collection_id = if is_a { collection_a } else { collection_b };
        let provider = if is_a {
            provider_a.clone()
        } else {
            provider_b.clone()
        };
        handles.push(tokio::spawn(async move {
            let qdrant = fileconv_server::storage::QdrantClient::new("http://127.0.0.1:6333")
                .expect("qdrant");
            let response = ask(
                &pool,
                &qdrant,
                None,
                Some(&provider),
                &ctx,
                AskRequest {
                    question: "Kinh phí được phê duyệt là bao nhiêu?".into(),
                    collection_ids: Some([collection_id].into_iter().collect()),
                    mode: VersionMode::Current,
                    limit: 5,
                    conflict_ids: vec![],
                },
            )
            .await
            .unwrap_or_else(|error| panic!("ask iteration {iteration} failed: {error}"));
            (iteration, is_a, ctx.org_id(), response)
        }));
    }

    let mut fabricated_leaked = 0usize;
    for handle in handles {
        let (iteration, is_a, org_id, response) = handle.await.expect("soak task join");
        let (own_value, other_value) = if is_a { ("15", "76") } else { ("76", "15") };
        assert_eq!(
            response.mode,
            AnswerMode::OfflineExtractive,
            "iteration {iteration}: fail-closed must stay extractive-only while entailment is unavailable: {response:?}"
        );
        assert!(
            response.warnings.iter().any(|w| w.contains("fail-closed")),
            "iteration {iteration}: expected fail-closed warning: {:?}",
            response.warnings
        );
        if response.answer.contains(&format!("{other_value} triệu")) {
            fabricated_leaked += 1;
        }
        assert!(
            response.answer.contains(own_value) || response.citations.is_empty(),
            "iteration {iteration}: extractive answer must ground in this tenant's own value: {}",
            response.answer
        );
        // No cross-tenant contamination under concurrent soak against one pool.
        for citation in &response.citations {
            assert_eq!(
                citation.org_id, org_id,
                "iteration {iteration}: cross-tenant citation leaked under concurrency"
            );
        }
    }
    assert_eq!(
        fabricated_leaked, 0,
        "wrong-delta/contradiction provider text must never leak into an ask() answer"
    );

    cleanup.cleanup().await.expect("clean soak bucket");
    ephemeral.drop().await;
}
