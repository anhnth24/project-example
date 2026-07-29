//! Live PostgreSQL HTTP contract tests for the membership + invite + usage
//! admin surface (P2-11 / P2-12, Wave 2).
//!
//! Skips cleanly when `MARKHAND_TEST_DATABASE_URL` / `MARKHAND_TEST_APP_DATABASE_URL`
//! are unset (see `common::boot_app_pool`). These are the close-condition
//! tests required by
//! `plans/reports/plan-260728-0231-markhand-web-membership-admin-slice.md`
//! section 7 (C1 cross-org denial, concurrent last-owner race, invite
//! replay/expiry, refresh rejected after remove/suspend/downgrade). They were
//! not run in this session (no Postgres available) — they must run in the
//! `rust-integration` CI job.

mod common;

use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{
    admin_database_url, app_database_url, boot_app_pool, build_router, login_access_token,
    login_tokens, seed_user_with_permissions, DualRoleEphemeralDb,
};
use deadpool_postgres::Pool;
use fileconv_server::auth::context::OrgContext;
use fileconv_server::db::audit as db_audit;
use fileconv_server::db::models::ResourceKind;
use fileconv_server::db::pool::with_org_txn;
use fileconv_server::services::quota;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;

const PASSWORD: &str = "correct-password-1";

async fn boot_pool() -> Option<(DualRoleEphemeralDb, Pool)> {
    let admin = admin_database_url()?;
    let app = app_database_url()?;
    Some(boot_app_pool(&admin, &app).await)
}

/// Seeds an owner-role member with the given extra permissions and logs in.
/// `seed_user_with_permissions` always seeds the `owner` role (P2-11's
/// membership model grants permissions per role, not per user), so calling
/// this twice for the same `org` with different users yields a two-owner org
/// — exactly the fixture the concurrent last-owner race and refresh-revoke
/// tests need.
async fn seed_admin(pool: &Pool, org: Uuid, user: Uuid, email: &str) -> String {
    seed_user_with_permissions(pool, org, user, email, PASSWORD, &["member.manage"]).await;
    login_access_token(pool, email, PASSWORD).await
}

/// Seeds a member with no special permissions (used purely as "some other
/// org's authenticated user" for accept-invite auth-only tests).
async fn seed_plain_member(pool: &Pool, org: Uuid, user: Uuid, email: &str) -> String {
    seed_user_with_permissions(pool, org, user, email, PASSWORD, &[]).await;
    login_access_token(pool, email, PASSWORD).await
}

