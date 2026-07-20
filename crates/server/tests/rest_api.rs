//! Live HTTP integration tests for the P1B-R04 REST API.
//!
//! These tests self-skip unless PostgreSQL and MinIO test endpoints are provided.
//! They are intentionally not run by the normal `cargo test -p fileconv-server --lib`
//! gate, but they compile under all-target checks.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use bytes::Bytes;
use deadpool_postgres::Pool;
use fileconv_server::auth::context::OrgContext;
use fileconv_server::auth::jwt::JwtKeys;
use fileconv_server::auth::provider::{AuthProvider, AuthRequestMeta, PasswordAuthProvider};
use fileconv_server::auth::session;
use fileconv_server::config::{
    Argon2Config, AuthConfig, JwtAlgorithm, MinioConfig, RuntimeEndpoints, SecretString,
    ServerConfig,
};
use fileconv_server::database::apply_migrations;
use fileconv_server::db::pool::{create_pool, with_org_txn};
use fileconv_server::http::{router, AppState};
use fileconv_server::state::RuntimeState;
use fileconv_server::storage::minio::{MinioClient, ObjectIdentityMeta};
use fileconv_server::storage::{quarantine_key, trusted_key};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio_postgres::NoTls;
use tower::ServiceExt;
use uuid::Uuid;

fn test_database_url() -> Option<String> {
    match std::env::var("MARKHAND_TEST_DATABASE_URL") {
        Ok(url) if !url.trim().is_empty() => Some(url),
        _ => {
            eprintln!("skipped: MARKHAND_TEST_DATABASE_URL unset");
            None
        }
    }
}

fn test_minio_client() -> Option<MinioClient> {
    let endpoint = match std::env::var("MARKHAND_TEST_MINIO_ENDPOINT") {
        Ok(url) if !url.trim().is_empty() => url,
        _ => {
            eprintln!("skipped: MARKHAND_TEST_MINIO_ENDPOINT unset");
            return None;
        }
    };
    let access_key = match std::env::var("MARKHAND_TEST_MINIO_ACCESS_KEY") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => {
            eprintln!("skipped: MARKHAND_TEST_MINIO_ACCESS_KEY unset");
            return None;
        }
    };
    let secret_key = match std::env::var("MARKHAND_TEST_MINIO_SECRET_KEY") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => {
            eprintln!("skipped: MARKHAND_TEST_MINIO_SECRET_KEY unset");
            return None;
        }
    };
    let region = std::env::var("MARKHAND_TEST_MINIO_REGION").unwrap_or_else(|_| "us-east-1".into());
    let bucket = format!("markhand-r04-{}", Uuid::new_v4().simple());
    std::env::set_var("RUST_S3_SKIP_LOCATION_CONSTRAINT", "true");
    let config = MinioConfig::new(
        endpoint,
        SecretString::new(access_key),
        SecretString::new(secret_key),
        bucket,
        region,
        true,
    )
    .expect("minio config");
    Some(MinioClient::from_config(&config).expect("minio client"))
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
    let (client, connection) = tokio_postgres::connect(database_url, NoTls)
        .await
        .unwrap_or_else(|error| panic!("database connection failed: {error}"));
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
        issuer: Some("https://issuer.r04.markhand.test".into()),
        audience: Some("markhand-api".into()),
        signing_key: Some(SecretString::new("r04-integration-signing-key-32b!")),
        alg: JwtAlgorithm::Hs256,
        kid: Some("r04-test-kid".into()),
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
    RuntimeState::from_config(ServerConfig::test_with_endpoints_and_auth(
        RuntimeEndpoints {
            database_url: SecretString::new(database_url),
            qdrant_url: "http://127.0.0.1:1".into(),
            minio_url: "http://127.0.0.1:1".into(),
        },
        test_auth_config(),
    ))
    .expect("runtime")
}

struct LiveEnv {
    db: EphemeralDb,
    pool: Pool,
    storage: MinioClient,
    provider: PasswordAuthProvider,
    app: axum::Router,
}

