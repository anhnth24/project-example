//! Live PostgreSQL HTTP contract tests for `GET /api/v1/audit` (1C-11 read
//! endpoint). `audit_log` was write-only before this slice; these are the
//! close-condition tests for the read surface: happy list + stable cursor
//! pagination, `action`/`actor`/`from`/`to` filters, 403 (+audit) without
//! `audit.view`, org isolation, and no metadata beyond the existing per-action
//! allowlist.
//!
//! Skips cleanly when `MARKHAND_TEST_DATABASE_URL` / `MARKHAND_TEST_APP_DATABASE_URL`
//! are unset (see `common::boot_app_pool`). Must run in the `rust-integration`
//! CI job (not run in this session without a live Postgres).

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{DateTime, Utc};
use common::{
    admin_database_url, app_database_url, boot_app_pool, build_router, login_access_token,
    seed_user_with_permissions, test_auth_config, DualRoleEphemeralDb,
};
use deadpool_postgres::Pool;
use fileconv_server::auth::context::OrgContext;
use fileconv_server::db::pool::with_org_txn;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;

/// Parses a `occurredAt` field back into a real timestamp for ordering
/// assertions. Chrono's default serde format trims trailing-zero fractional
/// digits (`AutoSi`), so two RFC 3339 strings are NOT always lexicographically
/// comparable in the same order as their instants — comparisons below always
/// go through this, never raw string `<=`/`>=`.
fn occurred_at(item: &Value) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(item["occurredAt"].as_str().expect("occurredAt is a string"))
        .expect("occurredAt is RFC 3339")
        .with_timezone(&Utc)
}

const PASSWORD: &str = "correct-password-1";

