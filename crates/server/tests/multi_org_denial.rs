//! Phase 1C executable multi-org denial tests using the shared world.

mod common;

use std::collections::BTreeSet;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::multi_org_denial::{
    assert_denial_no_leak, DenialExpectation, DenialResponse, MultiOrgDenialWorld,
};
use common::multi_org_denial_world::{
    ask_token_sequences_after, await_ask_stream_pre_revoke_evidence, await_ask_stream_terminal,
    BootedOrg, IndexedDenialRuntime,
};
use common::{
    admin_database_url, app_database_url, login_tokens, seed_user_with_permissions,
    test_minio_client, test_qdrant_url,
};
use futures::StreamExt;
use http_body_util::BodyExt;
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;

const PASSWORD: &str = "correct-password-1";
/// Bounded wait for worker-indexed chunks to become retrieval-visible under
/// parallel suite load. Unlike the world's DB-level ASK_STREAM_EVIDENCE polls,
/// these probes go through HTTP, so the backoff must stay well under the
/// expensive-route rate limit (60/min by default) or the poll itself draws
/// 429s (seen in CI run 30778007036).
const SEARCH_VISIBILITY_POLL_TIMEOUT: Duration = Duration::from_secs(30);
const SEARCH_VISIBILITY_POLL_BACKOFF: Duration = Duration::from_secs(1);

async fn boot_world_if_live() -> Option<MultiOrgDenialWorld> {
    admin_database_url()?;
    app_database_url()?;
    Some(MultiOrgDenialWorld::boot().await)
}

async fn boot_indexed_world_if_live() -> Option<(MultiOrgDenialWorld, IndexedDenialRuntime)> {
    admin_database_url()?;
    app_database_url()?;
    test_qdrant_url()?;
    test_minio_client()?;
    let mut world = MultiOrgDenialWorld::boot().await;
    let runtime = IndexedDenialRuntime::boot_and_index(&mut world)
        .await
        .expect("indexed denial runtime");
    Some((world, runtime))
}

async fn json_request(
    app: &axum::Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<serde_json::Value>,
) -> (StatusCode, Vec<u8>, Vec<(String, String)>) {
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
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.to_string(),
                String::from_utf8_lossy(value.as_bytes()).into_owned(),
            )
        })
        .collect();
    let body = response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    (status, body, headers)
}

fn header_refs(headers: &[(String, String)]) -> Vec<(&str, &str)> {
    headers
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect()
}

fn denial_response<'a>(
    status: StatusCode,
    body: &'a [u8],
    headers: &'a [(String, String)],
) -> DenialResponse<'a> {
    DenialResponse {
        status: status.as_u16(),
        body,
        headers: header_refs(headers),
    }
}

/// Stable path-denial envelope fields — avoids wall-clock timing oracles.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PathDenialShape {
    status: u16,
    code: Option<String>,
    has_request_id: bool,
}

fn path_denial_shape(status: StatusCode, body: &[u8]) -> PathDenialShape {
    let json: serde_json::Value =
        serde_json::from_slice(body).unwrap_or_else(|_| json!({ "raw": true }));
    PathDenialShape {
        status: status.as_u16(),
        code: json
            .get("code")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        has_request_id: json.get("requestId").is_some(),
    }
}

async fn assert_path_idor_not_found(
    app: &axum::Router,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<serde_json::Value>,
    foreign: &common::multi_org_denial::ForeignMarkers,
) {
    let (status, response_body, headers) = json_request(app, method, uri, Some(token), body).await;
    assert_denial_no_leak(
        &denial_response(status, &response_body, &headers),
        foreign,
        DenialExpectation::PathIdorNotFound,
    );
}

async fn assert_body_scope_forbidden(
    app: &axum::Router,
    method: &str,
    uri: &str,
    token: &str,
    body: serde_json::Value,
    foreign: &common::multi_org_denial::ForeignMarkers,
) {
    let (status, response_body, headers) =
        json_request(app, method, uri, Some(token), Some(body)).await;
    assert_denial_no_leak(
        &denial_response(status, &response_body, &headers),
        foreign,
        DenialExpectation::BodyScopeForbidden,
    );
}

async fn document_index_state(
    pool: &deadpool_postgres::Pool,
    org: &BootedOrg,
) -> Result<(String, i64), String> {
    use fileconv_server::auth::context::OrgContext;
    use fileconv_server::db::pool::with_org_txn;

    let owner = org.users.get("owner").ok_or("missing owner user")?;
    let org_id = org.org_id;
    let owner_id = owner.user_id;
    let collection_id = org.collections["org"].collection_id;
    let ctx = OrgContext::try_new(
        org_id,
        owner_id,
        ["qa.query", "doc.upload"],
        [collection_id],
    )
    .map_err(|err| err.to_string())?;
    let indexed = org
        .indexed_document
        .as_ref()
        .ok_or_else(|| format!("org {} has no worker-produced indexed document", org.slug))?;
    with_org_txn(pool, &ctx, {
        let document_id = indexed.document_id;
        let org_id = ctx.org_id();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT d.state::text,
                                (SELECT COUNT(*)::bigint FROM chunks c
                                 WHERE c.org_id = d.org_id AND c.document_id = d.id) AS chunk_count
                         FROM documents d
                         WHERE d.org_id = $1 AND d.id = $2",
                        &[&org_id, &document_id],
                    )
                    .await?;
                Ok((row.get::<_, String>(0), row.get::<_, i64>(1)))
            })
        }
    })
    .await
    .map_err(|err| format!("load document index state: {err}"))
}