impl LiveEnv {
    async fn boot() -> Option<Self> {
        let base_url = test_database_url()?;
        let storage = test_minio_client()?;
        storage.ensure_bucket().await.expect("ensure bucket");
        let db = EphemeralDb::create(&base_url).await;
        apply_migrations(&db.url).await.expect("apply migrations");
        let pool = create_pool(&db.url).expect("pool");
        let auth = test_auth_config();
        let provider = PasswordAuthProvider::new(
            pool.clone(),
            auth.clone(),
            JwtKeys::from_auth(&auth).expect("jwt keys"),
        );
        let app = router(
            AppState::from_parts_with_store(
                test_runtime(&db.url),
                pool.clone(),
                Some(PasswordAuthProvider::new(
                    pool.clone(),
                    auth.clone(),
                    JwtKeys::from_auth(&auth).expect("jwt keys"),
                )),
                Some(storage.clone()),
            )
            .expect("app state"),
        );
        Some(Self {
            db,
            pool,
            storage,
            provider,
            app,
        })
    }

    async fn token(&self, email: &str, password: &str) -> String {
        self.provider
            .login_password(
                email,
                password,
                &AuthRequestMeta {
                    request_id: Uuid::new_v4().to_string(),
                },
            )
            .await
            .expect("login")
            .tokens
            .access_token
            .expose()
            .to_string()
    }

    async fn drop(self) {
        self.db.drop().await;
    }
}

#[derive(Clone)]
struct Seeded {
    collection_id: Uuid,
    denied_collection_id: Uuid,
    document_id: Uuid,
    denied_document_id: Uuid,
    cross_org_document_id: Uuid,
    allowed_job_id: Uuid,
    denied_job_id: Uuid,
    upload_key: String,
    trusted_key: String,
    other_org_key: String,
    upload_sha: String,
}

async fn seed(env: &LiveEnv) -> Seeded {
    let org_id = Uuid::new_v4();
    let other_org_id = Uuid::new_v4();
    let user_id = Uuid::new_v4();
    let viewer_id = Uuid::new_v4();
    let no_query_id = Uuid::new_v4();
    let other_user_id = Uuid::new_v4();
    let cross_user_id = Uuid::new_v4();
    let collection_id = Uuid::new_v4();
    let denied_collection_id = Uuid::new_v4();
    let cross_collection_id = Uuid::new_v4();
    let document_id = Uuid::new_v4();
    let denied_document_id = Uuid::new_v4();
    let cross_org_document_id = Uuid::new_v4();
    let version_id = Uuid::new_v4();
    let second_version_id = Uuid::new_v4();
    let denied_version_id = Uuid::new_v4();
    let cross_version_id = Uuid::new_v4();
    let allowed_job_id = Uuid::new_v4();
    let denied_job_id = Uuid::new_v4();
    let upload_document_id = Uuid::new_v4();
    let upload_version_id = Uuid::new_v4();
    let upload_object_id = Uuid::new_v4();
    let trusted_object_id = Uuid::new_v4();
    let other_org_object_id = Uuid::new_v4();
    let upload_key = quarantine_key(org_id, upload_object_id, None).expect("upload key");
    let trusted_key =
        trusted_key(org_id, Uuid::new_v4(), trusted_object_id, None).expect("trusted key");
    let other_org_key =
        quarantine_key(other_org_id, other_org_object_id, None).expect("other org key");
    let upload_bytes = Bytes::from_static(b"server measured upload bytes");
    let upload_sha = hex::encode(Sha256::digest(&upload_bytes));
    env.storage
        .put_object(
            org_id,
            &upload_key,
            upload_bytes,
            &ObjectIdentityMeta {
                org_id,
                collection_id: None,
                document_id: Some(upload_document_id),
                version_id: Some(upload_version_id),
                original_filename: Some("upload.txt".into()),
                canonical_format: Some("txt".into()),
                content_sha256: Some(upload_sha.clone()),
                content_length: Some(28),
                disposition: Some("accepted".into()),
            },
            "text/plain",
        )
        .await
        .expect("put upload");

    let owner_ctx = OrgContext::try_new(
        org_id,
        user_id,
        ["doc.upload", "doc.delete", "doc.publish", "qa.query"],
        [collection_id],
    )
    .expect("owner ctx");
    seed_org_rows(
        &env.pool,
        &owner_ctx,
        SeedIds {
            org_id,
            user_id,
            viewer_id,
            no_query_id,
            other_user_id,
            collection_id,
            denied_collection_id,
            document_id,
            denied_document_id,
            version_id,
            second_version_id,
            denied_version_id,
            allowed_job_id,
            denied_job_id,
        },
    )
    .await;
    let cross_ctx = OrgContext::try_new(
        other_org_id,
        cross_user_id,
        ["doc.upload"],
        [cross_collection_id],
    )
    .expect("cross ctx");
    seed_cross_org_rows(
        &env.pool,
        &cross_ctx,
        other_org_id,
        cross_user_id,
        cross_collection_id,
        cross_org_document_id,
        cross_version_id,
    )
    .await;
    session::set_password_hash(
        &env.pool,
        user_id,
        "owner-password",
        &test_auth_config().argon2,
    )
    .await
    .expect("owner password");
    session::set_password_hash(
        &env.pool,
        viewer_id,
        "viewer-password",
        &test_auth_config().argon2,
    )
    .await
    .expect("viewer password");
    session::set_password_hash(
        &env.pool,
        no_query_id,
        "no-query-password",
        &test_auth_config().argon2,
    )
    .await
    .expect("no-query password");

    Seeded {
        collection_id,
        denied_collection_id,
        document_id,
        denied_document_id,
        cross_org_document_id,
        allowed_job_id,
        denied_job_id,
        upload_key: upload_key.as_str(),
        trusted_key: trusted_key.as_str(),
        other_org_key: other_org_key.as_str(),
        upload_sha,
    }
}

