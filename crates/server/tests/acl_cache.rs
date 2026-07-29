//! DB-gated tests for the 1C-05 `auth::context_cache::OrgContextCache`.
//!
//! Skips cleanly when `MARKHAND_TEST_DATABASE_URL` / `MARKHAND_TEST_APP_DATABASE_URL`
//! are unset (see `common::boot_app_pool`), same convention as
//! `tests/members.rs`/`tests/orgs.rs`.
//!
//! These prove the cache never serves a stale `OrgContext` after the exact
//! mutations `db::orgs::bump_acl_version` is wired into (role change,
//! suspend, remove, `acl_mutate` revoke) — i.e. revoke/suspend/removal stay
//! effective on the very next request, same as before this cache existed.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{
    admin_database_url, app_database_url, boot_app_pool, build_router, login_access_token,
    seed_user_with_permissions, test_auth_config,
};
use fileconv_server::auth::context::OrgContext;
use fileconv_server::auth::jwt::JwtKeys;
use fileconv_server::auth::provider::PasswordAuthProvider;
use fileconv_server::db::pool::with_org_txn;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;

const PASSWORD: &str = "correct-password-1";

async fn boot_pool() -> Option<(common::DualRoleEphemeralDb, deadpool_postgres::Pool)> {
    let admin = admin_database_url()?;
    let app = app_database_url()?;
    Some(boot_app_pool(&admin, &app).await)
}

/// Seeds a non-owner `admin`-role member holding only `member.manage`. Mirrors
/// `tests/members.rs::seed_admin_role_member` (kept local/duplicated rather
/// than shared, since that helper is private to its own test binary).
async fn seed_admin_role_member(
    pool: &deadpool_postgres::Pool,
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
    fileconv_server::auth::session::set_password_hash(
        pool,
        user,
        PASSWORD,
        &test_auth_config().argon2,
    )
    .await
    .expect("set admin password");
    login_access_token(pool, email, PASSWORD).await
}

async fn send(
    app: &axum::Router,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<Value>,
) -> StatusCode {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"));
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
    // Drain the body so keep-alive/tower plumbing does not complain.
    let _ = response.into_body().collect().await;
    status
}

