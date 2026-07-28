//! Live PostgreSQL HTTP contract tests for org lifecycle (1C-01, full slice):
//! POST /orgs (create), GET /orgs (list), GET /orgs/{orgId} (detail),
//! POST /orgs/switch.
//!
//! Skips cleanly when `MARKHAND_TEST_DATABASE_URL` / `MARKHAND_TEST_APP_DATABASE_URL`
//! are unset (see `common::boot_app_pool`). Must run in the `rust-integration`
//! CI job — not run in this session (no Postgres available here).

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{admin_database_url, app_database_url, boot_app_pool, build_router};
use deadpool_postgres::Pool;
use fileconv_server::auth::context::OrgContext;
use fileconv_server::auth::permissions::resolve_org_context_in_txn;
use fileconv_server::db::pool::with_org_txn;
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

/// A caller needs *some* existing membership to obtain a bearer token at all
/// (`login_password` has no path for a brand-new user with zero
/// memberships — same gap `members.rs::accept_invite`'s module doc already
/// records). Seeds a throwaway origin org purely so the caller can log in;
/// the org created via `POST /orgs` in the create tests below is a separate,
/// brand-new org the caller has no prior relationship to.
async fn seed_caller(pool: &Pool, email: &str) -> (Uuid, String) {
    let origin_org = Uuid::new_v4();
    let user = Uuid::new_v4();
    common::seed_user_with_permissions(pool, origin_org, user, email, PASSWORD, &[]).await;
    let (token, _) = common::login_tokens(pool, email, PASSWORD).await;
    (user, token)
}

/// Seeds a bare membership (owner role, no extra permissions) for `user` in
/// `org` — orgs/list/detail/switch need no permission beyond membership
/// itself. Reuses `common::seed_user_with_permissions`'s org/user/membership
/// bootstrap (idempotent per user id: calling it twice for the same
/// `user_id` with two different `org` values is exactly how these tests
/// build a two-org fixture — the second call's `users` insert is a no-op on
/// the primary key, but the `org_memberships` insert lands under the new
/// `(org, user)` key).
async fn seed_member(pool: &Pool, org: Uuid, user: Uuid, email: &str) {
    common::seed_user_with_permissions(pool, org, user, email, PASSWORD, &[]).await;
}

async fn login_tokens(pool: &Pool, email: &str) -> (String, String) {
    common::login_tokens(pool, email, PASSWORD).await
}

async fn suspend_membership(pool: &Pool, org: Uuid, user: Uuid) {
    let ctx = OrgContext::try_new(org, user, [] as [&str; 0], []).unwrap();
    with_org_txn(pool, &ctx, move |txn| {
        Box::pin(async move {
            txn.execute(
                "UPDATE org_memberships SET state = 'suspended' WHERE org_id = $1 AND user_id = $2",
                &[&org, &user],
            )
            .await?;
            Ok(())
        })
    })
    .await
    .expect("suspend membership");
}

/// `action` selects which audit action to filter on (`org.create` for the
/// create tests below, `org.switch` for the switch tests) — the two branches
/// this file was reconciled from each hardcoded their own action; a caller
/// arg keeps both call sites intact instead of duplicating this helper.
async fn audit_rows(pool: &Pool, org: Uuid, action: &str) -> Vec<(String, String, String)> {
    let ctx = OrgContext::try_new(org, org, [] as [&str; 0], []).unwrap();
    let action = action.to_string();
    with_org_txn(pool, &ctx, move |txn| {
        Box::pin(async move {
            let rows = txn
                .query(
                    "SELECT action, outcome, metadata::text
                     FROM audit_log WHERE org_id = $1 AND action = $2 ORDER BY seq",
                    &[&org, &action],
                )
                .await?;
            Ok(rows
                .into_iter()
                .map(|row| (row.get(0), row.get(1), row.get(2)))
                .collect::<Vec<_>>())
        })
    })
    .await
    .unwrap()
}