#[derive(Clone, Copy)]
struct SeedIds {
    org_id: Uuid,
    user_id: Uuid,
    viewer_id: Uuid,
    no_query_id: Uuid,
    other_user_id: Uuid,
    collection_id: Uuid,
    denied_collection_id: Uuid,
    document_id: Uuid,
    denied_document_id: Uuid,
    version_id: Uuid,
    second_version_id: Uuid,
    denied_version_id: Uuid,
    allowed_job_id: Uuid,
    denied_job_id: Uuid,
}

async fn seed_org_rows(pool: &Pool, ctx: &OrgContext, ids: SeedIds) {
    with_org_txn(pool, ctx, move |txn| {
        Box::pin(async move {
            txn.execute(
                "INSERT INTO orgs (id, slug, name)
                 VALUES ($1, 'r04-org', 'R04 Org')",
                &[&ids.org_id],
            )
            .await?;
            for (user_id, email, name) in [
                (ids.user_id, "owner-r04@example.test", "Owner R04"),
                (ids.viewer_id, "viewer-r04@example.test", "Viewer R04"),
                (ids.no_query_id, "no-query-r04@example.test", "No Query R04"),
                (ids.other_user_id, "other-r04@example.test", "Other R04"),
            ] {
                txn.execute(
                    "INSERT INTO users (id, email, display_name)
                     VALUES ($1, $2, $3)",
                    &[&user_id, &email, &name],
                )
                .await?;
            }
            txn.execute(
                "INSERT INTO org_memberships (org_id, user_id, role)
                 VALUES
                   ($1, $2, 'owner'),
                   ($1, $3, 'viewer'),
                   ($1, $4, 'editor'),
                   ($1, $5, 'owner')",
                &[
                    &ids.org_id,
                    &ids.user_id,
                    &ids.viewer_id,
                    &ids.no_query_id,
                    &ids.other_user_id,
                ],
            )
            .await?;
            seed_roles_and_permissions(txn, ids).await?;
            seed_collections_documents_and_jobs(txn, ids).await
        })
    })
    .await
    .expect("seed rows");
}

async fn seed_cross_org_rows(
    pool: &Pool,
    ctx: &OrgContext,
    org_id: Uuid,
    user_id: Uuid,
    collection_id: Uuid,
    document_id: Uuid,
    version_id: Uuid,
) {
    with_org_txn(pool, ctx, move |txn| {
        Box::pin(async move {
            txn.execute(
                "INSERT INTO orgs (id, slug, name)
                 VALUES ($1, 'r04-other', 'R04 Other')",
                &[&org_id],
            )
            .await?;
            txn.execute(
                "INSERT INTO users (id, email, display_name)
                 VALUES ($1, 'cross-r04@example.test', 'Cross R04')",
                &[&user_id],
            )
            .await?;
            txn.execute(
                "INSERT INTO org_memberships (org_id, user_id, role)
                 VALUES ($1, $2, 'owner')",
                &[&org_id, &user_id],
            )
            .await?;
            txn.execute(
                "INSERT INTO collections (id, org_id, name, slug, owner_user_id, visibility)
                 VALUES ($1, $2, 'Other', 'other', $3, 'private')",
                &[&collection_id, &org_id, &user_id],
            )
            .await?;
            insert_indexed_document(
                txn,
                org_id,
                collection_id,
                user_id,
                document_id,
                version_id,
                None,
                "Cross org document",
            )
            .await
        })
    })
    .await
    .expect("seed cross org rows");
}

