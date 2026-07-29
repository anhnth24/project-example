//! Live PostgreSQL HTTP contract tests for P2-18 (org -> project -> collection
//! -> document grouping): project CRUD, collection assign/unassign, and the
//! `projectId` filter on /search and /ask.
//!
//! Skips cleanly when `MARKHAND_TEST_DATABASE_URL` / `MARKHAND_TEST_APP_DATABASE_URL`
//! are unset (see `common::boot_app_pool`). Never needs MinIO: chunk body
//! text lives directly in `chunks.body` (Postgres), never fetched from
//! object storage — see `db::search::hydrate_chunks_by_identity`. Most tests
//! here also need no Qdrant: `state.vector_index()` being `None` 503s
//! `/search`/`/ask` before the `projectId` resolution this file mostly
//! exercises even runs (see `routes::search`'s ordering comment) — so the
//! 404-for-unknown-project and CRUD/assign tests run on PG alone. The two
//! tests that assert a real, positive search/ask result additionally require
//! `MARKHAND_TEST_QDRANT_URL` (soft-skipped without it, same convention every
//! other live-retrieval test in this crate uses) because that 503-before-404
//! ordering means a working vector index is the only way to reach retrieval
//! at all. Must run in the `rust-integration` CI job.

mod common;

use std::collections::BTreeSet;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{admin_database_url, app_database_url, boot_app_pool, build_router};
use deadpool_postgres::Pool;
use fileconv_server::auth::context::OrgContext;
use fileconv_server::db::collections::{self, NewCollection};
use fileconv_server::db::documents::{self, NewDocument};
use fileconv_server::db::models::{ArtifactKind, CollectionVisibility, DocumentState};
use fileconv_server::db::pool::with_org_txn;
use fileconv_server::services::chunking::prepare_chunks;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;

const PASSWORD: &str = "correct-password-1";

async fn boot_pool() -> Option<(common::DualRoleEphemeralDb, Pool)> {
    let admin = admin_database_url()?;
    let app = app_database_url()?;
    Some(boot_app_pool(&admin, &app).await)
}

async fn send(
    app: &axum::Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(match body {
            Some(value) => Body::from(value.to_string()),
            None => Body::empty(),
        })
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| json!({ "raw": String::from_utf8_lossy(&bytes) }));
    (status, json)
}

/// Full permission set this file's callers need across CRUD + assign + search/ask.
const FULL_PERMS: &[&str] = &["doc.upload", "doc.delete", "qa.query", "qa.history"];

async fn seed_caller(pool: &Pool, org: Uuid, user: Uuid, email: &str) -> (OrgContext, String) {
    common::seed_user_with_permissions(pool, org, user, email, PASSWORD, FULL_PERMS).await;
    let ctx = OrgContext::try_new(org, user, FULL_PERMS.iter().copied(), []).unwrap();
    let token = common::login_access_token(pool, email, PASSWORD).await;
    (ctx, token)
}

/// A caller with none of `FULL_PERMS` — used for the 403 tests. Must be
/// seeded in its OWN fresh org, never the org under test: `role_permissions`
/// is a per-(org, role) grant, not per-user, and `seed_user_with_permissions`
/// always seeds the `owner` role — reusing the org-under-test's `owner` role
/// for a second user would silently hand them every permission the first
/// (permitted) caller already granted that same role/org, same "seed a plain
/// member in some other org" convention `tests/members.rs`'s
/// `seed_plain_member` documents for the identical reason.
async fn seed_caller_without_permissions(
    pool: &Pool,
    org: Uuid,
    user: Uuid,
    email: &str,
) -> String {
    common::seed_user_with_permissions(pool, org, user, email, PASSWORD, &[]).await;
    common::login_access_token(pool, email, PASSWORD).await
}

