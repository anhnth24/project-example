//! Integration tests for password auth, rotating refresh sessions, and OrgContext.
//!
//! Gated on `MARKHAND_TEST_DATABASE_URL` (same ephemeral-DB pattern as
//! `repositories.rs` / `schema_migrations.rs`).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use deadpool_postgres::Pool;
use fileconv_server::auth::context::OrgContext;
use fileconv_server::auth::jwt::JwtKeys;
use fileconv_server::auth::permissions::resolve_org_context_in_txn;
use fileconv_server::auth::provider::{AuthProvider, AuthRequestMeta, PasswordAuthProvider};
use fileconv_server::auth::session::{self, SessionError};
use fileconv_server::config::{
    Argon2Config, AuthConfig, JwtAlgorithm, RuntimeEndpoints, SecretString, ServerConfig,
};
use fileconv_server::database::apply_migrations;
use fileconv_server::db::orgs;
use fileconv_server::db::pool::{create_pool, with_org_txn};
use fileconv_server::http::{router, AppState};
use fileconv_server::state::RuntimeState;
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

fn test_database_url() -> Option<String> {
    match std::env::var("MARKHAND_TEST_DATABASE_URL") {
        Ok(url) if !url.trim().is_empty() => Some(url),
        _ => {
            eprintln!(
                "skipped: MARKHAND_TEST_DATABASE_URL unset — auth integration tests require PostgreSQL"
            );
            None
        }
    }
}

fn rewrite_database_url(base_url: &str, database_name: &str) -> String {
    let (without_query, query) = match base_url.split_once('?') {
        Some((head, tail)) => (head, Some(tail)),
        None => (base_url, None),
    };
    let prefix = without_query
        .rsplit_once('/')
        .map(|(head, _)| head)
        .expect("database URL must include a path");
    match query {
        Some(tail) => format!("{prefix}/{database_name}?{tail}"),
        None => format!("{prefix}/{database_name}"),
    }
}

fn admin_database_url(base_url: &str) -> String {
    rewrite_database_url(base_url, "postgres")
}

async fn connect_raw(database_url: &str) -> tokio_postgres::Client {
    let (client, connection) = tokio_postgres::connect(database_url, tokio_postgres::NoTls)
        .await
        .unwrap_or_else(|error| panic!("connect failed for {database_url}: {error}"));
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client
}

struct EphemeralDb {
    admin_url: String,
    db_name: String,
    url: String,
}

impl EphemeralDb {
    async fn create(base_url: &str) -> Self {
        let db_name = format!("markhand_auth_{}", Uuid::new_v4().simple());
        let admin_url = admin_database_url(base_url);
        let admin = connect_raw(&admin_url).await;
        admin
            .batch_execute(&format!("CREATE DATABASE \"{db_name}\""))
            .await
            .expect("CREATE DATABASE");
        Self {
            admin_url,
            db_name: db_name.clone(),
            url: rewrite_database_url(base_url, &db_name),
        }
    }

    async fn drop(self) {
        let admin = connect_raw(&self.admin_url).await;
        let _ = admin
            .batch_execute(&format!(
                "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
                 WHERE datname = '{}' AND pid <> pg_backend_pid(); \
                 DROP DATABASE IF EXISTS \"{}\"",
                self.db_name, self.db_name
            ))
            .await;
    }
}

fn test_auth_config() -> AuthConfig {
    AuthConfig {
        issuer: Some("https://issuer.markhand.test".into()),
        audience: Some("markhand-api".into()),
        signing_key: Some(SecretString::new("integration-test-signing-key-32b!")),
        alg: JwtAlgorithm::Hs256,
        kid: Some("test-kid-1".into()),
        access_token_ttl_secs: 900,
        refresh_token_ttl_secs: 3_600,
        argon2: Argon2Config {
            memory_kib: 8_192,
            time_cost: 1,
            parallelism: 1,
        },
    }
}

fn test_runtime(database_url: &str) -> RuntimeState {
    RuntimeState::from_config(ServerConfig::test_with_endpoints(RuntimeEndpoints {
        database_url: SecretString::new(database_url),
        qdrant_url: "http://127.0.0.1:1".into(),
        minio_url: "http://127.0.0.1:1".into(),
    }))
    .expect("runtime")
}

