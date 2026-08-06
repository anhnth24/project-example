//! Live PostgreSQL HTTP contract tests for the Document Graph MVP
//! (`GET /api/v1/graph`, P2-17 — owner request 2026-07-29).
//!
//! Skips cleanly when `MARKHAND_TEST_DATABASE_URL` / `MARKHAND_TEST_APP_DATABASE_URL`
//! are unset (see `common::boot_app_pool`), same as every other live-PG suite
//! in this crate.
//!
//! `similarity` (Qdrant) HTTP-route coverage is still out of scope here:
//! `common::build_app_state` never configures `MARKHAND_EMBEDDING_*`, so
//! `AppState::embedder()` is always `None` in this binary's router and the
//! route never takes the similarity path. What *is* covered is
//! `services::graph::build_org_graph` called directly with a real
//! `QdrantClient` + a directly-constructed `ApprovedEmbeddingRuntime` (never
//! calls `.embed()`, so no live embedding provider is needed): one
//! PostgreSQL-backed regression uses a test-local TCP peer to complete the
//! representative-point scroll handshake, observe the real recommend query,
//! then close the connection so the Qdrant client fails; one
//! `MARKHAND_TEST_QDRANT_URL`-gated test exercises a live Qdrant instance.
//! This is the same "service takes plain dependencies, not AppState" shape the
//! route uses, exercised the way `tests/storage.rs` already exercises
//! `QdrantClient`.

mod common;

use std::collections::BTreeSet;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{admin_database_url, app_database_url, boot_app_pool, build_router};
use deadpool_postgres::Pool;
use fileconv_knowledge::identity::{
    chunk_identity, IndexSignature, BODY_TEXT_VERSION, RUNTIME_VLLM_LOCAL,
};
use fileconv_server::auth::context::OrgContext;
use fileconv_server::config::Profile;
use fileconv_server::db::ask_streams::{self, NewAskStreamSession};
use fileconv_server::db::collections::{self, NewCollection};
use fileconv_server::db::documents::{self, NewDocument};
use fileconv_server::db::error::DbError;
use fileconv_server::db::index_metadata::{self, EnsureGeneration};
use fileconv_server::db::models::{CollectionVisibility, EmbeddingRuntimePath};
use fileconv_server::db::pool::with_org_txn;
use fileconv_server::services::embedding::ApprovedEmbeddingRuntime;
use fileconv_server::services::graph::{build_org_graph, SimilarityDeps};
use fileconv_server::services::index_signature::CollectionName;
use fileconv_server::storage::qdrant::{
    ChunkPointPayload, QdrantAdminClient, QdrantClient, UpsertPoint, VectorScope,
};
use http_body_util::BodyExt;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
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
    // Intentionally no `qa.query`: asserts graph route returns 403 without base permission.
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
    // Owner fixture only seeds a private collection; the graph actor is `viewer` (has `qa.query`).
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

// ---------------------------------------------------------------------
// `similarity` edges: Qdrant-gated integration test.
//
// NOT RUN in this sandbox — there is no live Qdrant instance here, so
// `MARKHAND_TEST_QDRANT_URL` is unset and this test skips via the early
// `return` below (see `#[ignore]` + this crate's convention in
// `tests/storage.rs`). It is written to run for real on CI's
// `rust-integration` job, which does provide a Qdrant service.
// ---------------------------------------------------------------------

fn test_qdrant_url() -> Option<String> {
    common::test_qdrant_url()
}

fn test_qdrant_admin_client(_url: &str) -> QdrantAdminClient {
    common::test_qdrant_admin_client()
        .expect("admin client configured with MARKHAND_TEST_QDRANT_URL")
}