/// Seeds a genuinely non-owner member that holds `member.manage` — i.e. an
/// `admin`-role member. The shared `seed_user_with_permissions` only ever
/// seeds the `owner` role, so the admin role + its `member.manage` grant +
/// the admin-role membership row are inserted directly here. This is the
/// exact principal the privilege-escalation regression below needs: someone
/// who can call the member endpoints but must NOT be able to reach the owner
/// tier.
async fn seed_admin_role_member(pool: &Pool, org: Uuid, user: Uuid, email: &str) -> String {
    let ctx = OrgContext::try_new(org, user, ["member.manage"], []).unwrap();
    let email_owned = email.to_string();
    with_org_txn(pool, &ctx, {
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "INSERT INTO users (id, email, display_name)
                     VALUES ($1, $2, 'Admin Member') ON CONFLICT (id) DO NOTHING",
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
    .unwrap();
    // The shared seeder sets a password as a step after its own txn; this
    // hand-rolled admin seed must do the same or `login_access_token` below
    // panics on a credential-less user (the actual rust-integration failure).
    fileconv_server::auth::session::set_password_hash(
        pool,
        user,
        PASSWORD,
        &common::test_auth_config().argon2,
    )
    .await
    .expect("set admin password");
    login_access_token(pool, email, PASSWORD).await
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

async fn create_invite(app: &axum::Router, token: &str, email: &str, role: &str) -> (Uuid, String) {
    let (status, body) = send(
        app,
        "POST",
        "/api/v1/members/invites",
        Some(token),
        Some(json!({ "email": email, "role": role })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create invite: {body}");
    let invite_id = Uuid::parse_str(body["invite"]["id"].as_str().unwrap()).unwrap();
    let plaintext = body["token"].as_str().unwrap().to_string();
    (invite_id, plaintext)
}

// ---------------------------------------------------------------------
// Cross-org denial (plan section 2 caveat C1 — the close condition)
// ---------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn cross_org_denial_covers_every_member_endpoint() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);

    let org_a = Uuid::new_v4();
    let user_a = Uuid::new_v4();
    let token_a = seed_admin(&pool, org_a, user_a, "admin-a@members-it.test").await;

    let org_b = Uuid::new_v4();
    let user_b = Uuid::new_v4();
    let token_b = seed_admin(&pool, org_b, user_b, "admin-b@members-it.test").await;

    // 1) GET /members under org A must never surface org B's user.
    let (status, body) = send(&app, "GET", "/api/v1/members", Some(&token_a), None).await;
    assert_eq!(status, StatusCode::OK);
    let ids: Vec<String> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["userId"].as_str().unwrap().to_string())
        .collect();
    assert!(ids.contains(&user_a.to_string()));
    assert!(
        !ids.contains(&user_b.to_string()),
        "org A member list leaked org B user: {body}"
    );
    // Owner-reported UI gap (raw UUID in the admin members table): `list`
    // must join `users` so the response carries a name/email, not just the
    // id — see `db::members::MEMBERSHIP_COLUMNS`'s doc.
    let user_a_item = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["userId"] == user_a.to_string())
        .expect("org A caller's own row");
    assert_eq!(user_a_item["email"], "admin-a@members-it.test");
    assert_eq!(user_a_item["displayName"], "Integration User");

    // 2) PATCH org B's member using org A's token -> 404, not 403/500.
    let (status, body) = send(
        &app,
        "PATCH",
        &format!("/api/v1/members/{user_b}"),
        Some(&token_a),
        Some(json!({ "role": "viewer" })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "patch cross-org: {body}");
    assert_eq!(body["code"], "not_found");
    assert!(!body.to_string().contains(&org_b.to_string()));

    // 3) DELETE org B's member using org A's token -> 404.
    let (status, body) = send(
        &app,
        "DELETE",
        &format!("/api/v1/members/{user_b}"),
        Some(&token_a),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "delete cross-org: {body}");
    assert_eq!(body["code"], "not_found");

    // 4) Revoke org B's invite using org A's token -> 404.
    let (invite_b, _plaintext_b) =
        create_invite(&app, &token_b, "invitee-b@members-it.test", "viewer").await;
    let (status, body) = send(
        &app,
        "POST",
        &format!("/api/v1/members/invites/{invite_b}/revoke"),
        Some(&token_a),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "revoke cross-org: {body}");
    assert_eq!(body["code"], "not_found");

    // 5) Usage under org A must not reflect org B's committed quota.
    let ctx_b = OrgContext::try_new(org_b, user_b, [] as [&str; 0], []).unwrap();
    quota::reserve(
        &pool,
        &ctx_b,
        "cross-org-leak-check",
        ResourceKind::Documents,
        3,
        Duration::from_secs(60),
        None,
    )
    .await
    .expect("reserve org B quota");
    quota::finalize(&pool, &ctx_b, "cross-org-leak-check")
        .await
        .expect("finalize org B quota");

    let (status, body) = send(&app, "GET", "/api/v1/usage", Some(&token_a), None).await;
    assert_eq!(status, StatusCode::OK);
    let documents_committed = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["resource"] == "documents")
        .expect("documents usage entry")["committed"]
        .as_i64()
        .unwrap();
    assert_eq!(
        documents_committed, 0,
        "org A usage leaked org B's committed documents: {body}"
    );

    // 6) accept-invite: an org-A invite accepted by an org-B-only user must
    // land membership in org A (the token's embedded org), not org B, and
    // must not require org-A membership to call.
    let (invite_a, token_a_plain) =
        create_invite(&app, &token_a, "invitee-accept@members-it.test", "viewer").await;
    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/members/invites/accept",
        Some(&token_b), // org-B-only bearer; NOT a member of org A
        Some(json!({ "token": token_a_plain })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "accept by foreign user: {body}"
    );
    assert_eq!(body["userId"], user_b.to_string());
    let _ = invite_a;

    with_org_txn(
        &pool,
        &OrgContext::try_new(org_a, user_a, [] as [&str; 0], []).unwrap(),
        {
            move |txn| {
                Box::pin(async move {
                    let row = txn
                        .query_opt(
                            "SELECT 1 FROM org_memberships WHERE org_id = $1 AND user_id = $2",
                            &[&org_a, &user_b],
                        )
                        .await?;
                    assert!(row.is_some(), "accept must create membership in org A");
                    Ok(())
                })
            }
        },
    )
    .await
    .unwrap();
    with_org_txn(&pool, &ctx_b, move |txn| {
        Box::pin(async move {
            let count: i64 = txn
                .query_one(
                    "SELECT count(*)::bigint FROM org_memberships WHERE org_id = $1",
                    &[&org_b],
                )
                .await?
                .get(0);
            // Only user_b itself (seeded owner) — accept must not have
            // inserted a second row into org B.
            assert_eq!(count, 1, "accept leaked a membership row into org B");
            Ok(())
        })
    })
    .await
    .unwrap();

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn cross_org_invite_token_cannot_be_redirected_by_org_id_tampering() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);

    let org_a = Uuid::new_v4();
    let user_a = Uuid::new_v4();
    let token_a = seed_admin(&pool, org_a, user_a, "admin-a2@members-it.test").await;

    let org_b = Uuid::new_v4();
    let user_b = Uuid::new_v4();
    let token_b = seed_plain_member(&pool, org_b, user_b, "member-b2@members-it.test").await;

    let (_invite_a, plaintext) =
        create_invite(&app, &token_a, "tamper-target@members-it.test", "viewer").await;

    // mhinv1.<org_id>.<secret> — swap org A's id for org B's, keep the secret.
    let secret = plaintext.rsplit('.').next().unwrap();
    let tampered = format!("mhinv1.{org_b}.{secret}");

    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/members/invites/accept",
        Some(&token_b),
        Some(json!({ "token": tampered })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "org-id-swapped token must not resolve into org B: {body}"
    );
    assert_eq!(body["code"], "not_found");

    ephemeral.drop().await;
}

// ---------------------------------------------------------------------
// Concurrent last-owner race
// ---------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn concurrent_last_owner_race_exactly_one_survives() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);

    let org = Uuid::new_v4();
    let owner_a = Uuid::new_v4();
    let owner_b = Uuid::new_v4();
    let token_a = seed_admin(&pool, org, owner_a, "race-owner-a@members-it.test").await;
    let token_b = seed_admin(&pool, org, owner_b, "race-owner-b@members-it.test").await;

    let app_1 = app.clone();
    let token_1 = token_a.clone();
    let target_1 = owner_b;
    let task_1 = tokio::spawn(async move {
        send(
            &app_1,
            "DELETE",
            &format!("/api/v1/members/{target_1}"),
            Some(&token_1),
            None,
        )
        .await
    });

    let app_2 = app.clone();
    let token_2 = token_b.clone();
    let target_2 = owner_a;
    let task_2 = tokio::spawn(async move {
        send(
            &app_2,
            "DELETE",
            &format!("/api/v1/members/{target_2}"),
            Some(&token_2),
            None,
        )
        .await
    });

    let (status_1, body_1) = task_1.await.expect("task 1 join");
    let (status_2, body_2) = task_2.await.expect("task 2 join");

    // Exactly one removal succeeds; the other is denied. The loser is denied
    // one of two correct ways, and which one is timing-dependent — that is
    // *expected*, not flakiness to paper over:
    //   - 409 `last_owner`: the loser reached `guard_last_owner`, which under
    //     the owner-row `FOR UPDATE` lock saw the winner's removal and refused
    //     to drop the org to zero owners.
    //   - 403 `forbidden`: the winner removed the loser's OWN membership first,
    //     so `guard_owner_tier`'s in-transaction caller re-read found the caller
    //     no longer an active owner and denied before reaching the last-owner
    //     check.
    // Both deny the operation; the invariant checked below (exactly one active
    // owner remains) is the real guarantee, and holds in every interleaving
    // because the two removals serialize on the owner-row lock and can never
    // both succeed. (Neither a 5xx, a 404, nor a second 204 is acceptable.)
    let successes = [status_1, status_2]
        .iter()
        .filter(|status| **status == StatusCode::NO_CONTENT)
        .count();
    assert_eq!(
        successes, 1,
        "exactly one removal must succeed: {status_1} {body_1} / {status_2} {body_2}"
    );
    let (loser_status, loser_body) = if status_1 == StatusCode::NO_CONTENT {
        (status_2, &body_2)
    } else {
        (status_1, &body_1)
    };
    let denied_correctly = (loser_status == StatusCode::CONFLICT
        && loser_body["code"] == "last_owner")
        || (loser_status == StatusCode::FORBIDDEN && loser_body["code"] == "forbidden");
    assert!(
        denied_correctly,
        "loser must be denied via last_owner (409) or caller-no-longer-owner (403): \
         {loser_status} {loser_body}"
    );

    let ctx = OrgContext::try_new(org, owner_a, [] as [&str; 0], []).unwrap();
    with_org_txn(&pool, &ctx, move |txn| {
        Box::pin(async move {
            let active_owners: i64 = txn
                .query_one(
                    "SELECT count(*)::bigint FROM org_memberships
                     WHERE org_id = $1 AND role = 'owner' AND state = 'active'",
                    &[&org],
                )
                .await?
                .get(0);
            assert_eq!(active_owners, 1, "org must retain exactly one active owner");
            Ok(())
        })
    })
    .await
    .unwrap();

    ephemeral.drop().await;
}