async fn seed_roles_and_permissions(
    txn: &tokio_postgres::Transaction<'_>,
    ids: SeedIds,
) -> Result<(), fileconv_server::db::error::DbError> {
    for permission in ["doc.upload", "doc.delete", "doc.publish", "qa.query"] {
        let description = format!("{permission} permission");
        txn.execute(
            "INSERT INTO permissions (id, code, description)
             VALUES ($1, $2, $3)
             ON CONFLICT (code) DO NOTHING",
            &[&Uuid::new_v4(), &permission, &description],
        )
        .await?;
    }
    for (code, name) in [
        ("owner", "Owner"),
        ("viewer", "Viewer"),
        ("editor", "Editor"),
    ] {
        txn.execute(
            "INSERT INTO roles (id, org_id, code, name, is_system)
             VALUES ($1, $2, $3, $4, true)",
            &[&Uuid::new_v4(), &ids.org_id, &code, &name],
        )
        .await?;
    }
    for permission in ["doc.upload", "doc.delete", "doc.publish", "qa.query"] {
        grant_permission(txn, ids.org_id, "owner", permission).await?;
    }
    grant_permission(txn, ids.org_id, "viewer", "qa.query").await?;
    grant_permission(txn, ids.org_id, "editor", "doc.upload").await?;
    Ok(())
}

async fn grant_permission(
    txn: &tokio_postgres::Transaction<'_>,
    org_id: Uuid,
    role_code: &str,
    permission: &str,
) -> Result<(), fileconv_server::db::error::DbError> {
    let role_id: Uuid = txn
        .query_one(
            "SELECT id FROM roles WHERE org_id = $1 AND code = $2",
            &[&org_id, &role_code],
        )
        .await?
        .get(0);
    let permission_id: Uuid = txn
        .query_one("SELECT id FROM permissions WHERE code = $1", &[&permission])
        .await?
        .get(0);
    txn.execute(
        "INSERT INTO role_permissions (org_id, role_id, permission_id)
         VALUES ($1, $2, $3)
         ON CONFLICT DO NOTHING",
        &[&org_id, &role_id, &permission_id],
    )
    .await?;
    Ok(())
}