/// Same `AuthenticatedOrg` extractor + cache every real route uses. Populates
/// the cache on the first call (miss), then must observe a same-org mutation
/// on the very next call (hit-path freshness check) without ever serving a
/// stale allow.
#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL and MARKHAND_TEST_APP_DATABASE_URL"]
async fn cached_context_denies_immediately_after_role_downgrade() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);

    let org = Uuid::new_v4();
    let owner = Uuid::new_v4();
    let admin = Uuid::new_v4();
    seed_user_with_permissions(
        &pool,
        org,
        owner,
        "owner@acl-cache.test",
        PASSWORD,
        &["member.manage"],
    )
    .await;
    let owner_token = login_access_token(&pool, "owner@acl-cache.test", PASSWORD).await;
    let admin_token = seed_admin_role_member(&pool, org, admin, "admin@acl-cache.test").await;

    // Warm the cache for `admin` — must currently be allowed.
    let status = send(&app, "GET", "/api/v1/members", &admin_token, None).await;
    assert_eq!(status, StatusCode::OK, "admin must initially list members");

    // Owner downgrades admin -> viewer (drops member.manage) through the real
    // PATCH route, in the same request path Wave 2 ships.
    let status = send(
        &app,
        "PATCH",
        &format!("/api/v1/members/{admin}"),
        &owner_token,
        Some(json!({ "role": "viewer" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "owner must be able to downgrade");

    // Same bearer token, no re-login: the cache must not serve the
    // pre-downgrade permission set.
    let status = send(&app, "GET", "/api/v1/members", &admin_token, None).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "cached context must not outlive a role downgrade that drops member.manage"
    );

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL and MARKHAND_TEST_APP_DATABASE_URL"]
async fn cached_context_denies_immediately_after_suspend() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);

    let org = Uuid::new_v4();
    let owner = Uuid::new_v4();
    let admin = Uuid::new_v4();
    seed_user_with_permissions(
        &pool,
        org,
        owner,
        "owner@acl-cache-susp.test",
        PASSWORD,
        &["member.manage"],
    )
    .await;
    let owner_token = login_access_token(&pool, "owner@acl-cache-susp.test", PASSWORD).await;
    let admin_token = seed_admin_role_member(&pool, org, admin, "admin@acl-cache-susp.test").await;

    let status = send(&app, "GET", "/api/v1/members", &admin_token, None).await;
    assert_eq!(status, StatusCode::OK);

    let status = send(
        &app,
        "PATCH",
        &format!("/api/v1/members/{admin}"),
        &owner_token,
        Some(json!({ "state": "suspended" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "owner must be able to suspend");

    let status = send(&app, "GET", "/api/v1/members", &admin_token, None).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "cached context must not outlive a suspend (resolves as membership-missing)"
    );

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL and MARKHAND_TEST_APP_DATABASE_URL"]
async fn cached_context_denies_immediately_after_remove() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);

    let org = Uuid::new_v4();
    let owner = Uuid::new_v4();
    let admin = Uuid::new_v4();
    seed_user_with_permissions(
        &pool,
        org,
        owner,
        "owner@acl-cache-rm.test",
        PASSWORD,
        &["member.manage"],
    )
    .await;
    let owner_token = login_access_token(&pool, "owner@acl-cache-rm.test", PASSWORD).await;
    let admin_token = seed_admin_role_member(&pool, org, admin, "admin@acl-cache-rm.test").await;

    let status = send(&app, "GET", "/api/v1/members", &admin_token, None).await;
    assert_eq!(status, StatusCode::OK);

    let status = send(
        &app,
        "DELETE",
        &format!("/api/v1/members/{admin}"),
        &owner_token,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "owner must be able to remove"
    );

    let status = send(&app, "GET", "/api/v1/members", &admin_token, None).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "cached context must not outlive a hard removal"
    );

    ephemeral.drop().await;
}

/// Exercises `OrgContextCache::resolve` directly (same call the
/// `AuthenticatedOrg` extractor makes) around
/// `services::acl_mutate::revoke_collection_access_for_principal`, which
/// `tests/uploads.rs`/`tests/sse_stream_readiness.rs`'s in-flight ACL-revoke
/// tests already depend on taking effect immediately on the always-fresh
/// paths they use. This proves the *cached* path gives the same guarantee.
#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL and MARKHAND_TEST_APP_DATABASE_URL"]
async fn cached_context_drops_collection_access_immediately_after_acl_revoke() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };

    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let collection = Uuid::new_v4();
    seed_user_with_permissions(
        &pool,
        org,
        user,
        "owner@acl-cache-coll.test",
        PASSWORD,
        &["doc.upload"],
    )
    .await;

    // Give `user` a private collection they own (the resolver's
    // `owner_user_id = $2` branch), matching `tests/uploads.rs`'s ACL-revoke
    // fixture shape.
    let ctx = OrgContext::try_new(org, user, ["doc.upload"], []).unwrap();
    with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "INSERT INTO collections (id, org_id, name, slug, owner_user_id, visibility)
                     VALUES ($1, $2, 'ACL cache test', 'acl-cache-test', $3, 'private')",
                    &[&collection, &ctx.org_id(), &user],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("seed collection");

    let auth = PasswordAuthProvider::new(
        pool.clone(),
        test_auth_config(),
        JwtKeys::from_auth(&test_auth_config()).unwrap(),
    );

    let warm = auth
        .context_cache()
        .resolve(&pool, org, user)
        .await
        .expect("initial resolve");
    assert!(
        warm.allows_collection(collection),
        "owner must initially see their private collection"
    );

    let alt_owner = Uuid::new_v4();
    with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                fileconv_server::db::orgs::ensure_user(
                    txn,
                    &ctx,
                    alt_owner,
                    &format!("alt-{}@acl-cache-coll.test", alt_owner.simple()),
                    "Alt Owner",
                )
                .await?;
                txn.execute(
                    "INSERT INTO org_memberships (org_id, user_id, role)
                     VALUES ($1, $2, 'owner') ON CONFLICT (org_id, user_id) DO NOTHING",
                    &[&org, &alt_owner],
                )
                .await?;
                fileconv_server::services::acl_mutate::revoke_collection_access_for_principal(
                    txn, org, user, collection, alt_owner,
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("revoke collection acl");

    let after = auth
        .context_cache()
        .resolve(&pool, org, user)
        .await
        .expect("resolve after revoke");
    assert!(
        !after.allows_collection(collection),
        "cached context must not keep granting a collection revoked out from under it"
    );

    ephemeral.drop().await;
}