// ---------------------------------------------------------------------
// Invite replay / expiry
// ---------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn invite_replay_and_expiry_all_reject_and_valid_accept_is_audited() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);

    let org = Uuid::new_v4();
    let owner = Uuid::new_v4();
    let owner_token = seed_admin(&pool, org, owner, "invite-owner@members-it.test").await;

    // Three separate accepting users from three separate orgs (each just
    // needs a valid bearer token; none may already be a member of `org`).
    let other_org_1 = Uuid::new_v4();
    let user_1 = Uuid::new_v4();
    let user_1_token =
        seed_plain_member(&pool, other_org_1, user_1, "accept-1@members-it.test").await;

    let other_org_2 = Uuid::new_v4();
    let user_2 = Uuid::new_v4();
    let user_2_token =
        seed_plain_member(&pool, other_org_2, user_2, "accept-2@members-it.test").await;

    let other_org_3 = Uuid::new_v4();
    let user_3 = Uuid::new_v4();
    let user_3_token =
        seed_plain_member(&pool, other_org_3, user_3, "accept-3@members-it.test").await;

    // --- Valid accept + same-commit audit row ---
    let (invite_valid, token_valid) =
        create_invite(&app, &owner_token, "accepted@members-it.test", "viewer").await;
    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/members/invites/accept",
        Some(&user_1_token),
        Some(json!({ "token": token_valid })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "valid accept: {body}");
    assert_eq!(body["userId"], user_1.to_string());

    let ctx = OrgContext::try_new(org, owner, [] as [&str; 0], []).unwrap();
    let audited = with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let entries = db_audit::list_recent(txn, &ctx, 50).await?;
                Ok(entries.into_iter().any(|entry| {
                    entry.action == "member.invite_accept"
                        && entry.resource_id == Some(invite_valid.to_string())
                }))
            })
        }
    })
    .await
    .unwrap();
    assert!(
        audited,
        "valid accept must write a member.invite_accept audit row"
    );

    // --- Replay: accepting the same token again must reject ---
    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/members/invites/accept",
        Some(&user_2_token),
        Some(json!({ "token": token_valid })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "replayed accept: {body}");

    // --- Revoked invite cannot be accepted ---
    let (invite_revoked, token_revoked) =
        create_invite(&app, &owner_token, "revoked@members-it.test", "viewer").await;
    let (status, body) = send(
        &app,
        "POST",
        &format!("/api/v1/members/invites/{invite_revoked}/revoke"),
        Some(&owner_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "revoke: {body}");
    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/members/invites/accept",
        Some(&user_2_token),
        Some(json!({ "token": token_revoked })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "accept after revoke: {body}");

    // --- Expired invite cannot be accepted ---
    let (invite_expired, token_expired) =
        create_invite(&app, &owner_token, "expired@members-it.test", "viewer").await;
    with_org_txn(
        &pool,
        &OrgContext::try_new(org, owner, [] as [&str; 0], []).unwrap(),
        {
            move |txn| {
                Box::pin(async move {
                    txn.execute(
                    "UPDATE org_invites SET expires_at = now() - interval '1 hour' WHERE id = $1",
                    &[&invite_expired],
                )
                .await?;
                    Ok(())
                })
            }
        },
    )
    .await
    .unwrap();
    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/members/invites/accept",
        Some(&user_3_token),
        Some(json!({ "token": token_expired })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "accept after expiry: {body}");

    ephemeral.drop().await;
}

