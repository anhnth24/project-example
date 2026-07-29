//! Live PostgreSQL HTTP contract tests for the Document Graph MVP
//! (`GET /api/v1/graph`, P2-17 — owner request 2026-07-29).
//!
//! Skips cleanly when `MARKHAND_TEST_DATABASE_URL` / `MARKHAND_TEST_APP_DATABASE_URL`
//! are unset (see `common::boot_app_pool`), same as every other live-PG suite
//! in this crate.
//!
//! `similarity` (Qdrant) is deliberately out of scope here: `routes/graph.rs`
//! never queries Qdrant in this pass (see its module doc + the P2-17
//! report), so there is nothing DB-observable to assert about it, and this
//! suite has no `MARKHAND_TEST_QDRANT_URL` wiring.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{admin_database_url, app_database_url, boot_app_pool, build_router};
use deadpool_postgres::Pool;
use fileconv_server::auth::context::OrgContext;
use fileconv_server::db::ask_streams::{self, NewAskStreamSession};
use fileconv_server::db::collections::{self, NewCollection};
use fileconv_server::db::documents::{self, NewDocument};
use fileconv_server::db::error::DbError;
use fileconv_server::db::models::CollectionVisibility;
use fileconv_server::db::pool::with_org_txn;
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const PASSWORD: &str = "correct-password-1";

async fn boot_pool() -> Option<(common::DualRoleEphemeralDb, Pool)> {
    let admin = admin_database_url()?;
    let app = app_database_url()?;
    Some(boot_app_pool(&admin, &app).await)
}

async fn get(app: &axum::Router, uri: &str, token: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("GET")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| serde_json::json!({ "raw": String::from_utf8_lossy(&bytes) }));
    (status, json)
}

fn ids_of(nodes: &[Value]) -> Vec<String> {
    nodes
        .iter()
        .map(|n| n["id"].as_str().unwrap().to_string())
        .collect()
}

async fn seed_collection(
    pool: &Pool,
    org: Uuid,
    owner: Uuid,
    name: &str,
    visibility: CollectionVisibility,
) -> Uuid {
    let ctx = OrgContext::try_new(org, owner, [] as [&str; 0], []).unwrap();
    let id = Uuid::new_v4();
    let slug = format!("graph-col-{}", id.simple());
    let name = name.to_string();
    with_org_txn(pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                collections::insert(
                    txn,
                    &ctx,
                    NewCollection {
                        id,
                        name: &name,
                        slug: &slug,
                        description: None,
                        visibility,
                    },
                )
                .await
            })
        }
    })
    .await
    .expect("seed collection");
    id
}

async fn seed_document(
    pool: &Pool,
    org: Uuid,
    owner: Uuid,
    collection_id: Uuid,
    title: &str,
) -> Uuid {
    let ctx = OrgContext::try_new(org, owner, [] as [&str; 0], []).unwrap();
    let id = Uuid::new_v4();
    let title = title.to_string();
    with_org_txn(pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                documents::insert(
                    txn,
                    &ctx,
                    NewDocument {
                        id,
                        collection_id,
                        title: &title,
                    },
                )
                .await
            })
        }
    })
    .await
    .expect("seed document");
    id
}

/// Published version, required before a document can carry claims (claims'
/// lineage FK targets `document_versions`).
async fn seed_version(pool: &Pool, org: Uuid, owner: Uuid, document_id: Uuid) -> Uuid {
    let ctx = OrgContext::try_new(org, owner, [] as [&str; 0], []).unwrap();
    let version_id = Uuid::new_v4();
    let sha = "a".repeat(64);
    let object_key = format!("org/{org}/objects/graph-test-{}", version_id.simple());
    with_org_txn(pool, &ctx, move |txn| {
        Box::pin(async move {
            txn.execute(
                "INSERT INTO document_versions (
                    id, org_id, document_id, version_number, publication_state,
                    is_current, content_sha256, original_object_key, created_by_user_id
                 ) VALUES ($1,$2,$3,1,'published',true,$4,$5,$6)",
                &[&version_id, &org, &document_id, &sha, &object_key, &owner],
            )
            .await?;
            // `markhand_validate_document_invariant` requires
            // `documents.current_version_id` to agree with the version just
            // marked `is_current` (same order `tests/api_http_contracts.rs`
            // already follows for its own seeded versions).
            txn.execute(
                "UPDATE documents SET current_version_id = $1, state = 'indexed'
                 WHERE org_id = $2 AND id = $3",
                &[&version_id, &org, &document_id],
            )
            .await?;
            Ok::<_, DbError>(())
        })
    })
    .await
    .expect("seed version");
    version_id
}