/// Per-chunk snapshot of every predicate the FTS candidate query requires
/// (`db::search::fts_search`), so a timed-out search probe reports which
/// visibility condition was still unmet instead of failing blind.
async fn search_visibility_snapshot(
    pool: &deadpool_postgres::Pool,
    org: &BootedOrg,
    marker: &str,
) -> Result<String, String> {
    use fileconv_server::auth::context::OrgContext;
    use fileconv_server::db::pool::with_org_txn;

    let owner = org.users.get("owner").ok_or("missing owner user")?;
    let collection_id = org.collections["org"].collection_id;
    let ctx = OrgContext::try_new(
        org.org_id,
        owner.user_id,
        ["qa.query", "doc.upload"],
        [collection_id],
    )
    .map_err(|err| err.to_string())?;
    let indexed = org
        .indexed_document
        .as_ref()
        .ok_or_else(|| format!("org {} has no worker-produced indexed document", org.slug))?;
    let rows = with_org_txn(pool, &ctx, {
        let document_id = indexed.document_id;
        let org_id = ctx.org_id();
        let marker = marker.to_string();
        move |txn| {
            Box::pin(async move {
                let rows = txn
                    .query(
                        "SELECT c.id::text,
                                d.state::text AS doc_state,
                                dv.publication_state::text,
                                dv.is_current,
                                im.is_active,
                                im.state::text AS generation_state,
                                c.tsv @@ plainto_tsquery('simple', $3) AS tsv_match
                         FROM chunks c
                         JOIN documents d
                           ON d.org_id = c.org_id AND d.id = c.document_id
                         JOIN document_versions dv
                           ON dv.org_id = c.org_id
                          AND dv.document_id = c.document_id
                          AND dv.id = c.version_id
                         JOIN index_metadata im
                           ON im.org_id = c.org_id AND im.id = c.index_metadata_id
                         WHERE c.org_id = $1 AND c.document_id = $2
                         ORDER BY c.id",
                        &[&org_id, &document_id, &marker],
                    )
                    .await?;
                Ok(rows
                    .iter()
                    .map(|row| {
                        format!(
                            "chunk {} doc_state={} publication_state={} is_current={} \
                             generation_active={} generation_state={} tsv_match={}",
                            row.get::<_, String>(0),
                            row.get::<_, String>(1),
                            row.get::<_, String>(2),
                            row.get::<_, bool>(3),
                            row.get::<_, bool>(4),
                            row.get::<_, String>(5),
                            row.get::<_, bool>(6),
                        )
                    })
                    .collect::<Vec<_>>())
            })
        }
    })
    .await
    .map_err(|err| format!("load search visibility snapshot: {err}"))?;
    if rows.is_empty() {
        return Ok("no chunks joined the FTS visibility predicates".to_string());
    }
    Ok(rows.join("; "))
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

async fn seed_cross_org_bridge_user(world: &MultiOrgDenialWorld) -> (String, Uuid) {
    use fileconv_server::auth::context::OrgContext;
    use fileconv_server::db::pool::with_org_txn;

    let alpha = world.org("orgAlpha");
    let beta = world.org("orgBeta");
    let bridge_user = Uuid::new_v4();
    let email = format!("bridge-{}@denial.test", bridge_user.simple());
    let perms = ["qa.query"];
    seed_user_with_permissions(
        world.pool(),
        alpha.org_id,
        bridge_user,
        &email,
        PASSWORD,
        &perms,
    )
    .await;
    seed_user_with_permissions(
        world.pool(),
        beta.org_id,
        bridge_user,
        &email,
        PASSWORD,
        &perms,
    )
    .await;
    for org_id in [alpha.org_id, beta.org_id] {
        let ctx = OrgContext::try_new(org_id, bridge_user, perms, []).expect("bridge context");
        with_org_txn(world.pool(), &ctx, move |txn| {
            Box::pin(async move {
                txn.execute(
                    "UPDATE org_memberships
                     SET role = 'viewer'
                     WHERE org_id = $1 AND user_id = $2",
                    &[&org_id, &bridge_user],
                )
                .await?;
                txn.execute(
                    "INSERT INTO roles (id, org_id, code, name, is_system)
                     VALUES ($1, $2, 'viewer', 'Viewer', true)
                     ON CONFLICT (org_id, code) DO NOTHING",
                    &[&Uuid::new_v4(), &org_id],
                )
                .await?;
                txn.execute(
                    "INSERT INTO role_permissions (org_id, role_id, permission_id)
                     SELECT $1, r.id, p.id
                     FROM roles r
                     JOIN permissions p ON p.code = 'qa.query'
                     WHERE r.org_id = $1 AND r.code = 'viewer'
                     ON CONFLICT DO NOTHING",
                    &[&org_id],
                )
                .await?;
                Ok(())
            })
        })
        .await
        .expect("set bridge viewer membership");
    }
    let (token, _) = login_tokens(world.pool(), &email, PASSWORD).await;
    (token, bridge_user)
}

async fn switch_org(app: &axum::Router, token: &str, target_org_id: Uuid) -> (String, String) {
    let (status, body, _) = json_request(
        app,
        "POST",
        "/api/v1/orgs/switch",
        Some(token),
        Some(json!({ "orgId": target_org_id })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "org switch must succeed: {}",
        String::from_utf8_lossy(&body)
    );
    let json: serde_json::Value = serde_json::from_slice(&body).expect("switch json");
    (
        json["accessToken"]
            .as_str()
            .expect("accessToken")
            .to_string(),
        json["refreshToken"]
            .as_str()
            .expect("refreshToken")
            .to_string(),
    )
}

async fn assert_access_rejected(app: &axum::Router, access_token: &str) {
    let (status, body, _) =
        json_request(app, "GET", "/api/v1/auth/me", Some(access_token), None).await;
    if status == StatusCode::UNAUTHORIZED {
        return;
    }
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "stale access token must fail closed with 401 or 403: {}",
        String::from_utf8_lossy(&body)
    );
    let error: serde_json::Value = serde_json::from_slice(&body).expect("stale access denial JSON");
    assert!(
        matches!(
            error["code"].as_str(),
            Some("membership_missing" | "membership_inactive")
        ),
        "403 stale access denial must explicitly report inactive/missing membership: {error}"
    );
}

async fn assert_admin_members_forbidden(
    app: &axum::Router,
    access_token: &str,
    foreign: &common::multi_org_denial::ForeignMarkers,
) {
    let (status, body, headers) =
        json_request(app, "GET", "/api/v1/members", Some(access_token), None).await;
    assert_denial_no_leak(
        &denial_response(status, &body, &headers),
        foreign,
        DenialExpectation::BodyScopeForbidden,
    );
}

async fn assert_refresh_rejected(app: &axum::Router, refresh_token: &str) {
    let (status, body, _) = json_request(
        app,
        "POST",
        "/api/v1/auth/refresh",
        None,
        Some(json!({ "refreshToken": refresh_token })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "stale refresh token must be rejected: {}",
        String::from_utf8_lossy(&body)
    );
}

/// Cross-org HTTP surfaces that lack a dedicated legacy integration test.
#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn shared_world_http_surfaces_respect_org_scope() {
    let Some(world) = boot_world_if_live().await else {
        return;
    };
    world.assert_base_topology();

    let alpha = world.org("orgAlpha");
    let beta = world.org("orgBeta");
    let token_a = &alpha.users["owner"].access_token;
    let foreign = world.foreign_markers_for("orgAlpha");

    let (status, body, headers) =
        json_request(world.app(), "GET", "/api/v1/auth/me", Some(token_a), None).await;
    assert_denial_no_leak(
        &denial_response(status, &body, &headers),
        &foreign,
        DenialExpectation::AllowSuccess,
    );
    let me: serde_json::Value = serde_json::from_slice(&body).expect("auth me json");
    assert!(!me.to_string().contains(&beta.org_id.to_string()));

    let (status, body, headers) = json_request(
        world.app(),
        "GET",
        "/api/v1/collections",
        Some(token_a),
        None,
    )
    .await;
    assert_denial_no_leak(
        &denial_response(status, &body, &headers),
        &foreign,
        DenialExpectation::AllowSuccess,
    );
    let listed: serde_json::Value = serde_json::from_slice(&body).expect("collections json");
    let alpha_collection_ids: BTreeSet<String> = alpha
        .collections
        .values()
        .map(|c| c.collection_id.to_string())
        .collect();
    let beta_collection_ids: BTreeSet<String> = beta
        .collections
        .values()
        .map(|c| c.collection_id.to_string())
        .collect();
    let listed_ids: Vec<String> = listed
        .get("items")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("id").and_then(|id| id.as_str()))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    for id in &listed_ids {
        assert!(
            alpha_collection_ids.contains(id),
            "collections list must only include actor org ids; foreign id leaked: {id}"
        );
        assert!(
            !beta_collection_ids.contains(id),
            "collections list must not include foreign org id: {id}"
        );
    }

    let (status, body, headers) = json_request(
        world.app(),
        "POST",
        "/api/v1/collections",
        Some(token_a),
        Some(json!({
            "name": "Another",
            "slug": format!("denial-new-{}", uuid::Uuid::new_v4().simple()),
            "visibility": "org"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_denial_no_leak(
        &denial_response(status, &body, &headers),
        &foreign,
        DenialExpectation::AllowSuccess,
    );

    let foreign_collection = beta.collections["org"].collection_id;
    let (status, body, headers) = json_request(
        world.app(),
        "POST",
        &format!("/api/v1/collections/{foreign_collection}/assign-project"),
        Some(token_a),
        Some(json!({ "projectId": null })),
    )
    .await;
    assert_denial_no_leak(
        &denial_response(status, &body, &headers),
        &foreign,
        DenialExpectation::PathIdorNotFound,
    );

    let (status, body, headers) = json_request(
        world.app(),
        "GET",
        "/api/v1/members/invites",
        Some(token_a),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_denial_no_leak(
        &denial_response(status, &body, &headers),
        &foreign,
        DenialExpectation::AllowSuccess,
    );

    world.cleanup().await.expect("cleanup");
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL + MARKHAND_TEST_QDRANT_URL + MinIO"]
async fn indexed_fts_and_ask_never_return_foreign_marker() {
    let Some((world, runtime)) = boot_indexed_world_if_live().await else {
        return;
    };
    world.assert_base_topology();
    for (key, org) in &world.orgs {
        let (state, chunk_count) = document_index_state(world.pool(), org)
            .await
            .expect("indexed state");
        assert_eq!(
            state, "indexed",
            "org {key} must be worker-indexed before FTS/ask probes"
        );
        assert!(chunk_count > 0, "org {key} must have searchable chunks");
    }

    let alpha = world.org("orgAlpha");
    let alpha_indexed = alpha
        .indexed_document
        .as_ref()
        .expect("alpha indexed document");
    let token = &alpha.users["owner"].access_token;
    let foreign = world.foreign_markers_for("orgAlpha");
    let beta = world.org("orgBeta");
    let beta_indexed = beta
        .indexed_document
        .as_ref()
        .expect("beta indexed document");
    let app = world.app();

    // Retrieval visibility can trail the synchronously drained worker jobs
    // when the whole suite runs in parallel, so poll with a bounded backoff.
    // The denial property (no foreign marker) must hold on EVERY response;
    // only own-marker visibility is allowed to arrive late. A 429 from the
    // shared rate limiter carries no tenant data and is retried, not failed.
    let query = alpha.marker.clone();
    let deadline = tokio::time::Instant::now() + SEARCH_VISIBILITY_POLL_TIMEOUT;
    loop {
        let (status, body, headers) = json_request(
            app,
            "POST",
            "/api/v1/search",
            Some(token),
            Some(json!({ "query": query, "mode": "current", "limit": 10 })),
        )
        .await;
        if status == StatusCode::TOO_MANY_REQUESTS {
            assert!(
                tokio::time::Instant::now() < deadline,
                "search visibility poll stayed rate-limited past {}s",
                SEARCH_VISIBILITY_POLL_TIMEOUT.as_secs()
            );
            tokio::time::sleep(SEARCH_VISIBILITY_POLL_BACKOFF).await;
            continue;
        }
        assert_denial_no_leak(
            &denial_response(status, &body, &headers),
            &foreign,
            DenialExpectation::AllowSuccess,
        );
        let search: serde_json::Value = serde_json::from_slice(&body).expect("search json");
        let hits = search["hits"].as_array().expect("hits array");
        assert!(
            !hits.iter().any(|hit| {
                hit["documentId"].as_str() == Some(beta_indexed.document_id.to_string().as_str())
                    || hit["quote"]
                        .as_str()
                        .is_some_and(|quote| quote.contains(&beta.marker))
            }),
            "search must not return foreign indexed marker: {search}"
        );
        let own_marker_visible = hits.iter().any(|hit| {
            hit["documentId"].as_str() == Some(alpha_indexed.document_id.to_string().as_str())
                || hit["quote"]
                    .as_str()
                    .is_some_and(|quote| quote.contains(&alpha.marker))
        });
        if own_marker_visible {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            let visibility = search_visibility_snapshot(world.pool(), alpha, &alpha.marker)
                .await
                .unwrap_or_else(|error| format!("snapshot unavailable: {error}"));
            panic!(
                "actor search must surface own indexed marker within {}s; \
                 last response: {search}; visibility: {visibility}",
                SEARCH_VISIBILITY_POLL_TIMEOUT.as_secs()
            );
        }
        tokio::time::sleep(SEARCH_VISIBILITY_POLL_BACKOFF).await;
    }

    // The ask probe shares rate-limit budget with the search polls above, so
    // it retries 429s within the same bounded-backoff convention.
    let ask_deadline = tokio::time::Instant::now() + SEARCH_VISIBILITY_POLL_TIMEOUT;
    let body = loop {
        let (status, body, headers) = json_request(
            app,
            "POST",
            "/api/v1/ask",
            Some(token),
            Some(json!({
                "question": alpha.marker.clone(),
                "mode": "current",
                "limit": 5
            })),
        )
        .await;
        if status == StatusCode::TOO_MANY_REQUESTS {
            assert!(
                tokio::time::Instant::now() < ask_deadline,
                "ask probe stayed rate-limited past {}s",
                SEARCH_VISIBILITY_POLL_TIMEOUT.as_secs()
            );
            tokio::time::sleep(SEARCH_VISIBILITY_POLL_BACKOFF).await;
            continue;
        }
        assert_denial_no_leak(
            &denial_response(status, &body, &headers),
            &foreign,
            DenialExpectation::AllowSuccess,
        );
        break body;
    };
    let ask_text = String::from_utf8_lossy(&body);
    assert!(
        ask_text.contains(&alpha.marker),
        "ask answer must reference actor marker when grounded: {ask_text}"
    );
    assert!(
        !ask_text.contains(&beta.marker),
        "ask must not leak foreign marker: {ask_text}"
    );

    runtime.teardown().await;
    world.cleanup().await.expect("cleanup");
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn duplicate_names_across_orgs_do_not_create_an_oracle() {
    let Some(world) = boot_world_if_live().await else {
        return;
    };
    world.assert_base_topology();

    let alpha = world.org("orgAlpha");
    let beta = world.org("orgBeta");
    let token = &alpha.users["owner"].access_token;
    let foreign = world.foreign_markers_for("orgAlpha");
    let app = world.app();
    let shared_collection_name = &world.fixture.duplicate_names.collection;
    let shared_document_title = &world.fixture.duplicate_names.document;

    let (status, body, headers) =
        json_request(app, "GET", "/api/v1/collections", Some(token), None).await;
    assert_denial_no_leak(
        &denial_response(status, &body, &headers),
        &foreign,
        DenialExpectation::AllowSuccess,
    );
    let listed: serde_json::Value = serde_json::from_slice(&body).expect("collections json");
    let items = listed["items"].as_array().expect("collection items");
    let alpha_ids: BTreeSet<String> = alpha
        .collections
        .values()
        .map(|c| c.collection_id.to_string())
        .collect();
    let matching_names: Vec<_> = items
        .iter()
        .filter(|item| item["name"].as_str() == Some(shared_collection_name.as_str()))
        .collect();
    assert_eq!(
        matching_names.len(),
        1,
        "duplicate collection name must resolve to exactly one actor-org row: {listed}"
    );
    let matched_id = matching_names[0]["id"].as_str().expect("collection id");
    assert!(
        alpha_ids.contains(matched_id),
        "name collision must not surface foreign collection id: {listed}"
    );

    let (status, body, headers) = json_request(
        app,
        "GET",
        &format!(
            "/api/v1/collections/{}/documents",
            alpha.collections["org"].collection_id
        ),
        Some(token),
        None,
    )
    .await;
    assert_denial_no_leak(
        &denial_response(status, &body, &headers),
        &foreign,
        DenialExpectation::AllowSuccess,
    );
    let docs: serde_json::Value = serde_json::from_slice(&body).expect("documents json");
    let doc_items = docs["items"].as_array().expect("document items");
    let titles: Vec<_> = doc_items
        .iter()
        .filter_map(|item| item["title"].as_str())
        .collect();
    assert!(
        titles.contains(&shared_document_title.as_str()),
        "actor document list must include shared title: {docs}"
    );
    for item in doc_items {
        let id = item["id"].as_str().expect("document id");
        assert_ne!(
            id,
            beta.document.document_id.to_string(),
            "duplicate document title must not expose foreign document id: {docs}"
        );
    }

    let foreign_collection = beta.collections["org"].collection_id;
    let ghost_collection = Uuid::new_v4();
    let (foreign_status, foreign_collection_body, _) = json_request(
        app,
        "GET",
        &format!("/api/v1/collections/{foreign_collection}"),
        Some(token),
        None,
    )
    .await;
    let foreign_probe = path_denial_shape(foreign_status, &foreign_collection_body);
    let (ghost_status, ghost_collection_body, _) = json_request(
        app,
        "GET",
        &format!("/api/v1/collections/{ghost_collection}"),
        Some(token),
        None,
    )
    .await;
    let ghost_probe = path_denial_shape(ghost_status, &ghost_collection_body);
    assert_eq!(foreign_probe.status, StatusCode::NOT_FOUND.as_u16());
    assert_eq!(ghost_probe.status, StatusCode::NOT_FOUND.as_u16());
    assert_eq!(
        foreign_probe, ghost_probe,
        "foreign collection path must be indistinguishable from unknown id"
    );

    let foreign_doc = beta.document.document_id;
    let ghost_doc = Uuid::new_v4();
    let (foreign_status, foreign_body, foreign_headers) = json_request(
        app,
        "GET",
        &format!("/api/v1/documents/{foreign_doc}"),
        Some(token),
        None,
    )
    .await;
    assert_denial_no_leak(
        &denial_response(foreign_status, &foreign_body, &foreign_headers),
        &foreign,
        DenialExpectation::PathIdorNotFound,
    );
    let foreign_doc_shape = path_denial_shape(foreign_status, &foreign_body);
    let (ghost_status, ghost_body, _) = json_request(
        app,
        "GET",
        &format!("/api/v1/documents/{ghost_doc}"),
        Some(token),
        None,
    )
    .await;
    let ghost_doc_shape = path_denial_shape(ghost_status, &ghost_body);
    assert_eq!(
        foreign_doc_shape, ghost_doc_shape,
        "foreign document path must be indistinguishable from unknown id"
    );

    world.cleanup().await.expect("cleanup");
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL + MARKHAND_TEST_QDRANT_URL + MinIO"]
async fn org_switch_never_reuses_previous_org_cache_scope() {
    let Some((world, runtime)) = boot_indexed_world_if_live().await else {
        return;
    };
    world.assert_base_topology();

    let alpha = world.org("orgAlpha");
    let beta = world.org("orgBeta");
    let (bridge_token, _) = seed_cross_org_bridge_user(&world).await;
    let app = world.app();

    let (status, orgs_body, _) =
        json_request(app, "GET", "/api/v1/orgs", Some(&bridge_token), None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "bridge org inventory must be available: {}",
        String::from_utf8_lossy(&orgs_body)
    );
    let orgs: serde_json::Value =
        serde_json::from_slice(&orgs_body).expect("bridge org inventory JSON");
    let org_ids: BTreeSet<String> = orgs["items"]
        .as_array()
        .expect("bridge org items")
        .iter()
        .map(|item| item["id"].as_str().expect("bridge org id").to_string())
        .collect();
    assert_eq!(
        org_ids,
        BTreeSet::from([alpha.org_id.to_string(), beta.org_id.to_string()]),
        "bridge identity must expose exactly its two seeded org memberships: {orgs}"
    );

    let (status, me_body, _) =
        json_request(app, "GET", "/api/v1/auth/me", Some(&bridge_token), None).await;
    assert_eq!(status, StatusCode::OK);
    let me: serde_json::Value = serde_json::from_slice(&me_body).expect("auth me");
    let origin_org = Uuid::parse_str(me["orgId"].as_str().expect("orgId")).expect("org uuid");
    let origin_key = world.org_key_for_id(origin_org);
    let (target_org, target, origin, target_key) = if origin_org == alpha.org_id {
        (beta.org_id, beta, alpha, "orgBeta")
    } else {
        (alpha.org_id, alpha, beta, "orgAlpha")
    };

    let (status, warm_body, warm_headers) =
        json_request(app, "GET", "/api/v1/collections", Some(&bridge_token), None).await;
    assert_denial_no_leak(
        &denial_response(status, &warm_body, &warm_headers),
        &world.foreign_markers_for(origin_key),
        DenialExpectation::AllowSuccess,
    );
    let warm: serde_json::Value =
        serde_json::from_slice(&warm_body).expect("origin collections JSON");
    let warm_ids: BTreeSet<String> = warm["items"]
        .as_array()
        .expect("origin collection items")
        .iter()
        .filter_map(|item| item["id"].as_str())
        .map(str::to_string)
        .collect();
    assert_eq!(
        warm_ids,
        BTreeSet::from([origin.collections["org"].collection_id.to_string()]),
        "bridge viewer must warm only the origin org-visible collection"
    );

    let (switched_token, _) = switch_org(app, &bridge_token, target_org).await;
    let foreign_after_switch = world.foreign_markers_for(target_key);

    let (status, body, headers) = json_request(
        app,
        "GET",
        "/api/v1/collections",
        Some(&switched_token),
        None,
    )
    .await;
    assert_denial_no_leak(
        &denial_response(status, &body, &headers),
        &foreign_after_switch,
        DenialExpectation::AllowSuccess,
    );
    let listed: serde_json::Value = serde_json::from_slice(&body).expect("collections json");
    let listed_ids: BTreeSet<String> = listed["items"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .filter_map(|item| item.get("id").and_then(|id| id.as_str()))
        .map(str::to_string)
        .collect();
    let target_ids = BTreeSet::from([target.collections["org"].collection_id.to_string()]);
    let origin_ids: BTreeSet<String> = origin
        .collections
        .values()
        .map(|c| c.collection_id.to_string())
        .collect();
    assert_eq!(
        listed_ids, target_ids,
        "switched session must list only target org collections"
    );
    for origin_id in &origin_ids {
        assert!(
            !listed_ids.contains(origin_id),
            "switched session leaked previous-org collection id: {listed}"
        );
    }

    assert_path_idor_not_found(
        app,
        "GET",
        &format!(
            "/api/v1/collections/{}",
            origin.collections["org"].collection_id
        ),
        &switched_token,
        None,
        &foreign_after_switch,
    )
    .await;

    assert_body_scope_forbidden(
        app,
        "POST",
        "/api/v1/search",
        &switched_token,
        json!({
            "query": origin.marker,
            "collectionIds": [origin.collections["org"].collection_id],
            "limit": 5
        }),
        &foreign_after_switch,
    )
    .await;

    runtime.teardown().await;
    world.cleanup().await.expect("cleanup");
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn pre_revoke_tokens_fail_after_downgrade_suspend_and_remove() {
    let Some(world) = boot_world_if_live().await else {
        return;
    };
    world.assert_base_topology();
    assert!(world.fixture.pre_revoke_tokens);

    let alpha = world.org("orgAlpha");
    let beta = world.org("orgBeta");
    let owner_token = &alpha.users["owner"].access_token;
    let app = world.app();

    let admin = &alpha.users["admin"];
    let (admin_access, admin_refresh) = (admin.access_token.clone(), admin.refresh_token.clone());
    let foreign = world.foreign_markers_for("orgAlpha");
    let (status, _, _) =
        json_request(app, "GET", "/api/v1/auth/me", Some(&admin_access), None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "admin access token must work before downgrade"
    );

    let (status, body, _) = json_request(
        app,
        "PATCH",
        &format!("/api/v1/members/{}", admin.user_id),
        Some(owner_token),
        Some(json!({ "role": "viewer" })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "owner must downgrade admin via production route: {}",
        String::from_utf8_lossy(&body)
    );

    let (status, me_body, me_headers) =
        json_request(app, "GET", "/api/v1/auth/me", Some(&admin_access), None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "role downgrade is authorization invalidation, not auth revocation"
    );
    assert_denial_no_leak(
        &denial_response(status, &me_body, &me_headers),
        &foreign,
        DenialExpectation::AllowSuccess,
    );
    let me: serde_json::Value = serde_json::from_slice(&me_body).expect("auth me after downgrade");
    assert_eq!(
        me["permissions"].as_array().map(Vec::len),
        Some(0),
        "downgraded session must have no stale permissions: {me}"
    );
    assert_eq!(
        me["allowedCollectionIds"].as_array().map(Vec::len),
        Some(0),
        "downgraded session must have no stale collection scope: {me}"
    );
    assert_admin_members_forbidden(app, &admin_access, &foreign).await;

    let (refresh_status, refreshed, _) = json_request(
        app,
        "POST",
        "/api/v1/auth/refresh",
        None,
        Some(json!({ "refreshToken": admin_refresh })),
    )
    .await;
    if refresh_status == StatusCode::OK {
        let refreshed_json: serde_json::Value =
            serde_json::from_slice(&refreshed).expect("refresh response json");
        let rotated_access = refreshed_json["accessToken"]
            .as_str()
            .expect("rotated access token")
            .to_string();
        let (status, rotated_me, rotated_headers) =
            json_request(app, "GET", "/api/v1/auth/me", Some(&rotated_access), None).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "rotated session must stay authenticated"
        );
        assert_denial_no_leak(
            &denial_response(status, &rotated_me, &rotated_headers),
            &foreign,
            DenialExpectation::AllowSuccess,
        );
        let rotated: serde_json::Value =
            serde_json::from_slice(&rotated_me).expect("rotated auth me");
        assert_eq!(
            rotated["permissions"].as_array().map(Vec::len),
            Some(0),
            "rotated token must not restore stale admin permissions: {rotated}"
        );
        assert_eq!(
            rotated["allowedCollectionIds"].as_array().map(Vec::len),
            Some(0),
            "rotated token must not restore stale collection scope: {rotated}"
        );
        assert_admin_members_forbidden(app, &rotated_access, &foreign).await;
    } else {
        assert_eq!(
            refresh_status,
            StatusCode::UNAUTHORIZED,
            "if refresh is revoked on downgrade, it must fail closed: {}",
            String::from_utf8_lossy(&refreshed)
        );
    }

    let member = &alpha.users["member"];
    let (member_access, member_refresh) =
        (member.access_token.clone(), member.refresh_token.clone());
    let (status, _, _) =
        json_request(app, "GET", "/api/v1/auth/me", Some(&member_access), None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "member access token must work before suspend"
    );

    let (status, body, _) = json_request(
        app,
        "PATCH",
        &format!("/api/v1/members/{}", member.user_id),
        Some(owner_token),
        Some(json!({ "state": "suspended" })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "owner must suspend member via production route: {}",
        String::from_utf8_lossy(&body)
    );
    assert_access_rejected(app, &member_access).await;
    assert_refresh_rejected(app, &member_refresh).await;

    let remove_target = &beta.users["member"];
    let beta_owner = &beta.users["owner"].access_token;
    let (remove_access, remove_refresh) = (
        remove_target.access_token.clone(),
        remove_target.refresh_token.clone(),
    );
    let (status, _, _) =
        json_request(app, "GET", "/api/v1/auth/me", Some(&remove_access), None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "remove-target access token must work before removal"
    );

    let (status, body, _) = json_request(
        app,
        "DELETE",
        &format!("/api/v1/members/{}", remove_target.user_id),
        Some(beta_owner),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "owner must remove member via production route: {}",
        String::from_utf8_lossy(&body)
    );
    assert_access_rejected(app, &remove_access).await;
    assert_refresh_rejected(app, &remove_refresh).await;

    world.cleanup().await.expect("cleanup");
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL + MARKHAND_TEST_QDRANT_URL + MinIO"]
async fn preview_download_job_and_sse_hide_foreign_ids() {
    let Some((world, runtime)) = boot_indexed_world_if_live().await else {
        return;
    };
    world.assert_base_topology();

    let alpha = world.org("orgAlpha");
    let beta = world.org("orgBeta");
    let token = &alpha.users["owner"].access_token;
    let foreign = world.foreign_markers_for("orgAlpha");
    let app = world.app();

    assert_path_idor_not_found(
        app,
        "GET",
        &format!(
            "/api/v1/documents/{}/preview?version_id={}",
            beta.document.document_id, beta.document.version_id
        ),
        token,
        None,
        &foreign,
    )
    .await;

    assert_path_idor_not_found(
        app,
        "POST",
        &format!(
            "/api/v1/documents/{}/versions/{}/download-capability",
            beta.document.document_id, beta.document.version_id
        ),
        token,
        Some(json!({ "purpose": "original" })),
        &foreign,
    )
    .await;

    assert_path_idor_not_found(
        app,
        "GET",
        &format!("/api/v1/jobs/{}", beta.job_id),
        token,
        None,
        &foreign,
    )
    .await;

    let job_events = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/v1/jobs/{}/events?lastEventId=0", beta.job_id))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let job_status = job_events.status();
    let job_headers: Vec<(String, String)> = job_events
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.to_string(),
                String::from_utf8_lossy(value.as_bytes()).into_owned(),
            )
        })
        .collect();
    let job_body = job_events
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    assert_denial_no_leak(
        &denial_response(job_status, &job_body, &job_headers),
        &foreign,
        DenialExpectation::PathIdorNotFound,
    );

    let foreign_token = &beta.users["owner"].access_token;
    let (status, issued, headers) = json_request(
        app,
        "POST",
        &format!(
            "/api/v1/documents/{}/versions/{}/download-capability",
            beta.document.document_id, beta.document.version_id
        ),
        Some(foreign_token),
        Some(json!({ "purpose": "original" })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "foreign tenant must mint capability via production route: {}",
        String::from_utf8_lossy(&issued)
    );
    let issued_json: serde_json::Value = serde_json::from_slice(&issued).expect("issued json");
    let capability = issued_json["capability"]
        .as_str()
        .expect("capability")
        .to_string();
    let (status, body, redeem_headers) = json_request(
        app,
        "GET",
        &format!("/api/v1/downloads/{capability}"),
        Some(token),
        None,
    )
    .await;
    assert_denial_no_leak(
        &denial_response(status, &body, &redeem_headers),
        &foreign,
        DenialExpectation::PathIdorNotFound,
    );
    assert!(
        !String::from_utf8_lossy(&body).contains(&beta.marker),
        "download denial leaked foreign marker"
    );
    let _ = headers;

    assert_body_scope_forbidden(
        app,
        "POST",
        "/api/v1/search",
        token,
        json!({
            "query": beta.marker,
            "collectionIds": [beta.collections["org"].collection_id],
            "limit": 5
        }),
        &foreign,
    )
    .await;

    runtime.teardown().await;
    world.cleanup().await.expect("cleanup");
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL + MARKHAND_TEST_QDRANT_URL + MinIO"]
async fn in_flight_ask_emits_no_content_after_acl_revoke() {
    use fileconv_server::auth::context::OrgContext;
    use fileconv_server::db::pool::with_org_txn;

    let Some((world, runtime)) = boot_indexed_world_if_live().await else {
        return;
    };
    world.assert_base_topology();

    let alpha = world.org("orgAlpha");
    let foreign = world.foreign_markers_for("orgAlpha");
    let token = &alpha.users["owner"].access_token;
    let owner_id = alpha.users["owner"].user_id;
    let collection_id = alpha.collections["org"].collection_id;
    let indexed_document_id = alpha
        .indexed_document
        .as_ref()
        .expect("alpha indexed document")
        .document_id;
    let app = world.app();

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/ask/stream")
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "question": alpha.marker.clone(),
                        "mode": "current",
                        "limit": 5,
                        "collectionIds": [collection_id]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .expect("stream start");
    assert_eq!(response.status(), StatusCode::OK);

    let pre_revoke = await_ask_stream_pre_revoke_evidence(
        world.pool(),
        alpha.org_id,
        owner_id,
        indexed_document_id,
    )
    .await
    .expect("open cited session with durable pre-revoke ask.token");
    assert!(
        !pre_revoke.token_sequences.is_empty(),
        "in-flight proof requires durable content before revoke"
    );
    let session_id = pre_revoke.session_id;

    let org_id = alpha.org_id;
    let owner_ctx = OrgContext::try_new(
        org_id,
        owner_id,
        ["qa.query", "member.manage"],
        [collection_id],
    )
    .expect("owner ctx");
    let last_event_id = with_org_txn(world.pool(), &owner_ctx, move |txn| {
        Box::pin(async move {
            let revoked =
                fileconv_server::services::acl_mutate::revoke_role_permission_for_principal(
                    txn, org_id, owner_id, "qa.query",
                )
                .await?;
            assert_eq!(
                revoked, 1,
                "production ACL mutation must revoke the token principal's qa.query permission"
            );
            let last_event_id: i64 = txn
                .query_one(
                    "SELECT COALESCE(MAX(sequence_no), 0)::bigint
                     FROM ask_stream_events
                     WHERE org_id = $1 AND session_id = $2",
                    &[&org_id, &session_id],
                )
                .await?
                .get(0);
            Ok(last_event_id)
        })
    })
    .await
    .expect("revoke qa.query role permission through production path");
    assert!(
        pre_revoke
            .token_sequences
            .iter()
            .all(|sequence| *sequence <= last_event_id),
        "pre-revoke tokens must be at or below committed HWM {last_event_id}: {:?}",
        pre_revoke.token_sequences
    );

    let (original_buf, _, _) = read_sse_until(
        response,
        |buf, _, _| buf.contains("stream.closed"),
        Duration::from_secs(12),
    )
    .await;
    assert!(
        original_buf.contains("stream.closed"),
        "original live response must drain to terminal after revoke: {original_buf}"
    );

    let resumed = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/ask/stream?streamSessionId={session_id}&lastEventId={last_event_id}"
                ))
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"question":"ignored","mode":"current"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let resumed_status = resumed.status();
    let resumed_buf = if resumed_status == StatusCode::OK {
        read_sse_until(
            resumed,
            |buf, _, _| {
                buf.contains("citation_revoked")
                    || buf.contains("principal_denied")
                    || buf.contains("stream.closed")
            },
            Duration::from_secs(12),
        )
        .await
        .0
    } else {
        assert_eq!(
            resumed_status,
            StatusCode::UNAUTHORIZED,
            "resumed stream must fail closed after ACL revoke"
        );
        String::from_utf8_lossy(
            &resumed
                .into_body()
                .collect()
                .await
                .expect("stream denial body")
                .to_bytes(),
        )
        .into_owned()
    };

    let terminal = await_ask_stream_terminal(world.pool(), org_id, owner_id, session_id)
        .await
        .expect("durable terminal ask stream evidence");
    assert!(
        matches!(terminal.status.as_str(), "closed" | "error"),
        "revoked ask stream must be durable-terminal: {terminal:?}"
    );
    assert!(
        matches!(
            terminal.close_reason.as_deref(),
            Some("principal_denied" | "citation_revoked")
        ),
        "durable session reason must prove authorization revoke: {terminal:?}"
    );
    assert!(
        terminal
            .terminal_event_reasons
            .iter()
            .any(|reason| matches!(reason.as_str(), "principal_denied" | "citation_revoked")),
        "durable terminal event must prove authorization revoke: {terminal:?}"
    );

    let post_revoke_tokens =
        ask_token_sequences_after(world.pool(), org_id, owner_id, session_id, last_event_id)
            .await
            .expect("post-revoke durable token query");
    assert!(
        post_revoke_tokens.is_empty(),
        "ACL revoke emitted durable ask.token sequences after HWM {last_event_id}: {post_revoke_tokens:?}"
    );

    for (label, body) in [
        ("original live response", &original_buf),
        ("same-session resume", &resumed_buf),
    ] {
        for needle in foreign.all_needles() {
            assert!(
                !body.to_lowercase().contains(&needle.to_lowercase()),
                "{label} leaked foreign marker {needle}: {body}"
            );
        }
    }

    runtime.teardown().await;
    world.cleanup().await.expect("cleanup");
}