async fn boot(base_url: &str) -> (EphemeralDb, Pool, PasswordAuthProvider, Arc<AppState>) {
    let ephemeral = EphemeralDb::create(base_url).await;
    apply_migrations(&ephemeral.url)
        .await
        .expect("apply migrations");
    let pool = create_pool(&ephemeral.url).expect("pool");
    let auth = test_auth_config();
    let keys = JwtKeys::from_auth(&auth).expect("jwt keys");
    let provider = PasswordAuthProvider::new(pool.clone(), auth, keys);
    let runtime = test_runtime(&ephemeral.url);
    let state = AppState::from_parts(
        runtime,
        pool.clone(),
        Some(PasswordAuthProvider::new(
            pool.clone(),
            test_auth_config(),
            JwtKeys::from_auth(&test_auth_config()).unwrap(),
        )),
    )
    .expect("app state");
    (ephemeral, pool, provider, Arc::new(state))
}

async fn seed_user(pool: &Pool, org: Uuid, user: Uuid, email: &str, password: &str) {
    let ctx = OrgContext::try_new(org, user, ["doc.upload"], []).unwrap();
    let email = email.to_string();
    with_org_txn(pool, &ctx, {
        let owned = ctx.clone();
        move |txn| {
            Box::pin(async move {
                orgs::ensure_exists(txn, &owned, "authorg", "Auth Org").await?;
                orgs::ensure_user(txn, &owned, user, &email, "Auth User").await?;
                orgs::ensure_membership(txn, &owned).await?;
                // Seed matching system role + permission so /me has permissions.
                txn.execute(
                    "INSERT INTO permissions (id, code, description)
                     VALUES ($1, 'doc.upload', 'Upload')
                     ON CONFLICT (code) DO NOTHING",
                    &[&Uuid::new_v4()],
                )
                .await?;
                let role_id = Uuid::new_v4();
                txn.execute(
                    "INSERT INTO roles (id, org_id, code, name, is_system)
                     VALUES ($1, $2, 'owner', 'Owner', true)
                     ON CONFLICT (org_id, code) DO NOTHING",
                    &[&role_id, &org],
                )
                .await?;
                let role_id: Uuid = txn
                    .query_one(
                        "SELECT id FROM roles WHERE org_id = $1 AND code = 'owner'",
                        &[&org],
                    )
                    .await?
                    .get(0);
                let perm_id: Uuid = txn
                    .query_one("SELECT id FROM permissions WHERE code = 'doc.upload'", &[])
                    .await?
                    .get(0);
                txn.execute(
                    "INSERT INTO role_permissions (org_id, role_id, permission_id)
                     VALUES ($1, $2, $3)
                     ON CONFLICT DO NOTHING",
                    &[&org, &role_id, &perm_id],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("seed user");
    session::set_password_hash(pool, user, password, &test_auth_config().argon2)
        .await
        .expect("set password");
}

async fn json_request(
    app: axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
    bearer: Option<&str>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = bearer {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    builder = builder.header("x-request-id", "req-auth-test-1");
    let request = builder
        .body(match body {
            Some(value) => Body::from(serde_json::to_vec(&value).unwrap()),
            None => Body::empty(),
        })
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or(Value::String(String::from_utf8_lossy(&bytes).into_owned()))
    };
    (status, value)
}

fn assert_no_secrets(value: &Value, password: &str, refresh: Option<&str>, access: Option<&str>) {
    let rendered = value.to_string();
    assert!(!rendered.contains(password));
    if let Some(token) = refresh {
        assert!(!rendered.contains(token) || rendered.contains("refreshToken"));
        // Response bodies intentionally include tokens; audit metadata must not.
    }
    let _ = access;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn login_me_refresh_logout_and_audit() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let (ephemeral, pool, provider, state) = boot(&base_url).await;
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let email = format!("user-{}@example.com", user.simple());
    let password = "correct-horse-battery";
    seed_user(&pool, org, user, &email, password).await;

    let app = router(
        AppState::from_parts(
            state.runtime().clone(),
            pool.clone(),
            Some(PasswordAuthProvider::new(
                pool.clone(),
                test_auth_config(),
                JwtKeys::from_auth(&test_auth_config()).unwrap(),
            )),
        )
        .unwrap(),
    );

    let (status, body) = json_request(
        app.clone(),
        "POST",
        "/api/v1/auth/login",
        Some(serde_json::json!({ "email": email, "password": password })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let access = body["accessToken"].as_str().unwrap().to_string();
    let refresh = body["refreshToken"].as_str().unwrap().to_string();
    assert!(refresh.starts_with("mh1."));
    assert!(!format!("{body:?}").contains(password));

    let (status, me) =
        json_request(app.clone(), "GET", "/api/v1/auth/me", None, Some(&access)).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["email"], email);
    assert_eq!(me["orgId"], org.to_string());
    assert!(me["permissions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p == "doc.upload"));

    // Wrong password rejected + audited.
    let (status, err) = json_request(
        app.clone(),
        "POST",
        "/api/v1/auth/login",
        Some(serde_json::json!({ "email": email, "password": "wrong-password" })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(err["code"], "invalid_credentials");
    assert!(!err.to_string().contains(password));
    assert!(!err.to_string().contains("wrong-password"));

    // Unknown user rejected.
    let (status, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/auth/login",
        Some(serde_json::json!({
            "email": "nobody@example.com",
            "password": password
        })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Refresh rotates.
    let (status, rotated) = json_request(
        app.clone(),
        "POST",
        "/api/v1/auth/refresh",
        Some(serde_json::json!({ "refreshToken": refresh })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{rotated}");
    let refresh2 = rotated["refreshToken"].as_str().unwrap().to_string();
    assert_ne!(refresh, refresh2);

    let (status, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/auth/refresh",
        Some(serde_json::json!({ "refreshToken": refresh })),
        None,
    )
    .await;
    // Old refresh was rotated → reuse → family revoke.
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // New refresh from before reuse should also be dead after family revoke.
    let (status, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/auth/refresh",
        Some(serde_json::json!({ "refreshToken": refresh2 })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Fresh login for logout path.
    let meta = AuthRequestMeta {
        request_id: Uuid::new_v4().to_string(),
    };
    let session = provider
        .login_password(&email, password, &meta)
        .await
        .unwrap();
    let refresh3 = session.tokens.refresh_token.expose().to_string();
    let (status, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/auth/logout",
        Some(serde_json::json!({ "refreshToken": refresh3 })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = json_request(
        app,
        "POST",
        "/api/v1/auth/refresh",
        Some(serde_json::json!({ "refreshToken": refresh3 })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Audit rows exist without secret material.
    let ctx = OrgContext::try_new(org, user, [] as [&str; 0], []).unwrap();
    let audits = with_org_txn(&pool, &ctx, move |txn| {
        Box::pin(async move {
            let rows = txn
                .query(
                    "SELECT action, outcome, metadata::text, request_id
                     FROM audit_log WHERE org_id = $1 ORDER BY seq",
                    &[&org],
                )
                .await?;
            Ok(rows
                .into_iter()
                .map(|row| {
                    (
                        row.get::<_, String>(0),
                        row.get::<_, String>(1),
                        row.get::<_, String>(2),
                        row.get::<_, Option<String>>(3),
                    )
                })
                .collect::<Vec<_>>())
        })
    })
    .await
    .unwrap();
    assert!(audits
        .iter()
        .any(|(action, outcome, _, _)| { action == "auth.login" && outcome == "success" }));
    assert!(audits
        .iter()
        .any(|(action, outcome, _, _)| { action == "auth.login" && outcome == "deny" }));
    assert!(audits
        .iter()
        .any(|(action, _, _, _)| action == "auth.refresh.reuse"));
    for (_, _, metadata, request_id) in &audits {
        assert!(!metadata.contains(password));
        assert!(!metadata.contains(&refresh));
        assert!(!metadata.contains("mh1."));
        let rid = request_id.as_deref().expect("audit request_id required");
        assert!(
            Uuid::parse_str(rid).is_ok(),
            "audit request_id must be a server-minted uuid, got {rid}"
        );
        assert_ne!(rid, "req-auth-test-1");
        assert_no_secrets(&Value::String(metadata.clone()), password, None, None);
    }

    // Client-controlled x-request-id that looks like a refresh token must never be audited.
    let malicious_rid = format!("mh1.{org}.{}", "a".repeat(40));
    let (status, _) = {
        let builder = Request::builder()
            .method("POST")
            .uri("/api/v1/auth/login")
            .header("content-type", "application/json")
            .header("x-request-id", &malicious_rid);
        let request = builder
            .body(Body::from(
                serde_json::to_vec(&serde_json::json!({
                    "email": email,
                    "password": "wrong-password-again"
                }))
                .unwrap(),
            ))
            .unwrap();
        let app = router(
            AppState::from_parts(
                state.runtime().clone(),
                pool.clone(),
                Some(PasswordAuthProvider::new(
                    pool.clone(),
                    test_auth_config(),
                    JwtKeys::from_auth(&test_auth_config()).unwrap(),
                )),
            )
            .unwrap(),
        );
        let response = app.oneshot(request).await.unwrap();
        (response.status(), ())
    };
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let audits_after = with_org_txn(&pool, &ctx, move |txn| {
        Box::pin(async move {
            let rows = txn
                .query(
                    "SELECT request_id FROM audit_log WHERE org_id = $1",
                    &[&org],
                )
                .await?;
            Ok(rows
                .into_iter()
                .map(|row| row.get::<_, Option<String>>(0))
                .collect::<Vec<_>>())
        })
    })
    .await
    .unwrap();
    for rid in audits_after.into_iter().flatten() {
        assert_ne!(rid, malicious_rid);
        assert!(!rid.contains("mh1."));
        assert!(
            Uuid::parse_str(&rid).is_ok(),
            "expected uuid request_id, got {rid}"
        );
    }

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn refresh_reuse_revokes_family_under_concurrency() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let (ephemeral, pool, provider, _) = boot(&base_url).await;
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let email = format!("race-{}@example.com", user.simple());
    let password = "race-password-value";
    seed_user(&pool, org, user, &email, password).await;

    let meta = AuthRequestMeta {
        request_id: Uuid::new_v4().to_string(),
    };
    let session = provider
        .login_password(&email, password, &meta)
        .await
        .unwrap();
    let refresh = session.tokens.refresh_token.expose().to_string();
    let family_id = session.tokens.family_id;

    // Force real contention: hold the family-first advisory lock so refreshes block.
    let hold = session::acquire_family_lock_for_test(&pool, org, family_id)
        .await
        .expect("hold family lock");
    let meta_a = AuthRequestMeta {
        request_id: Uuid::new_v4().to_string(),
    };
    let meta_b = AuthRequestMeta {
        request_id: Uuid::new_v4().to_string(),
    };
    let refresh_a = refresh.clone();
    let refresh_b = refresh.clone();
    let provider_a = PasswordAuthProvider::new(
        pool.clone(),
        test_auth_config(),
        JwtKeys::from_auth(&test_auth_config()).unwrap(),
    );
    let provider_b = PasswordAuthProvider::new(
        pool.clone(),
        test_auth_config(),
        JwtKeys::from_auth(&test_auth_config()).unwrap(),
    );
    let handle_a = tokio::spawn(async move { provider_a.refresh(&refresh_a, &meta_a).await });
    let handle_b = tokio::spawn(async move { provider_b.refresh(&refresh_b, &meta_b).await });

    // While the family lock is held, neither refresh may complete.
    // If family-first locking were removed, these would finish immediately.
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    assert!(
        !handle_a.is_finished(),
        "refresh A must block on family-first advisory lock"
    );
    assert!(
        !handle_b.is_finished(),
        "refresh B must block on family-first advisory lock"
    );

    hold.release().await.expect("release family lock");
    let a = handle_a.await.expect("join A");
    let b = handle_b.await.expect("join B");
    let results = [a, b];
    let successes = results.iter().filter(|r| r.is_ok()).count();
    let reuses = results
        .iter()
        .filter(|r| matches!(r, Err(SessionError::RefreshReuse)))
        .count();
    assert_eq!(
        successes, 1,
        "exactly one refresh must succeed: {results:?}"
    );
    assert_eq!(reuses, 1, "loser must report reuse: {results:?}");

    // Family fully revoked — winner's new refresh also unusable after reuse revoke.
    let winner_refresh = results
        .iter()
        .find_map(|r| r.as_ref().ok())
        .map(|s| s.tokens.refresh_token.expose().to_string())
        .unwrap();
    let meta = AuthRequestMeta {
        request_id: Uuid::new_v4().to_string(),
    };
    let follow = provider.refresh(&winner_refresh, &meta).await;
    assert!(
        matches!(
            follow,
            Err(SessionError::RefreshReuse) | Err(SessionError::InvalidRefresh)
        ),
        "family must be unusable after concurrent reuse: {follow:?}"
    );

    let ctx = OrgContext::try_new(org, user, [] as [&str; 0], []).unwrap();
    let active = with_org_txn(&pool, &ctx, move |txn| {
        Box::pin(async move {
            let count: i64 = txn
                .query_one(
                    "SELECT count(*)::bigint FROM refresh_tokens
                     WHERE org_id = $1 AND family_id = $2 AND revoked_at IS NULL",
                    &[&org, &family_id],
                )
                .await?
                .get(0);
            Ok(count)
        })
    })
    .await
    .unwrap();
    assert_eq!(active, 0, "all family tokens must be revoked");

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn rotated_token_vs_active_successor_revokes_family() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let (ephemeral, pool, provider, _) = boot(&base_url).await;
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let email = format!("succ-{}@example.com", user.simple());
    let password = "successor-password";
    seed_user(&pool, org, user, &email, password).await;

    let meta = AuthRequestMeta {
        request_id: Uuid::new_v4().to_string(),
    };
    let session = provider
        .login_password(&email, password, &meta)
        .await
        .unwrap();
    let rotated = session.tokens.refresh_token.expose().to_string();
    let family_id = session.tokens.family_id;

    // Rotate once so `rotated` is revoked/replaced and `active` is the live successor.
    let rotated_session = provider.refresh(&rotated, &meta).await.unwrap();
    let active = rotated_session.tokens.refresh_token.expose().to_string();

    // Hold family lock, then race reuse(rotated) against refresh(active).
    let hold = session::acquire_family_lock_for_test(&pool, org, family_id)
        .await
        .expect("hold family lock");
    let meta_reuse = AuthRequestMeta {
        request_id: Uuid::new_v4().to_string(),
    };
    let meta_active = AuthRequestMeta {
        request_id: Uuid::new_v4().to_string(),
    };
    let provider_reuse = PasswordAuthProvider::new(
        pool.clone(),
        test_auth_config(),
        JwtKeys::from_auth(&test_auth_config()).unwrap(),
    );
    let provider_active = PasswordAuthProvider::new(
        pool.clone(),
        test_auth_config(),
        JwtKeys::from_auth(&test_auth_config()).unwrap(),
    );
    let handle_reuse =
        tokio::spawn(async move { provider_reuse.refresh(&rotated, &meta_reuse).await });
    let handle_active =
        tokio::spawn(async move { provider_active.refresh(&active, &meta_active).await });

    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    assert!(
        !handle_reuse.is_finished() && !handle_active.is_finished(),
        "both paths must block on family-first lock; without it this race can deadlock/partial-commit"
    );

    hold.release().await.expect("release");
    let reuse_result = handle_reuse.await.expect("join reuse");
    let active_result = handle_active.await.expect("join active");

    // Reuse must be detected; active path may succeed first then get revoked, or fail as reuse.
    assert!(
        matches!(reuse_result, Err(SessionError::RefreshReuse)),
        "rotated token must report reuse: {reuse_result:?}"
    );
    assert!(
        matches!(
            active_result,
            Ok(_) | Err(SessionError::RefreshReuse) | Err(SessionError::InvalidRefresh)
        ),
        "active successor outcome: {active_result:?}"
    );

    let ctx = OrgContext::try_new(org, user, [] as [&str; 0], []).unwrap();
    let active_count = with_org_txn(&pool, &ctx, move |txn| {
        Box::pin(async move {
            let count: i64 = txn
                .query_one(
                    "SELECT count(*)::bigint FROM refresh_tokens
                     WHERE org_id = $1 AND family_id = $2 AND revoked_at IS NULL",
                    &[&org, &family_id],
                )
                .await?
                .get(0);
            Ok(count)
        })
    })
    .await
    .unwrap();
    assert_eq!(
        active_count, 0,
        "reuse must revoke the whole family including any successor minted in the race"
    );

    // Any token the active path returned must also be dead.
    if let Ok(session) = active_result {
        let follow = provider
            .refresh(
                session.tokens.refresh_token.expose(),
                &AuthRequestMeta {
                    request_id: Uuid::new_v4().to_string(),
                },
            )
            .await;
        assert!(
            matches!(
                follow,
                Err(SessionError::RefreshReuse) | Err(SessionError::InvalidRefresh)
            ),
            "successor minted during reuse race must not remain usable: {follow:?}"
        );
    }

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn disabled_user_and_removed_membership_deny_org_context() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let (ephemeral, pool, provider, _) = boot(&base_url).await;
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let email = format!("disabled-{}@example.com", user.simple());
    let password = "disable-me-now";
    seed_user(&pool, org, user, &email, password).await;

    let meta = AuthRequestMeta {
        request_id: Uuid::new_v4().to_string(),
    };
    let session = provider
        .login_password(&email, password, &meta)
        .await
        .unwrap();
    let access = session.tokens.access_token.expose().to_string();
    let refresh = session.tokens.refresh_token.expose().to_string();

    // Disable user.
    let client = pool.get().await.unwrap();
    client
        .execute(
            "UPDATE users SET disabled_at = now() WHERE id = $1",
            &[&user],
        )
        .await
        .unwrap();
    drop(client);

    assert_eq!(
        provider
            .login_password(&email, password, &meta)
            .await
            .unwrap_err(),
        SessionError::InvalidCredentials,
        "disabled login must return the same generic error as bad credentials"
    );
    assert_eq!(
        provider.refresh(&refresh, &meta).await.unwrap_err(),
        SessionError::UserDisabled
    );
    assert_eq!(
        resolve_org_context_in_txn(&pool, org, user)
            .await
            .unwrap_err(),
        fileconv_server::auth::permissions::ResolveError::UserDisabled
    );

    // Re-enable, remove membership, access token still cryptographically valid but OrgContext fails.
    let client = pool.get().await.unwrap();
    client
        .execute(
            "UPDATE users SET disabled_at = NULL WHERE id = $1",
            &[&user],
        )
        .await
        .unwrap();
    drop(client);

    // Remove membership: delete refresh token rows first (FK to org_memberships).
    let ctx = OrgContext::try_new(org, user, [] as [&str; 0], []).unwrap();
    with_org_txn(&pool, &ctx, move |txn| {
        Box::pin(async move {
            txn.execute(
                "DELETE FROM refresh_tokens WHERE org_id = $1 AND user_id = $2",
                &[&org, &user],
            )
            .await?;
            txn.execute(
                "DELETE FROM org_memberships WHERE org_id = $1 AND user_id = $2",
                &[&org, &user],
            )
            .await?;
            Ok(())
        })
    })
    .await
    .unwrap();

    // JWT still verifies, but current-state OrgContext resolution fails.
    let keys = JwtKeys::from_auth(&test_auth_config()).unwrap();
    assert!(keys.verify_access_token(&access).is_ok());
    assert_eq!(
        resolve_org_context_in_txn(&pool, org, user)
            .await
            .unwrap_err(),
        fileconv_server::auth::permissions::ResolveError::MembershipMissing
    );

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn concurrent_refresh_and_revoke_all_leaves_no_usable_token() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let (ephemeral, pool, provider, _) = boot(&base_url).await;
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let email = format!("revoke-race-{}@example.com", user.simple());
    let password = "revoke-race-password";
    seed_user(&pool, org, user, &email, password).await;

    let meta = AuthRequestMeta {
        request_id: Uuid::new_v4().to_string(),
    };
    let session = provider
        .login_password(&email, password, &meta)
        .await
        .unwrap();
    let refresh = session.tokens.refresh_token.expose().to_string();
    let family_id = session.tokens.family_id;

    // Hold the family-first lock so refresh blocks mid-path (after hash lookup).
    let hold = session::acquire_family_lock_for_test(&pool, org, family_id)
        .await
        .expect("hold family lock");

    let meta_refresh = AuthRequestMeta {
        request_id: Uuid::new_v4().to_string(),
    };
    let provider_refresh = PasswordAuthProvider::new(
        pool.clone(),
        test_auth_config(),
        JwtKeys::from_auth(&test_auth_config()).unwrap(),
    );
    let refresh_handle =
        tokio::spawn(async move { provider_refresh.refresh(&refresh, &meta_refresh).await });

    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    assert!(
        !refresh_handle.is_finished(),
        "refresh must block on family lock"
    );

    // revoke_all must also wait on the same family lock (and user lock). Without that
    // discipline it finishes while refresh is still blocked, then refresh inserts a
    // successor that survives the revoke snapshot.
    let request_id = Uuid::new_v4().to_string();
    let pool_revoke = pool.clone();
    let revoke_handle = tokio::spawn(async move {
        session::revoke_all_user_families(&pool_revoke, org, user, &request_id, "disable").await
    });

    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    assert!(
        !revoke_handle.is_finished(),
        "revoke_all_user_families must block on family advisory locks; \
         without them it races refresh and can miss the successor"
    );

    hold.release().await.expect("release family lock");
    let refresh_result = refresh_handle.await.expect("join refresh");
    let revoke_result = revoke_handle.await.expect("join revoke");
    assert!(
        revoke_result.is_ok(),
        "revoke_all must succeed: {revoke_result:?}"
    );

    let ctx = OrgContext::try_new(org, user, [] as [&str; 0], []).unwrap();
    let active = with_org_txn(&pool, &ctx, move |txn| {
        Box::pin(async move {
            let count: i64 = txn
                .query_one(
                    "SELECT count(*)::bigint FROM refresh_tokens
                     WHERE org_id = $1 AND user_id = $2 AND revoked_at IS NULL",
                    &[&org, &user],
                )
                .await?
                .get(0);
            Ok(count)
        })
    })
    .await
    .unwrap();
    assert_eq!(
        active, 0,
        "no refresh token may remain active after concurrent refresh + revoke_all"
    );

    if let Ok(session) = refresh_result {
        let follow = provider
            .refresh(
                session.tokens.refresh_token.expose(),
                &AuthRequestMeta {
                    request_id: Uuid::new_v4().to_string(),
                },
            )
            .await;
        assert!(
            matches!(
                follow,
                Err(SessionError::RefreshReuse) | Err(SessionError::InvalidRefresh)
            ),
            "successor from raced refresh must not be usable: {follow:?}"
        );
    }

    ephemeral.drop().await;
}

/// Regression: a forged `mh1.<org>.<random>` refresh/logout token must not write an
/// audit row into the victim org (append-only audit-log injection) and a non-existent
/// org must fail identically to a real org with a bad secret (no 500-vs-401 org oracle).
#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn forged_refresh_token_neither_injects_audit_nor_leaks_org_existence() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let (ephemeral, pool, _provider, state) = boot(&base_url).await;
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let email = format!("user-{}@example.com", user.simple());
    let password = "correct-horse-battery";
    seed_user(&pool, org, user, &email, password).await;

    let app = router(
        AppState::from_parts(
            state.runtime().clone(),
            pool.clone(),
            Some(PasswordAuthProvider::new(
                pool.clone(),
                test_auth_config(),
                JwtKeys::from_auth(&test_auth_config()).unwrap(),
            )),
        )
        .unwrap(),
    );

    let audit_count = |org_id: Uuid| {
        let pool = pool.clone();
        async move {
            let ctx = OrgContext::try_new(org_id, org_id, [] as [&str; 0], []).unwrap();
            with_org_txn(&pool, &ctx, move |txn| {
                Box::pin(async move {
                    let row = txn
                        .query_one(
                            "SELECT count(*)::bigint FROM audit_log WHERE org_id = $1",
                            &[&org_id],
                        )
                        .await?;
                    Ok(row.get::<_, i64>(0))
                })
            })
            .await
            .unwrap()
        }
    };

    let before = audit_count(org).await;

    // Forged token naming the victim org with a syntactically valid but unknown secret.
    let forged = format!("mh1.{org}.{}", "a".repeat(40));
    let (status, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/auth/refresh",
        Some(serde_json::json!({ "refreshToken": forged })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Idempotent logout must also refuse to inject an audit row.
    let (status, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/auth/logout",
        Some(serde_json::json!({ "refreshToken": forged })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // A non-existent org must return the SAME status as a real org with a bad secret.
    let ghost = format!("mh1.{}.{}", Uuid::new_v4(), "b".repeat(40));
    let (status, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/auth/refresh",
        Some(serde_json::json!({ "refreshToken": ghost })),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "non-existent org must not surface a 500 audit FK oracle"
    );

    let after = audit_count(org).await;
    assert_eq!(
        before, after,
        "forged tokens must not write any audit row into the victim org"
    );

    ephemeral.drop().await;
}