// ---------------------------------------------------------------------
// POST /orgs — create a new org; caller becomes its owner.
// ---------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn create_org_succeeds_and_caller_becomes_owner() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);

    let (user_id, token) = seed_caller(&pool, "create-happy@orgs-it.test").await;
    let slug = format!("acme-{}", Uuid::new_v4().simple());

    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/orgs",
        Some(&token),
        Some(json!({ "slug": slug, "name": "Acme Inc" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["slug"], slug);
    assert_eq!(body["name"], "Acme Inc");
    assert_eq!(body["role"], "owner");
    let org_id: Uuid = body["id"].as_str().unwrap().parse().unwrap();

    // Acceptance contract: after create, the caller resolves full owner
    // permissions in the brand-new org — including `member.manage` — with
    // NOTHING beyond `POST /orgs` having touched `roles`/`role_permissions`
    // for this org. Asserted at the same resolver every route's
    // `AuthenticatedOrg` extractor calls in production
    // (`auth::permissions::resolve_org_context_in_txn`), same as the
    // `/orgs/switch` flow below does after minting its token.
    let ctx = resolve_org_context_in_txn(&pool, org_id, user_id)
        .await
        .expect("owner must resolve in the new org with zero manual seeding");
    assert!(
        ctx.has_permission("member.manage"),
        "owner must hold member.manage: {:?}",
        ctx.permissions()
    );
    assert!(ctx.has_permission("doc.upload"));
    assert!(ctx.has_permission("audit.view"));

    let rows = audit_rows(&pool, org_id, "org.create").await;
    assert!(
        rows.iter()
            .any(|(action, outcome, _)| action == "org.create" && outcome == "success"),
        "create must be audited: {rows:?}"
    );

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn create_org_rejects_a_duplicate_slug() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);

    let (_user_id, token) = seed_caller(&pool, "create-dup@orgs-it.test").await;
    let slug = format!("dup-{}", Uuid::new_v4().simple());

    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/orgs",
        Some(&token),
        Some(json!({ "slug": slug, "name": "First" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let (status_dup, body_dup) = send(
        &app,
        "POST",
        "/api/v1/orgs",
        Some(&token),
        Some(json!({ "slug": slug, "name": "Second" })),
    )
    .await;
    assert_eq!(status_dup, StatusCode::CONFLICT, "{body_dup}");
    assert_eq!(body_dup["code"], "slug_taken");

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn create_org_requires_a_bearer_token() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);

    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/orgs",
        None,
        Some(json!({ "slug": "no-auth-org", "name": "No Auth" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn create_org_rejects_an_invalid_slug() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);

    let (_user_id, token) = seed_caller(&pool, "create-invalid@orgs-it.test").await;

    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/orgs",
        Some(&token),
        Some(json!({ "slug": "Not_Valid!", "name": "Whatever" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "validation_failed");

    ephemeral.drop().await;
}

// ---------------------------------------------------------------------
// GET /orgs — only orgs the caller is an active member of.
// ---------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn list_orgs_shows_only_the_callers_own_orgs() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);

    let org_a = Uuid::new_v4();
    let user_a = Uuid::new_v4();
    seed_member(&pool, org_a, user_a, "user-a@orgs-it.test").await;
    let (token_a, _) = login_tokens(&pool, "user-a@orgs-it.test").await;

    // A second, unrelated org/user — must never appear for user_a.
    let org_b = Uuid::new_v4();
    let user_b = Uuid::new_v4();
    seed_member(&pool, org_b, user_b, "user-b@orgs-it.test").await;

    let (status, body) = send(&app, "GET", "/api/v1/orgs", Some(&token_a), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let ids: Vec<String> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_string())
        .collect();
    assert!(ids.contains(&org_a.to_string()));
    assert!(
        !ids.contains(&org_b.to_string()),
        "user_a must never see org_b: {body}"
    );

    // No bearer token at all → 401, not a 500/empty-list.
    let (status, _) = send(&app, "GET", "/api/v1/orgs", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn list_orgs_shows_both_orgs_for_a_genuine_two_org_member() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);

    let org_a = Uuid::new_v4();
    let org_b = Uuid::new_v4();
    let user = Uuid::new_v4();
    seed_member(&pool, org_a, user, "two-org@orgs-it.test").await;
    seed_member(&pool, org_b, user, "two-org@orgs-it.test").await;
    let (token, _) = login_tokens(&pool, "two-org@orgs-it.test").await;

    let (status, body) = send(&app, "GET", "/api/v1/orgs", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let ids: Vec<String> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_string())
        .collect();
    assert!(ids.contains(&org_a.to_string()));
    assert!(ids.contains(&org_b.to_string()));

    ephemeral.drop().await;
}

// ---------------------------------------------------------------------
// GET /orgs/{orgId} — no existence oracle for non-members.
// ---------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn get_org_detail_is_identical_for_nonexistent_and_not_a_member() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);

    let org_a = Uuid::new_v4();
    let user_a = Uuid::new_v4();
    seed_member(&pool, org_a, user_a, "detail-a@orgs-it.test").await;
    let (token_a, _) = login_tokens(&pool, "detail-a@orgs-it.test").await;

    // Own org: 200.
    let (status, body) = send(
        &app,
        "GET",
        &format!("/api/v1/orgs/{org_a}"),
        Some(&token_a),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["id"], org_a.to_string());

    // A real org the caller does not belong to.
    let org_b = Uuid::new_v4();
    let user_b = Uuid::new_v4();
    seed_member(&pool, org_b, user_b, "detail-b@orgs-it.test").await;
    let (status_real_other, body_real_other) = send(
        &app,
        "GET",
        &format!("/api/v1/orgs/{org_b}"),
        Some(&token_a),
        None,
    )
    .await;

    // An org id that was never created at all.
    let ghost = Uuid::new_v4();
    let (status_ghost, body_ghost) = send(
        &app,
        "GET",
        &format!("/api/v1/orgs/{ghost}"),
        Some(&token_a),
        None,
    )
    .await;

    assert_eq!(
        status_real_other,
        StatusCode::NOT_FOUND,
        "{body_real_other}"
    );
    assert_eq!(status_ghost, StatusCode::NOT_FOUND, "{body_ghost}");
    assert_eq!(
        status_real_other, status_ghost,
        "a real org the caller isn't a member of must be indistinguishable from a nonexistent one"
    );
    assert_eq!(body_real_other["code"], body_ghost["code"]);

    ephemeral.drop().await;
}

// ---------------------------------------------------------------------
// POST /orgs/switch — re-verify from PostgreSQL, mint new session, audit.
// ---------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn switch_denies_and_audits_a_real_org_the_caller_is_not_a_member_of() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);

    let org_a = Uuid::new_v4();
    let user_a = Uuid::new_v4();
    seed_member(&pool, org_a, user_a, "switch-a@orgs-it.test").await;
    let (token_a, _) = login_tokens(&pool, "switch-a@orgs-it.test").await;

    let org_b = Uuid::new_v4();
    let user_b = Uuid::new_v4();
    seed_member(&pool, org_b, user_b, "switch-b@orgs-it.test").await;

    let before = audit_rows(&pool, org_b, "org.switch").await;
    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/orgs/switch",
        Some(&token_a),
        Some(json!({ "orgId": org_b })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "membership_missing");

    let after = audit_rows(&pool, org_b, "org.switch").await;
    assert_eq!(
        after.len(),
        before.len() + 1,
        "deny must be audited: {after:?}"
    );
    let (action, outcome, metadata) = after.last().unwrap();
    assert_eq!(action, "org.switch");
    assert_eq!(outcome, "deny");
    assert!(metadata.contains("membership_missing"));

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn switch_denies_a_forged_nonexistent_org_without_writing_any_audit_row() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);

    let org_a = Uuid::new_v4();
    let user_a = Uuid::new_v4();
    seed_member(&pool, org_a, user_a, "switch-forge@orgs-it.test").await;
    let (token_a, _) = login_tokens(&pool, "switch-forge@orgs-it.test").await;

    let ghost = Uuid::new_v4();
    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/orgs/switch",
        Some(&token_a),
        Some(json!({ "orgId": ghost })),
    )
    .await;
    // Same status/code as denying a real org the caller isn't a member of —
    // no existence oracle via the switch endpoint either.
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "membership_missing");

    // Writing into a nonexistent org_id would violate the audit_log FK; the
    // handler must skip the audit write entirely for a forged target rather
    // than surface a 500 (which would itself be an oracle: "org doesn't
    // exist" vs "exists but you're not a member" would then differ by
    // status code). Nothing to assert on `ghost` (no audit_log FK target
    // exists at all) beyond the 403 above — the request completing cleanly
    // is the proof no FK violation occurred.

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn switch_denies_a_suspended_membership_and_audits() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);

    let org_a = Uuid::new_v4();
    let org_b = Uuid::new_v4();
    let user = Uuid::new_v4();
    seed_member(&pool, org_a, user, "switch-suspend@orgs-it.test").await;
    seed_member(&pool, org_b, user, "switch-suspend@orgs-it.test").await;
    let (token, _) = login_tokens(&pool, "switch-suspend@orgs-it.test").await;

    suspend_membership(&pool, org_b, user).await;

    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/orgs/switch",
        Some(&token),
        Some(json!({ "orgId": org_b })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "membership_missing");

    let rows = audit_rows(&pool, org_b, "org.switch").await;
    assert!(rows
        .iter()
        .any(|(action, outcome, metadata)| action == "org.switch"
            && outcome == "deny"
            && metadata.contains("membership_missing")));

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn switch_succeeds_for_a_two_org_member_and_mints_an_independent_session() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);

    let org_a = Uuid::new_v4();
    let org_b = Uuid::new_v4();
    let user = Uuid::new_v4();
    seed_member(&pool, org_a, user, "switch-two@orgs-it.test").await;
    seed_member(&pool, org_b, user, "switch-two@orgs-it.test").await;
    let (token_a, refresh_a) = login_tokens(&pool, "switch-two@orgs-it.test").await;

    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/orgs/switch",
        Some(&token_a),
        Some(json!({ "orgId": org_b })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["orgId"], org_b.to_string());
    let token_b = body["accessToken"].as_str().unwrap().to_string();
    let refresh_b = body["refreshToken"].as_str().unwrap().to_string();
    assert_ne!(
        refresh_a, refresh_b,
        "switch must mint an independent family"
    );

    // New token actually resolves org B via /auth/me (current-state re-check).
    let (status, me) = send(&app, "GET", "/api/v1/auth/me", Some(&token_b), None).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["orgId"], org_b.to_string());

    // The source org's own session is untouched — org A's refresh still works.
    let (status, refreshed) = send(
        &app,
        "POST",
        "/api/v1/auth/refresh",
        None,
        Some(json!({ "refreshToken": refresh_a })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "switching to org B must not revoke org A's session: {refreshed}"
    );

    // Success is audited too.
    let rows = audit_rows(&pool, org_b, "org.switch").await;
    assert!(rows
        .iter()
        .any(|(action, outcome, _)| action == "org.switch" && outcome == "success"));

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn switch_requires_a_bearer_token_and_never_trusts_the_body_alone() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);

    let org_a = Uuid::new_v4();
    let user_a = Uuid::new_v4();
    seed_member(&pool, org_a, user_a, "switch-noauth@orgs-it.test").await;

    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/orgs/switch",
        None,
        Some(json!({ "orgId": org_a })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");

    ephemeral.drop().await;
}
