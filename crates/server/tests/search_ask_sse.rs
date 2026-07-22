//! HTTP contract tests for P1B-R05 search/ask/closed-snapshot SSE.
//!
//! Hermetic coverage: Last-Event-ID / headers in `api/sse` + unauthenticated route.
//! Live PostgreSQL HTTP tests require `MARKHAND_TEST_DATABASE_URL`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use deadpool_postgres::Pool;
use fileconv_server::api::sse::{
    last_event_id_from_headers, parse_last_event_id, sse_response_headers, LastEventIdError,
};
use fileconv_server::auth::context::OrgContext;
use fileconv_server::auth::jwt::JwtKeys;
use fileconv_server::auth::provider::{AuthProvider, AuthRequestMeta, PasswordAuthProvider};
use fileconv_server::auth::session;
use fileconv_server::config::{
    Argon2Config, AuthConfig, JwtAlgorithm, RuntimeEndpoints, SecretString, ServerConfig,
};
use fileconv_server::database::apply_migrations;
use fileconv_server::db::collections::{self, NewCollection};
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
        _ => None,
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
        let db_name = format!("markhand_r05_{}", Uuid::new_v4().simple());
        let admin_url = rewrite_database_url(base_url, "postgres");
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

fn sha64(ch: char) -> String {
    ch.to_string().repeat(64)
}

struct Fixture {
    ephemeral: EphemeralDb,
    pool: Pool,
    org: Uuid,
    user: Uuid,
    other_org: Uuid,
    other_user: Uuid,
    collection: Uuid,
    document: Uuid,
    version: Uuid,
    access: String,
    other_access: String,
    refresh: String,
    auth: AuthConfig,
    app: axum::Router,
    database_url: String,
}

async fn resolve(pool: &Pool, org: Uuid, user: Uuid) -> OrgContext {
    fileconv_server::auth::permissions::resolve_org_context_in_txn(pool, org, user)
        .await
        .expect("resolve")
}

async fn seed_member(
    pool: &Pool,
    org: Uuid,
    user: Uuid,
    email: &str,
    password: &str,
    role: &str,
    permissions: &[&str],
) {
    let ctx = OrgContext::try_new(org, user, permissions.iter().copied(), []).unwrap();
    let email = email.to_string();
    let role = role.to_string();
    let permissions: Vec<String> = permissions
        .iter()
        .map(|value| (*value).to_string())
        .collect();
    with_org_txn(pool, &ctx, {
        let owned = ctx.clone();
        move |txn| {
            Box::pin(async move {
                orgs::ensure_exists(txn, &owned, &format!("org-{}", org.simple()), "Org").await?;
                orgs::ensure_user(txn, &owned, user, &email, "User").await?;
                txn.execute(
                    "INSERT INTO org_memberships (org_id, user_id, role)
                     VALUES ($1, $2, $3)
                     ON CONFLICT (org_id, user_id) DO UPDATE SET role = EXCLUDED.role",
                    &[&org, &user, &role],
                )
                .await?;
                let role_id = Uuid::new_v4();
                txn.execute(
                    "INSERT INTO roles (id, org_id, code, name, is_system)
                     VALUES ($1, $2, $3, $3, true)
                     ON CONFLICT (org_id, code) DO NOTHING",
                    &[&role_id, &org, &role],
                )
                .await?;
                let role_id: Uuid = txn
                    .query_one(
                        "SELECT id FROM roles WHERE org_id = $1 AND code = $2",
                        &[&org, &role],
                    )
                    .await?
                    .get(0);
                for code in &permissions {
                    txn.execute(
                        "INSERT INTO permissions (id, code, description)
                         VALUES ($1, $2, $2)
                         ON CONFLICT (code) DO NOTHING",
                        &[&Uuid::new_v4(), code],
                    )
                    .await?;
                    let perm_id: Uuid = txn
                        .query_one("SELECT id FROM permissions WHERE code = $1", &[code])
                        .await?
                        .get(0);
                    txn.execute(
                        "INSERT INTO role_permissions (org_id, role_id, permission_id)
                         VALUES ($1, $2, $3)
                         ON CONFLICT DO NOTHING",
                        &[&org, &role_id, &perm_id],
                    )
                    .await?;
                }
                Ok(())
            })
        }
    })
    .await
    .expect("seed member");
    session::set_password_hash(pool, user, password, &test_auth_config().argon2)
        .await
        .expect("set password");
}