async fn boot_pool() -> Option<(DualRoleEphemeralDb, Pool)> {
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

/// Seeds a member holding both `member.manage` (to generate audit rows via
/// invite create/revoke) and `audit.view` (to read them back).
async fn seed_auditor_admin(pool: &Pool, org: Uuid, user: Uuid, email: &str) -> String {
    seed_user_with_permissions(
        pool,
        org,
        user,
        email,
        PASSWORD,
        &["member.manage", "audit.view"],
    )
    .await;
    login_access_token(pool, email, PASSWORD).await
}

/// Seeds a member holding `member.manage` but deliberately WITHOUT
/// `audit.view` — the exact principal the 403+deny-audit test needs.
///
/// Must NOT reuse `seed_user_with_permissions` (it always seeds the `owner`
/// role — see its doc comment). `role_permissions` are per-*role*, not
/// per-user: if this hand-seeded user shared the org's `owner` role with a
/// caller already seeded via `seed_auditor_admin`, it would silently inherit
/// `audit.view` too. So this creates a genuinely separate `admin` role in
/// the same org, granted only `member.manage`, exactly mirroring
/// `seed_admin_role_member` in `tests/members.rs` (same underlying
/// privilege-separation reason).
async fn seed_manager_without_audit_view(
    pool: &Pool,
    org: Uuid,
    user: Uuid,
    email: &str,
) -> String {
    let ctx = OrgContext::try_new(org, user, ["member.manage"], []).unwrap();
    let email_owned = email.to_string();
    with_org_txn(pool, &ctx, {
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "INSERT INTO users (id, email, display_name)
                     VALUES ($1, $2, 'Audit Test Manager') ON CONFLICT (id) DO NOTHING",
                    &[&user, &email_owned],
                )
                .await?;
                txn.execute(
                    "INSERT INTO roles (id, org_id, code, name, is_system)
                     VALUES ($1, $2, 'admin', 'Admin', true)
                     ON CONFLICT (org_id, code) DO NOTHING",
                    &[&Uuid::new_v4(), &org],
                )
                .await?;
                let admin_role_id: Uuid = txn
                    .query_one(
                        "SELECT id FROM roles WHERE org_id = $1 AND code = 'admin'",
                        &[&org],
                    )
                    .await?
                    .get(0);
                let perm_id: Uuid = txn
                    .query_one(
                        "SELECT id FROM permissions WHERE code = 'member.manage'",
                        &[],
                    )
                    .await?
                    .get(0);
                txn.execute(
                    "INSERT INTO role_permissions (org_id, role_id, permission_id)
                     VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
                    &[&org, &admin_role_id, &perm_id],
                )
                .await?;
                txn.execute(
                    "INSERT INTO org_memberships (org_id, user_id, role)
                     VALUES ($1, $2, 'admin') ON CONFLICT (org_id, user_id) DO NOTHING",
                    &[&org, &user],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("seed admin-role member without audit.view");
    fileconv_server::auth::session::set_password_hash(
        pool,
        user,
        PASSWORD,
        &test_auth_config().argon2,
    )
    .await
    .expect("set password");
    login_access_token(pool, email, PASSWORD).await
}

async fn create_invite(app: &axum::Router, token: &str, email: &str) -> Value {
    let (status, body) = send(
        app,
        "POST",
        "/api/v1/members/invites",
        Some(token),
        Some(json!({ "email": email, "role": "viewer" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create invite: {body}");
    body
}

fn items_of(body: &Value) -> Vec<Value> {
    body["items"].as_array().cloned().unwrap_or_default()
}

// ---------------------------------------------------------------------
// Permission guard + deny audit
// ---------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn list_audit_requires_audit_view_and_audits_the_denial() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);

    let org = Uuid::new_v4();
    let owner = Uuid::new_v4();
    let owner_token = seed_auditor_admin(&pool, org, owner, "auditor@audit-read-it.test").await;

    let unprivileged = Uuid::new_v4();
    let unprivileged_token =
        seed_manager_without_audit_view(&pool, org, unprivileged, "manager@audit-read-it.test")
            .await;

    let (status, body) = send(
        &app,
        "GET",
        "/api/v1/audit",
        Some(&unprivileged_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "denied read: {body}");
    assert_eq!(body["code"], "forbidden");

    // The denial itself must be durably audited (unlike plain
    // list_members/list_invites/usage, reading the audit trail is sensitive
    // enough to audit even the failed attempt) — the owner, who does hold
    // audit.view, must be able to see that row.
    let (status, body) = send(&app, "GET", "/api/v1/audit", Some(&owner_token), None).await;
    assert_eq!(status, StatusCode::OK, "owner read: {body}");
    let items = items_of(&body);
    let deny_row = items.iter().find(|entry| {
        entry["action"] == "audit.read"
            && entry["outcome"] == "deny"
            && entry["actorId"] == unprivileged.to_string()
    });
    assert!(
        deny_row.is_some(),
        "expected an audit.read/deny row for the unprivileged caller: {items:?}"
    );
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn list_audit_requires_a_bearer_token() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);
    let (status, _) = send(&app, "GET", "/api/v1/audit", None, None).await;
    assert_ne!(status, StatusCode::OK);
}

// ---------------------------------------------------------------------
// Happy path + stable cursor pagination
// ---------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn list_audit_paginates_stably_newest_first_with_no_gaps_or_dupes() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);

    let org = Uuid::new_v4();
    let owner = Uuid::new_v4();
    let owner_token = seed_auditor_admin(&pool, org, owner, "owner@audit-read-it.test").await;

    for i in 0..5 {
        create_invite(
            &app,
            &owner_token,
            &format!("invitee-{i}@audit-read-it.test"),
        )
        .await;
    }

    // Walk every page at limit=2, collecting occurredAt in visit order.
    let mut seen_ids = std::collections::HashSet::new();
    let mut ordered_ats: Vec<DateTime<Utc>> = Vec::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    loop {
        pages += 1;
        assert!(pages < 100, "pagination loop did not terminate");
        let uri = match &cursor {
            Some(c) => format!("/api/v1/audit?limit=2&cursor={c}"),
            None => "/api/v1/audit?limit=2".to_string(),
        };
        let (status, body) = send(&app, "GET", &uri, Some(&owner_token), None).await;
        assert_eq!(status, StatusCode::OK, "list page: {body}");
        let items = items_of(&body);
        assert!(items.len() <= 2, "page exceeded requested limit: {body}");
        for item in &items {
            let id = item["id"].as_str().unwrap().to_string();
            assert!(
                seen_ids.insert(id.clone()),
                "duplicate id across pages: {id}"
            );
            ordered_ats.push(occurred_at(item));
        }
        let has_more = body["page"]["hasMore"].as_bool().unwrap();
        if !has_more {
            // `hasMore` alone is the authoritative "stop paginating" signal
            // (matches `db::documents::list_in_collection`'s route
            // convention: `nextCursor` is still populated from the last row
            // even on a terminal page, it just must not be followed).
            break;
        }
        cursor = Some(body["page"]["nextCursor"].as_str().unwrap().to_string());
    }

    // Newest-first: occurredAt must be non-increasing across the whole walk.
    for window in ordered_ats.windows(2) {
        assert!(
            window[0] >= window[1],
            "page ordering is not newest-first: {ordered_ats:?}"
        );
    }
    // At least the 5 member.invite rows must show up somewhere in the walk.
    assert!(
        seen_ids.len() >= 5,
        "expected at least 5 audit rows from the 5 invites, saw {}",
        seen_ids.len()
    );
}

// ---------------------------------------------------------------------
// Filters: action / actor / from / to
// ---------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn list_audit_filters_by_action_and_actor() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);

    let org = Uuid::new_v4();
    let actor_a = Uuid::new_v4();
    let token_a = seed_auditor_admin(&pool, org, actor_a, "actor-a@audit-read-it.test").await;
    let actor_b = Uuid::new_v4();
    let token_b = seed_auditor_admin(&pool, org, actor_b, "actor-b@audit-read-it.test").await;

    create_invite(&app, &token_a, "from-a@audit-read-it.test").await;
    create_invite(&app, &token_b, "from-b@audit-read-it.test").await;

    // Filter by actor=A: every row must be attributed to A (includes A's own
    // member.invite and any audit.read rows A's own queries generate), never B.
    let (status, body) = send(
        &app,
        "GET",
        &format!("/api/v1/audit?actor={actor_a}"),
        Some(&token_a),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "filter by actor: {body}");
    let items = items_of(&body);
    assert!(
        !items.is_empty(),
        "expected at least actor A's own invite row"
    );
    for item in &items {
        assert_eq!(
            item["actorId"],
            actor_a.to_string(),
            "leaked non-A actor: {item}"
        );
    }

    // Filter by action=member.invite: only that action, from either actor.
    let (status, body) = send(
        &app,
        "GET",
        "/api/v1/audit?action=member.invite",
        Some(&token_a),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "filter by action: {body}");
    let items = items_of(&body);
    assert!(items.len() >= 2, "expected both invite rows: {items:?}");
    for item in &items {
        assert_eq!(item["action"], "member.invite");
    }
    let actor_ids: std::collections::HashSet<String> = items
        .iter()
        .map(|item| item["actorId"].as_str().unwrap().to_string())
        .collect();
    assert!(actor_ids.contains(&actor_a.to_string()));
    assert!(actor_ids.contains(&actor_b.to_string()));
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn list_audit_filters_by_time_range() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);

    let org = Uuid::new_v4();
    let owner = Uuid::new_v4();
    let owner_token = seed_auditor_admin(&pool, org, owner, "owner@audit-read-it.test").await;

    create_invite(&app, &owner_token, "first@audit-read-it.test").await;
    create_invite(&app, &owner_token, "second@audit-read-it.test").await;
    create_invite(&app, &owner_token, "third@audit-read-it.test").await;

    let (status, body) = send(
        &app,
        "GET",
        "/api/v1/audit?action=member.invite",
        Some(&owner_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "unfiltered baseline: {body}");
    let all = items_of(&body);
    assert_eq!(
        all.len(),
        3,
        "expected exactly 3 member.invite rows: {all:?}"
    );
    // `all` is newest-first; pick the middle row's timestamp as an inclusive
    // upper bound and expect only itself + the older one to survive.
    let to_bound_at = occurred_at(&all[1]);
    let to_bound = all[1]["occurredAt"].as_str().unwrap().to_string();

    let (status, body) = send(
        &app,
        "GET",
        &format!("/api/v1/audit?action=member.invite&to={to_bound}"),
        Some(&owner_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "to-bounded: {body}");
    let bounded = items_of(&body);
    assert_eq!(
        bounded.len(),
        2,
        "expected the newest row excluded by `to`: {bounded:?}"
    );
    assert!(bounded.iter().all(|item| occurred_at(item) <= to_bound_at));

    let (status, body) = send(
        &app,
        "GET",
        &format!("/api/v1/audit?action=member.invite&from={to_bound}"),
        Some(&owner_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "from-bounded: {body}");
    let bounded_from = items_of(&body);
    assert_eq!(
        bounded_from.len(),
        2,
        "expected the oldest row excluded by `from`: {bounded_from:?}"
    );
}

// ---------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn list_audit_rejects_invalid_action_cursor_and_inverted_time_range() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);

    let org = Uuid::new_v4();
    let owner = Uuid::new_v4();
    let owner_token = seed_auditor_admin(&pool, org, owner, "owner@audit-read-it.test").await;

    let (status, body) = send(
        &app,
        "GET",
        "/api/v1/audit?action=not_a_real_action",
        Some(&owner_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "invalid action: {body}");
    assert_eq!(body["code"], "validation_failed");

    let (status, body) = send(
        &app,
        "GET",
        "/api/v1/audit?cursor=%%%not-base64%%%",
        Some(&owner_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "invalid cursor: {body}");
    assert_eq!(body["code"], "validation_failed");

    let (status, body) = send(
        &app,
        "GET",
        "/api/v1/audit?from=2030-01-01T00:00:00Z&to=2020-01-01T00:00:00Z",
        Some(&owner_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "inverted range: {body}");
    assert_eq!(body["code"], "validation_failed");
}

// ---------------------------------------------------------------------
// Org isolation + no metadata beyond the existing allowlist
// ---------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn list_audit_never_leaks_across_orgs() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);

    let org_a = Uuid::new_v4();
    let owner_a = Uuid::new_v4();
    let token_a = seed_auditor_admin(&pool, org_a, owner_a, "owner-a@audit-read-it.test").await;

    let org_b = Uuid::new_v4();
    let owner_b = Uuid::new_v4();
    let token_b = seed_auditor_admin(&pool, org_b, owner_b, "owner-b@audit-read-it.test").await;

    create_invite(&app, &token_a, "secret-org-a@audit-read-it.test").await;

    let (status, body) = send(&app, "GET", "/api/v1/audit", Some(&token_b), None).await;
    assert_eq!(status, StatusCode::OK, "org b read: {body}");
    let items = items_of(&body);
    for item in &items {
        assert_ne!(
            item["actorId"],
            owner_a.to_string(),
            "org B audit read leaked org A's actor: {item}"
        );
    }
    assert!(
        !body.to_string().contains("secret-org-a@audit-read-it.test"),
        "org B response body must never contain org A's invite email: {body}"
    );
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn list_audit_entries_never_expose_metadata_beyond_the_allowlist() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);

    let org = Uuid::new_v4();
    let owner = Uuid::new_v4();
    let owner_token = seed_auditor_admin(&pool, org, owner, "owner@audit-read-it.test").await;

    create_invite(&app, &owner_token, "allowlist-check@audit-read-it.test").await;

    let (status, body) = send(
        &app,
        "GET",
        "/api/v1/audit?action=member.invite",
        Some(&owner_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let items = items_of(&body);
    assert!(!items.is_empty());
    for item in &items {
        let metadata = item["metadata"]
            .as_object()
            .expect("metadata must be an object");
        // member.invite's allowlist is exactly {reason, invite_id, role} —
        // see services::audit::AuditAction::metadata_keys. Never the
        // recipient email, even though the route handler received it.
        for key in metadata.keys() {
            assert!(
                ["reason", "invite_id", "role"].contains(&key.as_str()),
                "unexpected metadata key {key} on member.invite row: {item}"
            );
        }
        assert!(
            !item
                .to_string()
                .contains("allowlist-check@audit-read-it.test"),
            "invite email leaked into an audit entry: {item}"
        );
    }
}