async fn seed_collections_documents_and_jobs(
    txn: &tokio_postgres::Transaction<'_>,
    ids: SeedIds,
) -> Result<(), fileconv_server::db::error::DbError> {
    txn.execute(
        "INSERT INTO collections (id, org_id, name, slug, owner_user_id, visibility)
         VALUES
           ($1, $2, 'Allowed', 'allowed', $3, 'private'),
           ($4, $2, 'Denied', 'denied', $5, 'private')",
        &[
            &ids.collection_id,
            &ids.org_id,
            &ids.user_id,
            &ids.denied_collection_id,
            &ids.other_user_id,
        ],
    )
    .await?;
    txn.execute(
        "INSERT INTO collection_user_access (id, org_id, collection_id, user_id, access_level)
         VALUES
           ($1, $2, $3, $4, 'read'),
           ($5, $2, $3, $6, 'read'),
           ($7, $2, $8, $9, 'read')",
        &[
            &Uuid::new_v4(),
            &ids.org_id,
            &ids.collection_id,
            &ids.viewer_id,
            &Uuid::new_v4(),
            &ids.no_query_id,
            &Uuid::new_v4(),
            &ids.denied_collection_id,
            &ids.user_id,
        ],
    )
    .await?;
    insert_indexed_document(
        txn,
        ids.org_id,
        ids.collection_id,
        ids.user_id,
        ids.document_id,
        ids.version_id,
        Some(ids.second_version_id),
        "Allowed document",
    )
    .await?;
    insert_indexed_document(
        txn,
        ids.org_id,
        ids.denied_collection_id,
        ids.other_user_id,
        ids.denied_document_id,
        ids.denied_version_id,
        None,
        "Denied document",
    )
    .await?;
    let payload = json!({ "document_id": ids.document_id, "version_id": ids.version_id });
    let denied_payload =
        json!({ "document_id": ids.denied_document_id, "version_id": ids.denied_version_id });
    txn.execute(
        "INSERT INTO jobs (
            id, org_id, job_type, payload_version, payload, idempotency_key,
            document_id, version_id, last_error
         )
         VALUES
           ($1, $2, 'convert', 2, $3, 'status-allowed', $4, $5, 'raw secret error'),
           ($6, $2, 'convert', 2, $7, 'status-denied', $8, $9, 'raw secret error')",
        &[
            &ids.allowed_job_id,
            &ids.org_id,
            &payload,
            &ids.document_id,
            &ids.version_id,
            &ids.denied_job_id,
            &denied_payload,
            &ids.denied_document_id,
            &ids.denied_version_id,
        ],
    )
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_indexed_document(
    txn: &tokio_postgres::Transaction<'_>,
    org_id: Uuid,
    collection_id: Uuid,
    user_id: Uuid,
    document_id: Uuid,
    version_id: Uuid,
    second_version_id: Option<Uuid>,
    title: &str,
) -> Result<(), fileconv_server::db::error::DbError> {
    let sha = "a".repeat(64);
    let key = format!("quarantine/test/{version_id}");
    txn.execute(
        "INSERT INTO documents (id, org_id, collection_id, title, state, created_by_user_id)
         VALUES ($1, $2, $3, $4, 'indexed', $5)",
        &[&document_id, &org_id, &collection_id, &title, &user_id],
    )
    .await?;
    txn.execute(
        "INSERT INTO document_versions (
            id, org_id, document_id, version_number, publication_state, is_current,
            content_sha256, original_object_key, source_content_type, byte_size,
            created_by_user_id
         )
         VALUES ($1, $2, $3, 1, 'published', true, $4, $5, 'text/plain', 10, $6)",
        &[&version_id, &org_id, &document_id, &sha, &key, &user_id],
    )
    .await?;
    if let Some(second_version_id) = second_version_id {
        let second_key = format!("quarantine/test/{second_version_id}");
        txn.execute(
            "INSERT INTO document_versions (
                id, org_id, document_id, version_number, publication_state, is_current,
                content_sha256, original_object_key, source_content_type, byte_size,
                created_by_user_id
             )
             VALUES ($1, $2, $3, 2, 'draft', false, $4, $5, 'text/plain', 11, $6)",
            &[
                &second_version_id,
                &org_id,
                &document_id,
                &sha,
                &second_key,
                &user_id,
            ],
        )
        .await?;
    }
    txn.execute(
        "UPDATE documents
         SET current_version_id = $3, updated_at = clock_timestamp()
         WHERE org_id = $1 AND id = $2",
        &[&org_id, &document_id, &version_id],
    )
    .await?;
    Ok(())
}

async fn json_request(
    app: axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
    bearer: &str,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {bearer}"));
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(match body {
            Some(value) => Body::from(serde_json::to_vec(&value).expect("serialize body")),
            None => Body::empty(),
        })
        .expect("request");
    let response = app.oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("json body")
    };
    (status, body)
}

fn tamper_cursor(cursor: &str) -> String {
    let mut bytes = cursor.as_bytes().to_vec();
    let last = bytes.last_mut().expect("cursor is non-empty");
    *last = if *last == b'A' { b'B' } else { b'A' };
    String::from_utf8(bytes).expect("cursor remains utf8")
}