/// Creates an org-visible collection directly (bypassing the HTTP layer,
/// same convention `tests/ask_grounding_matrix.rs`'s `seed_ask_doc` uses).
async fn seed_collection(pool: &Pool, ctx: &OrgContext, name: &str) -> Uuid {
    let collection_id = Uuid::new_v4();
    // Matches the `collections.slug` CHECK constraint (migrations/0004):
    // lowercase alnum/hyphen only — `name` here is a free-text display name
    // ("Alpha Collection") that may contain spaces, so it must be slugified,
    // not just lowercased.
    let slugified_name: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let slug = format!("{slugified_name}-{}", collection_id.simple());
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        let name = name.to_string();
        move |txn| {
            Box::pin(async move {
                collections::insert(
                    txn,
                    &ctx,
                    NewCollection {
                        id: collection_id,
                        name: &name,
                        slug: &slug,
                        description: None,
                        visibility: CollectionVisibility::Org,
                    },
                )
                .await
            })
        }
    })
    .await
    .expect("seed collection");
    collection_id
}

/// Seeds one fully indexed document + chunk (Postgres only — no MinIO/Qdrant,
/// see this file's module doc) with `markdown` as searchable body text.
async fn seed_indexed_document(pool: &Pool, ctx: &OrgContext, collection_id: Uuid, markdown: &str) {
    let document_id = Uuid::new_v4();
    let version_id = Uuid::new_v4();
    let index_meta_id = Uuid::new_v4();
    let content_sha = common::sha256_hex(markdown.as_bytes());
    let signature = format!("{:0>64}", index_meta_id.as_u128());
    let chunks = prepare_chunks(document_id, version_id, markdown, "md");
    let md_len = markdown.len() as i64;
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        let content_sha = content_sha.clone();
        let signature = signature.clone();
        let chunks = chunks.clone();
        move |txn| {
            Box::pin(async move {
                documents::insert(
                    txn,
                    &ctx,
                    NewDocument {
                        id: document_id,
                        collection_id,
                        title: "Projects IT doc",
                    },
                )
                .await?;
                txn.execute(
                    "INSERT INTO document_versions (
                        id, org_id, document_id, version_number, publication_state,
                        is_current, content_sha256, original_object_key,
                        source_content_type, byte_size, created_by_user_id
                     ) VALUES ($1,$2,$3,1,'published',true,$4,$5,'text/markdown',$6,$7)",
                    &[
                        &version_id,
                        &ctx.org_id(),
                        &document_id,
                        &content_sha,
                        &format!("test-object-key-{version_id}"),
                        &md_len,
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
                            body_text_version: fileconv_knowledge::identity::BODY_TEXT_VERSION,
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
                let _ = ArtifactKind::Markdown; // silence unused import if chunk loop is ever removed
                Ok(())
            })
        }
    })
    .await
    .expect("seed indexed document");
}