fn build_app(database_url: &str, pool: &Pool, auth: &AuthConfig) -> axum::Router {
    let keys = JwtKeys::from_auth(auth).expect("jwt keys");
    let runtime = test_runtime(database_url);
    let state = AppState::from_parts(
        runtime,
        pool.clone(),
        Some(PasswordAuthProvider::new(pool.clone(), auth.clone(), keys)),
    )
    .expect("app state");
    router(state)
}

async fn boot_fixture(base_url: &str) -> Fixture {
    let ephemeral = EphemeralDb::create(base_url).await;
    apply_migrations(&ephemeral.url)
        .await
        .expect("apply migrations");
    let pool = create_pool(&ephemeral.url).expect("pool");
    let auth = test_auth_config();
    let keys = JwtKeys::from_auth(&auth).expect("jwt keys");
    let provider = PasswordAuthProvider::new(pool.clone(), auth.clone(), keys);
    let app = build_app(&ephemeral.url, &pool, &auth);

    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let other_org = Uuid::new_v4();
    let other_user = Uuid::new_v4();
    let password = "correct-horse-battery";

    seed_member(
        &pool,
        org,
        user,
        &format!("owner-{}@example.com", user.simple()),
        password,
        "owner",
        &["qa.query", "qa.history", "doc.upload"],
    )
    .await;
    seed_member(
        &pool,
        other_org,
        other_user,
        &format!("other-{}@example.com", other_user.simple()),
        password,
        "owner",
        &["qa.query", "qa.history"],
    )
    .await;

    let owner_ctx = resolve(&pool, org, user).await;
    let collection = Uuid::new_v4();
    with_org_txn(&pool, &owner_ctx, {
        let ctx = owner_ctx.clone();
        move |txn| {
            Box::pin(async move {
                collections::insert(
                    txn,
                    &ctx,
                    NewCollection {
                        id: collection,
                        name: "Lib",
                        slug: "lib-a",
                        description: None,
                        visibility: fileconv_server::db::models::CollectionVisibility::Org,
                    },
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("collection");
    let owner_ctx = resolve(&pool, org, user).await;

    let document = Uuid::new_v4();
    let version = Uuid::new_v4();
    let meta_id = Uuid::new_v4();
    let chunk_id = Uuid::new_v4();
    let content_sha = sha64('e');
    let identity = sha64('c');
    let sig = sha64('a');

    with_org_txn(&pool, &owner_ctx, {
        let ctx = owner_ctx.clone();
        let content_sha = content_sha.clone();
        let identity = identity.clone();
        let sig = sig.clone();
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "INSERT INTO documents (
                        id, org_id, collection_id, title, state, created_by_user_id
                     ) VALUES ($1, $2, $3, 'Doc', 'indexed', $4)",
                    &[&document, &ctx.org_id(), &collection, &ctx.user_id()],
                )
                .await?;
                txn.execute(
                    "INSERT INTO document_versions (
                        id, org_id, document_id, version_number, publication_state, is_current,
                        content_sha256, original_object_key, effective_from, created_by_user_id
                     ) VALUES ($1,$2,$3,1,'published',true,$4,'k1', now(), $5)",
                    &[
                        &version,
                        &ctx.org_id(),
                        &document,
                        &content_sha,
                        &ctx.user_id(),
                    ],
                )
                .await?;
                txn.execute(
                    "UPDATE documents SET current_version_id = $1 WHERE id = $2",
                    &[&version, &document],
                )
                .await?;
                txn.execute(
                    "INSERT INTO index_metadata (
                        id, org_id, collection_id, index_signature_sha256, embedding_family,
                        embedding_revision, dimensions, runtime_path, generation, is_active, state
                     ) VALUES ($1,$2,$3,$4,'f','r',8,'local-hash',1,true,'active')",
                    &[&meta_id, &ctx.org_id(), &collection, &sig],
                )
                .await?;
                txn.execute(
                    "INSERT INTO chunks (
                        id, org_id, document_id, version_id, ordinal, heading_path, body,
                        chunk_identity_sha256, index_metadata_id, index_signature,
                        page, span_start, span_end
                     ) VALUES (
                        $1,$2,$3,$4,0,ARRAY['Đối soát'],'Đối soát giao dịch theo ngày ngân hàng',
                        $5,$6,$7,1,0,40
                     )",
                    &[
                        &chunk_id,
                        &ctx.org_id(),
                        &document,
                        &version,
                        &identity,
                        &meta_id,
                        &sig,
                    ],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("seed retrieval corpus");

    let meta = AuthRequestMeta {
        request_id: "r05-login".into(),
    };
    let owner_session = provider
        .login_password(
            &format!("owner-{}@example.com", user.simple()),
            password,
            &meta,
        )
        .await
        .expect("owner login");
    let other_session = provider
        .login_password(
            &format!("other-{}@example.com", other_user.simple()),
            password,
            &meta,
        )
        .await
        .expect("other login");

    Fixture {
        database_url: ephemeral.url.clone(),
        ephemeral,
        pool,
        org,
        user,
        other_org,
        other_user,
        collection,
        document,
        version,
        access: owner_session.tokens.access_token.expose().to_string(),
        other_access: other_session.tokens.access_token.expose().to_string(),
        refresh: owner_session.tokens.refresh_token.expose().to_string(),
        auth,
        app,
    }
}

async fn json_request(
    app: axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    access: Option<&str>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(token) = access {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(match body {
            Some(value) => Body::from(serde_json::to_vec(&value).unwrap()),
            None => Body::empty(),
        })
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or(Value::String(String::from_utf8_lossy(&bytes).into_owned()))
    };
    (status, json)
}

async fn sse_request(
    app: axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    access: Option<&str>,
    last_event_id: Option<&str>,
) -> (StatusCode, HeaderMapLite, String) {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(token) = access {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    if let Some(id) = last_event_id {
        builder = builder.header("last-event-id", id);
    }
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(match body {
            Some(value) => Body::from(serde_json::to_vec(&value).unwrap()),
            None => Body::empty(),
        })
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let headers = HeaderMapLite {
        content_type: response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string(),
        cache_control: response
            .headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string(),
        request_id: response
            .headers()
            .get("x-request-id")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string(),
    };
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        headers,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

struct HeaderMapLite {
    content_type: String,
    cache_control: String,
    request_id: String,
}

fn parse_sse_envelopes(body: &str) -> Vec<Value> {
    let mut out = Vec::new();
    for block in body.split("\n\n") {
        for line in block.lines() {
            if let Some(data) = line.strip_prefix("data:") {
                let data = data.trim();
                if let Ok(value) = serde_json::from_str::<Value>(data) {
                    out.push(value);
                }
            }
        }
    }
    out
}

fn assert_contiguous_sequences(envelopes: &[Value]) {
    let sequences: Vec<u64> = envelopes
        .iter()
        .map(|e| e["sequence"].as_u64().expect("sequence"))
        .collect();
    assert!(!sequences.is_empty());
    for window in sequences.windows(2) {
        assert_eq!(window[1], window[0] + 1, "gap in sequences {sequences:?}");
    }
}

#[test]
fn hermetic_last_event_id_and_headers() {
    assert_eq!(parse_last_event_id(Some("0")).unwrap(), Some(0));
    assert_eq!(
        parse_last_event_id(Some("01")).unwrap_err(),
        LastEventIdError::InvalidSyntax
    );
    let mut headers = axum::http::HeaderMap::new();
    assert_eq!(last_event_id_from_headers(&headers).unwrap(), None);
    headers.insert(
        "last-event-id",
        axum::http::HeaderValue::from_bytes(&[0xff]).unwrap(),
    );
    assert_eq!(
        last_event_id_from_headers(&headers).unwrap_err(),
        LastEventIdError::InvalidSyntax
    );
    let headers = sse_response_headers();
    assert!(headers
        .get(axum::http::header::CACHE_CONTROL)
        .unwrap()
        .to_str()
        .unwrap()
        .contains("no-store"));
}

#[tokio::test]
async fn hermetic_search_without_auth_config_is_unavailable() {
    let runtime = test_runtime("postgres://markhand_app:markhand_app@127.0.0.1:5432/markhand_test");
    let pool = create_pool("postgres://markhand_app:markhand_app@127.0.0.1:5432/markhand_test")
        .expect("pool");
    let app = router(AppState::from_parts(runtime, pool, None).unwrap());
    let (status, body) = json_request(
        app,
        "POST",
        "/api/v1/search",
        Some(serde_json::json!({"query": "x", "limit": 1})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["code"], "auth_unavailable");
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn search_ask_closed_snapshot_restart_expiry_revoke() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let fx = boot_fixture(&base_url).await;

    // Search returns authorized hit + locator.
    let (status, body) = json_request(
        fx.app.clone(),
        "POST",
        "/api/v1/search",
        Some(serde_json::json!({
            "query": "doi soat",
            "collectionIds": [fx.collection],
            "mode": { "type": "current" },
            "limit": 5
        })),
        Some(&fx.access),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!body["hits"].as_array().unwrap().is_empty());
    assert!(body["hits"][0]["locator"]["spanStart"].as_u64().is_some());
    assert!(body["hits"][0].get("body").is_none());

    // Ask limit > 32 rejected pre-retrieval.
    let (status, body) = json_request(
        fx.app.clone(),
        "POST",
        "/api/v1/ask",
        Some(serde_json::json!({
            "question": "doi soat",
            "collectionIds": [fx.collection],
            "limit": 33,
            "useProvider": false
        })),
        Some(&fx.access),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "validation_failed");

    // Closed snapshot stream: contiguous sequences, metadata→token*→close.
    let (status, headers, sse_body) = sse_request(
        fx.app.clone(),
        "POST",
        "/api/v1/ask/stream",
        Some(serde_json::json!({
            "question": "doi soat giao dich",
            "collectionIds": [fx.collection],
            "limit": 5,
            "useProvider": false
        })),
        Some(&fx.access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{sse_body}");
    assert!(headers.content_type.contains("text/event-stream"));
    assert!(headers.cache_control.contains("no-store"));
    let envelopes = parse_sse_envelopes(&sse_body);
    assert_eq!(envelopes[0]["event"], "metadata");
    assert_eq!(envelopes[0]["sequence"], 1);
    assert_contiguous_sequences(&envelopes);
    assert!(envelopes
        .last()
        .and_then(|e| e["event"].as_str())
        .is_some_and(|e| e == "close" || e == "error"));
    let stream_id = Uuid::parse_str(&headers.request_id).expect("stream id");

    // Durable closed: no open rows.
    let owner_ctx = resolve(&fx.pool, fx.org, fx.user).await;
    let open_count: i64 = with_org_txn(&fx.pool, &owner_ctx, {
        let ctx = owner_ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT count(*)::bigint FROM sse_stream_requests
                         WHERE org_id = $1 AND status = 'open'",
                        &[&ctx.org_id()],
                    )
                    .await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .expect("open count");
    assert_eq!(open_count, 0);

    // Strict Last-Event-ID: after 1 has no seq<=1; missing header replays all.
    let (status, _h, resume_body) = sse_request(
        fx.app.clone(),
        "GET",
        &format!("/api/v1/events/{stream_id}"),
        None,
        Some(&fx.access),
        Some("1"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resume_body}");
    let resumed = parse_sse_envelopes(&resume_body);
    assert!(resumed.iter().all(|e| e["sequence"].as_u64().unwrap() > 1));
    assert_contiguous_sequences(&resumed);

    // Invalid Last-Event-ID → 400.
    let (status, _h, body) = sse_request(
        fx.app.clone(),
        "GET",
        &format!("/api/v1/events/{stream_id}"),
        None,
        Some(&fx.access),
        Some("01"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    // Reconstruct router/AppState to simulate worker/server restart from DB.
    let restarted = build_app(&fx.database_url, &fx.pool, &fx.auth);
    let (status, _h, restart_body) = sse_request(
        restarted,
        "GET",
        &format!("/api/v1/events/{stream_id}"),
        None,
        Some(&fx.access),
        Some("1"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{restart_body}");
    let from_db = parse_sse_envelopes(&restart_body);
    assert_eq!(
        from_db
            .iter()
            .map(|e| e["sequence"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        resumed
            .iter()
            .map(|e| e["sequence"].as_u64().unwrap())
            .collect::<Vec<_>>()
    );

    // IDOR → 404.
    let (status, _h, body) = sse_request(
        fx.app.clone(),
        "GET",
        &format!("/api/v1/events/{stream_id}"),
        None,
        Some(&fx.other_access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    // Expiry cleanup → 410 then gone.
    with_org_txn(&fx.pool, &owner_ctx, {
        let ctx = owner_ctx.clone();
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "UPDATE sse_stream_requests
                     SET created_at = clock_timestamp() - interval '2 hours',
                         expires_at = clock_timestamp() - interval '1 second'
                     WHERE org_id = $1 AND id = $2",
                    &[&ctx.org_id(), &stream_id],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("force expire");
    let (status, _h, body) = sse_request(
        fx.app.clone(),
        "GET",
        &format!("/api/v1/events/{stream_id}"),
        None,
        Some(&fx.access),
        Some("1"),
    )
    .await;
    assert_eq!(status, StatusCode::GONE, "{body}");
    let gone: Value = serde_json::from_str(&body).expect("gone json");
    assert_eq!(gone["code"], "stream_expired");

    // After cleanup, missing → 404.
    let (status, _h, body) = sse_request(
        fx.app.clone(),
        "GET",
        &format!("/api/v1/events/{stream_id}"),
        None,
        Some(&fx.access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    // Fresh stream for revoke / history / collection tests.
    let (status, headers, _) = sse_request(
        fx.app.clone(),
        "POST",
        "/api/v1/ask/stream",
        Some(serde_json::json!({
            "question": "doi soat giao dich",
            "collectionIds": [fx.collection],
            "limit": 5,
            "useProvider": false
        })),
        Some(&fx.access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let stream_id = Uuid::parse_str(&headers.request_id).expect("stream id");

    // Mark stream as history-scoped, then revoke qa.history → reconnect denied.
    with_org_txn(&fx.pool, &owner_ctx, {
        let ctx = owner_ctx.clone();
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "UPDATE sse_stream_requests
                     SET version_mode = 'history', requires_history = true
                     WHERE org_id = $1 AND id = $2",
                    &[&ctx.org_id(), &stream_id],
                )
                .await?;
                txn.execute(
                    "DELETE FROM role_permissions rp
                     USING permissions p, roles r
                     WHERE rp.org_id = $1
                       AND rp.role_id = r.id
                       AND r.org_id = $1
                       AND r.code = 'owner'
                       AND rp.permission_id = p.id
                       AND p.code = 'qa.history'",
                    &[&ctx.org_id()],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("revoke history");
    let (status, _h, body) = sse_request(
        fx.app.clone(),
        "GET",
        &format!("/api/v1/events/{stream_id}"),
        None,
        Some(&fx.access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    // Restore history; revoke collection by making it private under another member.
    with_org_txn(&fx.pool, &owner_ctx, {
        let ctx = owner_ctx.clone();
        move |txn| {
            Box::pin(async move {
                let perm: Uuid = txn
                    .query_one("SELECT id FROM permissions WHERE code = 'qa.history'", &[])
                    .await?
                    .get(0);
                let role: Uuid = txn
                    .query_one(
                        "SELECT id FROM roles WHERE org_id = $1 AND code = 'owner'",
                        &[&ctx.org_id()],
                    )
                    .await?
                    .get(0);
                txn.execute(
                    "INSERT INTO role_permissions (org_id, role_id, permission_id)
                     VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
                    &[&ctx.org_id(), &role, &perm],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("restore history");
    let sibling = Uuid::new_v4();
    seed_member(
        &fx.pool,
        fx.org,
        sibling,
        &format!("sib-{}@example.com", sibling.simple()),
        "correct-horse-battery",
        "viewer",
        &["qa.query"],
    )
    .await;
    with_org_txn(&fx.pool, &owner_ctx, {
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "UPDATE collections
                     SET visibility = 'private', owner_user_id = $3
                     WHERE org_id = $1 AND id = $2",
                    &[&fx.org, &fx.collection, &sibling],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("reassign collection");
    let (status, _h, body) = sse_request(
        fx.app.clone(),
        "GET",
        &format!("/api/v1/events/{stream_id}"),
        None,
        Some(&fx.access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    // Restore collection for session revoke test on a new stream.
    with_org_txn(&fx.pool, &owner_ctx, {
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "UPDATE collections
                     SET visibility = 'org', owner_user_id = $3
                     WHERE org_id = $1 AND id = $2",
                    &[&fx.org, &fx.collection, &fx.user],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("restore collection");

    let (status, headers, _) = sse_request(
        fx.app.clone(),
        "POST",
        "/api/v1/ask/stream",
        Some(serde_json::json!({
            "question": "doi soat",
            "collectionIds": [fx.collection],
            "limit": 5,
            "useProvider": false
        })),
        Some(&fx.access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let stream_id = Uuid::parse_str(&headers.request_id).unwrap();

    // Refresh family revoke → reconnect denied (app-level).
    session::logout_session(&fx.pool, &fx.refresh, "r05-logout")
        .await
        .expect("logout");
    let (status, _h, body) = sse_request(
        fx.app.clone(),
        "GET",
        &format!("/api/v1/events/{stream_id}"),
        None,
        Some(&fx.access),
        Some("1"),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");

    let _ = (
        fx.other_org,
        fx.other_user,
        fx.document,
        fx.version,
        sibling,
    );
    fx.ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn ask_stream_resume_tail_pin_expiry_family() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let fx = boot_fixture(&base_url).await;
    let owner_ctx = resolve(&fx.pool, fx.org, fx.user).await;

    let (status, headers, sse_body) = sse_request(
        fx.app.clone(),
        "POST",
        "/api/v1/ask/stream",
        Some(serde_json::json!({
            "question": "doi soat giao dich",
            "collectionIds": [fx.collection],
            "limit": 5,
            "useProvider": false
        })),
        Some(&fx.access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{sse_body}");
    let full = parse_sse_envelopes(&sse_body);
    assert!(full.len() >= 3, "{full:?}");
    let stream_id = Uuid::parse_str(&headers.request_id).expect("stream id");
    let after = full[1]["sequence"].as_u64().expect("seq");
    let expected_tail: Vec<Value> = full
        .iter()
        .filter(|e| e["sequence"].as_u64().unwrap() > after)
        .cloned()
        .collect();

    // Exact resumed tail equality after partial consume (Last-Event-ID).
    let (status, _h, resume_body) = sse_request(
        fx.app.clone(),
        "GET",
        &format!("/api/v1/events/{stream_id}"),
        None,
        Some(&fx.access),
        Some(&after.to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resume_body}");
    let resumed = parse_sse_envelopes(&resume_body);
    assert_eq!(resumed, expected_tail);

    // Drop-then-reconnect: body cancel is HTTP-only; DB tail stays identical.
    let (status, _h, again) = sse_request(
        fx.app.clone(),
        "GET",
        &format!("/api/v1/events/{stream_id}"),
        None,
        Some(&fx.access),
        Some(&after.to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(parse_sse_envelopes(&again), expected_tail);

    // Tombstoned cited pin → Deleted (404) on reconnect probe.
    with_org_txn(&fx.pool, &owner_ctx, {
        let ctx = owner_ctx.clone();
        let document = fx.document;
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "UPDATE documents
                     SET state = 'tombstoned',
                         deleted_at = COALESCE(deleted_at, clock_timestamp())
                     WHERE org_id = $1 AND id = $2",
                    &[&ctx.org_id(), &document],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("tombstone cited doc");
    let (status, _h, body) = sse_request(
        fx.app.clone(),
        "GET",
        &format!("/api/v1/events/{stream_id}"),
        None,
        Some(&fx.access),
        Some("1"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    // Fresh stream + expired refresh family (not revoked) → Deny.
    with_org_txn(&fx.pool, &owner_ctx, {
        let ctx = owner_ctx.clone();
        let document = fx.document;
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "UPDATE documents
                     SET state = 'indexed', deleted_at = NULL
                     WHERE org_id = $1 AND id = $2",
                    &[&ctx.org_id(), &document],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("restore doc");

    let (status, headers, _) = sse_request(
        fx.app.clone(),
        "POST",
        "/api/v1/ask/stream",
        Some(serde_json::json!({
            "question": "doi soat",
            "collectionIds": [fx.collection],
            "limit": 5,
            "useProvider": false
        })),
        Some(&fx.access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let stream_id = Uuid::parse_str(&headers.request_id).unwrap();

    with_org_txn(&fx.pool, &owner_ctx, {
        let ctx = owner_ctx.clone();
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "UPDATE refresh_tokens
                     SET expires_at = clock_timestamp() - interval '1 second'
                     WHERE org_id = $1 AND user_id = $2 AND revoked_at IS NULL",
                    &[&ctx.org_id(), &ctx.user_id()],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("expire refresh family");
    let (status, _h, body) = sse_request(
        fx.app.clone(),
        "GET",
        &format!("/api/v1/events/{stream_id}"),
        None,
        Some(&fx.access),
        Some("1"),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");

    // Expiry: first GET is 410 (not pre-deleted to 404); second GET is 404.
    let meta = AuthRequestMeta {
        request_id: "r05-relogin".into(),
    };
    let keys = JwtKeys::from_auth(&fx.auth).expect("jwt keys");
    let provider = PasswordAuthProvider::new(fx.pool.clone(), fx.auth.clone(), keys);
    let relogin = provider
        .login_password(
            &format!("owner-{}@example.com", fx.user.simple()),
            "correct-horse-battery",
            &meta,
        )
        .await
        .expect("relogin");
    let access = relogin.tokens.access_token.expose().to_string();

    let (status, headers, _) = sse_request(
        fx.app.clone(),
        "POST",
        "/api/v1/ask/stream",
        Some(serde_json::json!({
            "question": "doi soat",
            "collectionIds": [fx.collection],
            "limit": 5,
            "useProvider": false
        })),
        Some(&access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let stream_id = Uuid::parse_str(&headers.request_id).unwrap();
    with_org_txn(&fx.pool, &owner_ctx, {
        let ctx = owner_ctx.clone();
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "UPDATE sse_stream_requests
                     SET created_at = clock_timestamp() - interval '2 hours',
                         expires_at = clock_timestamp() - interval '1 second'
                     WHERE org_id = $1 AND id = $2",
                    &[&ctx.org_id(), &stream_id],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("force expire");
    let (status, _h, body) = sse_request(
        fx.app.clone(),
        "GET",
        &format!("/api/v1/events/{stream_id}"),
        None,
        Some(&access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::GONE, "{body}");
    let gone: Value = serde_json::from_str(&body).expect("gone json");
    assert_eq!(gone["code"], "stream_expired");
    let (status, _h, body) = sse_request(
        fx.app.clone(),
        "GET",
        &format!("/api/v1/events/{stream_id}"),
        None,
        Some(&access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    fx.ephemeral.drop().await;
}