#[tokio::test]
async fn collection_document_job_rest_api_contract() {
    let Some(env) = LiveEnv::boot().await else {
        return;
    };
    let seeded = seed(&env).await;
    let owner_token = env.token("owner-r04@example.test", "owner-password").await;
    let viewer_token = env
        .token("viewer-r04@example.test", "viewer-password")
        .await;
    let no_query_token = env
        .token("no-query-r04@example.test", "no-query-password")
        .await;

    let (status, page1) = json_request(
        env.app.clone(),
        "GET",
        "/api/v1/collections?limit=1",
        None,
        &owner_token,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page1}");
    assert_eq!(page1["items"].as_array().unwrap().len(), 1);
    assert_eq!(page1["pageInfo"]["hasMore"], true);
    let cursor = page1["pageInfo"]["nextCursor"].as_str().unwrap();
    let tampered_cursor = tamper_cursor(cursor);
    let (status, tampered) = json_request(
        env.app.clone(),
        "GET",
        &format!("/api/v1/collections?limit=1&cursor={tampered_cursor}"),
        None,
        &owner_token,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{tampered}");
    assert_eq!(tampered["code"], "validation_failed");
    let (status, page2) = json_request(
        env.app.clone(),
        "GET",
        &format!("/api/v1/collections?limit=1&cursor={cursor}"),
        None,
        &owner_token,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page2}");
    assert_eq!(page2["pageInfo"]["hasMore"], false);
    assert_eq!(
        page2["items"][0]["id"],
        seeded.denied_collection_id.to_string()
    );

    let (status, viewer_collections) = json_request(
        env.app.clone(),
        "GET",
        "/api/v1/collections",
        None,
        &viewer_token,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{viewer_collections}");
    assert_eq!(viewer_collections["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        viewer_collections["items"][0]["id"],
        seeded.collection_id.to_string()
    );

    let (status, owner_doc) = json_request(
        env.app.clone(),
        "GET",
        &format!("/api/v1/documents/{}", seeded.document_id),
        None,
        &owner_token,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{owner_doc}");
    assert_eq!(owner_doc["id"], seeded.document_id.to_string());
    let (status, get_action_rejected) = json_request(
        env.app.clone(),
        "GET",
        &format!("/api/v1/documents/{}:reindex", seeded.document_id),
        None,
        &owner_token,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{get_action_rejected}");
    let (status, viewer_doc) = json_request(
        env.app.clone(),
        "GET",
        &format!("/api/v1/documents/{}", seeded.document_id),
        None,
        &viewer_token,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{viewer_doc}");
    let (status, no_query_doc) = json_request(
        env.app.clone(),
        "GET",
        &format!("/api/v1/documents/{}", seeded.document_id),
        None,
        &no_query_token,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{no_query_doc}");
    assert_eq!(no_query_doc["code"], "permission_denied");

    let (status, unauthorized_doc) = json_request(
        env.app.clone(),
        "GET",
        &format!("/api/v1/documents/{}", seeded.denied_document_id),
        None,
        &viewer_token,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(unauthorized_doc["code"], "not_found");
    let (status, cross_org_doc) = json_request(
        env.app.clone(),
        "GET",
        &format!("/api/v1/documents/{}", seeded.cross_org_document_id),
        None,
        &owner_token,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(cross_org_doc["code"], "not_found");

    let (status, documents) = json_request(
        env.app.clone(),
        "GET",
        &format!(
            "/api/v1/collections/{}/documents",
            seeded.denied_collection_id
        ),
        None,
        &viewer_token,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{documents}");

    let (status, created) = json_request(
        env.app.clone(),
        "POST",
        &format!("/api/v1/collections/{}/documents", seeded.collection_id),
        Some(json!({
            "objectKey": seeded.upload_key,
            "title": "Created from quarantine",
            "contentSha256": "0".repeat(64)
        })),
        &owner_token,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["version"]["contentSha256"], seeded.upload_sha);
    assert!(created["jobId"].as_str().is_some());

    let (status, trusted_rejected) = json_request(
        env.app.clone(),
        "POST",
        &format!("/api/v1/collections/{}/documents", seeded.collection_id),
        Some(json!({ "objectKey": seeded.trusted_key, "title": "Bad key" })),
        &owner_token,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{trusted_rejected}");
    let (status, other_org_rejected) = json_request(
        env.app.clone(),
        "POST",
        &format!("/api/v1/collections/{}/documents", seeded.collection_id),
        Some(json!({ "objectKey": seeded.other_org_key, "title": "Other org" })),
        &owner_token,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{other_org_rejected}");

    let (status, versions1) = json_request(
        env.app.clone(),
        "GET",
        &format!("/api/v1/documents/{}/versions?limit=1", seeded.document_id),
        None,
        &owner_token,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{versions1}");
    assert_eq!(versions1["items"].as_array().unwrap().len(), 1);
    assert_eq!(versions1["pageInfo"]["hasMore"], true);
    let (status, no_query_versions) = json_request(
        env.app.clone(),
        "GET",
        &format!("/api/v1/documents/{}/versions", seeded.document_id),
        None,
        &no_query_token,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{no_query_versions}");
    assert_eq!(no_query_versions["code"], "permission_denied");
    let version_cursor = versions1["pageInfo"]["nextCursor"].as_str().unwrap();
    let (status, versions2) = json_request(
        env.app.clone(),
        "GET",
        &format!(
            "/api/v1/documents/{}/versions?limit=1&cursor={version_cursor}",
            seeded.document_id
        ),
        None,
        &owner_token,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{versions2}");
    assert_eq!(versions2["items"][0]["versionNumber"], 2);

    let (status, reindex_plain_rejected) = json_request(
        env.app.clone(),
        "POST",
        &format!("/api/v1/documents/{}", seeded.document_id),
        None,
        &owner_token,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{reindex_plain_rejected}");
    let (status, reindex1) = json_request(
        env.app.clone(),
        "POST",
        &format!("/api/v1/documents/{}:reindex", seeded.document_id),
        None,
        &owner_token,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{reindex1}");
    let (status, reindex2) = json_request(
        env.app.clone(),
        "POST",
        &format!("/api/v1/documents/{}:reindex", seeded.document_id),
        None,
        &owner_token,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{reindex2}");
    assert_eq!(reindex1["id"], reindex2["id"]);

    let (status, viewer_reindex) = json_request(
        env.app.clone(),
        "POST",
        &format!("/api/v1/documents/{}:reindex", seeded.document_id),
        None,
        &viewer_token,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{viewer_reindex}");

    let (status, job) = json_request(
        env.app.clone(),
        "GET",
        &format!("/api/v1/jobs/{}", seeded.allowed_job_id),
        None,
        &owner_token,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{job}");
    for forbidden in [
        "payload",
        "checkpoint",
        "leaseOwner",
        "leaseExpiresAt",
        "heartbeatAt",
        "lastError",
    ] {
        assert!(job.get(forbidden).is_none(), "{forbidden} leaked in {job}");
    }
    let (status, no_query_job) = json_request(
        env.app.clone(),
        "GET",
        &format!("/api/v1/jobs/{}", seeded.allowed_job_id),
        None,
        &no_query_token,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{no_query_job}");
    assert_eq!(no_query_job["code"], "permission_denied");
    let (status, denied_job) = json_request(
        env.app.clone(),
        "GET",
        &format!("/api/v1/jobs/{}", seeded.denied_job_id),
        None,
        &viewer_token,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{denied_job}");

    let (status, delete_action_rejected) = json_request(
        env.app.clone(),
        "DELETE",
        &format!("/api/v1/documents/{}:reindex", seeded.document_id),
        None,
        &owner_token,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{delete_action_rejected}");
    assert_eq!(delete_action_rejected["code"], "validation_failed");

    let (status, deleted) = json_request(
        env.app.clone(),
        "DELETE",
        &format!("/api/v1/documents/{}", seeded.document_id),
        None,
        &owner_token,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{deleted}");
    assert_eq!(deleted["document"]["state"], "tombstoned");
    let (status, deleted_again) = json_request(
        env.app.clone(),
        "DELETE",
        &format!("/api/v1/documents/{}", seeded.document_id),
        None,
        &owner_token,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{deleted_again}");
    let (status, viewer_delete) = json_request(
        env.app.clone(),
        "DELETE",
        &format!("/api/v1/documents/{}", seeded.denied_document_id),
        None,
        &viewer_token,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{viewer_delete}");

    let (status, viewer_create) = json_request(
        env.app.clone(),
        "POST",
        "/api/v1/collections",
        Some(json!({
            "name": "Viewer create",
            "slug": "viewer-create",
            "visibility": "private"
        })),
        &viewer_token,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{viewer_create}");
    let (status, malformed) = json_request(
        env.app.clone(),
        "GET",
        "/api/v1/documents/not-a-uuid",
        None,
        &owner_token,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(malformed["code"], "validation_failed");
    let (status, oversized) = json_request(
        env.app.clone(),
        "GET",
        "/api/v1/collections?limit=101",
        None,
        &owner_token,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(oversized["code"], "validation_failed");

    env.drop().await;
}