/// Seeds an active index generation for `collection_id` matching `signature`
/// — the durable row `services::graph::compute_similarity_edges` looks up
/// via `db::index_metadata::list_active_for_collections` to decide which
/// Qdrant collection/scope to query. Mirrors what
/// `services::indexing::ensure_generation` writes for a real index job.
async fn seed_active_generation(
    pool: &Pool,
    org: Uuid,
    owner: Uuid,
    collection_id: Uuid,
    signature: &IndexSignature<'_>,
) {
    let ctx = OrgContext::try_new(org, owner, [] as [&str; 0], []).unwrap();
    let runtime_path = EmbeddingRuntimePath::parse(signature.runtime_path).expect("runtime path");
    let dimensions = i32::try_from(signature.dimensions).expect("dimensions fit i32");
    let digest = signature.digest();
    let chunking_version = signature.chunking_version.to_string();
    let body_text_version = signature.body_text_version.to_string();
    let query_normalization_version = signature.query_normalization_version.to_string();
    let embedding_family = signature.embedding_family.to_string();
    let embedding_revision = signature.embedding_revision.to_string();
    let normalized = signature.normalized;
    with_org_txn(pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                index_metadata::ensure_active_generation(
                    txn,
                    &ctx,
                    EnsureGeneration {
                        collection_id: Some(collection_id),
                        signature_sha256: &digest,
                        chunking_version: &chunking_version,
                        body_text_version: &body_text_version,
                        query_normalization_version: &query_normalization_version,
                        embedding_family: &embedding_family,
                        embedding_revision: &embedding_revision,
                        dimensions,
                        normalized,
                        runtime_path,
                    },
                )
                .await
            })
        }
    })
    .await
    .expect("seed active generation");
}

/// Upserts one current chunk point for `document_id`, using the index
/// worker's own write path (`QdrantClient::upsert_points` — the same call
/// `services::indexing::persist_chunk_batch` makes) rather than any
/// test-only shortcut.
#[allow(clippy::too_many_arguments)]
async fn seed_similarity_point(
    qdrant: &QdrantClient,
    collection_name: &CollectionName,
    org: Uuid,
    collection_id: Uuid,
    document_id: Uuid,
    version_id: Uuid,
    vector: Vec<f32>,
) {
    let chunk = chunk_identity(
        &document_id.to_string(),
        &version_id.to_string(),
        0,
        "H",
        &format!("similarity-fixture-{}", document_id.simple()),
        BODY_TEXT_VERSION,
    );
    let scope = VectorScope::new(org, [collection_id]);
    let point = UpsertPoint {
        chunk_identity: chunk.clone(),
        vector,
        payload: ChunkPointPayload {
            org_id: org,
            collection_id,
            document_id,
            version_id,
            chunk_id: chunk,
            ordinal: 0,
            is_current: true,
            is_effective: true,
            index_generation: 1,
        },
    };
    qdrant
        .upsert_points(collection_name, &scope, &[point])
        .await
        .expect("upsert similarity fixture point");
}

const MAX_QDRANT_TEST_REQUEST_BYTES: usize = 64 * 1024;

#[derive(Debug)]
struct RecordedQdrantRequest {
    method: String,
    path: String,
    body: Value,
}

#[derive(Debug)]
struct RecordedQdrantTraffic {
    scroll: RecordedQdrantRequest,
    recommend: RecordedQdrantRequest,
}

async fn read_qdrant_test_request(stream: &mut TcpStream) -> RecordedQdrantRequest {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    let header_end = loop {
        let read = stream.read(&mut buffer).await.expect("read Qdrant request");
        assert!(read > 0, "Qdrant test peer received EOF before headers");
        request.extend_from_slice(&buffer[..read]);
        assert!(
            request.len() <= MAX_QDRANT_TEST_REQUEST_BYTES,
            "Qdrant test request exceeded byte bound"
        );
        if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };

    let headers = std::str::from_utf8(&request[..header_end]).expect("HTTP headers are UTF-8");
    let mut lines = headers.split("\r\n");
    let request_line = lines.next().expect("HTTP request line");
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts.next().expect("HTTP method").to_string();
    let path = request_parts.next().expect("HTTP path").to_string();
    assert_eq!(
        request_parts.next(),
        Some("HTTP/1.1"),
        "expected HTTP/1.1 request"
    );
    assert!(
        request_parts.next().is_none(),
        "unexpected request-line fields: {request_line}"
    );

    let content_length = lines
        .filter_map(|line| line.split_once(':'))
        .find_map(|(name, value)| {
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().expect("valid Content-Length"))
        })
        .expect("Qdrant JSON request must have Content-Length");
    assert!(
        header_end + content_length <= MAX_QDRANT_TEST_REQUEST_BYTES,
        "Qdrant test request body exceeded byte bound"
    );
    while request.len() < header_end + content_length {
        let read = stream.read(&mut buffer).await.expect("read Qdrant body");
        assert!(read > 0, "Qdrant test peer received EOF before body");
        request.extend_from_slice(&buffer[..read]);
        assert!(
            request.len() <= MAX_QDRANT_TEST_REQUEST_BYTES,
            "Qdrant test request exceeded byte bound"
        );
    }
    let body = serde_json::from_slice(&request[header_end..header_end + content_length])
        .expect("Qdrant request body is JSON");
    RecordedQdrantRequest { method, path, body }
}