/// Proves the version-check mechanism itself (not merely "the app-level
/// helper works"): a bare `orgs.acl_version` bump — with no other production
/// code involved — is enough to force a fresh resolve on the next cache
/// lookup instead of serving the entry cached before the bump.
#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL and MARKHAND_TEST_APP_DATABASE_URL"]
async fn bare_acl_version_bump_forces_fresh_resolve() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };

    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    seed_user_with_permissions(
        &pool,
        org,
        user,
        "owner@acl-cache-version.test",
        PASSWORD,
        &["doc.upload"],
    )
    .await;

    let auth = PasswordAuthProvider::new(
        pool.clone(),
        test_auth_config(),
        JwtKeys::from_auth(&test_auth_config()).unwrap(),
    );

    let warm = auth
        .context_cache()
        .resolve(&pool, org, user)
        .await
        .expect("initial resolve");
    assert!(warm.has_permission("doc.upload"));

    // Simulate an out-of-band grant (bypassing `services::acl_mutate`
    // entirely) plus the version bump every real mutation path performs.
    // `roles`/`role_permissions` are FORCE RLS'd by `org_id`
    // (migrations/0010), so this must run inside a transaction with the
    // `app.org_id` GUC set — a plain `pool.get()` client would have the
    // `roles` read filtered down to zero rows by RLS and silently grant
    // nothing (caught during verification: this failed with the plain-client
    // version until switched to `with_org_txn`).
    {
        let client = pool.get().await.expect("client");
        client
            .execute(
                "INSERT INTO permissions (id, code, description)
                 VALUES ($1, 'doc.publish', 'doc.publish') ON CONFLICT (code) DO NOTHING",
                &[&Uuid::new_v4()],
            )
            .await
            .expect("insert permission");
    }
    let ctx = OrgContext::try_new(org, user, [] as [&str; 0], []).unwrap();
    with_org_txn(&pool, &ctx, {
        move |txn| {
            Box::pin(async move {
                let n = txn
                    .execute(
                        "INSERT INTO role_permissions (org_id, role_id, permission_id)
                         SELECT r.org_id, r.id, p.id
                         FROM roles r, permissions p
                         WHERE r.org_id = $1 AND r.code = 'owner' AND p.code = 'doc.publish'
                         ON CONFLICT DO NOTHING",
                        &[&org],
                    )
                    .await?;
                assert_eq!(n, 1, "grant must actually insert exactly one row");
                txn.execute(
                    "UPDATE orgs SET acl_version = acl_version + 1 WHERE id = $1",
                    &[&org],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("grant doc.publish to owner role and bump acl_version");

    let after = auth
        .context_cache()
        .resolve(&pool, org, user)
        .await
        .expect("resolve after bump");
    assert!(
        after.has_permission("doc.publish"),
        "a bare acl_version bump must force a fresh resolve, not serve the pre-bump cached entry"
    );

    ephemeral.drop().await;
}