// ---------------------------------------------------------------------
// Project CRUD
// ---------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn create_project_succeeds_and_appears_in_list() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let (_ctx, token) = seed_caller(&pool, org, user, "create-project@projects-it.test").await;

    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/projects",
        Some(&token),
        Some(json!({ "name": "Marketing" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["name"], "Marketing");
    let project_id = body["id"].as_str().unwrap().to_string();

    let (status, list) = send(&app, "GET", "/api/v1/projects", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let names: Vec<String> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_string())
        .collect();
    assert!(names.contains(&project_id));

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn create_project_rejects_empty_name() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let (_ctx, token) = seed_caller(&pool, org, user, "invalid-project@projects-it.test").await;

    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/projects",
        Some(&token),
        Some(json!({ "name": "   " })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "validation_failed");

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn create_project_denied_without_permission() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let token =
        seed_caller_without_permissions(&pool, org, user, "no-perm-project@projects-it.test").await;

    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/projects",
        Some(&token),
        Some(json!({ "name": "Denied" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "forbidden");

    ephemeral.drop().await;
}

/// UX-3(b): duplicate `(org_id, name)` on `POST /projects` maps to `409
/// name_taken`, the same precedent `services::orgs::CreateOrgError::SlugTaken`
/// set for `POST /orgs` — a unique-violation on `uq_projects__org_name`
/// (migrations/0032) must never surface as an opaque 500.
#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn create_project_rejects_a_duplicate_name_with_409() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let (_ctx, token) = seed_caller(&pool, org, user, "dup-project@projects-it.test").await;

    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/projects",
        Some(&token),
        Some(json!({ "name": "Marketing" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let (status_dup, body_dup) = send(
        &app,
        "POST",
        "/api/v1/projects",
        Some(&token),
        Some(json!({ "name": "Marketing" })),
    )
    .await;
    assert_eq!(status_dup, StatusCode::CONFLICT, "{body_dup}");
    assert_eq!(body_dup["code"], "name_taken");

    ephemeral.drop().await;
}

/// Same 409 mapping precedent as `create_project_rejects_a_duplicate_name_with_409`,
/// for `POST /collections`'s own `uq_collections__org_name` (migrations/0004).
/// HTTP-level (not a direct DB insert) so this exercises the actual route's
/// `RouteError::from_db` mapping, not just the constraint itself.
#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn create_collection_rejects_a_duplicate_name_with_409() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let (_ctx, token) = seed_caller(&pool, org, user, "dup-collection@projects-it.test").await;

    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/collections",
        Some(&token),
        Some(json!({
            "name": "Support Docs",
            "slug": format!("support-docs-{}", Uuid::new_v4().simple()),
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let (status_dup, body_dup) = send(
        &app,
        "POST",
        "/api/v1/collections",
        Some(&token),
        Some(json!({
            "name": "Support Docs",
            "slug": format!("support-docs-{}", Uuid::new_v4().simple()),
        })),
    )
    .await;
    assert_eq!(status_dup, StatusCode::CONFLICT, "{body_dup}");
    assert_eq!(body_dup["code"], "name_taken");

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn update_project_renames_and_requires_permission() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let (_ctx, token) = seed_caller(&pool, org, user, "rename-project@projects-it.test").await;

    let (status, created) = send(
        &app,
        "POST",
        "/api/v1/projects",
        Some(&token),
        Some(json!({ "name": "Old Name" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let project_id = created["id"].as_str().unwrap();

    let (status, renamed) = send(
        &app,
        "PATCH",
        &format!("/api/v1/projects/{project_id}"),
        Some(&token),
        Some(json!({ "name": "New Name" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{renamed}");
    assert_eq!(renamed["name"], "New Name");

    let no_perm_org = Uuid::new_v4();
    let no_perm_user = Uuid::new_v4();
    let no_perm_token = seed_caller_without_permissions(
        &pool,
        no_perm_org,
        no_perm_user,
        "rename-no-perm@projects-it.test",
    )
    .await;
    let (status, denied) = send(
        &app,
        "PATCH",
        &format!("/api/v1/projects/{project_id}"),
        Some(&no_perm_token),
        Some(json!({ "name": "Should Not Apply" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{denied}");

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn update_project_404_for_unknown_id() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let (_ctx, token) = seed_caller(&pool, org, user, "unknown-project@projects-it.test").await;

    let ghost = Uuid::new_v4();
    let (status, body) = send(
        &app,
        "PATCH",
        &format!("/api/v1/projects/{ghost}"),
        Some(&token),
        Some(json!({ "name": "Ghost" })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    ephemeral.drop().await;
}

// ---------------------------------------------------------------------
// Org isolation — org B must never see or mutate org A's projects.
// ---------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn org_b_cannot_see_or_rename_org_as_project() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);

    let org_a = Uuid::new_v4();
    let user_a = Uuid::new_v4();
    let (_ctx_a, token_a) = seed_caller(&pool, org_a, user_a, "org-a-owner@projects-it.test").await;
    let (status, created) = send(
        &app,
        "POST",
        "/api/v1/projects",
        Some(&token_a),
        Some(json!({ "name": "Org A Project" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let project_a_id = created["id"].as_str().unwrap().to_string();

    let org_b = Uuid::new_v4();
    let user_b = Uuid::new_v4();
    let (_ctx_b, token_b) = seed_caller(&pool, org_b, user_b, "org-b-owner@projects-it.test").await;

    let (status, list_b) = send(&app, "GET", "/api/v1/projects", Some(&token_b), None).await;
    assert_eq!(status, StatusCode::OK, "{list_b}");
    let ids_b: Vec<String> = list_b["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_string())
        .collect();
    assert!(
        !ids_b.contains(&project_a_id),
        "org B must never see org A's project: {list_b}"
    );

    let (status, denied) = send(
        &app,
        "PATCH",
        &format!("/api/v1/projects/{project_a_id}"),
        Some(&token_b),
        Some(json!({ "name": "Hijacked" })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "org B renaming org A's project must 404, not succeed: {denied}"
    );

    ephemeral.drop().await;
}

// ---------------------------------------------------------------------
// Collection <-> project assign/unassign
// ---------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn assign_and_unassign_collection_project() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let (ctx, token) = seed_caller(&pool, org, user, "assign@projects-it.test").await;

    let collection_id = seed_collection(&pool, &ctx, "Assignable Collection").await;
    let (status, created) = send(
        &app,
        "POST",
        "/api/v1/projects",
        Some(&token),
        Some(json!({ "name": "Assign Target" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let project_id = created["id"].as_str().unwrap().to_string();

    let (status, assigned) = send(
        &app,
        "POST",
        &format!("/api/v1/collections/{collection_id}/assign-project"),
        Some(&token),
        Some(json!({ "projectId": project_id })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{assigned}");
    assert_eq!(assigned["projectId"], project_id);
    assert_eq!(assigned["projectName"], "Assign Target");

    // GET /collections reflects the assignment too (Library nav grouping).
    let (status, list) = send(&app, "GET", "/api/v1/collections", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let entry = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == collection_id.to_string())
        .expect("collection present");
    assert_eq!(entry["projectId"], project_id);

    let (status, unassigned) = send(
        &app,
        "POST",
        &format!("/api/v1/collections/{collection_id}/assign-project"),
        Some(&token),
        Some(json!({ "projectId": Value::Null })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{unassigned}");
    assert!(unassigned["projectId"].is_null());
    assert!(unassigned["projectName"].is_null());

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn assign_project_404_for_unknown_project_id() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let (ctx, token) = seed_caller(&pool, org, user, "assign-ghost@projects-it.test").await;
    let collection_id = seed_collection(&pool, &ctx, "Ghost Target Collection").await;

    let ghost_project = Uuid::new_v4();
    let (status, body) = send(
        &app,
        "POST",
        &format!("/api/v1/collections/{collection_id}/assign-project"),
        Some(&token),
        Some(json!({ "projectId": ghost_project })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn assign_project_denied_without_permission() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let (ctx, _token) = seed_caller(&pool, org, user, "assign-owner@projects-it.test").await;
    let collection_id = seed_collection(&pool, &ctx, "Owner Collection").await;

    let no_perm_org = Uuid::new_v4();
    let no_perm_user = Uuid::new_v4();
    let no_perm_token = seed_caller_without_permissions(
        &pool,
        no_perm_org,
        no_perm_user,
        "assign-no-perm@projects-it.test",
    )
    .await;
    let (status, body) = send(
        &app,
        "POST",
        &format!("/api/v1/collections/{collection_id}/assign-project"),
        Some(&no_perm_token),
        Some(json!({ "projectId": Value::Null })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    ephemeral.drop().await;
}

// ---------------------------------------------------------------------
// /search + /ask projectId filter — the actual retrieval-scoping contract.
// ---------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL + MARKHAND_TEST_QDRANT_URL"]
async fn search_project_filter_returns_exactly_that_projects_documents() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    // A real (reachable) Qdrant is required: `state.vector_index()` must be
    // `Some` for /search to run at all (a `None` vector index 503s before
    // retrieval even starts — see routes::search's own comment on ordering
    // this against the projectId 404 check). The 404-only project-filter
    // tests above/below need no Qdrant precisely because they never get
    // past that check; a real positive search result does.
    let Some(qdrant_url) = common::take_live(
        std::env::var("MARKHAND_TEST_QDRANT_URL")
            .ok()
            .filter(|url| !url.trim().is_empty()),
        "MARKHAND_TEST_QDRANT_URL",
    ) else {
        return;
    };
    let qdrant = fileconv_server::storage::QdrantClient::new(&qdrant_url).expect("qdrant client");
    let app = fileconv_server::http::router(
        common::build_app_state(pool.clone(), &ephemeral.app_url, None)
            .with_retrieval_backends(qdrant, None),
    );
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let (ctx, token) = seed_caller(&pool, org, user, "search-scope@projects-it.test").await;

    // Two projects, one collection each, one uniquely-keyworded document each.
    let collection_alpha = seed_collection(&pool, &ctx, "Alpha Collection").await;
    let collection_beta = seed_collection(&pool, &ctx, "Beta Collection").await;
    seed_indexed_document(
        &pool,
        &ctx,
        collection_alpha,
        "# Alpha\n\nkeywordalpha xuất hiện trong tài liệu alpha duy nhất.",
    )
    .await;
    seed_indexed_document(
        &pool,
        &ctx,
        collection_beta,
        "# Beta\n\nkeywordbeta xuất hiện trong tài liệu beta duy nhất.",
    )
    .await;

    let (status, project_alpha) = send(
        &app,
        "POST",
        "/api/v1/projects",
        Some(&token),
        Some(json!({ "name": "Project Alpha" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{project_alpha}");
    let project_alpha_id = project_alpha["id"].as_str().unwrap().to_string();

    let (status, assigned) = send(
        &app,
        "POST",
        &format!("/api/v1/collections/{collection_alpha}/assign-project"),
        Some(&token),
        Some(json!({ "projectId": project_alpha_id })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{assigned}");
    // Beta stays unassigned — "all projects" must still surface it.

    // Scoped to project Alpha: a query that only matches Beta's document
    // returns nothing.
    let (status, scoped_miss) = send(
        &app,
        "POST",
        "/api/v1/search",
        Some(&token),
        Some(json!({ "query": "keywordbeta", "projectId": project_alpha_id, "limit": 10 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{scoped_miss}");
    assert_eq!(
        scoped_miss["hits"].as_array().unwrap().len(),
        0,
        "project Alpha scope must not surface Beta's document: {scoped_miss}"
    );

    // Scoped to project Alpha: a query matching Alpha's document succeeds.
    let (status, scoped_hit) = send(
        &app,
        "POST",
        "/api/v1/search",
        Some(&token),
        Some(json!({ "query": "keywordalpha", "projectId": project_alpha_id, "limit": 10 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{scoped_hit}");
    assert!(
        !scoped_hit["hits"].as_array().unwrap().is_empty(),
        "project Alpha scope must surface Alpha's own document: {scoped_hit}"
    );
    let scoped_collection_ids: BTreeSet<String> = scoped_hit["hits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|hit| hit["collectionId"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        scoped_collection_ids,
        BTreeSet::from([collection_alpha.to_string()]),
        "every hit under the Alpha project filter must belong to Alpha's collection: {scoped_hit}"
    );

    // "All projects" (no projectId) still finds Beta's document.
    let (status, unscoped) = send(
        &app,
        "POST",
        "/api/v1/search",
        Some(&token),
        Some(json!({ "query": "keywordbeta", "limit": 10 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{unscoped}");
    assert!(
        !unscoped["hits"].as_array().unwrap().is_empty(),
        "no projectId ('all projects') must still find Beta's document: {unscoped}"
    );

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn search_and_ask_404_for_unknown_project_id() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let (_ctx, token) = seed_caller(&pool, org, user, "ghost-scope@projects-it.test").await;

    let ghost = Uuid::new_v4();
    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/search",
        Some(&token),
        Some(json!({ "query": "anything", "projectId": ghost })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/ask",
        Some(&token),
        Some(json!({ "question": "anything?", "projectId": ghost })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL + MARKHAND_TEST_QDRANT_URL"]
async fn ask_project_filter_narrows_grounded_answer_scope() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    // See `search_project_filter_returns_exactly_that_projects_documents`'s
    // comment: a real Qdrant is required for /ask to get past its
    // `vector_index()` availability check at all.
    let Some(qdrant_url) = common::take_live(
        std::env::var("MARKHAND_TEST_QDRANT_URL")
            .ok()
            .filter(|url| !url.trim().is_empty()),
        "MARKHAND_TEST_QDRANT_URL",
    ) else {
        return;
    };
    let qdrant = fileconv_server::storage::QdrantClient::new(&qdrant_url).expect("qdrant client");
    let app = fileconv_server::http::router(
        common::build_app_state(pool.clone(), &ephemeral.app_url, None)
            .with_retrieval_backends(qdrant, None),
    );
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let (ctx, token) = seed_caller(&pool, org, user, "ask-scope@projects-it.test").await;

    let collection_alpha = seed_collection(&pool, &ctx, "Ask Alpha Collection").await;
    let collection_beta = seed_collection(&pool, &ctx, "Ask Beta Collection").await;
    seed_indexed_document(
        &pool,
        &ctx,
        collection_alpha,
        "# Ask Alpha\n\naskkeywordalpha xuất hiện duy nhất ở đây.",
    )
    .await;
    seed_indexed_document(
        &pool,
        &ctx,
        collection_beta,
        "# Ask Beta\n\naskkeywordbeta xuất hiện duy nhất ở đây.",
    )
    .await;

    let (status, project_alpha) = send(
        &app,
        "POST",
        "/api/v1/projects",
        Some(&token),
        Some(json!({ "name": "Ask Project Alpha" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{project_alpha}");
    let project_alpha_id = project_alpha["id"].as_str().unwrap().to_string();
    let (status, assigned) = send(
        &app,
        "POST",
        &format!("/api/v1/collections/{collection_alpha}/assign-project"),
        Some(&token),
        Some(json!({ "projectId": project_alpha_id })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{assigned}");

    // Scoped to Alpha, asking about Beta's unique keyword must ground on
    // nothing from Beta (no citation pointing at Beta's collection).
    let (status, answer) = send(
        &app,
        "POST",
        "/api/v1/ask",
        Some(&token),
        Some(json!({
            "question": "askkeywordbeta la gi?",
            "projectId": project_alpha_id,
            "limit": 5
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    let citations = answer["citations"].as_array().unwrap();
    assert!(
        citations.is_empty(),
        "project Alpha scope must not cite Beta's document: {answer}"
    );

    ephemeral.drop().await;
}

// ---------------------------------------------------------------------
// P2-19 — multi-project `projectIds[]` filter (union), deprecated
// `projectId` kept working, both fields together, bounds, empty array.
// ---------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL + MARKHAND_TEST_QDRANT_URL"]
async fn search_project_ids_filter_unions_multiple_projects() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    // See `search_project_filter_returns_exactly_that_projects_documents`'s
    // comment: a real Qdrant is required for /search to get past its
    // `vector_index()` availability check at all.
    let Some(qdrant_url) = common::take_live(
        std::env::var("MARKHAND_TEST_QDRANT_URL")
            .ok()
            .filter(|url| !url.trim().is_empty()),
        "MARKHAND_TEST_QDRANT_URL",
    ) else {
        return;
    };
    let qdrant = fileconv_server::storage::QdrantClient::new(&qdrant_url).expect("qdrant client");
    let app = fileconv_server::http::router(
        common::build_app_state(pool.clone(), &ephemeral.app_url, None)
            .with_retrieval_backends(qdrant, None),
    );
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let (ctx, token) = seed_caller(&pool, org, user, "search-projectids@projects-it.test").await;

    // Three projects, one collection each, one uniquely-keyworded document
    // each. Gamma stays unassigned — "all projects" territory.
    let collection_alpha = seed_collection(&pool, &ctx, "PIDs Alpha Collection").await;
    let collection_beta = seed_collection(&pool, &ctx, "PIDs Beta Collection").await;
    let collection_gamma = seed_collection(&pool, &ctx, "PIDs Gamma Collection").await;
    seed_indexed_document(
        &pool,
        &ctx,
        collection_alpha,
        "# PIDs Alpha\n\npidskeywordalpha xuất hiện duy nhất ở đây.",
    )
    .await;
    seed_indexed_document(
        &pool,
        &ctx,
        collection_beta,
        "# PIDs Beta\n\npidskeywordbeta xuất hiện duy nhất ở đây.",
    )
    .await;
    seed_indexed_document(
        &pool,
        &ctx,
        collection_gamma,
        "# PIDs Gamma\n\npidskeywordgamma xuất hiện duy nhất ở đây.",
    )
    .await;

    let (status, project_alpha) = send(
        &app,
        "POST",
        "/api/v1/projects",
        Some(&token),
        Some(json!({ "name": "PIDs Project Alpha" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{project_alpha}");
    let project_alpha_id = project_alpha["id"].as_str().unwrap().to_string();
    let (status, project_beta) = send(
        &app,
        "POST",
        "/api/v1/projects",
        Some(&token),
        Some(json!({ "name": "PIDs Project Beta" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{project_beta}");
    let project_beta_id = project_beta["id"].as_str().unwrap().to_string();

    let (status, assigned) = send(
        &app,
        "POST",
        &format!("/api/v1/collections/{collection_alpha}/assign-project"),
        Some(&token),
        Some(json!({ "projectId": project_alpha_id })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{assigned}");
    let (status, assigned) = send(
        &app,
        "POST",
        &format!("/api/v1/collections/{collection_beta}/assign-project"),
        Some(&token),
        Some(json!({ "projectId": project_beta_id })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{assigned}");

    // projectIds: [alpha, beta] must surface both Alpha's and Beta's
    // documents but never Gamma's (unassigned, outside the union).
    for (keyword, expect_hit) in [
        ("pidskeywordalpha", true),
        ("pidskeywordbeta", true),
        ("pidskeywordgamma", false),
    ] {
        let (status, result) = send(
            &app,
            "POST",
            "/api/v1/search",
            Some(&token),
            Some(json!({
                "query": keyword,
                "projectIds": [project_alpha_id, project_beta_id],
                "limit": 10
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{result}");
        let hit_count = result["hits"].as_array().unwrap().len();
        if expect_hit {
            assert!(
                hit_count > 0,
                "projectIds union must surface '{keyword}': {result}"
            );
        } else {
            assert_eq!(
                hit_count, 0,
                "projectIds union must not surface Gamma's '{keyword}': {result}"
            );
        }
    }

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL + MARKHAND_TEST_QDRANT_URL"]
async fn search_project_id_and_project_ids_given_together_union() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let Some(qdrant_url) = common::take_live(
        std::env::var("MARKHAND_TEST_QDRANT_URL")
            .ok()
            .filter(|url| !url.trim().is_empty()),
        "MARKHAND_TEST_QDRANT_URL",
    ) else {
        return;
    };
    let qdrant = fileconv_server::storage::QdrantClient::new(&qdrant_url).expect("qdrant client");
    let app = fileconv_server::http::router(
        common::build_app_state(pool.clone(), &ephemeral.app_url, None)
            .with_retrieval_backends(qdrant, None),
    );
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let (ctx, token) = seed_caller(&pool, org, user, "search-both-fields@projects-it.test").await;

    let collection_alpha = seed_collection(&pool, &ctx, "Both Alpha Collection").await;
    let collection_beta = seed_collection(&pool, &ctx, "Both Beta Collection").await;
    seed_indexed_document(
        &pool,
        &ctx,
        collection_alpha,
        "# Both Alpha\n\nbothkeywordalpha xuất hiện duy nhất ở đây.",
    )
    .await;
    seed_indexed_document(
        &pool,
        &ctx,
        collection_beta,
        "# Both Beta\n\nbothkeywordbeta xuất hiện duy nhất ở đây.",
    )
    .await;

    let (status, project_alpha) = send(
        &app,
        "POST",
        "/api/v1/projects",
        Some(&token),
        Some(json!({ "name": "Both Project Alpha" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{project_alpha}");
    let project_alpha_id = project_alpha["id"].as_str().unwrap().to_string();
    let (status, project_beta) = send(
        &app,
        "POST",
        "/api/v1/projects",
        Some(&token),
        Some(json!({ "name": "Both Project Beta" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{project_beta}");
    let project_beta_id = project_beta["id"].as_str().unwrap().to_string();

    let (status, assigned) = send(
        &app,
        "POST",
        &format!("/api/v1/collections/{collection_alpha}/assign-project"),
        Some(&token),
        Some(json!({ "projectId": project_alpha_id })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{assigned}");
    let (status, assigned) = send(
        &app,
        "POST",
        &format!("/api/v1/collections/{collection_beta}/assign-project"),
        Some(&token),
        Some(json!({ "projectId": project_beta_id })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{assigned}");

    // projectId: alpha (deprecated singular) + projectIds: [beta] together
    // must union to both — neither field alone would find the other's doc.
    let (status, result) = send(
        &app,
        "POST",
        "/api/v1/search",
        Some(&token),
        Some(json!({
            "query": "bothkeywordbeta",
            "projectId": project_alpha_id,
            "projectIds": [project_beta_id],
            "limit": 10
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert!(
        !result["hits"].as_array().unwrap().is_empty(),
        "projectId + projectIds together must union, finding Beta's doc: {result}"
    );

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn search_and_ask_404_for_unknown_project_id_in_array() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let (_ctx, token) = seed_caller(&pool, org, user, "ghost-scope-array@projects-it.test").await;

    // One real project alongside one that never existed — a single bad id
    // anywhere in the array must 404 the whole request, same as the
    // singular `projectId` contract.
    let (status, real_project) = send(
        &app,
        "POST",
        "/api/v1/projects",
        Some(&token),
        Some(json!({ "name": "Real Project For Ghost Array" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{real_project}");
    let real_project_id = real_project["id"].as_str().unwrap().to_string();
    let ghost = Uuid::new_v4();

    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/search",
        Some(&token),
        Some(json!({
            "query": "anything",
            "projectIds": [real_project_id, ghost.to_string()]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/ask",
        Some(&token),
        Some(json!({
            "question": "anything?",
            "projectIds": [ghost.to_string()]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn search_project_ids_rejects_more_than_twenty() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let (_ctx, token) = seed_caller(&pool, org, user, "too-many-projectids@projects-it.test").await;

    let ids: Vec<String> = (0..21).map(|_| Uuid::new_v4().to_string()).collect();
    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/search",
        Some(&token),
        Some(json!({ "query": "anything", "projectIds": ids })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "validation_failed");

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn search_empty_project_ids_array_behaves_like_absent() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    // No Qdrant wired here on purpose: both "no project fields at all" and
    // "projectIds: []" must resolve their (non-existent) project scope
    // identically and fall through to the *next* check in routes::search
    // (vector_index availability, 503 without a real backend) rather than
    // 404 — proving an empty array is never mistaken for an unknown id.
    let app = build_router(pool.clone(), &ephemeral.app_url, None);
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let (_ctx, token) = seed_caller(&pool, org, user, "empty-projectids@projects-it.test").await;

    let (status_absent, body_absent) = send(
        &app,
        "POST",
        "/api/v1/search",
        Some(&token),
        Some(json!({ "query": "anything" })),
    )
    .await;
    let (status_empty, body_empty) = send(
        &app,
        "POST",
        "/api/v1/search",
        Some(&token),
        Some(json!({ "query": "anything", "projectIds": [] })),
    )
    .await;
    assert_eq!(
        status_absent,
        StatusCode::SERVICE_UNAVAILABLE,
        "{body_absent}"
    );
    assert_eq!(
        status_empty, status_absent,
        "projectIds: [] must resolve identically to omitting it: {body_empty}"
    );

    ephemeral.drop().await;
}