async fn accept_qdrant_test_request(listener: &TcpListener) -> (TcpStream, RecordedQdrantRequest) {
    let (mut stream, _) = tokio::time::timeout(Duration::from_secs(3), listener.accept())
        .await
        .expect("Qdrant test peer accept must stay bounded")
        .expect("accept Qdrant test connection");
    let request = tokio::time::timeout(
        Duration::from_secs(3),
        read_qdrant_test_request(&mut stream),
    )
    .await
    .expect("Qdrant test request read must stay bounded");
    (stream, request)
}

async fn start_failing_qdrant_test_peer(
    org: Uuid,
    collection_id: Uuid,
    document_id: Uuid,
    version_id: Uuid,
) -> (String, tokio::task::JoinHandle<RecordedQdrantTraffic>, Uuid) {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind Qdrant test peer");
    let address = listener.local_addr().expect("Qdrant test peer address");
    let representative_point_id = Uuid::new_v4();
    let task = tokio::spawn(async move {
        let (mut scroll_stream, scroll) = accept_qdrant_test_request(&listener).await;
        let scroll_response = serde_json::json!({
            "result": {
                "points": [{
                    "id": representative_point_id.to_string(),
                    "payload": {
                        "org_id": org.to_string(),
                        "collection_id": collection_id.to_string(),
                        "document_id": document_id.to_string(),
                        "version_id": version_id.to_string(),
                        "chunk_id": "a".repeat(64),
                        "ordinal": 0,
                        "is_current": true,
                        "is_effective": true,
                        "index_generation": 1
                    }
                }],
                "next_page_offset": null
            },
            "status": "ok",
            "time": 0.0
        })
        .to_string();
        let content_length = scroll_response.len();
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {content_length}\r\nconnection: close\r\n\r\n{scroll_response}"
        );
        scroll_stream
            .write_all(response.as_bytes())
            .await
            .expect("write Qdrant scroll response");
        scroll_stream
            .shutdown()
            .await
            .expect("close Qdrant scroll response");

        let (mut recommend_stream, recommend) = accept_qdrant_test_request(&listener).await;
        // The real Qdrant recommend request has now reached the test peer. Close
        // without an HTTP response so reqwest deterministically reports a
        // transport error to `compute_similarity_edges`.
        recommend_stream
            .shutdown()
            .await
            .expect("close Qdrant recommend connection");
        RecordedQdrantTraffic { scroll, recommend }
    });
    (format!("http://{address}"), task, representative_point_id)
}

