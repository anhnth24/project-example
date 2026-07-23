//! Live router evidence for P1B-R05 (SSE) and P1B-R06 (proxy/rate/readiness/OpenAPI).

mod common;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use fileconv_knowledge::ask::AnswerMode;
use fileconv_knowledge::identity::BODY_TEXT_VERSION;
use fileconv_server::api::{
    embedded_openapi_yaml, resolve_last_event_id, router_openapi_parity_gaps, LastEventIdError,
    ROUTE_INVENTORY,
};
use fileconv_server::auth::context::OrgContext;
use fileconv_server::auth::jwt::JwtKeys;
use fileconv_server::db::collections::{self, NewCollection};
use fileconv_server::db::documents::{self, NewDocument};
use fileconv_server::db::models::{ArtifactKind, DocumentState, JobType};
use fileconv_server::db::pool::with_org_txn;
use fileconv_server::http::{parse_trusted_proxies, router, AppState};
use fileconv_server::jobs::{self, EnqueueJob, EventPayload, JobPayload};
use fileconv_server::middleware::rate_limit::{RateLimitConfig, RateLimiter};
use fileconv_server::middleware::{client_ip_from_xff, resolve_client_ip};
use fileconv_server::services::chunking::prepare_chunks;
use fileconv_server::services::qa::provider::{ChatProvider, StreamingStaticProvider};
use fileconv_server::storage::minio::ObjectIdentityMeta;
use futures::StreamExt;
use http_body_util::BodyExt;
use tower::ServiceExt;
use uuid::Uuid;

use common::{
    admin_database_url, app_database_url, assert_markhand_app_role, boot_app_pool, build_app_state,
    build_router, login_access_token, login_tokens, put_bytes, seed_user_with_permissions,
    sha256_hex, test_auth_config, test_minio_client, trusted_key, MinioCleanupGuard,
};

#[test]
fn openapi_route_method_status_parity_is_structural() {
    let gaps = router_openapi_parity_gaps(embedded_openapi_yaml());
    assert!(gaps.is_empty(), "{}", gaps.join("; "));
    assert!(ROUTE_INVENTORY.len() >= 30);
}

#[test]
fn last_event_id_parser_rejects_malformed_negative_future_conflict() {
    assert_eq!(resolve_last_event_id(None, None, None).unwrap(), 0);
    assert_eq!(
        resolve_last_event_id(Some("-1"), None, None),
        Err(LastEventIdError::Negative)
    );
    assert_eq!(
        resolve_last_event_id(Some("abc"), None, None),
        Err(LastEventIdError::Malformed)
    );
    assert_eq!(
        resolve_last_event_id(Some("3"), Some("4"), None),
        Err(LastEventIdError::Conflicting)
    );
    assert_eq!(
        resolve_last_event_id(Some("9"), None, Some(5)),
        Err(LastEventIdError::OutOfRange)
    );
}

#[test]
fn trusted_proxy_parser_fail_fast_and_xff_right_to_left() {
    assert!(parse_trusted_proxies("10.0.0.1,not-an-ip").is_err());
    assert!(parse_trusted_proxies("").unwrap().is_empty());
    let trusted = [IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))];
    assert_eq!(
        client_ip_from_xff("203.0.113.9, 10.0.0.1", &trusted).unwrap(),
        "203.0.113.9"
    );
    assert_eq!(
        client_ip_from_xff("198.51.100.2, 203.0.113.9, 10.0.0.1", &trusted).unwrap(),
        "203.0.113.9"
    );
    assert!(client_ip_from_xff("10.0.0.1", &trusted).is_err());
}

#[tokio::test]
async fn connect_info_direct_untrusted_and_trusted_xff_chain() {
    let trusted = vec![IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))];
    let mut request = Request::builder()
        .uri("/api/v1/health/live")
        .header("x-forwarded-for", "203.0.113.50")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(axum::extract::ConnectInfo(
        "192.0.2.10:443".parse::<SocketAddr>().unwrap(),
    ));
    assert_eq!(resolve_client_ip(&request, &trusted).unwrap(), "192.0.2.10");

    let mut request = Request::builder()
        .uri("/api/v1/health/live")
        .header("x-forwarded-for", "203.0.113.9, 10.0.0.1")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(axum::extract::ConnectInfo(
        "10.0.0.1:443".parse::<SocketAddr>().unwrap(),
    ));
    assert_eq!(
        resolve_client_ip(&request, &trusted).unwrap(),
        "203.0.113.9"
    );

    let mut request = Request::builder()
        .uri("/api/v1/health/live")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(axum::extract::ConnectInfo(
        "10.0.0.1:443".parse::<SocketAddr>().unwrap(),
    ));
    assert!(resolve_client_ip(&request, &trusted).is_err());
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn live_router_trusted_proxy_and_rate_limit_429_metadata() {
    let Some(admin_url) = admin_database_url() else {
        return;
    };
    let Some(app_url) = app_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_app_pool(&admin_url, &app_url).await;
    assert_markhand_app_role(&pool).await;

    let limiter = RateLimiter::new(RateLimitConfig {
        auth_per_minute: 1_000,
        user_per_minute: 1_000,
        ip_per_minute: 2,
        expensive_route_per_minute: 1_000,
    });
    let state = build_app_state(pool, &ephemeral.app_url, None)
        .with_rate_limiter(limiter)
        .with_trusted_proxies(vec![IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))]);
    let app = router(state);

    for _ in 0..2 {
        let mut request = Request::builder()
            .uri("/api/v1/health/live")
            .header("x-forwarded-for", "198.51.100.20")
            .body(Body::empty())
            .unwrap();
        request.extensions_mut().insert(axum::extract::ConnectInfo(
            "127.0.0.1:9".parse::<SocketAddr>().unwrap(),
        ));
        // Baseline IP limiter exempts /health/* — hit a non-health path for 429.
        let _ = request;
    }
    for _ in 0..2 {
        let mut request = Request::builder()
            .uri("/api/v1/openapi.yaml")
            .header("x-forwarded-for", "198.51.100.20")
            .body(Body::empty())
            .unwrap();
        request.extensions_mut().insert(axum::extract::ConnectInfo(
            "127.0.0.1:9".parse::<SocketAddr>().unwrap(),
        ));
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
    let mut request = Request::builder()
        .uri("/api/v1/openapi.yaml")
        .header("x-forwarded-for", "198.51.100.20")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(axum::extract::ConnectInfo(
        "127.0.0.1:9".parse::<SocketAddr>().unwrap(),
    ));
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .expect("Retry-After header");
    assert!(retry_after >= 1);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], "rate_limited");
    assert_eq!(json["details"]["scope"], "ip");
    assert_eq!(json["details"]["quota"], "rate_limit");
    assert_eq!(
        json["details"]["retryAfterSeconds"].as_u64().unwrap(),
        retry_after
    );
    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn live_health_start_live_ready_contracts() {
    let Some(admin_url) = admin_database_url() else {
        return;
    };
    let Some(app_url) = app_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_app_pool(&admin_url, &app_url).await;
    let app = build_router(pool, &ephemeral.app_url, None);
    for path in ["/api/v1/health/live", "/api/v1/health/start"] {
        let response = app
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "ok");
        assert!(json["requestId"].as_str().is_some());
    }
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/health/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], "dependency_unavailable");
    assert!(json["details"]["probe"].as_str().is_some());
    ephemeral.drop().await;
}