/// Seeds one claim on each document and an open conflict between them —
/// same shape `tests/api_http_contracts.rs` already seeds for conflict
/// routes, reused here for the graph's `conflict` edge.
async fn seed_conflict(
    pool: &Pool,
    org: Uuid,
    owner: Uuid,
    doc_a: Uuid,
    version_a: Uuid,
    doc_b: Uuid,
    version_b: Uuid,
) {
    let ctx = OrgContext::try_new(org, owner, [] as [&str; 0], []).unwrap();
    let claim_on_a = Uuid::new_v4();
    let claim_on_b = Uuid::new_v4();
    let (claim_a_id, claim_b_id) = if claim_on_a < claim_on_b {
        (claim_on_a, claim_on_b)
    } else {
        (claim_on_b, claim_on_a)
    };
    let conflict_id = Uuid::new_v4();
    with_org_txn(pool, &ctx, move |txn| {
        Box::pin(async move {
            txn.execute(
                "INSERT INTO claims (
                    id, org_id, document_id, version_id, claim_key, subject, predicate,
                    value_type, value_money, unit, scope, effective_from, citation_quote
                 ) VALUES ($1,$2,$3,$4,'budget','Kinh phí','is','money',15,'triệu','',now(),'15 triệu')",
                &[&claim_on_a, &org, &doc_a, &version_a],
            )
            .await?;
            txn.execute(
                "INSERT INTO claims (
                    id, org_id, document_id, version_id, claim_key, subject, predicate,
                    value_type, value_money, unit, scope, effective_from, citation_quote
                 ) VALUES ($1,$2,$3,$4,'budget','Kinh phí','is','money',20,'triệu','',now(),'20 triệu')",
                &[&claim_on_b, &org, &doc_b, &version_b],
            )
            .await?;
            txn.execute(
                "INSERT INTO conflicts (
                    id, org_id, status, severity, conflict_type, claim_a_id, claim_b_id,
                    first_detected_version_id
                 ) VALUES ($1,$2,'open','warning','numeric',$3,$4,$5)",
                &[&conflict_id, &org, &claim_a_id, &claim_b_id, &version_a],
            )
            .await?;
            Ok::<_, DbError>(())
        })
    })
    .await
    .expect("seed conflict");
}

/// Seeds an ask-stream session that cited both documents — the graph's only
/// source for `co_citation` edges (`db::graph::co_citation_edges_among`,
/// backed by `ask_stream_sessions.cited_document_ids`).
async fn seed_co_citation(pool: &Pool, org: Uuid, user: Uuid, doc_a: Uuid, doc_b: Uuid) {
    let ctx = OrgContext::try_new(org, user, [] as [&str; 0], []).unwrap();
    with_org_txn(pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                ask_streams::create_session(
                    txn,
                    &ctx,
                    NewAskStreamSession {
                        id: Uuid::new_v4(),
                        version_mode: "current".to_string(),
                        collection_ids: vec![],
                        cited_document_ids: vec![doc_a, doc_b],
                        cited_version_ids: vec![],
                        pinned_snapshot: serde_json::json!({}),
                        max_events: 10,
                        max_bytes: 4096,
                        ttl_secs: 3600,
                    },
                )
                .await
            })
        }
    })
    .await
    .expect("seed ask-stream session");
}

#[tokio::test]
async fn graph_requires_qa_query_permission() {
    let Some((_db, pool)) = boot_pool().await else {
        return;
    };
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    common::seed_user_with_permissions(&pool, org, user, "graph-noperm@example.com", PASSWORD, &[])
        .await;
    let token = common::login_access_token(&pool, "graph-noperm@example.com", PASSWORD).await;
    let app = build_router(pool.clone(), &app_database_url().unwrap(), None);

    let (status, body) = get(&app, "/api/v1/graph", &token).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "body: {body}");
}