#[tokio::test]
async fn graph_qdrant_failure_preserves_acl_scoped_conflict_graph() {
    let Some((_db, pool)) = boot_pool().await else {
        return;
    };
    let org = Uuid::new_v4();
    let viewer = Uuid::new_v4();
    let private_owner = Uuid::new_v4();
    common::seed_user_with_permissions(
        &pool,
        org,
        viewer,
        "graph-qdrant-failure@example.com",
        PASSWORD,
        &["qa.query"],
    )
    .await;
    common::seed_user_with_permissions(
        &pool,
        org,
        private_owner,
        "graph-qdrant-failure-private@example.com",
        PASSWORD,
        &[],
    )
    .await;

    let visible_collection = seed_collection(
        &pool,
        org,
        viewer,
        "Đồ thị fail-soft",
        CollectionVisibility::Org,
    )
    .await;
    let visible_a = seed_document(
        &pool,
        org,
        viewer,
        visible_collection,
        "Tài liệu hiển thị A",
    )
    .await;
    let visible_b = seed_document(
        &pool,
        org,
        viewer,
        visible_collection,
        "Tài liệu hiển thị B",
    )
    .await;
    let version_a = seed_version(&pool, org, viewer, visible_a).await;
    let version_b = seed_version(&pool, org, viewer, visible_b).await;
    seed_conflict(
        &pool, org, viewer, visible_a, version_a, visible_b, version_b,
    )
    .await;
    seed_co_citation(&pool, org, viewer, visible_a, visible_b).await;

    let private_collection = seed_collection(
        &pool,
        org,
        private_owner,
        "Đồ thị riêng tư",
        CollectionVisibility::Private,
    )
    .await;
    let hidden = seed_document(
        &pool,
        org,
        private_owner,
        private_collection,
        "Tài liệu bị ẩn",
    )
    .await;
    seed_co_citation(&pool, org, private_owner, visible_a, hidden).await;

    let foreign_org = Uuid::new_v4();
    let foreign_owner = Uuid::new_v4();
    common::seed_user_with_permissions(
        &pool,
        foreign_org,
        foreign_owner,
        "graph-qdrant-failure-foreign@example.com",
        PASSWORD,
        &["qa.query"],
    )
    .await;
    let foreign_collection = seed_collection(
        &pool,
        foreign_org,
        foreign_owner,
        "Đồ thị tenant khác",
        CollectionVisibility::Org,
    )
    .await;
    let foreign_a = seed_document(
        &pool,
        foreign_org,
        foreign_owner,
        foreign_collection,
        "Tài liệu tenant khác A",
    )
    .await;
    let foreign_b = seed_document(
        &pool,
        foreign_org,
        foreign_owner,
        foreign_collection,
        "Tài liệu tenant khác B",
    )
    .await;
    seed_co_citation(&pool, foreign_org, foreign_owner, foreign_a, foreign_b).await;

    let embedder = ApprovedEmbeddingRuntime::new(
        "http://embedding.invalid/v1".into(),
        "test-key".into(),
        "mock".into(),
        format!("graph-fail-soft-{}", Uuid::new_v4().simple()),
        "r1".into(),
        8,
        RUNTIME_VLLM_LOCAL.into(),
        Profile::Test,
        false,
        None,
    )
    .expect("embedder");
    let signature = embedder.plan().index_signature(8).expect("signature");
    seed_active_generation(&pool, org, viewer, visible_collection, &signature).await;

    let (qdrant_url, mut qdrant_peer, representative_point_id) =
        start_failing_qdrant_test_peer(org, visible_collection, visible_a, version_a).await;
    let qdrant = QdrantClient::new(qdrant_url).expect("Qdrant client");
    let ctx = OrgContext::try_new(org, viewer, [] as [&str; 0], [visible_collection]).unwrap();
    let graph_result = tokio::time::timeout(
        Duration::from_secs(5),
        build_org_graph(
            &pool,
            &ctx,
            None,
            Some(SimilarityDeps {
                vector_index: &qdrant,
                embedder: &embedder,
            }),
        ),
    )
    .await;
    let traffic = match tokio::time::timeout(Duration::from_secs(4), &mut qdrant_peer).await {
        Ok(result) => result.expect("Qdrant test peer task"),
        Err(_) => {
            qdrant_peer.abort();
            let _ = qdrant_peer.await;
            panic!("similarity path did not reach the Qdrant test peer");
        }
    };
    let graph = graph_result
        .expect("Qdrant failure path must stay bounded")
        .expect("Qdrant failure must not fail the document graph");

    assert_eq!(traffic.scroll.method, "POST");
    assert!(
        traffic
            .scroll
            .path
            .starts_with("/collections/markhand_chunks_")
            && traffic.scroll.path.ends_with("/points/scroll"),
        "unexpected representative-point request path: {}",
        traffic.scroll.path
    );
    assert_eq!(traffic.scroll.body["with_vector"], false);
    assert_eq!(traffic.recommend.method, "POST");
    assert!(
        traffic
            .recommend
            .path
            .starts_with("/collections/markhand_chunks_")
            && traffic.recommend.path.ends_with("/points/query"),
        "unexpected recommend request path: {}",
        traffic.recommend.path
    );
    assert_eq!(
        traffic.recommend.body["query"]["recommend"]["positive"],
        serde_json::json!([representative_point_id.to_string()]),
        "Qdrant query must be recommend-by-point"
    );
    assert!(
        traffic.recommend.body["filter"]["must"]
            .as_array()
            .is_some_and(|clauses| {
                clauses.iter().any(|clause| clause["key"] == "org_id")
                    && clauses
                        .iter()
                        .any(|clause| clause["key"] == "collection_id")
            }),
        "recommend request must carry mandatory tenant scope: {}",
        traffic.recommend.body
    );

    let node_ids: BTreeSet<_> = graph.nodes.iter().map(|node| node.id).collect();
    assert_eq!(node_ids, BTreeSet::from([visible_a, visible_b]));
    let excluded_ids = BTreeSet::from([hidden, foreign_a, foreign_b]);
    assert!(
        node_ids.is_disjoint(&excluded_ids),
        "private/foreign ACL node leaked: {node_ids:?}"
    );

    assert_eq!(graph.edges.len(), 2, "unexpected edges: {:?}", graph.edges);
    let edge_kinds: BTreeSet<_> = graph.edges.iter().map(|edge| edge.kind.as_str()).collect();
    assert_eq!(edge_kinds, BTreeSet::from(["co_citation", "conflict"]));
    assert!(graph.edges.iter().all(|edge| {
        BTreeSet::from([edge.source, edge.target]) == BTreeSet::from([visible_a, visible_b])
    }));
    assert!(
        graph.edges.iter().all(|edge| edge.kind != "similarity"),
        "failed Qdrant call must not fabricate similarity edges: {:?}",
        graph.edges
    );
    assert!(
        graph.edges.iter().all(
            |edge| !excluded_ids.contains(&edge.source) && !excluded_ids.contains(&edge.target)
        ),
        "private/foreign ACL edge leaked: {:?}",
        graph.edges
    );
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_QDRANT_URL — not run in this sandbox, see module doc"]
async fn graph_similarity_edges_from_qdrant_recommend() {
    let Some((_db, pool)) = boot_pool().await else {
        return;
    };
    let Some(qdrant_url) = test_qdrant_url() else {
        return;
    };
    let qdrant = QdrantClient::new(&qdrant_url).expect("qdrant client");
    let admin = test_qdrant_admin_client(&qdrant_url);

    // A directly-constructed embedder — `.embed()` is never called on this
    // path (recommend is by point id, not by query vector), so no live
    // embedding provider is required. `unique_family` keeps this run's
    // digest/collection from colliding with any other test's.
    let unique_family = format!("graph-similarity-itest-{}", Uuid::new_v4().simple());
    let embedder = ApprovedEmbeddingRuntime::new(
        "http://embedding.invalid/v1".into(),
        "test-key".into(),
        "mock".into(),
        unique_family,
        "r1".into(),
        8,
        RUNTIME_VLLM_LOCAL.into(),
        Profile::Test,
        false,
        None,
    )
    .expect("embedder");
    let signature = embedder.plan().index_signature(8).expect("signature");
    let digest = signature.digest();
    let collection_name = qdrant
        .ensure_collection_for_digest(&digest, 8, true)
        .await
        .expect("ensure collection");

    let org_a = Uuid::new_v4();
    let owner_a = Uuid::new_v4();
    // Creates the org/user/membership rows first — collections.org_id is a
    // real FK; seeding a collection into a nonexistent org fails E23503
    // (caught on this test's first live CI run).
    common::seed_user_with_permissions(
        &pool,
        org_a,
        owner_a,
        "graph-similarity-a@example.com",
        PASSWORD,
        &["qa.query"],
    )
    .await;
    let collection_a = seed_collection(
        &pool,
        org_a,
        owner_a,
        "Tương đồng A",
        CollectionVisibility::Org,
    )
    .await;
    let doc_near_1 = seed_document(
        &pool,
        org_a,
        owner_a,
        collection_a,
        "Chính sách nghỉ phép 2024",
    )
    .await;
    let doc_near_2 = seed_document(
        &pool,
        org_a,
        owner_a,
        collection_a,
        "Chính sách nghỉ phép 2025",
    )
    .await;
    let doc_far = seed_document(
        &pool,
        org_a,
        owner_a,
        collection_a,
        "Báo cáo tài chính quý 3",
    )
    .await;
    seed_active_generation(&pool, org_a, owner_a, collection_a, &signature).await;

    let org_b = Uuid::new_v4();
    let owner_b = Uuid::new_v4();
    common::seed_user_with_permissions(
        &pool,
        org_b,
        owner_b,
        "graph-similarity-b@example.com",
        PASSWORD,
        &["qa.query"],
    )
    .await;
    let collection_b = seed_collection(
        &pool,
        org_b,
        owner_b,
        "Tương đồng B",
        CollectionVisibility::Org,
    )
    .await;
    let doc_cross_org = seed_document(
        &pool,
        org_b,
        owner_b,
        collection_b,
        "Chính sách nghỉ phép org khác",
    )
    .await;
    seed_active_generation(&pool, org_b, owner_b, collection_b, &signature).await;

    // Cosine-scored fixture vectors: near_1/near_2 point in (almost) the same
    // direction (cosine ~1.0, well above `SIMILARITY_SCORE_THRESHOLD`); far
    // is orthogonal to both (cosine ~0.0, filtered by the threshold); the
    // cross-org document reuses near_1's exact vector — if org isolation
    // ever regressed, it would otherwise be near_1's single closest neighbor.
    let near_vector_1 = vec![1.0, 0.05, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    let near_vector_2 = vec![0.98, 0.1, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    let far_vector = vec![0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0];

    seed_similarity_point(
        &qdrant,
        &collection_name,
        org_a,
        collection_a,
        doc_near_1,
        Uuid::new_v4(),
        near_vector_1.clone(),
    )
    .await;
    seed_similarity_point(
        &qdrant,
        &collection_name,
        org_a,
        collection_a,
        doc_near_2,
        Uuid::new_v4(),
        near_vector_2,
    )
    .await;
    seed_similarity_point(
        &qdrant,
        &collection_name,
        org_a,
        collection_a,
        doc_far,
        Uuid::new_v4(),
        far_vector,
    )
    .await;
    seed_similarity_point(
        &qdrant,
        &collection_name,
        org_b,
        collection_b,
        doc_cross_org,
        Uuid::new_v4(),
        near_vector_1,
    )
    .await;

    let ctx_a = OrgContext::try_new(org_a, owner_a, [] as [&str; 0], [collection_a]).unwrap();
    let graph = build_org_graph(
        &pool,
        &ctx_a,
        None,
        Some(SimilarityDeps {
            vector_index: &qdrant,
            embedder: &embedder,
        }),
    )
    .await
    .expect("build org graph for org a");

    let similarity_edges: Vec<_> = graph
        .edges
        .iter()
        .filter(|e| e.kind == "similarity")
        .collect();
    assert!(
        similarity_edges.iter().any(|e| {
            let endpoints = [e.source, e.target];
            endpoints.contains(&doc_near_1) && endpoints.contains(&doc_near_2)
        }),
        "expected a similarity edge between the two near documents, got: {similarity_edges:?}"
    );
    assert!(
        similarity_edges
            .iter()
            .all(|e| e.source != doc_far && e.target != doc_far),
        "far document must be filtered out by the score threshold, got: {similarity_edges:?}"
    );
    assert!(
        similarity_edges
            .iter()
            .all(|e| e.source != doc_cross_org && e.target != doc_cross_org),
        "org B's document must never appear in org A's similarity edges, got: {similarity_edges:?}"
    );

    let near_1_node = graph
        .nodes
        .iter()
        .find(|n| n.id == doc_near_1)
        .expect("near_1 node present");
    assert!(
        near_1_node.degree >= 1,
        "expected near_1's degree to reflect its similarity edge (prune/degree wiring)"
    );

    // Org isolation, other direction: org B's own graph must never surface
    // a similarity edge (it has one visible document — no cross-document
    // neighbor exists within its own node set, and it must not see org A's).
    let ctx_b = OrgContext::try_new(org_b, owner_b, [] as [&str; 0], [collection_b]).unwrap();
    let graph_b = build_org_graph(
        &pool,
        &ctx_b,
        None,
        Some(SimilarityDeps {
            vector_index: &qdrant,
            embedder: &embedder,
        }),
    )
    .await
    .expect("build org graph for org b");
    assert!(
        graph_b.edges.iter().all(|e| e.kind != "similarity"),
        "org B must not see any similarity edge, got: {:?}",
        graph_b.edges
    );

    admin.delete_collection(&collection_name).await.ok();
}
