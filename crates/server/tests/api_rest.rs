//! HTTP contract tests for P1B-R04 collection/document/job REST API.
//!
//! Live PostgreSQL tests are ignored unless explicitly run with
//! `MARKHAND_TEST_DATABASE_URL` set (same pattern as auth/repositories).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use deadpool_postgres::Pool;
use fileconv_server::api::{decode_cursor, encode_cursor, CreatedAtIdCursor, PageParams};
use fileconv_server::auth::context::OrgContext;
use fileconv_server::auth::jwt::JwtKeys;
use fileconv_server::auth::provider::{AuthProvider, AuthRequestMeta, PasswordAuthProvider};
use fileconv_server::auth::session;
use fileconv_server::config::{
    Argon2Config, AuthConfig, JwtAlgorithm, RuntimeEndpoints, SecretString, ServerConfig,
};
use fileconv_server::database::apply_migrations;
use fileconv_server::db::collections::{self, NewCollection};
use fileconv_server::db::documents::{self, NewDocument};
use fileconv_server::db::models::{CollectionVisibility, JobType};
use fileconv_server::db::orgs;
use fileconv_server::db::pool::{create_pool, with_org_txn};
use fileconv_server::http::{router, AppState};
use fileconv_server::jobs::{self, EnqueueJob, JobPayload};
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
        let db_name = format!("markhand_r04_{}", Uuid::new_v4().simple());
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

struct Fixture {
    ephemeral: EphemeralDb,
    pool: Pool,
    org: Uuid,
    user: Uuid,
    other_org: Uuid,
    other_user: Uuid,
    collection: Uuid,
    other_collection: Uuid,
    document: Uuid,
    other_document: Uuid,
    version: Uuid,
    access: String,
    viewer_access: String,
    other_access: String,
    app: axum::Router,
}