#[tokio::test]
async fn graph_returns_conflict_edge_between_documents() {
    let Some((_db, pool)) = boot_pool().await else {
        return;
    };
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    common::seed_user_with_permissions(
        &pool,
        org,
        user,
        "graph-conflict@example.com",
        PASSWORD,
        &["qa.query"],
    )
    .await;
    let collection =
        seed_collection(&pool, org, user, "Ngân sách", CollectionVisibility::Org).await;
    let doc_a = seed_document(&pool, org, user, collection, "Đề xuất ngân sách A").await;
    let doc_b = seed_document(&pool, org, user, collection, "Đề xuất ngân sách B").await;
    let version_a = seed_version(&pool, org, user, doc_a).await;
    let version_b = seed_version(&pool, org, user, doc_b).await;
    seed_conflict(&pool, org, user, doc_a, version_a, doc_b, version_b).await;

    let token = common::login_access_token(&pool, "graph-conflict@example.com", PASSWORD).await;
    let app = build_router(pool.clone(), &app_database_url().unwrap(), None);
    let (status, body) = get(&app, "/api/v1/graph", &token).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    let nodes = body["nodes"].as_array().unwrap();
    let node_ids = ids_of(nodes);
    assert!(node_ids.contains(&doc_a.to_string()));
    assert!(node_ids.contains(&doc_b.to_string()));

    let edges = body["edges"].as_array().unwrap();
    let conflict_edge = edges
        .iter()
        .find(|e| e["kind"] == "conflict")
        .unwrap_or_else(|| panic!("expected a conflict edge, got: {edges:?}"));
    let endpoints = [
        conflict_edge["source"].as_str().unwrap(),
        conflict_edge["target"].as_str().unwrap(),
    ];
    assert!(endpoints.contains(&doc_a.to_string().as_str()));
    assert!(endpoints.contains(&doc_b.to_string().as_str()));
    let weight = conflict_edge["weight"].as_f64().unwrap();
    assert!(
        weight > 0.0 && weight < 1.0,
        "weight out of (0,1): {weight}"
    );

    let communities = body["communities"].as_array().unwrap();
    let shared = communities
        .iter()
        .find(|c| {
            let members: Vec<String> = c["nodeIds"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect();
            members.contains(&doc_a.to_string()) && members.contains(&doc_b.to_string())
        })
        .unwrap_or_else(|| {
            panic!("expected a community containing both docs, got: {communities:?}")
        });
    assert_eq!(shared["size"].as_i64().unwrap(), 2);
}

#[tokio::test]
async fn graph_returns_co_citation_edge_from_ask_stream_sessions() {
    let Some((_db, pool)) = boot_pool().await else {
        return;
    };
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    common::seed_user_with_permissions(
        &pool,
        org,
        user,
        "graph-cocitation@example.com",
        PASSWORD,
        &["qa.query"],
    )
    .await;
    let collection =
        seed_collection(&pool, org, user, "Chính sách", CollectionVisibility::Org).await;
    let doc_a = seed_document(&pool, org, user, collection, "Chính sách A").await;
    let doc_b = seed_document(&pool, org, user, collection, "Chính sách B").await;
    seed_co_citation(&pool, org, user, doc_a, doc_b).await;

    let token = common::login_access_token(&pool, "graph-cocitation@example.com", PASSWORD).await;
    let app = build_router(pool.clone(), &app_database_url().unwrap(), None);
    let (status, body) = get(&app, "/api/v1/graph", &token).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    let edges = body["edges"].as_array().unwrap();
    let co_citation_edge = edges
        .iter()
        .find(|e| e["kind"] == "co_citation")
        .unwrap_or_else(|| panic!("expected a co_citation edge, got: {edges:?}"));
    let endpoints = [
        co_citation_edge["source"].as_str().unwrap(),
        co_citation_edge["target"].as_str().unwrap(),
    ];
    assert!(endpoints.contains(&doc_a.to_string().as_str()));
    assert!(endpoints.contains(&doc_b.to_string().as_str()));
}

#[tokio::test]
async fn graph_org_isolation_org_b_does_not_see_org_a_nodes() {
    let Some((_db, pool)) = boot_pool().await else {
        return;
    };
    let org_a = Uuid::new_v4();
    let user_a = Uuid::new_v4();
    common::seed_user_with_permissions(
        &pool,
        org_a,
        user_a,
        "graph-orga@example.com",
        PASSWORD,
        &["qa.query"],
    )
    .await;
    let collection_a =
        seed_collection(&pool, org_a, user_a, "Org A", CollectionVisibility::Org).await;
    let doc_a = seed_document(&pool, org_a, user_a, collection_a, "Tài liệu org A").await;

    let org_b = Uuid::new_v4();
    let user_b = Uuid::new_v4();
    common::seed_user_with_permissions(
        &pool,
        org_b,
        user_b,
        "graph-orgb@example.com",
        PASSWORD,
        &["qa.query"],
    )
    .await;

    let token_b = common::login_access_token(&pool, "graph-orgb@example.com", PASSWORD).await;
    let app = build_router(pool.clone(), &app_database_url().unwrap(), None);
    let (status, body) = get(&app, "/api/v1/graph", &token_b).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let node_ids = ids_of(body["nodes"].as_array().unwrap());
    assert!(
        !node_ids.contains(&doc_a.to_string()),
        "org B must never see org A's document node: {node_ids:?}"
    );
}

#[tokio::test]
async fn graph_acl_hides_documents_in_a_private_collection_the_caller_cannot_access() {
    let Some((_db, pool)) = boot_pool().await else {
        return;
    };
    let org = Uuid::new_v4();
    let owner = Uuid::new_v4();
    let viewer = Uuid::new_v4();
    common::seed_user_with_permissions(&pool, org, owner, "graph-owner@example.com", PASSWORD, &[])
        .await;
    common::seed_user_with_permissions(
        &pool,
        org,
        viewer,
        "graph-viewer@example.com",
        PASSWORD,
        &["qa.query"],
    )
    .await;

    // Visible: an org-wide collection with one document.
    let visible_collection =
        seed_collection(&pool, org, viewer, "Công khai", CollectionVisibility::Org).await;
    let visible_doc =
        seed_document(&pool, org, viewer, visible_collection, "Tài liệu công khai").await;

    // Hidden: a private collection owned by someone else, no grant to `viewer`.
    let private_collection =
        seed_collection(&pool, org, owner, "Riêng tư", CollectionVisibility::Private).await;
    let hidden_doc =
        seed_document(&pool, org, owner, private_collection, "Tài liệu riêng tư").await;

    let token = common::login_access_token(&pool, "graph-viewer@example.com", PASSWORD).await;
    let app = build_router(pool.clone(), &app_database_url().unwrap(), None);
    let (status, body) = get(&app, "/api/v1/graph", &token).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    let node_ids = ids_of(body["nodes"].as_array().unwrap());
    assert!(node_ids.contains(&visible_doc.to_string()));
    assert!(
        !node_ids.contains(&hidden_doc.to_string()),
        "ACL-restricted document leaked into the graph: {node_ids:?}"
    );

    // Filtering by the private collection id must 404, not silently return
    // an empty (or worse, populated) graph.
    let (status, body) = get(
        &app,
        &format!("/api/v1/graph?collectionId={private_collection}"),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "body: {body}");
}

#[tokio::test]
async fn graph_caps_nodes_at_the_documented_bound() {
    let Some((_db, pool)) = boot_pool().await else {
        return;
    };
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    common::seed_user_with_permissions(
        &pool,
        org,
        user,
        "graph-bounded@example.com",
        PASSWORD,
        &["qa.query"],
    )
    .await;
    let collection =
        seed_collection(&pool, org, user, "Số lượng lớn", CollectionVisibility::Org).await;

    // 510 isolated (degree-0) documents — one more than the documented
    // 500-node cap (`routes/graph.rs::MAX_NODES`). All tie on degree, so
    // `prune()` keeps the 500 smallest-uuid documents deterministically.
    let ctx = OrgContext::try_new(org, user, [] as [&str; 0], []).unwrap();
    with_org_txn(&pool, &ctx, move |txn| {
        Box::pin(async move {
            txn.execute(
                "INSERT INTO documents (id, org_id, collection_id, title, state, created_by_user_id)
                 SELECT gen_random_uuid(), $1, $2, 'Bulk doc ' || gs, 'indexed', $3
                 FROM generate_series(1, 510) AS gs",
                &[&org, &collection, &user],
            )
            .await?;
            Ok::<_, DbError>(())
        })
    })
    .await
    .expect("bulk-seed documents");

    let token = common::login_access_token(&pool, "graph-bounded@example.com", PASSWORD).await;
    let app = build_router(pool.clone(), &app_database_url().unwrap(), None);
    let (status, body) = get(&app, "/api/v1/graph", &token).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let nodes = body["nodes"].as_array().unwrap();
    assert_eq!(
        nodes.len(),
        500,
        "expected the documented 500-node cap to bind"
    );
}