// ---------------------------------------------------------------------
// Refresh rejected after remove / suspend / role downgrade
// ---------------------------------------------------------------------

async fn seed_two_owner_org(pool: &Pool, prefix: &str) -> (Uuid, Uuid, Uuid, String) {
    let org = Uuid::new_v4();
    let caller = Uuid::new_v4();
    let target = Uuid::new_v4();
    let caller_token = seed_admin(
        pool,
        org,
        caller,
        &format!("{prefix}-caller@members-it.test"),
    )
    .await;
    seed_admin(
        pool,
        org,
        target,
        &format!("{prefix}-target@members-it.test"),
    )
    .await;
    (org, caller, target, caller_token)
}

async fn assert_refresh_rejected(app: &axum::Router, refresh_token: &str) {
    let (status, body) = send(
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
        "stale refresh token must be rejected: {body}"
    );
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn refresh_rejected_after_remove() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);
    let (_org, _caller, target, caller_token) = seed_two_owner_org(&pool, "remove").await;
    let (_access, refresh) = login_tokens(&pool, "remove-target@members-it.test", PASSWORD).await;

    let (status, body) = send(
        &app,
        "DELETE",
        &format!("/api/v1/members/{target}"),
        Some(&caller_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "remove target: {body}");

    assert_refresh_rejected(&app, &refresh).await;
    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn refresh_rejected_after_suspend() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);
    let (_org, _caller, target, caller_token) = seed_two_owner_org(&pool, "suspend").await;
    let (_access, refresh) = login_tokens(&pool, "suspend-target@members-it.test", PASSWORD).await;

    let (status, body) = send(
        &app,
        "PATCH",
        &format!("/api/v1/members/{target}"),
        Some(&caller_token),
        Some(json!({ "state": "suspended" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "suspend target: {body}");
    assert_eq!(body["state"], "suspended");
    // The PATCH response must carry the same joined name/email `list_members`
    // does (owner-reported UI gap, closed) — not just the target's role/state.
    assert_eq!(body["email"], "suspend-target@members-it.test");
    assert_eq!(body["displayName"], "Integration User");

    assert_refresh_rejected(&app, &refresh).await;
    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn refresh_rejected_after_role_downgrade() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);
    let (_org, _caller, target, caller_token) = seed_two_owner_org(&pool, "downgrade").await;
    let (_access, refresh) =
        login_tokens(&pool, "downgrade-target@members-it.test", PASSWORD).await;

    let (status, body) = send(
        &app,
        "PATCH",
        &format!("/api/v1/members/{target}"),
        Some(&caller_token),
        Some(json!({ "role": "admin" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "downgrade target: {body}");
    assert_eq!(body["role"], "admin");
    assert_eq!(body["email"], "downgrade-target@members-it.test");
    assert_eq!(body["displayName"], "Integration User");

    assert_refresh_rejected(&app, &refresh).await;
    ephemeral.drop().await;
}

// ---------------------------------------------------------------------
// Privilege escalation (adversarial review finding #1 — BLOCKER)
// A non-owner member.manage holder must not reach the owner tier.
// ---------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL and MARKHAND_TEST_APP_DATABASE_URL"]
async fn non_owner_admin_cannot_reach_the_owner_tier() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &app_database_url().unwrap(), None);

    let org = Uuid::new_v4();
    let owner = Uuid::new_v4();
    let admin = Uuid::new_v4();
    // An owner must exist so the org is validly owned and the admin's attempts
    // are the only thing under test.
    seed_admin(&pool, org, owner, "esc-owner@members-it.test").await;
    let admin_token = seed_admin_role_member(&pool, org, admin, "esc-admin@members-it.test").await;

    // 1. Self-promotion to owner — the exact reported exploit — must be denied.
    let (status, body) = send(
        &app,
        "PATCH",
        &format!("/api/v1/members/{admin}"),
        Some(&admin_token),
        Some(json!({ "role": "owner" })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "self-promote to owner: {body}"
    );

    // 2. Granting owner to anyone else — same gate.
    let (status, _) = send(
        &app,
        "POST",
        "/api/v1/members/invites",
        Some(&admin_token),
        Some(json!({ "email": "new-owner@members-it.test", "role": "owner" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "invite an owner as admin");

    // 3. Managing the existing owner (demote / suspend / remove) — all denied,
    //    "admin không quản owner".
    let (status, _) = send(
        &app,
        "PATCH",
        &format!("/api/v1/members/{owner}"),
        Some(&admin_token),
        Some(json!({ "role": "admin" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "admin demoting the owner");

    let (status, _) = send(
        &app,
        "PATCH",
        &format!("/api/v1/members/{owner}"),
        Some(&admin_token),
        Some(json!({ "state": "suspended" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "admin suspending the owner");

    let (status, _) = send(
        &app,
        "DELETE",
        &format!("/api/v1/members/{owner}"),
        Some(&admin_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "admin removing the owner");

    ephemeral.drop().await;
}

// ---------------------------------------------------------------------
// Deterministic last-owner denial (complements the concurrent race above,
// which is inherently racy about 409 vs 403). A single-owner org acting on
// itself has no interleaving to race against: the caller IS the sole owner,
// so `guard_owner_tier` always passes (an active owner may always manage an
// owner) and `guard_last_owner` is the only gate — this must deterministically
// 409, including when the owner targets their own membership.
// ---------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn sole_owner_cannot_downgrade_or_remove_themselves() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);

    let org = Uuid::new_v4();
    let owner = Uuid::new_v4();
    let owner_token = seed_admin(&pool, org, owner, "sole-owner@members-it.test").await;

    // Self-downgrade: the org's only active owner may not demote themselves.
    let (status, body) = send(
        &app,
        "PATCH",
        &format!("/api/v1/members/{owner}"),
        Some(&owner_token),
        Some(json!({ "role": "admin" })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "self-downgrade sole owner: {body}"
    );
    assert_eq!(body["code"], "last_owner");

    // Self-remove: same invariant, the DELETE path.
    let (status, body) = send(
        &app,
        "DELETE",
        &format!("/api/v1/members/{owner}"),
        Some(&owner_token),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "self-remove sole owner: {body}"
    );
    assert_eq!(body["code"], "last_owner");

    // Self-suspend: state transitions share the same guard.
    let (status, body) = send(
        &app,
        "PATCH",
        &format!("/api/v1/members/{owner}"),
        Some(&owner_token),
        Some(json!({ "state": "suspended" })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "self-suspend sole owner: {body}"
    );
    assert_eq!(body["code"], "last_owner");

    // The membership must be untouched by all three rejected attempts.
    let ctx = OrgContext::try_new(org, owner, [] as [&str; 0], []).unwrap();
    with_org_txn(&pool, &ctx, move |txn| {
        Box::pin(async move {
            let row = txn
                .query_one(
                    "SELECT role, state FROM org_memberships WHERE org_id = $1 AND user_id = $2",
                    &[&org, &owner],
                )
                .await?;
            let role: String = row.get(0);
            let state: String = row.get(1);
            assert_eq!(
                role, "owner",
                "role must be unchanged after rejected downgrade"
            );
            assert_eq!(
                state, "active",
                "state must be unchanged after rejected suspend"
            );
            Ok(())
        })
    })
    .await
    .unwrap();

    ephemeral.drop().await;
}

// ---------------------------------------------------------------------
// Missing `member.manage` permission must deny PATCH/DELETE with 403, not
// leak whether the target user id exists (permission check runs before the
// existence pre-check in both route handlers).
// ---------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn member_manage_permission_required_for_patch_and_delete() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);

    let org = Uuid::new_v4();
    let plain_user = Uuid::new_v4();
    // Same org, but zero permissions granted (see `seed_plain_member` doc) —
    // an authenticated caller with no `member.manage`.
    let plain_token = seed_plain_member(&pool, org, plain_user, "no-perm@members-it.test").await;

    // Target need not exist: the permission gate must deny before any lookup.
    let some_target = Uuid::new_v4();

    let (status, body) = send(
        &app,
        "PATCH",
        &format!("/api/v1/members/{some_target}"),
        Some(&plain_token),
        Some(json!({ "role": "admin" })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "patch without member.manage: {body}"
    );
    assert_eq!(body["code"], "forbidden");

    let (status, body) = send(
        &app,
        "DELETE",
        &format!("/api/v1/members/{some_target}"),
        Some(&plain_token),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "delete without member.manage: {body}"
    );
    assert_eq!(body["code"], "forbidden");

    // Both denials must have been audited (route layer calls
    // `audit::record_deny` before returning, see routes/members.rs).
    let ctx = OrgContext::try_new(org, plain_user, [] as [&str; 0], []).unwrap();
    let (role_change_denied, remove_denied) = with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let entries = db_audit::list_recent(txn, &ctx, 50).await?;
                let role_change_denied = entries.iter().any(|entry| {
                    entry.action == "member.role_change"
                        && entry.resource_id == Some(some_target.to_string())
                        && entry.outcome == fileconv_server::db::models::AuditOutcome::Deny
                });
                let remove_denied = entries.iter().any(|entry| {
                    entry.action == "member.remove"
                        && entry.resource_id == Some(some_target.to_string())
                        && entry.outcome == fileconv_server::db::models::AuditOutcome::Deny
                });
                Ok((role_change_denied, remove_denied))
            })
        }
    })
    .await
    .unwrap();
    assert!(
        role_change_denied,
        "PATCH permission denial must be audited"
    );
    assert!(remove_denied, "DELETE permission denial must be audited");

    ephemeral.drop().await;
}

// ---------------------------------------------------------------------
// A user id that never had a membership row in the caller's own org (not a
// cross-org row hidden by RLS, but a genuinely nonexistent one) must also
// 404 — same no-oracle contract as the cross-org case.
// ---------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn nonexistent_member_returns_404_for_patch_and_delete() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);

    let org = Uuid::new_v4();
    let owner = Uuid::new_v4();
    let owner_token = seed_admin(&pool, org, owner, "nonexistent-owner@members-it.test").await;
    let never_seeded = Uuid::new_v4();

    let (status, body) = send(
        &app,
        "PATCH",
        &format!("/api/v1/members/{never_seeded}"),
        Some(&owner_token),
        Some(json!({ "role": "admin" })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "patch nonexistent member: {body}"
    );
    assert_eq!(body["code"], "not_found");

    let (status, body) = send(
        &app,
        "DELETE",
        &format!("/api/v1/members/{never_seeded}"),
        Some(&owner_token),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "delete nonexistent member: {body}"
    );
    assert_eq!(body["code"], "not_found");

    ephemeral.drop().await;
}

// ---------------------------------------------------------------------
// Happy-path role change and remove must each write an audit row carrying
// old->new (role change) / old role (remove) in metadata, in addition to the
// session-revocation behavior already covered by the `refresh_rejected_*`
// tests above.
// ---------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn role_change_and_remove_write_audit_rows_with_before_after() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);

    // Two-owner org so downgrading/removing one target never trips the
    // last-owner invariant.
    let (org, _caller, target, caller_token) = seed_two_owner_org(&pool, "audit-role-remove").await;

    let (status, body) = send(
        &app,
        "PATCH",
        &format!("/api/v1/members/{target}"),
        Some(&caller_token),
        Some(json!({ "role": "admin" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "downgrade target: {body}");

    let ctx = OrgContext::try_new(org, target, [] as [&str; 0], []).unwrap();
    let role_change_entry = with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let entries = db_audit::list_recent(txn, &ctx, 50).await?;
                Ok(entries.into_iter().find(|entry| {
                    entry.action == "member.role_change"
                        && entry.resource_id == Some(target.to_string())
                }))
            })
        }
    })
    .await
    .unwrap();
    let role_change_entry = role_change_entry.expect("role_change audit row must exist");
    assert_eq!(role_change_entry.metadata["old_role"], "owner");
    assert_eq!(role_change_entry.metadata["new_role"], "admin");

    let (status, body) = send(
        &app,
        "DELETE",
        &format!("/api/v1/members/{target}"),
        Some(&caller_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "remove target: {body}");

    let remove_entry = with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let entries = db_audit::list_recent(txn, &ctx, 50).await?;
                Ok(entries.into_iter().find(|entry| {
                    entry.action == "member.remove" && entry.resource_id == Some(target.to_string())
                }))
            })
        }
    })
    .await
    .unwrap();
    let remove_entry = remove_entry.expect("member.remove audit row must exist");
    // `target` was downgraded to `admin` just above, so the removed row's
    // last known role is `admin`, not the original `owner`.
    assert_eq!(remove_entry.metadata["old_role"], "admin");

    ephemeral.drop().await;
}