async fn boot_fixture(base_url: &str) -> Fixture {
    let ephemeral = EphemeralDb::create(base_url).await;
    apply_migrations(&ephemeral.url)
        .await
        .expect("apply migrations");
    let pool = create_pool(&ephemeral.url).expect("pool");
    let auth = test_auth_config();
    let keys = JwtKeys::from_auth(&auth).expect("jwt keys");
    let provider = PasswordAuthProvider::new(pool.clone(), auth.clone(), keys.clone());
    let runtime = test_runtime(&ephemeral.url);
    let state = AppState::from_parts(
        runtime,
        pool.clone(),
        Some(PasswordAuthProvider::new(
            pool.clone(),
            auth.clone(),
            JwtKeys::from_auth(&auth).unwrap(),
        )),
    )
    .expect("app state");
    let app = router(state);

    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let viewer = Uuid::new_v4();
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
        &[
            "doc.upload",
            "doc.delete",
            "doc.publish",
            "qa.query",
            "member.manage",
        ],
    )
    .await;
    seed_member(
        &pool,
        org,
        viewer,
        &format!("viewer-{}@example.com", viewer.simple()),
        password,
        "viewer",
        &["qa.query"],
    )
    .await;
    seed_member(
        &pool,
        other_org,
        other_user,
        &format!("other-{}@example.com", other_user.simple()),
        password,
        "owner",
        &["doc.upload", "doc.delete", "doc.publish", "qa.query"],
    )
    .await;

    let owner_ctx = resolve(&pool, org, user).await;
    let other_ctx = resolve(&pool, other_org, other_user).await;
    let collection = seed_collection(&pool, &owner_ctx, "lib-a").await;
    let other_collection = seed_collection(&pool, &other_ctx, "lib-b").await;
    // Refresh contexts so allow-lists include the new org-visible collections.
    let owner_ctx = resolve(&pool, org, user).await;
    let other_ctx = resolve(&pool, other_org, other_user).await;

    let document = Uuid::new_v4();
    let other_document = Uuid::new_v4();
    let version = Uuid::new_v4();
    with_org_txn(&pool, &owner_ctx, {
        let ctx = owner_ctx.clone();
        move |txn| {
            Box::pin(async move {
                documents::insert(
                    txn,
                    &ctx,
                    NewDocument {
                        id: document,
                        collection_id: collection,
                        title: "Doc A",
                    },
                )
                .await?;
                let content_sha =
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
                let object_key = format!("orgs/{}/quarantine/{}/obj", ctx.org_id(), Uuid::new_v4());
                txn.execute(
                    "INSERT INTO document_versions (
                        id, org_id, document_id, version_number, publication_state, is_current,
                        content_sha256, original_object_key, created_by_user_id
                     ) VALUES (
                        $1, $2, $3, 1, 'published', true, $4, $5, $6
                     )",
                    &[
                        &version,
                        &ctx.org_id(),
                        &document,
                        &content_sha,
                        &object_key,
                        &ctx.user_id(),
                    ],
                )
                .await?;
                txn.execute(
                    "UPDATE documents
                     SET state = 'indexed', current_version_id = $2
                     WHERE id = $1",
                    &[&document, &version],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("seed document");

    with_org_txn(&pool, &other_ctx, {
        let ctx = other_ctx.clone();
        move |txn| {
            Box::pin(async move {
                documents::insert(
                    txn,
                    &ctx,
                    NewDocument {
                        id: other_document,
                        collection_id: other_collection,
                        title: "Doc B",
                    },
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("seed other document");

    // Ensure active index generation for reindex tests.
    with_org_txn(&pool, &owner_ctx, {
        let ctx = owner_ctx.clone();
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "INSERT INTO index_metadata (
                        org_id, collection_id, index_signature_sha256, chunking_version,
                        body_text_version, query_normalization_version, embedding_family,
                        embedding_revision, dimensions, normalized, runtime_path, generation,
                        is_active, state
                     ) VALUES (
                        $1, $2, $3, 'c1', 'b1', 'q1', 'family', 'rev', 8,                         true, 'local-hash', 1,
                        true, 'active'
                     )
                     ON CONFLICT DO NOTHING",
                    &[
                        &ctx.org_id(),
                        &collection,
                        &"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                    ],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("seed index metadata");

    let meta = AuthRequestMeta {
        request_id: "r04-login".into(),
    };
    let owner_session = provider
        .login_password(
            &format!("owner-{}@example.com", user.simple()),
            password,
            &meta,
        )
        .await
        .expect("owner login");
    let viewer_session = provider
        .login_password(
            &format!("viewer-{}@example.com", viewer.simple()),
            password,
            &meta,
        )
        .await
        .expect("viewer login");
    let other_session = provider
        .login_password(
            &format!("other-{}@example.com", other_user.simple()),
            password,
            &meta,
        )
        .await
        .expect("other login");

    Fixture {
        ephemeral,
        pool,
        org,
        user,
        other_org,
        other_user,
        collection,
        other_collection,
        document,
        other_document,
        version,
        access: owner_session.tokens.access_token.expose().to_string(),
        viewer_access: viewer_session.tokens.access_token.expose().to_string(),
        other_access: other_session.tokens.access_token.expose().to_string(),
        app,
    }
}

async fn resolve(pool: &Pool, org: Uuid, user: Uuid) -> OrgContext {
    fileconv_server::auth::permissions::resolve_org_context_in_txn(pool, org, user)
        .await
        .expect("resolve org context")
}

async fn grant_permission(pool: &Pool, org: Uuid, role: &str, code: &str) {
    let provisional = OrgContext::try_new(org, Uuid::new_v4(), [] as [&str; 0], []).unwrap();
    let role = role.to_string();
    let code = code.to_string();
    with_org_txn(pool, &provisional, {
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "INSERT INTO permissions (id, code, description)
                     VALUES ($1, $2, $2)
                     ON CONFLICT (code) DO NOTHING",
                    &[&Uuid::new_v4(), &code],
                )
                .await?;
                txn.execute(
                    "INSERT INTO role_permissions (org_id, role_id, permission_id)
                     SELECT $1, r.id, p.id
                     FROM roles r
                     CROSS JOIN permissions p
                     WHERE r.org_id = $1 AND r.code = $2 AND p.code = $3
                     ON CONFLICT DO NOTHING",
                    &[&org, &role, &code],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("grant permission");
}

async fn revoke_permission(pool: &Pool, org: Uuid, role: &str, code: &str) {
    let provisional = OrgContext::try_new(org, Uuid::new_v4(), [] as [&str; 0], []).unwrap();
    let role = role.to_string();
    let code = code.to_string();
    with_org_txn(pool, &provisional, {
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "DELETE FROM role_permissions
                     WHERE org_id = $1
                       AND role_id = (SELECT id FROM roles WHERE org_id = $1 AND code = $2)
                       AND permission_id = (SELECT id FROM permissions WHERE code = $3)",
                    &[&org, &role, &code],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("revoke permission");
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

async fn seed_collection(pool: &Pool, context: &OrgContext, slug: &str) -> Uuid {
    let id = Uuid::new_v4();
    let slug = slug.to_string();
    with_org_txn(pool, context, {
        let owned = context.clone();
        move |txn| {
            Box::pin(async move {
                collections::insert(
                    txn,
                    &owned,
                    NewCollection {
                        id,
                        name: &slug,
                        slug: &slug,
                        description: None,
                        visibility: CollectionVisibility::Org,
                    },
                )
                .await?;
                Ok(id)
            })
        }
    })
    .await
    .expect("seed collection")
}

async fn json_request(
    app: axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
    bearer: Option<&str>,
    idempotency: Option<&str>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = bearer {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    if let Some(key) = idempotency {
        builder = builder.header("idempotency-key", key);
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
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or(Value::String(String::from_utf8_lossy(&bytes).into_owned()))
    };
    (status, value)
}

fn assert_stable_error(body: &Value, code: &str) {
    assert_eq!(body["code"], code, "{body}");
    assert!(body["requestId"].as_str().is_some(), "{body}");
    let rendered = body.to_string();
    assert!(!rendered.contains("postgres"));
    assert!(!rendered.contains("tokio_postgres"));
    assert!(!rendered.contains("password"));
    assert!(!rendered.contains("secret"));
}

#[test]
fn hermetic_page_bounds_and_cursor_contract() {
    assert!(PageParams::new(0, None).is_err());
    assert!(PageParams::new(101, None).is_err());
    assert!(PageParams::new(1, None).is_ok());
    assert!(PageParams::new(100, None).is_ok());
    let cursor = CreatedAtIdCursor {
        created_at: chrono::Utc::now(),
        id: Uuid::new_v4(),
    };
    let encoded = encode_cursor(&cursor).unwrap();
    let decoded: CreatedAtIdCursor = decode_cursor(&encoded).unwrap();
    assert_eq!(decoded.id, cursor.id);
    assert!(PageParams::new(10, Some("!!!".into())).is_err());

    let limit_err = PageParams::from_query(Some(0), None, "r").unwrap_err();
    assert_eq!(limit_err.body().details.as_ref().unwrap()["field"], "limit");
    let cursor_err = PageParams::from_query(Some(10), Some("!!!".into()), "r").unwrap_err();
    assert_eq!(
        cursor_err.body().details.as_ref().unwrap()["field"],
        "cursor"
    );
}

async fn refresh_owner_access(fx: &Fixture) -> String {
    let provider = PasswordAuthProvider::new(
        fx.pool.clone(),
        test_auth_config(),
        JwtKeys::from_auth(&test_auth_config()).unwrap(),
    );
    provider
        .login_password(
            &format!("owner-{}@example.com", fx.user.simple()),
            "correct-horse-battery",
            &AuthRequestMeta {
                request_id: "r04-refresh".into(),
            },
        )
        .await
        .unwrap()
        .tokens
        .access_token
        .expose()
        .to_string()
}

async fn seed_indexed_document(
    pool: &Pool,
    ctx: &OrgContext,
    collection_id: Uuid,
    title: &str,
) -> (Uuid, Uuid) {
    let document = Uuid::new_v4();
    let version = Uuid::new_v4();
    let title = title.to_string();
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                documents::insert(
                    txn,
                    &ctx,
                    NewDocument {
                        id: document,
                        collection_id,
                        title: &title,
                    },
                )
                .await?;
                let content_sha =
                    "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
                let object_key = format!("orgs/{}/quarantine/{}/obj", ctx.org_id(), Uuid::new_v4());
                txn.execute(
                    "INSERT INTO document_versions (
                        id, org_id, document_id, version_number, publication_state, is_current,
                        content_sha256, original_object_key, created_by_user_id
                     ) VALUES (
                        $1, $2, $3, 1, 'published', true, $4, $5, $6
                     )",
                    &[
                        &version,
                        &ctx.org_id(),
                        &document,
                        &content_sha,
                        &object_key,
                        &ctx.user_id(),
                    ],
                )
                .await?;
                txn.execute(
                    "UPDATE documents
                     SET state = 'indexed', current_version_id = $2
                     WHERE id = $1",
                    &[&document, &version],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("seed indexed document");
    (document, version)
}

async fn seed_draft_version(
    pool: &Pool,
    ctx: &OrgContext,
    document_id: Uuid,
    parent_version_id: Uuid,
    version_number: i32,
) -> Uuid {
    let version = Uuid::new_v4();
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let content_sha =
                    "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
                let object_key = format!("orgs/{}/quarantine/{}/obj", ctx.org_id(), Uuid::new_v4());
                txn.execute(
                    "INSERT INTO document_versions (
                        id, org_id, document_id, version_number, parent_version_id,
                        publication_state, is_current, content_sha256, original_object_key,
                        effective_from, created_by_user_id
                     ) VALUES (
                        $1, $2, $3, $4, $5, 'draft', false, $6, $7,
                        clock_timestamp() - interval '1 hour', $8
                     )",
                    &[
                        &version,
                        &ctx.org_id(),
                        &document_id,
                        &version_number,
                        &parent_version_id,
                        &content_sha,
                        &object_key,
                        &ctx.user_id(),
                    ],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("seed draft version");
    version
}

struct SeededConflict {
    conflict_id: Uuid,
    claim_a: Uuid,
    claim_b: Uuid,
    version_a: Uuid,
    version_b: Uuid,
    evidence_ids: Vec<Uuid>,
}

async fn seed_conflict_bundle(
    pool: &Pool,
    ctx: &OrgContext,
    collection_a: Uuid,
    collection_b: Uuid,
) -> SeededConflict {
    let (doc_a, version_a) = seed_indexed_document(pool, ctx, collection_a, "conflict-a").await;
    let (doc_b, version_b) = seed_indexed_document(pool, ctx, collection_b, "conflict-b").await;
    let claim_a = Uuid::new_v4();
    let claim_b = Uuid::new_v4();
    let (low, high, low_doc, high_doc, low_ver, high_ver) = if claim_a < claim_b {
        (claim_a, claim_b, doc_a, doc_b, version_a, version_b)
    } else {
        (claim_b, claim_a, doc_b, doc_a, version_b, version_a)
    };
    let conflict_id = Uuid::new_v4();
    let evidence_ids = vec![Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()];
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        let evidence_ids = evidence_ids.clone();
        move |txn| {
            Box::pin(async move {
                for (claim_id, document_id, version_id, key, amount) in [
                    (low, low_doc, low_ver, "k-a", "1"),
                    (high, high_doc, high_ver, "k-b", "2"),
                ] {
                    txn.execute(
                        &format!(
                            "INSERT INTO claims (
                                id, org_id, document_id, version_id, claim_key, subject, predicate,
                                value_type, value_money, unit, scope, effective_from, citation_quote
                             ) VALUES (
                                $1,$2,$3,$4,$5,'subject','predicate','money',{amount},'VND','', now(), $6
                             )"
                        ),
                        &[
                            &claim_id,
                            &ctx.org_id(),
                            &document_id,
                            &version_id,
                            &key,
                            &format!("quote-{key}"),
                        ],
                    )
                    .await?;
                }
                txn.execute(
                    "INSERT INTO conflicts (
                        id, org_id, status, severity, conflict_type, claim_a_id, claim_b_id,
                        first_detected_version_id
                     ) VALUES ($1,$2,'open','warning','numeric',$3,$4,$5)",
                    &[&conflict_id, &ctx.org_id(), &low, &high, &low_ver],
                )
                .await?;
                for (idx, evidence_id) in evidence_ids.iter().enumerate() {
                    let claim_id = if idx == 1 { high } else { low };
                    let role = match idx {
                        0 => "left",
                        1 => "right",
                        _ => "supporting",
                    };
                    txn.execute(
                        "INSERT INTO conflict_evidence (
                            id, org_id, conflict_id, claim_id, evidence_role, citation_quote
                         ) VALUES ($1,$2,$3,$4,$5,$6)",
                        &[
                            evidence_id,
                            &ctx.org_id(),
                            &conflict_id,
                            &claim_id,
                            &role,
                            &format!("evidence-quote-{idx}"),
                        ],
                    )
                    .await?;
                }
                Ok(())
            })
        }
    })
    .await
    .expect("seed conflict bundle");
    SeededConflict {
        conflict_id,
        claim_a: low,
        claim_b: high,
        version_a: low_ver,
        version_b: high_ver,
        evidence_ids,
    }
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn rest_collections_documents_jobs_contract() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let fx = boot_fixture(&base_url).await;

    // Success: list/get collections (pageInfo convention).
    let (status, body) = json_request(
        fx.app.clone(),
        "GET",
        "/api/v1/collections?limit=10",
        None,
        Some(&fx.access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["items"].as_array().unwrap().iter().any(|item| {
        item["id"] == fx.collection.to_string() && item["requestId"].as_str().is_some()
    }));
    assert_eq!(body["pageInfo"]["hasMore"], false);
    assert!(body.get("page").is_none());

    let (status, body) = json_request(
        fx.app.clone(),
        "GET",
        &format!("/api/v1/collections/{}", fx.collection),
        None,
        Some(&fx.access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["id"], fx.collection.to_string());

    // Viewer denied on collection write.
    let (status, body) = json_request(
        fx.app.clone(),
        "POST",
        "/api/v1/collections",
        Some(serde_json::json!({
            "name": "Viewer Library",
            "slug": "viewer-library",
            "visibility": "org"
        })),
        Some(&fx.viewer_access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_stable_error(&body, "permission_denied");

    // Create + update collection (owner with doc.upload).
    let (status, created) = json_request(
        fx.app.clone(),
        "POST",
        "/api/v1/collections",
        Some(serde_json::json!({
            "name": "New Library",
            "slug": "new-library",
            "visibility": "org"
        })),
        Some(&fx.access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let created_id = created["id"].as_str().unwrap().to_string();

    // Collection validation and uniqueness conflicts use stable API errors.
    let (status, body) = json_request(
        fx.app.clone(),
        "POST",
        "/api/v1/collections",
        Some(serde_json::json!({
            "name": "Duplicate Slug",
            "slug": "new-library",
            "visibility": "org"
        })),
        Some(&fx.access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_stable_error(&body, "collection_conflict");

    let (status, body) = json_request(
        fx.app.clone(),
        "POST",
        "/api/v1/collections",
        Some(serde_json::json!({
            "name": "Invalid Slug",
            "slug": "Produces_Invalid_Slug"
        })),
        Some(&fx.access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_stable_error(&body, "validation_failed");

    let access = refresh_owner_access(&fx).await;

    let (status, updated) = json_request(
        fx.app.clone(),
        "PATCH",
        &format!("/api/v1/collections/{created_id}"),
        Some(serde_json::json!({ "name": "Renamed Library" })),
        Some(&access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["name"], "Renamed Library");

    let (status, body) = json_request(
        fx.app.clone(),
        "PATCH",
        &format!("/api/v1/collections/{created_id}"),
        Some(serde_json::json!({})),
        Some(&access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_stable_error(&body, "validation_failed");

    let (status, body) = json_request(
        fx.app.clone(),
        "PATCH",
        &format!("/api/v1/collections/{created_id}"),
        Some(serde_json::json!({ "name": "Viewer Rename" })),
        Some(&fx.viewer_access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_stable_error(&body, "permission_denied");

    // Multipage keyset traversal via pageInfo.nextCursor.
    let owner_ctx = resolve(&fx.pool, fx.org, fx.user).await;
    for idx in 0..3 {
        let _ = seed_collection(&fx.pool, &owner_ctx, &format!("page-{idx}")).await;
    }
    let access = refresh_owner_access(&fx).await;
    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let uri = match &cursor {
            Some(value) => format!("/api/v1/collections?limit=1&cursor={value}"),
            None => "/api/v1/collections?limit=1".to_string(),
        };
        let (status, page) =
            json_request(fx.app.clone(), "GET", &uri, None, Some(&access), None).await;
        assert_eq!(status, StatusCode::OK, "{page}");
        assert!(page.get("page").is_none());
        let items = page["items"].as_array().unwrap();
        assert_eq!(items.len(), 1, "{page}");
        seen.push(items[0]["id"].as_str().unwrap().to_string());
        if page["pageInfo"]["hasMore"] == false {
            assert!(
                page["pageInfo"].get("nextCursor").is_none()
                    || page["pageInfo"]["nextCursor"].is_null()
            );
            break;
        }
        cursor = Some(
            page["pageInfo"]["nextCursor"]
                .as_str()
                .expect("nextCursor")
                .to_string(),
        );
        assert!(seen.len() < 20, "pagination failed to terminate");
    }
    assert!(seen.len() >= 4);
    let mut unique = seen.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), seen.len(), "cursor pages must not repeat");

    // Documents list/get.
    let (status, body) = json_request(
        fx.app.clone(),
        "GET",
        &format!("/api/v1/documents?collectionId={}&limit=10", fx.collection),
        None,
        Some(&access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["id"] == fx.document.to_string()));
    assert!(body.get("pageInfo").is_some());

    let (status, body) = json_request(
        fx.app.clone(),
        "GET",
        &format!("/api/v1/documents/{}", fx.document),
        None,
        Some(&access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["state"], "indexed");

    // Version list/get/diff + immutability (no mutation fields).
    let (status, body) = json_request(
        fx.app.clone(),
        "GET",
        &format!("/api/v1/documents/{}/versions", fx.document),
        None,
        Some(&access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["items"][0]["id"], fx.version.to_string());
    assert!(body["items"][0].get("originalObjectKey").is_none());
    assert!(body["items"][0].get("markdownObjectKey").is_none());

    let (status, body) = json_request(
        fx.app.clone(),
        "GET",
        &format!("/api/v1/documents/{}/versions/{}", fx.document, fx.version),
        None,
        Some(&access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["id"], fx.version.to_string());
    assert_eq!(body["documentId"], fx.document.to_string());
    assert!(body.get("originalObjectKey").is_none());

    let (status, body) = json_request(
        fx.app.clone(),
        "GET",
        &format!(
            "/api/v1/documents/{}/versions/{}/diff/{}",
            fx.document, fx.version, fx.version
        ),
        None,
        Some(&access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["contentSha256Changed"], false);

    // Publish: draft→published/current; already-current idempotent; superseded → 4xx.
    let draft_version = seed_draft_version(&fx.pool, &owner_ctx, fx.document, fx.version, 2).await;
    let (status, body) = json_request(
        fx.app.clone(),
        "POST",
        &format!(
            "/api/v1/documents/{}/versions/{}/publish",
            fx.document, draft_version
        ),
        Some(serde_json::json!({})),
        Some(&fx.viewer_access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_stable_error(&body, "permission_denied");

    let (status, published) = json_request(
        fx.app.clone(),
        "POST",
        &format!(
            "/api/v1/documents/{}/versions/{}/publish",
            fx.document, draft_version
        ),
        Some(serde_json::json!({})),
        Some(&access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{published}");
    assert_eq!(published["versionId"], draft_version.to_string());
    assert!(published["jobId"].as_str().is_some());
    let (status, body) = json_request(
        fx.app.clone(),
        "GET",
        &format!("/api/v1/documents/{}", fx.document),
        None,
        Some(&access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["currentVersionId"], draft_version.to_string());

    // Already-current publish is idempotent.
    let (status, again) = json_request(
        fx.app.clone(),
        "POST",
        &format!(
            "/api/v1/documents/{}/versions/{}/publish",
            fx.document, draft_version
        ),
        Some(serde_json::json!({})),
        Some(&access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["versionId"], draft_version.to_string());

    // Superseded: prior published version that is no longer current.
    let (status, body) = json_request(
        fx.app.clone(),
        "POST",
        &format!(
            "/api/v1/documents/{}/versions/{}/publish",
            fx.document, fx.version
        ),
        Some(serde_json::json!({})),
        Some(&access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_stable_error(&body, "version_superseded");

    // Without qa.history, superseded version metadata is omitted from the list.
    let (status, body) = json_request(
        fx.app.clone(),
        "GET",
        &format!("/api/v1/documents/{}/versions", fx.document),
        None,
        Some(&fx.viewer_access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let viewer_ids: Vec<String> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(viewer_ids, vec![draft_version.to_string()]);
    assert!(!viewer_ids.contains(&fx.version.to_string()));

    let (status, body) = json_request(
        fx.app.clone(),
        "GET",
        &format!("/api/v1/documents/{}/versions", fx.document),
        None,
        Some(&access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let owner_ids: Vec<String> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        owner_ids,
        vec![draft_version.to_string()],
        "owner fixture lacks qa.history so superseded rows stay hidden"
    );

    grant_permission(&fx.pool, fx.org, "owner", "qa.history").await;
    let history_access = refresh_owner_access(&fx).await;
    let (status, body) = json_request(
        fx.app.clone(),
        "GET",
        &format!("/api/v1/documents/{}/versions", fx.document),
        None,
        Some(&history_access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let history_ids: Vec<String> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_string())
        .collect();
    assert!(history_ids.contains(&fx.version.to_string()));
    assert!(history_ids.contains(&draft_version.to_string()));

    // Identical Idempotency-Key replay; mismatched document reuse conflicts.
    let (status, first) = json_request(
        fx.app.clone(),
        "POST",
        &format!("/api/v1/documents/{}/reindex", fx.document),
        Some(serde_json::json!({})),
        Some(&access),
        Some("reindex-key-1"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let job_id = first["jobId"].as_str().unwrap().to_string();
    let (status, second) = json_request(
        fx.app.clone(),
        "POST",
        &format!("/api/v1/documents/{}/reindex", fx.document),
        Some(serde_json::json!({})),
        Some(&access),
        Some("reindex-key-1"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{second}");
    assert_eq!(second["jobId"], job_id);
    assert_eq!(second["created"], first["created"]);
    assert_eq!(second["versionId"], first["versionId"]);

    let (doc2, _) =
        seed_indexed_document(&fx.pool, &owner_ctx, fx.collection, "reindex-peer").await;
    // Ensure index metadata exists for peer (same collection already seeded).
    let (status, body) = json_request(
        fx.app.clone(),
        "POST",
        &format!("/api/v1/documents/{doc2}/reindex"),
        Some(serde_json::json!({})),
        Some(&access),
        Some("reindex-key-1"),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_stable_error(&body, "idempotency_key_conflict");

    // Job get/list.
    let (status, body) = json_request(
        fx.app.clone(),
        "GET",
        &format!("/api/v1/jobs/{job_id}"),
        None,
        Some(&access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["jobType"], "index");
    assert!(body.get("leaseOwner").is_none());
    assert!(body.get("payload").is_none());

    let (status, body) = json_request(
        fx.app.clone(),
        "GET",
        &format!("/api/v1/jobs?documentId={}&limit=10", fx.document),
        None,
        Some(&access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!body["items"].as_array().unwrap().is_empty());
    assert!(body.get("pageInfo").is_some());

    // Malformed pagination / body envelope.
    let (status, body) = json_request(
        fx.app.clone(),
        "GET",
        "/api/v1/documents?limit=0",
        None,
        Some(&access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_stable_error(&body, "validation_failed");

    let (status, body) = json_request(
        fx.app.clone(),
        "GET",
        "/api/v1/documents?limit=101",
        None,
        Some(&access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_stable_error(&body, "validation_failed");

    let (status, body) = json_request(
        fx.app.clone(),
        "GET",
        "/api/v1/documents?cursor=!!!",
        None,
        Some(&access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_stable_error(&body, "validation_failed");
    assert_eq!(body["details"]["field"], "cursor");

    let (status, body) = json_request(
        fx.app.clone(),
        "GET",
        "/api/v1/documents?limit=0&cursor=!!!",
        None,
        Some(&access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["details"]["field"], "limit");

    // Canonical JSON 404/405 (no MinIO dependency).
    let (status, body) = json_request(
        fx.app.clone(),
        "GET",
        "/api/v1/no-such-route",
        None,
        Some(&access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_stable_error(&body, "not_found");
    let (status, body) = json_request(
        fx.app.clone(),
        "DELETE",
        "/api/v1/collections",
        None,
        Some(&access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED, "{body}");
    assert_stable_error(&body, "method_not_allowed");

    let (status, body) = json_request(
        fx.app.clone(),
        "POST",
        "/api/v1/collections",
        Some(serde_json::json!({ "name": "", "slug": "bad" })),
        Some(&access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_stable_error(&body, "validation_failed");

    // Malformed JSON body → canonical ApiError envelope.
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/collections")
        .header("authorization", format!("Bearer {access}"))
        .header("content-type", "application/json")
        .body(Body::from("{not-json"))
        .unwrap();
    let response = fx.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_stable_error(&body, "validation_failed");

    // Malformed UUID path → 400 canonical envelope.
    let (status, body) = json_request(
        fx.app.clone(),
        "GET",
        "/api/v1/documents/not-a-uuid",
        None,
        Some(&access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_stable_error(&body, "validation_failed");

    // Cross-org / IDOR → 404.
    let (status, body) = json_request(
        fx.app.clone(),
        "GET",
        &format!("/api/v1/documents/{}", fx.other_document),
        None,
        Some(&access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_stable_error(&body, "not_found");

    let (status, body) = json_request(
        fx.app.clone(),
        "GET",
        &format!("/api/v1/collections/{}", fx.other_collection),
        None,
        Some(&access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_stable_error(&body, "not_found");

    // Missing permission (viewer cannot delete / reindex).
    let (status, body) = json_request(
        fx.app.clone(),
        "DELETE",
        &format!("/api/v1/documents/{}", fx.document),
        None,
        Some(&fx.viewer_access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_stable_error(&body, "permission_denied");

    let (status, body) = json_request(
        fx.app.clone(),
        "POST",
        &format!("/api/v1/documents/{}/reindex", fx.document),
        Some(serde_json::json!({})),
        Some(&fx.viewer_access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_stable_error(&body, "permission_denied");

    // Conflict detail / triage / evidence authorization + bounded evidence pages.
    let private_id = Uuid::new_v4();
    with_org_txn(&fx.pool, &owner_ctx, {
        let owned = owner_ctx.clone();
        move |txn| {
            Box::pin(async move {
                collections::insert(
                    txn,
                    &owned,
                    NewCollection {
                        id: private_id,
                        name: "secret",
                        slug: "secret-lib",
                        description: None,
                        visibility: CollectionVisibility::Private,
                    },
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("seed private collection");
    let owner_ctx = resolve(&fx.pool, fx.org, fx.user).await;
    let conflict = seed_conflict_bundle(&fx.pool, &owner_ctx, fx.collection, private_id).await;

    let (status, body) = json_request(
        fx.app.clone(),
        "GET",
        "/api/v1/conflicts?status=open&limit=1",
        None,
        Some(&access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    assert_eq!(body["items"][0]["id"], conflict.conflict_id.to_string());
    assert_eq!(body["items"][0]["status"], "open");
    assert!(body.get("pageInfo").is_some());

    let (status, body) = json_request(
        fx.app.clone(),
        "GET",
        "/api/v1/conflicts?status=not-a-status",
        None,
        Some(&access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_stable_error(&body, "validation_failed");

    let (status, body) = json_request(
        fx.app.clone(),
        "GET",
        &format!("/api/v1/conflicts/{}", conflict.conflict_id),
        None,
        Some(&access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["id"], conflict.conflict_id.to_string());

    // Viewer lacks private collection → conflict hidden (no quote leak via detail).
    let (status, body) = json_request(
        fx.app.clone(),
        "GET",
        &format!("/api/v1/conflicts/{}", conflict.conflict_id),
        None,
        Some(&fx.viewer_access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_stable_error(&body, "not_found");
    assert!(!body.to_string().contains("evidence-quote"));

    let (status, body) = json_request(
        fx.app.clone(),
        "GET",
        &format!(
            "/api/v1/conflicts/{}/evidence?limit=1",
            conflict.conflict_id
        ),
        None,
        Some(&fx.viewer_access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_stable_error(&body, "not_found");
    assert!(!body.to_string().contains("evidence-quote"));

    let (status, page1) = json_request(
        fx.app.clone(),
        "GET",
        &format!(
            "/api/v1/conflicts/{}/evidence?limit=1",
            conflict.conflict_id
        ),
        None,
        Some(&access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page1}");
    assert_eq!(page1["items"].as_array().unwrap().len(), 1);
    assert_eq!(page1["pageInfo"]["hasMore"], true);
    let quote = page1["items"][0]["citationQuote"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(quote.starts_with("evidence-quote-"));
    let cursor = page1["pageInfo"]["nextCursor"].as_str().unwrap();
    let (status, page2) = json_request(
        fx.app.clone(),
        "GET",
        &format!(
            "/api/v1/conflicts/{}/evidence?limit=1&cursor={cursor}",
            conflict.conflict_id
        ),
        None,
        Some(&access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page2}");
    assert_eq!(page2["items"].as_array().unwrap().len(), 1);
    assert_ne!(
        page2["items"][0]["id"], page1["items"][0]["id"],
        "evidence keyset must advance"
    );

    // Supersede claim-pinned versions → evidence quotes require qa.history.
    with_org_txn(&fx.pool, &owner_ctx, {
        let owned = owner_ctx.clone();
        let version_a = conflict.version_a;
        let version_b = conflict.version_b;
        move |txn| {
            Box::pin(async move {
                for version_id in [version_a, version_b] {
                    let document_id: Uuid = txn
                        .query_one(
                            "SELECT document_id FROM document_versions
                             WHERE org_id = $1 AND id = $2",
                            &[&owned.org_id(), &version_id],
                        )
                        .await?
                        .get(0);
                    let draft_id = Uuid::new_v4();
                    txn.execute(
                        "INSERT INTO document_versions (
                            id, org_id, document_id, version_number, parent_version_id,
                            publication_state, is_current, content_sha256, original_object_key,
                            effective_from, created_by_user_id
                         )
                         SELECT $1, org_id, document_id, version_number + 1, id,
                                'draft', false,
                                'eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee',
                                original_object_key, clock_timestamp() - interval '1 hour', $3
                         FROM document_versions
                         WHERE org_id = $2 AND id = $4",
                        &[&draft_id, &owned.org_id(), &owned.user_id(), &version_id],
                    )
                    .await?;
                    txn.query_one(
                        "SELECT markhand_publish_document_version($1, $2, $3)",
                        &[&owned.org_id(), &document_id, &draft_id],
                    )
                    .await?;
                }
                Ok(())
            })
        }
    })
    .await
    .expect("supersede conflict claim versions");

    revoke_permission(&fx.pool, fx.org, "owner", "qa.history").await;
    let no_history_access = refresh_owner_access(&fx).await;
    let (status, body) = json_request(
        fx.app.clone(),
        "GET",
        &format!(
            "/api/v1/conflicts/{}/evidence?limit=10",
            conflict.conflict_id
        ),
        None,
        Some(&no_history_access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["items"].as_array().unwrap().is_empty(),
        "superseded claim evidence must not leak without qa.history: {body}"
    );

    grant_permission(&fx.pool, fx.org, "owner", "qa.history").await;
    let with_history_access = refresh_owner_access(&fx).await;
    let (status, body) = json_request(
        fx.app.clone(),
        "GET",
        &format!(
            "/api/v1/conflicts/{}/evidence?limit=10",
            conflict.conflict_id
        ),
        None,
        Some(&with_history_access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["items"].as_array().unwrap().len(),
        3,
        "qa.history restores superseded claim evidence: {body}"
    );

    // Triage requires publish permission and a terminal status.
    let (status, body) = json_request(
        fx.app.clone(),
        "POST",
        &format!("/api/v1/conflicts/{}/triage", conflict.conflict_id),
        Some(serde_json::json!({ "status": "resolved" })),
        Some(&fx.viewer_access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_stable_error(&body, "permission_denied");

    let (status, body) = json_request(
        fx.app.clone(),
        "POST",
        &format!("/api/v1/conflicts/{}/triage", conflict.conflict_id),
        Some(serde_json::json!({ "status": "open" })),
        Some(&access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_stable_error(&body, "validation_failed");

    // Invalid resolution version → stable 4xx (not FK 500).
    let (status, body) = json_request(
        fx.app.clone(),
        "POST",
        &format!("/api/v1/conflicts/{}/triage", conflict.conflict_id),
        Some(serde_json::json!({
            "status": "resolved",
            "resolutionVersionAId": Uuid::new_v4(),
            "resolutionVersionBId": conflict.version_b
        })),
        Some(&access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_stable_error(&body, "validation_failed");

    let (status, body) = json_request(
        fx.app.clone(),
        "POST",
        &format!("/api/v1/conflicts/{}/triage", conflict.conflict_id),
        Some(serde_json::json!({
            "status": "resolved",
            "resolutionNote": "ok",
            "resolutionVersionAId": conflict.version_a,
            "resolutionVersionBId": conflict.version_b
        })),
        Some(&access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "resolved");
    assert_eq!(body["resolutionVersionAId"], conflict.version_a.to_string());
    assert_eq!(body["resolutionVersionBId"], conflict.version_b.to_string());

    // Reindex after delete → not found (single-txn tombstone check).
    let (status, body) = json_request(
        fx.app.clone(),
        "DELETE",
        &format!("/api/v1/documents/{}", fx.document),
        None,
        Some(&access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["state"], "tombstoned");
    assert!(body["deletedAt"].as_str().is_some());

    let (status, body) = json_request(
        fx.app.clone(),
        "DELETE",
        &format!("/api/v1/documents/{}", fx.document),
        None,
        Some(&access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["state"], "tombstoned");

    let (status, undisclosed) = json_request(
        fx.app.clone(),
        "GET",
        &format!("/api/v1/documents?collectionId={}", fx.collection),
        None,
        Some(&access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{undisclosed}");
    assert!(!undisclosed["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["id"] == fx.document.to_string()));

    let (status, disclosed) = json_request(
        fx.app.clone(),
        "GET",
        &format!(
            "/api/v1/documents?collectionId={}&includeDeleted=true",
            fx.collection
        ),
        None,
        Some(&access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{disclosed}");
    assert!(disclosed["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["id"] == fx.document.to_string() && item["state"] == "tombstoned"));

    let (status, body) = json_request(
        fx.app.clone(),
        "POST",
        &format!("/api/v1/documents/{}/reindex", fx.document),
        Some(serde_json::json!({})),
        Some(&access),
        Some("reindex-after-delete"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_stable_error(&body, "not_found");

    // Other-org token cannot see tombstoned/local docs.
    let (status, body) = json_request(
        fx.app.clone(),
        "GET",
        &format!("/api/v1/documents/{}", fx.document),
        None,
        Some(&fx.other_access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_stable_error(&body, "not_found");

    // Invalid idempotency key.
    let (status, body) = json_request(
        fx.app.clone(),
        "POST",
        &format!("/api/v1/documents/{doc2}/reindex"),
        Some(serde_json::json!({})),
        Some(&access),
        Some("bad key with spaces"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_stable_error(&body, "validation_failed");

    let _ = (
        fx.org,
        fx.other_org,
        fx.other_user,
        fx.pool,
        conflict.claim_a,
        conflict.claim_b,
        conflict.evidence_ids,
    );
    fx.ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn rest_job_enqueue_visibility_for_owner() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let fx = boot_fixture(&base_url).await;
    let ctx = resolve(&fx.pool, fx.org, fx.user).await;
    let outcome = jobs::enqueue(
        &fx.pool,
        &ctx,
        EnqueueJob::new(
            JobType::Convert,
            JobPayload {
                document_id: Some(fx.document),
                version_id: Some(fx.version),
                ..JobPayload::default()
            },
            format!("convert:{}", fx.document),
        ),
    )
    .await
    .expect("enqueue convert");

    let (status, body) = json_request(
        fx.app.clone(),
        "GET",
        &format!("/api/v1/jobs/{}", outcome.job.id),
        None,
        Some(&fx.access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["jobType"], "convert");

    let (status, body) = json_request(
        fx.app.clone(),
        "GET",
        &format!("/api/v1/jobs/{}", outcome.job.id),
        None,
        Some(&fx.other_access),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_stable_error(&body, "not_found");

    fx.ephemeral.drop().await;
}