async fn seed_indexed_doc(
    pool: &deadpool_postgres::Pool,
    store: &fileconv_server::storage::minio::MinioClient,
    org: Uuid,
    user: Uuid,
) -> (Uuid, Uuid, Uuid, OrgContext) {
    let collection_id = Uuid::new_v4();
    let document_id = Uuid::new_v4();
    let version_id = Uuid::new_v4();
    let artifact_id = Uuid::new_v4();
    let index_meta_id = Uuid::new_v4();
    let markdown = "# Ngân sách\n\nKinh phí hiện tại là 15 triệu đồng.\n";
    let markdown_sha = sha256_hex(markdown.as_bytes());
    let key = trusted_key(org, version_id, Uuid::new_v4(), None).unwrap();
    let ctx = OrgContext::try_new(
        org,
        user,
        [
            "qa.query",
            "qa.history",
            "doc.upload",
            "doc.delete",
            "jobs.system",
        ],
        [collection_id],
    )
    .unwrap();
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
                        name: "SSE collection",
                        slug: &format!("sse-{}", collection_id.simple()),
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
                        title: "SSE doc",
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
    .expect("seed indexed doc");
    (collection_id, document_id, version_id, ctx)
}

async fn read_sse_until(
    response: axum::response::Response,
    predicate: impl Fn(&str, &[u64], Option<Uuid>) -> bool,
    timeout: Duration,
) -> (String, Vec<u64>, Option<Uuid>) {
    let mut body = response.into_body().into_data_stream();
    let mut buf = String::new();
    let mut sequences = Vec::new();
    let mut session_id = None;
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        tokio::select! {
            next = body.next() => {
                let Some(Ok(chunk)) = next else { break; };
                buf.push_str(&String::from_utf8_lossy(&chunk));
                for line in buf.lines() {
                    if let Some(data) = line.strip_prefix("data:") {
                        if let Ok(envelope) = serde_json::from_str::<serde_json::Value>(data.trim()) {
                            if let Some(seq) = envelope["sequence"].as_u64() {
                                if !sequences.contains(&seq) {
                                    sequences.push(seq);
                                }
                            }
                            if session_id.is_none() {
                                if let Some(id) = envelope["data"]["streamSessionId"].as_str() {
                                    session_id = Uuid::parse_str(id).ok();
                                }
                            }
                        }
                    }
                }
                if predicate(&buf, &sequences, session_id) {
                    break;
                }
            }
            _ = tokio::time::sleep_until(deadline) => break,
        }
    }
    (buf, sequences, session_id)
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL/APP + MinIO + Qdrant"]
async fn live_ask_stream_reconnect_order_and_auth_barriers() {
    let Some(admin_url) = admin_database_url() else {
        return;
    };
    let Some(app_url) = app_database_url() else {
        return;
    };
    let Some(store) = test_minio_client() else {
        return;
    };
    let guard = MinioCleanupGuard::new(store.clone());
    let (ephemeral, pool) = boot_app_pool(&admin_url, &app_url).await;
    assert_markhand_app_role(&pool).await;

    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let other_org = Uuid::new_v4();
    let other_user = Uuid::new_v4();
    seed_user_with_permissions(
        &pool,
        org,
        user,
        &format!("{user}@sse.test"),
        "correct-password-1",
        &[
            "qa.query",
            "qa.history",
            "doc.upload",
            "doc.delete",
            "jobs.system",
        ],
    )
    .await;
    seed_user_with_permissions(
        &pool,
        other_org,
        other_user,
        &format!("{other_user}@sse-other.test"),
        "correct-password-1",
        &["qa.query"],
    )
    .await;
    let (token, refresh) =
        login_tokens(&pool, &format!("{user}@sse.test"), "correct-password-1").await;
    let other_token = login_access_token(
        &pool,
        &format!("{other_user}@sse-other.test"),
        "correct-password-1",
    )
    .await;
    let (collection_id, _document_id, _version_id, _ctx) =
        seed_indexed_doc(&pool, &store, org, user).await;

    let qdrant_url = std::env::var("MARKHAND_TEST_QDRANT_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:6333".into());
    let qdrant = fileconv_server::storage::QdrantClient::new(&qdrant_url).expect("qdrant");
    let provider = ChatProvider::StreamingStatic(StreamingStaticProvider::new(
        vec!["Kinh ".into(), "phí ".into(), "15 triệu.".into()],
        AnswerMode::LocalLlm,
    ));
    let state = build_app_state(pool.clone(), &ephemeral.app_url, Some(store))
        .with_retrieval_backends(qdrant, None)
        .with_chat_provider(provider);
    let app = router(state);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/ask/stream")
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "question": "Kinh phí là bao nhiêu?",
                        "mode": "current",
                        "limit": 5,
                        "collectionIds": [collection_id]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let (_buf, sequences, session_id) = read_sse_until(
        response,
        |_, seqs, session| session.is_some() && seqs.len() >= 2,
        Duration::from_secs(12),
    )
    .await;
    let session_id = session_id.expect("streamSessionId");
    assert!(sequences.windows(2).all(|w| w[0] < w[1]));

    let idor = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/ask/stream?streamSessionId={session_id}&lastEventId=0"
                ))
                .header("authorization", format!("Bearer {other_token}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({"question":"x","mode":"current"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(idor.status(), StatusCode::NOT_FOUND);

    let last = *sequences.last().unwrap();
    let resume = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/ask/stream?streamSessionId={session_id}&lastEventId={last}"
                ))
                .header("authorization", format!("Bearer {token}"))
                .header("last-event-id", last.to_string())
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({"question":"ignored","mode":"current"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resume.status(), StatusCode::OK);
    let (resume_buf, resume_seqs, _) = read_sse_until(
        resume,
        |buf, _, _| buf.contains("stream.closed") || buf.contains("ask.completed"),
        Duration::from_secs(12),
    )
    .await;
    assert!(
        resume_seqs.iter().all(|seq| *seq > last),
        "resume must not replay acked events: last={last} got={resume_seqs:?} buf={resume_buf}"
    );

    // Production-router logout barrier (family lock shared with stream pull).
    let logout_started = std::time::Instant::now();
    let logout = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/logout")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "refreshToken": refresh }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);
    assert!(
        logout_started.elapsed() < Duration::from_secs(2),
        "logout must not block behind held stream locks: {:?}",
        logout_started.elapsed()
    );

    let revoked = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/ask/stream?streamSessionId={session_id}&lastEventId=0"
                ))
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({"question":"x","mode":"current"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        matches!(
            revoked.status(),
            StatusCode::UNAUTHORIZED | StatusCode::OK | StatusCode::NOT_FOUND
        ),
        "unexpected {}",
        revoked.status()
    );
    if revoked.status() == StatusCode::OK {
        let (buf, seqs, _) = read_sse_until(
            revoked,
            |buf, _, _| buf.contains("session_revoked") || buf.contains("stream.closed"),
            Duration::from_secs(8),
        )
        .await;
        assert!(
            buf.contains("session_revoked") || buf.contains("stream.closed"),
            "expected revoke close: {buf}"
        );
        assert!(
            !buf.contains("ask.token") && seqs.is_empty(),
            "logout must not emit content sequences after commit: seqs={seqs:?} buf={buf}"
        );
    }

    guard.cleanup().await.expect("minio cleanup");
    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL/APP + MinIO + Qdrant"]
async fn live_ask_stream_jwt_exp_membership_and_delete_barriers() {
    use fileconv_server::services::qa::ask_stream::{live_tail_ask_session, start_ask_stream};
    use fileconv_server::services::retrieval::VersionMode;

    let Some(admin_url) = admin_database_url() else {
        return;
    };
    let Some(app_url) = app_database_url() else {
        return;
    };
    let Some(store) = test_minio_client() else {
        return;
    };
    let guard = MinioCleanupGuard::new(store.clone());
    let (ephemeral, pool) = boot_app_pool(&admin_url, &app_url).await;
    assert_markhand_app_role(&pool).await;

    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    seed_user_with_permissions(
        &pool,
        org,
        user,
        &format!("{user}@sse-barrier.test"),
        "correct-password-1",
        &["qa.query", "qa.history", "doc.upload", "doc.delete"],
    )
    .await;
    let token = login_access_token(
        &pool,
        &format!("{user}@sse-barrier.test"),
        "correct-password-1",
    )
    .await;
    let (collection_id, document_id, _version_id, ctx) =
        seed_indexed_doc(&pool, &store, org, user).await;
    let qdrant_url = std::env::var("MARKHAND_TEST_QDRANT_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:6333".into());
    let qdrant = fileconv_server::storage::QdrantClient::new(&qdrant_url).expect("qdrant");
    let keys = JwtKeys::from_auth(&test_auth_config()).unwrap();
    let claims = keys.verify_access_token(&token).unwrap();

    let started = start_ask_stream(
        &pool,
        &qdrant,
        None,
        None,
        &ctx,
        claims.clone(),
        "req-barrier".into(),
        "Kinh phí là bao nhiêu?".into(),
        Some([collection_id].into_iter().collect()),
        VersionMode::Current,
        5,
        vec![],
    )
    .await
    .expect("start ask stream");

    // JWT exp barrier on live-tail poll (claims minted valid, then expired).
    let mut expired_claims = claims.clone();
    expired_claims.exp = chrono::Utc::now().timestamp() - 1;
    let mut rx = live_tail_ask_session(
        pool.clone(),
        expired_claims,
        started.session_id,
        "req-exp".into(),
        started.cited_document_ids.clone(),
        0,
        None,
    )
    .await;
    let mut exp_buf = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
            Ok(Some(Ok(event))) => {
                exp_buf.push_str(&format!("{event:?}"));
                if exp_buf.contains("token_expired") {
                    break;
                }
            }
            _ => break,
        }
    }
    assert!(
        exp_buf.contains("token_expired"),
        "expected token_expired close: {exp_buf}"
    );
    assert!(
        !exp_buf.contains("ask.token"),
        "expired JWT must not emit content after barrier: {exp_buf}"
    );

    // User suspend / membership-equivalent principal barrier.
    with_org_txn(&pool, &ctx, {
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
    .expect("suspend user");
    let mut rx = live_tail_ask_session(
        pool.clone(),
        claims.clone(),
        started.session_id,
        "req-member".into(),
        started.cited_document_ids.clone(),
        0,
        None,
    )
    .await;
    let mut member_buf = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
            Ok(Some(Ok(event))) => {
                member_buf.push_str(&format!("{event:?}"));
                if member_buf.contains("principal_denied") {
                    break;
                }
            }
            _ => break,
        }
    }
    assert!(
        member_buf.contains("principal_denied"),
        "expected principal_denied close: {member_buf}"
    );
    assert!(
        !member_buf.contains("ask.token"),
        "suspend/membership must not emit content after commit: {member_buf}"
    );

    // Re-enable and start a fresh session for citation-delete (prior session may
    // already hold a durable principal_denied terminal from suspend).
    with_org_txn(&pool, &ctx, {
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "UPDATE users SET disabled_at = NULL WHERE id = $1",
                    &[&user],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("re-enable user");

    // Collection ACL revoke via production helper (principal authz lock).
    let started_acl = start_ask_stream(
        &pool,
        &qdrant,
        None,
        None,
        &ctx,
        claims.clone(),
        "req-barrier-acl".into(),
        "Kinh phí là bao nhiêu?".into(),
        Some([collection_id].into_iter().collect()),
        VersionMode::Current,
        5,
        vec![],
    )
    .await
    .expect("start ask stream for acl barrier");
    tokio::time::sleep(Duration::from_millis(150)).await;

    let alt_owner = Uuid::new_v4();
    with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                fileconv_server::db::orgs::ensure_user(
                    txn,
                    &ctx,
                    alt_owner,
                    &format!("alt-{}@sse-acl.test", alt_owner.simple()),
                    "Alt Owner",
                )
                .await?;
                txn.execute(
                    "INSERT INTO org_memberships (org_id, user_id, role)
                     VALUES ($1, $2, 'owner')
                     ON CONFLICT (org_id, user_id) DO NOTHING",
                    &[&org, &alt_owner],
                )
                .await?;
                fileconv_server::services::acl_mutate::revoke_collection_access_for_principal(
                    txn,
                    org,
                    user,
                    collection_id,
                    alt_owner,
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("revoke collection acl");

    let mut rx = live_tail_ask_session(
        pool.clone(),
        claims.clone(),
        started_acl.session_id,
        "req-acl".into(),
        vec![document_id],
        0,
        None,
    )
    .await;
    let mut acl_buf = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
            Ok(Some(Ok(event))) => {
                acl_buf.push_str(&format!("{event:?}"));
                if acl_buf.contains("citation_revoked") || acl_buf.contains("principal_denied") {
                    break;
                }
            }
            _ => break,
        }
    }
    assert!(
        acl_buf.contains("citation_revoked") || acl_buf.contains("principal_denied"),
        "expected ACL revoke close: {acl_buf}"
    );
    assert!(
        !acl_buf.contains("ask.token"),
        "ACL collection revoke must not emit new sequenced content after commit: {acl_buf}"
    );

    // Restore collection access for permission-revoke barrier.
    with_org_txn(&pool, &ctx, {
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "UPDATE collections
                     SET visibility = 'org', owner_user_id = $3, updated_at = now()
                     WHERE org_id = $1 AND id = $2",
                    &[&org, &collection_id, &user],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("restore collection");

    let started_perm = start_ask_stream(
        &pool,
        &qdrant,
        None,
        None,
        &ctx,
        claims.clone(),
        "req-barrier-perm".into(),
        "Kinh phí là bao nhiêu?".into(),
        Some([collection_id].into_iter().collect()),
        VersionMode::Current,
        5,
        vec![],
    )
    .await
    .expect("start ask stream for perm barrier");
    tokio::time::sleep(Duration::from_millis(100)).await;

    with_org_txn(&pool, &ctx, {
        move |txn| {
            Box::pin(async move {
                fileconv_server::services::acl_mutate::revoke_role_permission_for_principal(
                    txn, org, user, "qa.query",
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("revoke qa.query");

    let mut rx = live_tail_ask_session(
        pool.clone(),
        claims,
        started_perm.session_id,
        "req-perm".into(),
        vec![document_id],
        0,
        None,
    )
    .await;
    let mut perm_buf = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
            Ok(Some(Ok(event))) => {
                perm_buf.push_str(&format!("{event:?}"));
                if perm_buf.contains("principal_denied") || perm_buf.contains("stream.closed") {
                    break;
                }
            }
            _ => break,
        }
    }
    assert!(
        perm_buf.contains("principal_denied") || perm_buf.contains("stream.closed"),
        "expected permission revoke close: {perm_buf}"
    );
    assert!(
        !perm_buf.contains("ask.token"),
        "role permission revoke must not emit new sequenced content after commit: {perm_buf}"
    );

    guard.cleanup().await.expect("minio cleanup");
    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL/APP + MinIO + Qdrant"]
async fn live_ask_stream_last_event_id_purge_and_delayed_reconnect() {
    use fileconv_server::db::ask_streams;
    use fileconv_server::services::qa::ask_stream::start_ask_stream;
    use fileconv_server::services::retrieval::VersionMode;

    let Some(admin_url) = admin_database_url() else {
        return;
    };
    let Some(app_url) = app_database_url() else {
        return;
    };
    let Some(store) = test_minio_client() else {
        return;
    };
    let guard = MinioCleanupGuard::new(store.clone());
    let (ephemeral, pool) = boot_app_pool(&admin_url, &app_url).await;
    assert_markhand_app_role(&pool).await;

    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    seed_user_with_permissions(
        &pool,
        org,
        user,
        &format!("{user}@sse-leid.test"),
        "correct-password-1",
        &["qa.query", "qa.history", "doc.upload", "doc.delete"],
    )
    .await;
    let token = login_access_token(
        &pool,
        &format!("{user}@sse-leid.test"),
        "correct-password-1",
    )
    .await;
    let (collection_id, _document_id, _version_id, ctx) =
        seed_indexed_doc(&pool, &store, org, user).await;
    let qdrant_url = std::env::var("MARKHAND_TEST_QDRANT_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:6333".into());
    let qdrant = fileconv_server::storage::QdrantClient::new(&qdrant_url).expect("qdrant");
    let provider = ChatProvider::StreamingStatic(StreamingStaticProvider::new(
        vec![
            "Chậm ".into(),
            "từ ".into(),
            "từ ".into(),
            "từ ".into(),
            "xong.".into(),
        ],
        AnswerMode::LocalLlm,
    ));
    let state = build_app_state(pool.clone(), &ephemeral.app_url, Some(store))
        .with_retrieval_backends(qdrant.clone(), None)
        .with_chat_provider(provider);
    let app = router(state);

    // Conflicting Last-Event-ID → 400.
    let bad = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/ask/stream?lastEventId=1")
                .header("authorization", format!("Bearer {token}"))
                .header("last-event-id", "2")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "question": "Kinh phí?",
                        "mode": "current",
                        "collectionIds": [collection_id]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);

    let keys = JwtKeys::from_auth(&test_auth_config()).unwrap();
    let claims = keys.verify_access_token(&token).unwrap();
    let started = start_ask_stream(
        &pool,
        &qdrant,
        None,
        Some(ChatProvider::StreamingStatic(StreamingStaticProvider::new(
            vec!["A ".into(), "B ".into(), "C.".into()],
            AnswerMode::LocalLlm,
        ))),
        &ctx,
        claims.clone(),
        "req-delay".into(),
        "Kinh phí là bao nhiêu?".into(),
        Some([collection_id].into_iter().collect()),
        VersionMode::Current,
        5,
        vec![],
    )
    .await
    .expect("start");

    // Delayed producer / reconnect: wait for some durable events, then resume after last.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let high_water = with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        let sid = started.session_id;
        move |txn| {
            Box::pin(async move {
                let session = ask_streams::get_owned_session(txn, &ctx, sid).await?;
                Ok(session.high_water_sequence())
            })
        }
    })
    .await
    .expect("hw");
    assert!(high_water >= 1, "producer should append before resume");

    let resume = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/ask/stream?streamSessionId={}&lastEventId={high_water}",
                    started.session_id
                ))
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({"question":"ignored","mode":"current"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resume.status(), StatusCode::OK);
    let (buf, seqs, _) = read_sse_until(
        resume,
        |buf, _, _| buf.contains("stream.closed") || buf.contains("ask.completed"),
        Duration::from_secs(12),
    )
    .await;
    assert!(
        seqs.iter().all(|s| *s > high_water as u64),
        "resume must not replay acked; hw={high_water} seqs={seqs:?} buf={buf}"
    );
    // Control closes must not invent unreserved ids in the sequence list.
    assert!(
        !buf.contains(&format!("\"sequence\":{}", high_water + 1))
            || seqs.contains(&((high_water as u64) + 1)),
        "synthetic cursor+1 without durable event forbidden: {buf}"
    );

    // Purge expired sessions (bounded SKIP LOCKED).
    with_org_txn(&pool, &ctx, {
        let sid = started.session_id;
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "UPDATE ask_stream_sessions
                     SET created_at = clock_timestamp() - interval '2 seconds',
                         expires_at = clock_timestamp() - interval '1 second'
                     WHERE id = $1",
                    &[&sid],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("expire");
    let before = ask_streams::purged_sessions_total();
    let (sessions, _events, _recovered) = ask_streams::run_maintenance(&pool, 50)
        .await
        .expect("purge");
    assert!(sessions >= 1 || ask_streams::purged_sessions_total() > before);

    guard.cleanup().await.expect("minio cleanup");
    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn live_job_sse_replay_worker_restart_and_cross_org_idor() {
    let Some(admin_url) = admin_database_url() else {
        return;
    };
    let Some(app_url) = app_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_app_pool(&admin_url, &app_url).await;
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let other_org = Uuid::new_v4();
    let other_user = Uuid::new_v4();
    seed_user_with_permissions(
        &pool,
        org,
        user,
        &format!("{user}@job-sse.test"),
        "correct-password-1",
        &["qa.query", "doc.upload", "jobs.system"],
    )
    .await;
    seed_user_with_permissions(
        &pool,
        other_org,
        other_user,
        &format!("{other_user}@job-sse-other.test"),
        "correct-password-1",
        &["qa.query", "jobs.system"],
    )
    .await;
    let token =
        login_access_token(&pool, &format!("{user}@job-sse.test"), "correct-password-1").await;
    let other_token = login_access_token(
        &pool,
        &format!("{other_user}@job-sse-other.test"),
        "correct-password-1",
    )
    .await;

    let collection_id = Uuid::new_v4();
    let document_id = Uuid::new_v4();
    let version_id = Uuid::new_v4();
    let ctx = OrgContext::try_new(
        org,
        user,
        ["qa.query", "doc.upload", "jobs.system"],
        [collection_id],
    )
    .unwrap();
    with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                collections::insert(
                    txn,
                    &ctx,
                    NewCollection {
                        id: collection_id,
                        name: "Jobs",
                        slug: &format!("jobs-{}", collection_id.simple()),
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
                        title: "Job doc",
                    },
                )
                .await?;
                let sha = "b".repeat(64);
                let key = format!("org/{org}/doc/{document_id}/v/{version_id}/source");
                txn.execute(
                    "INSERT INTO document_versions (
                        id, org_id, document_id, version_number, publication_state,
                        is_current, content_sha256, original_object_key, markdown_object_key,
                        source_content_type, byte_size, created_by_user_id
                     ) VALUES ($1,$2,$3,1,'published',true,$4,$5,$5,'text/markdown',1,$6)",
                    &[&version_id, &org, &document_id, &sha, &key, &user],
                )
                .await?;
                txn.execute(
                    "UPDATE documents SET current_version_id=$3, state='indexed'
                     WHERE org_id=$1 AND id=$2",
                    &[&org, &document_id, &version_id],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("seed job lineage");
    let job = jobs::enqueue(
        &pool,
        &ctx,
        EnqueueJob::new(
            JobType::Convert,
            JobPayload {
                document_id: Some(document_id),
                version_id: Some(version_id),
                ..JobPayload::default()
            },
            format!("sse-job-{}", Uuid::new_v4()),
        ),
    )
    .await
    .expect("enqueue job")
    .job;

    // Worker-restart simulation: durable event_log rows survive process death.
    jobs::append_event(
        &pool,
        &ctx,
        "job.progress",
        EventPayload {
            job_id: Some(job.id),
            document_id: Some(document_id),
            version_id: Some(version_id),
            outbox_event_id: None,
        },
    )
    .await
    .expect("progress 1");
    jobs::append_event(
        &pool,
        &ctx,
        "job.progress",
        EventPayload {
            job_id: Some(job.id),
            document_id: Some(document_id),
            version_id: Some(version_id),
            outbox_event_id: None,
        },
    )
    .await
    .expect("progress after restart");

    let app = build_router(pool.clone(), &ephemeral.app_url, None);
    let idor = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/jobs/{}/events", job.id))
                .header("authorization", format!("Bearer {other_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(idor.status(), StatusCode::NOT_FOUND);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/jobs/{}/events?lastEventId=0", job.id))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let (buf, seqs, _) = read_sse_until(
        response,
        |buf, seqs, _| seqs.len() >= 2 || buf.contains("stream.closed"),
        Duration::from_secs(10),
    )
    .await;
    assert!(seqs.len() >= 2, "expected durable job events, got {buf}");
    assert!(seqs.windows(2).all(|w| w[0] < w[1]));

    // Future Last-Event-ID against authoritative high-water → 400, side-effect free.
    let future = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/jobs/{}/events?lastEventId=999999", job.id))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(future.status(), StatusCode::BAD_REQUEST);

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn live_supported_upgrade_legacy_event_payload_router_replay() {
    // After migrations (incl. 0025 snake_case canonical + camel compat), legacy
    // event_log rows with NULL id columns still replay via payload fallback.
    let Some(admin_url) = admin_database_url() else {
        return;
    };
    let Some(app_url) = app_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_app_pool(&admin_url, &app_url).await;
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    seed_user_with_permissions(
        &pool,
        org,
        user,
        &format!("{user}@legacy-events.test"),
        "correct-password-1",
        &["qa.query", "doc.upload", "jobs.system"],
    )
    .await;
    let token = login_access_token(
        &pool,
        &format!("{user}@legacy-events.test"),
        "correct-password-1",
    )
    .await;

    let collection_id = Uuid::new_v4();
    let document_id = Uuid::new_v4();
    let version_id = Uuid::new_v4();
    let ctx = OrgContext::try_new(
        org,
        user,
        ["qa.query", "doc.upload", "jobs.system"],
        [collection_id],
    )
    .unwrap();
    with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                collections::insert(
                    txn,
                    &ctx,
                    NewCollection {
                        id: collection_id,
                        name: "Legacy",
                        slug: &format!("legacy-{}", collection_id.simple()),
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
                        title: "Legacy doc",
                    },
                )
                .await?;
                let sha = "c".repeat(64);
                let key = format!("org/{org}/doc/{document_id}/v/{version_id}/source");
                txn.execute(
                    "INSERT INTO document_versions (
                        id, org_id, document_id, version_number, publication_state,
                        is_current, content_sha256, original_object_key, markdown_object_key,
                        source_content_type, byte_size, created_by_user_id
                     ) VALUES ($1,$2,$3,1,'published',true,$4,$5,$5,'text/markdown',1,$6)",
                    &[&version_id, &org, &document_id, &sha, &key, &user],
                )
                .await?;
                txn.execute(
                    "UPDATE documents SET current_version_id=$3, state='indexed'
                     WHERE org_id=$1 AND id=$2",
                    &[&org, &document_id, &version_id],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("seed legacy lineage");

    let job = jobs::enqueue(
        &pool,
        &ctx,
        EnqueueJob::new(
            JobType::Convert,
            JobPayload {
                document_id: Some(document_id),
                version_id: Some(version_id),
                ..JobPayload::default()
            },
            format!("legacy-job-{}", Uuid::new_v4()),
        ),
    )
    .await
    .expect("enqueue")
    .job;

    // Simulate pre-backfill writers: NULL columns + snake_case / camelCase payload.
    with_org_txn(&pool, &ctx, {
        let job_id = job.id;
        move |txn| {
            Box::pin(async move {
                let seq: i64 = txn
                    .query_one(
                        "SELECT COALESCE(MAX(sequence_no), 0)::bigint + 1 FROM event_log WHERE org_id=$1",
                        &[&org],
                    )
                    .await?
                    .get(0);
                txn.execute(
                    "INSERT INTO event_log (
                        org_id, sequence_no, event_type, payload_version, payload,
                        job_id, document_id, version_id
                     ) VALUES ($1,$2,'job.progress',1,$3::jsonb,NULL,NULL,NULL)",
                    &[
                        &org,
                        &seq,
                        &serde_json::json!({
                            "job_id": job_id,
                            "document_id": document_id,
                            "version_id": version_id,
                        }),
                    ],
                )
                .await?;
                let seq2 = seq + 1;
                txn.execute(
                    "INSERT INTO event_log (
                        org_id, sequence_no, event_type, payload_version, payload,
                        job_id, document_id, version_id
                     ) VALUES ($1,$2,'job.progress',1,$3::jsonb,NULL,NULL,NULL)",
                    &[
                        &org,
                        &seq2,
                        &serde_json::json!({
                            "jobId": job_id,
                            "documentId": document_id,
                            "versionId": version_id,
                        }),
                    ],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("insert legacy payload rows");

    let app = build_router(pool.clone(), &ephemeral.app_url, None);
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/jobs/{}/events?lastEventId=0", job.id))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let (buf, seqs, _) = read_sse_until(
        response,
        |buf, seqs, _| seqs.len() >= 2 || buf.contains("stream.closed"),
        Duration::from_secs(10),
    )
    .await;
    assert!(
        seqs.len() >= 2,
        "router must replay snake_case + camelCase legacy payload events: {buf}"
    );
    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn live_job_sse_real_logout_barrier_zero_events_after_commit() {
    let Some(admin_url) = admin_database_url() else {
        return;
    };
    let Some(app_url) = app_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_app_pool(&admin_url, &app_url).await;
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    seed_user_with_permissions(
        &pool,
        org,
        user,
        &format!("{user}@job-logout.test"),
        "correct-password-1",
        &["qa.query", "doc.upload", "jobs.system"],
    )
    .await;
    let (token, refresh) = login_tokens(
        &pool,
        &format!("{user}@job-logout.test"),
        "correct-password-1",
    )
    .await;

    let collection_id = Uuid::new_v4();
    let document_id = Uuid::new_v4();
    let version_id = Uuid::new_v4();
    let ctx = OrgContext::try_new(
        org,
        user,
        ["qa.query", "doc.upload", "jobs.system"],
        [collection_id],
    )
    .unwrap();
    with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                collections::insert(
                    txn,
                    &ctx,
                    NewCollection {
                        id: collection_id,
                        name: "Logout Jobs",
                        slug: &format!("logout-jobs-{}", collection_id.simple()),
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
                        title: "Logout job doc",
                    },
                )
                .await?;
                let sha = "d".repeat(64);
                let key = format!("org/{org}/doc/{document_id}/v/{version_id}/source");
                txn.execute(
                    "INSERT INTO document_versions (
                        id, org_id, document_id, version_number, publication_state,
                        is_current, content_sha256, original_object_key, markdown_object_key,
                        source_content_type, byte_size, created_by_user_id
                     ) VALUES ($1,$2,$3,1,'published',true,$4,$5,$5,'text/markdown',1,$6)",
                    &[&version_id, &org, &document_id, &sha, &key, &user],
                )
                .await?;
                txn.execute(
                    "UPDATE documents SET current_version_id=$3, state='indexed'
                     WHERE org_id=$1 AND id=$2",
                    &[&org, &document_id, &version_id],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("seed");

    let job = jobs::enqueue(
        &pool,
        &ctx,
        EnqueueJob::new(
            JobType::Convert,
            JobPayload {
                document_id: Some(document_id),
                version_id: Some(version_id),
                ..JobPayload::default()
            },
            format!("logout-job-{}", Uuid::new_v4()),
        ),
    )
    .await
    .expect("enqueue")
    .job;
    for _ in 0..3 {
        jobs::append_event(
            &pool,
            &ctx,
            "job.progress",
            EventPayload {
                job_id: Some(job.id),
                document_id: Some(document_id),
                version_id: Some(version_id),
                outbox_event_id: None,
            },
        )
        .await
        .expect("progress");
    }

    let app = build_router(pool.clone(), &ephemeral.app_url, None);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/jobs/{}/events?lastEventId=0", job.id))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Trickle-read one event, then production logout; further sequences must stop.
    let mut body = response.into_body().into_data_stream();
    let mut buf = String::new();
    let mut seqs_before = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline && seqs_before.is_empty() {
        match tokio::time::timeout(Duration::from_millis(400), body.next()).await {
            Ok(Some(Ok(chunk))) => {
                buf.push_str(&String::from_utf8_lossy(&chunk));
                for line in buf.lines() {
                    if let Some(id) = line.strip_prefix("id:") {
                        if let Ok(seq) = id.trim().parse::<u64>() {
                            if !seqs_before.contains(&seq) {
                                seqs_before.push(seq);
                            }
                        }
                    }
                }
            }
            _ => break,
        }
    }
    assert!(
        !seqs_before.is_empty(),
        "expected at least one job event before logout: {buf}"
    );

    let logout_started = std::time::Instant::now();
    let logout = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/logout")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "refreshToken": refresh }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);
    assert!(
        logout_started.elapsed() < Duration::from_secs(2),
        "job SSE must release family lock before send; logout took {:?}",
        logout_started.elapsed()
    );

    // Drain remaining stream. Reserve-before-select allows at most one event that
    // was authorized before logout commit; nothing further may be selected/enqueued.
    let mut seqs_after = Vec::new();
    let drain_deadline = tokio::time::Instant::now() + Duration::from_secs(4);
    while tokio::time::Instant::now() < drain_deadline {
        match tokio::time::timeout(Duration::from_millis(300), body.next()).await {
            Ok(Some(Ok(chunk))) => {
                let text = String::from_utf8_lossy(&chunk);
                buf.push_str(&text);
                for line in text.lines() {
                    if let Some(id) = line.strip_prefix("id:") {
                        if let Ok(seq) = id.trim().parse::<u64>() {
                            if !seqs_before.contains(&seq) {
                                seqs_after.push(seq);
                            }
                        }
                    }
                }
                if buf.contains("session_revoked") || buf.contains("stream.closed") {
                    break;
                }
            }
            _ => break,
        }
    }
    assert!(
        seqs_after.len() <= 1,
        "at most one in-flight job event after logout; no buffered batch: before={seqs_before:?} after={seqs_after:?} buf={buf}"
    );
    assert!(
        buf.contains("session_revoked") || buf.contains("stream.closed"),
        "expected logout close frame: {buf}"
    );
    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL/APP + MinIO + Qdrant"]
async fn live_ask_stream_slow_trickle_logout_releases_locks() {
    let Some(admin_url) = admin_database_url() else {
        return;
    };
    let Some(app_url) = app_database_url() else {
        return;
    };
    let Some(store) = test_minio_client() else {
        return;
    };
    let guard = MinioCleanupGuard::new(store.clone());
    let (ephemeral, pool) = boot_app_pool(&admin_url, &app_url).await;
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    seed_user_with_permissions(
        &pool,
        org,
        user,
        &format!("{user}@trickle.test"),
        "correct-password-1",
        &["qa.query", "qa.history", "doc.upload", "doc.delete"],
    )
    .await;
    let (token, refresh) =
        login_tokens(&pool, &format!("{user}@trickle.test"), "correct-password-1").await;
    let (collection_id, _document_id, _version_id, _ctx) =
        seed_indexed_doc(&pool, &store, org, user).await;
    let qdrant_url = std::env::var("MARKHAND_TEST_QDRANT_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:6333".into());
    let qdrant = fileconv_server::storage::QdrantClient::new(&qdrant_url).expect("qdrant");
    let provider = ChatProvider::StreamingStatic(StreamingStaticProvider::new(
        (0..40).map(|i| format!("t{i} ")).collect(),
        AnswerMode::LocalLlm,
    ));
    let state = build_app_state(pool.clone(), &ephemeral.app_url, Some(store))
        .with_retrieval_backends(qdrant, None)
        .with_chat_provider(provider);
    let app = router(state);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/ask/stream")
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "question": "Kinh phí là bao nhiêu?",
                        "mode": "current",
                        "limit": 5,
                        "collectionIds": [collection_id]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let mut body = response.into_body().into_data_stream();
    // Slow trickle: read one chunk then sleep (reserve waits; locks not held).
    let _ = tokio::time::timeout(Duration::from_secs(3), body.next()).await;
    tokio::time::sleep(Duration::from_millis(800)).await;

    let logout_started = std::time::Instant::now();
    let logout = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/logout")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "refreshToken": refresh }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let logout_elapsed = logout_started.elapsed();
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);
    assert!(
        logout_elapsed < Duration::from_secs(2),
        "slow/trickle ask SSE must not hold family/pool locks across send: {logout_elapsed:?}"
    );

    // Remaining trickle must not invent post-logout content sequences.
    let mut buf = String::new();
    let mut post_seqs = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(250), body.next()).await {
            Ok(Some(Ok(chunk))) => {
                let text = String::from_utf8_lossy(&chunk);
                buf.push_str(&text);
                for line in text.lines() {
                    if let Some(id) = line.strip_prefix("id:") {
                        if let Ok(seq) = id.trim().parse::<u64>() {
                            post_seqs.push(seq);
                        }
                    }
                }
                if buf.contains("session_revoked") || buf.contains("stream.closed") {
                    break;
                }
            }
            _ => break,
        }
    }
    // Reserve-before-select: at most one in-flight authorized event; no batch.
    assert!(
        post_seqs.len() <= 1,
        "trickle after logout must not flush a prebuffered batch: {post_seqs:?} buf={buf}"
    );

    guard.cleanup().await.expect("minio cleanup");
    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL/APP + MinIO + Qdrant"]
async fn live_ask_stream_slow_trickle_concurrent_delete_releases_locks() {
    use fileconv_server::auth::jwt::JwtKeys;
    use fileconv_server::services::qa::ask_stream::start_ask_stream;
    use fileconv_server::services::retrieval::VersionMode;

    let Some(admin_url) = admin_database_url() else {
        return;
    };
    let Some(app_url) = app_database_url() else {
        return;
    };
    let Some(store) = test_minio_client() else {
        return;
    };
    let guard = MinioCleanupGuard::new(store.clone());
    let (ephemeral, pool) = boot_app_pool(&admin_url, &app_url).await;
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    seed_user_with_permissions(
        &pool,
        org,
        user,
        &format!("{user}@trickle-del.test"),
        "correct-password-1",
        &["qa.query", "qa.history", "doc.upload", "doc.delete"],
    )
    .await;
    let token = login_access_token(
        &pool,
        &format!("{user}@trickle-del.test"),
        "correct-password-1",
    )
    .await;
    let (collection_id, document_id, version_id, ctx) =
        seed_indexed_doc(&pool, &store, org, user).await;
    let qdrant_url = std::env::var("MARKHAND_TEST_QDRANT_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:6333".into());
    let qdrant = fileconv_server::storage::QdrantClient::new(&qdrant_url).expect("qdrant");
    let keys = JwtKeys::from_auth(&test_auth_config()).unwrap();
    let claims = keys.verify_access_token(&token).unwrap();
    let provider = ChatProvider::StreamingStatic(StreamingStaticProvider::new(
        (0..40).map(|i| format!("t{i} ")).collect(),
        AnswerMode::LocalLlm,
    ));

    // Prepare a session that pins the cited document (not an empty-hit stream).
    let started = start_ask_stream(
        &pool,
        &qdrant,
        None,
        Some(provider),
        &ctx,
        claims,
        "req-trickle-del".into(),
        "Kinh phí là bao nhiêu?".into(),
        Some([collection_id].into_iter().collect()),
        VersionMode::Current,
        5,
        vec![],
    )
    .await
    .expect("start ask stream");
    // FTS-only environments may return zero hits; pin the citation on the durable
    // session so the router live-tail fence observes document delete/ACL revoke.
    if !started.cited_document_ids.contains(&document_id) {
        with_org_txn(&pool, &ctx, {
            let session_id = started.session_id;
            move |txn| {
                Box::pin(async move {
                    txn.execute(
                        "UPDATE ask_stream_sessions
                         SET cited_document_ids = $3::uuid[],
                             cited_version_ids = $4::uuid[]
                         WHERE org_id = $1 AND id = $2",
                        &[&org, &session_id, &vec![document_id], &vec![version_id]],
                    )
                    .await?;
                    Ok(())
                })
            }
        })
        .await
        .expect("pin citation on session");
    }

    let state = build_app_state(pool.clone(), &ephemeral.app_url, Some(store))
        .with_retrieval_backends(qdrant, None);
    let app = router(state);

    // Router resume/live-tail against the pinned session (production path).
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/ask/stream?streamSessionId={}&lastEventId=0",
                    started.session_id
                ))
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({"question":"ignored","mode":"current"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let mut body = response.into_body().into_data_stream();
    let _ = tokio::time::timeout(Duration::from_secs(3), body.next()).await;
    tokio::time::sleep(Duration::from_millis(400)).await;

    // Production document delete while client trickles (must not block on held locks).
    let delete_started = std::time::Instant::now();
    let delete = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/documents/{document_id}"))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let delete_elapsed = delete_started.elapsed();
    assert!(
        delete.status().is_success() || delete.status() == StatusCode::NO_CONTENT,
        "delete status {}",
        delete.status()
    );
    assert!(
        delete_elapsed < Duration::from_secs(2),
        "concurrent delete must not block behind ask SSE locks: {delete_elapsed:?}"
    );

    let mut buf = String::new();
    let mut post_seqs = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(250), body.next()).await {
            Ok(Some(Ok(chunk))) => {
                let text = String::from_utf8_lossy(&chunk);
                buf.push_str(&text);
                for line in text.lines() {
                    if let Some(id) = line.strip_prefix("id:") {
                        if let Ok(seq) = id.trim().parse::<u64>() {
                            post_seqs.push(seq);
                        }
                    }
                }
                if buf.contains("citation_revoked") || buf.contains("stream.closed") {
                    break;
                }
            }
            _ => break,
        }
    }
    assert!(
        post_seqs.len() <= 1,
        "delete during trickle must not flush buffered content batch: {post_seqs:?} buf={buf}"
    );
    assert!(
        buf.contains("citation_revoked") || buf.contains("stream.closed"),
        "expected citation revoke close: {buf}"
    );

    guard.cleanup().await.expect("minio cleanup");
    ephemeral.drop().await;
}

// Keep AppState import used for type inference in helpers.
#[allow(dead_code)]
fn _app_state_ty(_: AppState) {}
