use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use bytes::Bytes;
use deadpool_postgres::Pool;
use fileconv_knowledge::embedding::{local_vector, LOCAL_VECTOR_DIMENSIONS};
use fileconv_server::auth::context::OrgContext;
use fileconv_server::auth::jwt::JwtKeys;
use fileconv_server::auth::provider::{AuthProvider, AuthRequestMeta, PasswordAuthProvider};
use fileconv_server::auth::session;
use fileconv_server::config::{
    Argon2Config, AuthConfig, JwtAlgorithm, MinioConfig, RuntimeEndpoints, SecretString,
    ServerConfig,
};
use fileconv_server::database::apply_migrations;
use fileconv_server::db::models::JobStatus;
use fileconv_server::db::pool::{create_pool, with_org_txn};
use fileconv_server::http::{router, AppState};
use fileconv_server::jobs;
use fileconv_server::services::embedding::approved_plan;
use fileconv_server::services::index_signature::collection_name_for_digest;
use fileconv_server::services::indexing::OutboxJobSink;
use fileconv_server::state::RuntimeState;
use fileconv_server::storage::minio::MinioClient;
use fileconv_server::storage::qdrant::{
    point_id_from_org_collection_and_chunk, QdrantClient, VectorScope,
};
use fileconv_server::workers::convert::{ConvertWorker, ConvertWorkerConfig, ConvertWorkerRun};
use fileconv_server::workers::delete::{DeleteWorker, DeleteWorkerConfig, DeleteWorkerRun};
use fileconv_server::workers::index::{IndexWorker, IndexWorkerConfig, IndexWorkerRun};
use fileconv_server::workers::limits::ResourceLimits;
use fileconv_server::workers::sandbox::{self, SandboxConfig};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tokio_postgres::NoTls;
use tower::ServiceExt;
use uuid::Uuid;

pub const BOUNDARY: &str = "----markhandE2eBoundary7MA4YWxk";
pub const PASSWORD: &str = "correct-e2e-password-1";

const LLM_ENV_KEYS: &[&str] = &[
    "FILECONV_LLM_PROVIDER",
    "FILECONV_LLM_API_KEY",
    "FILECONV_LLM_MODEL",
    "FILECONV_LLM_BASE_URL",
];

pub fn test_auth_config() -> AuthConfig {
    AuthConfig {
        issuer: Some("https://issuer.e2e.markhand.test".into()),
        audience: Some("markhand-api".into()),
        signing_key: Some(SecretString::new("e2e-integration-signing-key-32b!")),
        alg: JwtAlgorithm::Hs256,
        kid: Some("e2e-test-kid".into()),
        access_token_ttl_secs: 900,
        refresh_token_ttl_secs: 3_600,
        argon2: Argon2Config {
            memory_kib: 8_192,
            time_cost: 1,
            parallelism: 1,
        },
    }
}

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
    let bucket = format!("markhand-e2e-{}", Uuid::new_v4().simple());
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

fn test_qdrant_client() -> Option<QdrantClient> {
    let url = match std::env::var("MARKHAND_TEST_QDRANT_URL") {
        Ok(url) if !url.trim().is_empty() => url,
        _ => {
            eprintln!("skipped: MARKHAND_TEST_QDRANT_URL unset");
            return None;
        }
    };
    let api_key = std::env::var("MARKHAND_TEST_QDRANT_API_KEY")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(SecretString::new);
    Some(QdrantClient::with_api_key(url, api_key).expect("qdrant client"))
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

pub struct EphemeralDb {
    admin_url: String,
    db_name: String,
    url: String,
}

impl EphemeralDb {
    async fn create(base_url: &str) -> Self {
        let db_name = format!("markhand_e2e_{}", Uuid::new_v4().simple());
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

pub struct LiveEnv {
    db: EphemeralDb,
    pub pool: Pool,
    pub storage: MinioClient,
    pub qdrant: QdrantClient,
    provider: PasswordAuthProvider,
    pub app: axum::Router,
}

impl LiveEnv {
    pub async fn boot() -> Option<Self> {
        let base_url = test_database_url()?;
        let storage = test_minio_client()?;
        let qdrant = test_qdrant_client()?;
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
        let runtime = RuntimeState::from_config(ServerConfig::test_with_endpoints_and_auth(
            RuntimeEndpoints {
                database_url: SecretString::new(&db.url),
                qdrant_url: std::env::var("MARKHAND_TEST_QDRANT_URL")
                    .unwrap_or_else(|_| "http://127.0.0.1:1".into()),
                minio_url: std::env::var("MARKHAND_TEST_MINIO_ENDPOINT")
                    .unwrap_or_else(|_| "http://127.0.0.1:1".into()),
            },
            auth.clone(),
        ))
        .expect("runtime");
        let app = router(
            AppState::from_parts_with_clients(
                runtime,
                pool.clone(),
                Some(PasswordAuthProvider::new(
                    pool.clone(),
                    auth.clone(),
                    JwtKeys::from_auth(&auth).expect("jwt keys"),
                )),
                Some(storage.clone()),
                qdrant.clone(),
            )
            .expect("app state"),
        );
        Some(Self {
            db,
            pool,
            storage,
            qdrant,
            provider,
            app,
        })
    }

    pub async fn token(&self, email: &str) -> String {
        self.provider
            .login_password(
                email,
                PASSWORD,
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

    pub async fn drop(self) {
        self.db.drop().await;
    }
}

#[derive(Clone)]
pub struct SeededOrg {
    pub org_id: Uuid,
    pub owner_id: Uuid,
    pub editor_id: Uuid,
    pub viewer_id: Uuid,
    pub no_acl_id: Uuid,
    pub no_query_id: Uuid,
    pub other_org_id: Uuid,
    pub other_user_id: Uuid,
    pub collection_id: Uuid,
    pub other_collection_id: Uuid,
    pub cross_collection_id: Uuid,
    pub owner_email: String,
    pub editor_email: String,
    pub viewer_email: String,
    pub no_acl_email: String,
    pub no_query_email: String,
    pub cross_email: String,
}

impl SeededOrg {
    pub fn worker_ctx(&self) -> OrgContext {
        OrgContext::try_new(
            self.org_id,
            self.owner_id,
            ["doc.upload", "doc.delete", "doc.publish", "qa.query"],
            [self.collection_id, self.other_collection_id],
        )
        .expect("worker context")
    }

    pub fn cross_worker_ctx(&self) -> OrgContext {
        OrgContext::try_new(
            self.other_org_id,
            self.other_user_id,
            ["doc.upload", "doc.delete", "doc.publish", "qa.query"],
            [self.cross_collection_id],
        )
        .expect("cross worker context")
    }
}

pub async fn seed_org(env: &LiveEnv) -> SeededOrg {
    let seeded = SeededOrg {
        org_id: Uuid::new_v4(),
        owner_id: Uuid::new_v4(),
        editor_id: Uuid::new_v4(),
        viewer_id: Uuid::new_v4(),
        no_acl_id: Uuid::new_v4(),
        no_query_id: Uuid::new_v4(),
        other_org_id: Uuid::new_v4(),
        other_user_id: Uuid::new_v4(),
        collection_id: Uuid::new_v4(),
        other_collection_id: Uuid::new_v4(),
        cross_collection_id: Uuid::new_v4(),
        owner_email: String::new(),
        editor_email: String::new(),
        viewer_email: String::new(),
        no_acl_email: String::new(),
        no_query_email: String::new(),
        cross_email: String::new(),
    };
    let seeded = SeededOrg {
        owner_email: format!("owner-{}@e2e.example.test", seeded.owner_id.simple()),
        editor_email: format!("editor-{}@e2e.example.test", seeded.editor_id.simple()),
        viewer_email: format!("viewer-{}@e2e.example.test", seeded.viewer_id.simple()),
        no_acl_email: format!("no-acl-{}@e2e.example.test", seeded.no_acl_id.simple()),
        no_query_email: format!("no-query-{}@e2e.example.test", seeded.no_query_id.simple()),
        cross_email: format!("cross-{}@e2e.example.test", seeded.other_user_id.simple()),
        ..seeded
    };

    let ctx = seeded.worker_ctx();
    with_org_txn(&env.pool, &ctx, {
        let s = seeded.clone();
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "INSERT INTO orgs (id, slug, name)
                     VALUES ($1, $2, 'E2E Org')",
                    &[&s.org_id, &format!("e2e-{}", s.org_id.simple())],
                )
                .await?;
                for (user_id, email, name) in [
                    (s.owner_id, s.owner_email.as_str(), "Owner E2E"),
                    (s.editor_id, s.editor_email.as_str(), "Editor E2E"),
                    (s.viewer_id, s.viewer_email.as_str(), "Viewer E2E"),
                    (s.no_acl_id, s.no_acl_email.as_str(), "No ACL E2E"),
                    (s.no_query_id, s.no_query_email.as_str(), "No Query E2E"),
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
                       ($1, $3, 'editor'),
                       ($1, $4, 'viewer'),
                       ($1, $5, 'viewer'),
                       ($1, $6, 'editor')",
                    &[
                        &s.org_id,
                        &s.owner_id,
                        &s.editor_id,
                        &s.viewer_id,
                        &s.no_acl_id,
                        &s.no_query_id,
                    ],
                )
                .await?;
                seed_roles(txn, s.org_id).await?;
                txn.execute(
                    "INSERT INTO org_quotas (
                        org_id, max_storage_bytes, max_documents,
                        max_concurrent_jobs, max_monthly_tokens
                     )
                     VALUES ($1, 1000000000, 100000, 100, 1000000000)",
                    &[&s.org_id],
                )
                .await?;
                txn.execute(
                    "INSERT INTO collections (id, org_id, name, slug, owner_user_id, visibility)
                     VALUES
                       ($1, $2, 'Primary E2E', $3, $4, 'private'),
                       ($5, $2, 'Other E2E', $6, $7, 'private')",
                    &[
                        &s.collection_id,
                        &s.org_id,
                        &format!("primary-{}", s.collection_id.simple()),
                        &s.owner_id,
                        &s.other_collection_id,
                        &format!("other-{}", s.other_collection_id.simple()),
                        &s.editor_id,
                    ],
                )
                .await?;
                txn.execute(
                    "INSERT INTO collection_user_access
                        (id, org_id, collection_id, user_id, access_level)
                     VALUES
                        ($1, $2, $3, $4, 'read'),
                        ($5, $2, $3, $6, 'read')",
                    &[
                        &Uuid::new_v4(),
                        &s.org_id,
                        &s.collection_id,
                        &s.viewer_id,
                        &Uuid::new_v4(),
                        &s.no_query_id,
                    ],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("seed primary org");

    let cross_ctx = OrgContext::try_new(
        seeded.other_org_id,
        seeded.other_user_id,
        ["doc.upload", "doc.delete", "doc.publish", "qa.query"],
        [seeded.cross_collection_id],
    )
    .expect("cross ctx");
    with_org_txn(&env.pool, &cross_ctx, {
        let s = seeded.clone();
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "INSERT INTO orgs (id, slug, name)
                     VALUES ($1, $2, 'Cross E2E Org')",
                    &[
                        &s.other_org_id,
                        &format!("cross-{}", s.other_org_id.simple()),
                    ],
                )
                .await?;
                txn.execute(
                    "INSERT INTO users (id, email, display_name)
                     VALUES ($1, $2, 'Cross E2E')",
                    &[&s.other_user_id, &s.cross_email],
                )
                .await?;
                txn.execute(
                    "INSERT INTO org_memberships (org_id, user_id, role)
                     VALUES ($1, $2, 'owner')",
                    &[&s.other_org_id, &s.other_user_id],
                )
                .await?;
                seed_roles(txn, s.other_org_id).await?;
                txn.execute(
                    "INSERT INTO org_quotas (
                        org_id, max_storage_bytes, max_documents,
                        max_concurrent_jobs, max_monthly_tokens
                     )
                     VALUES ($1, 1000000000, 100000, 100, 1000000000)",
                    &[&s.other_org_id],
                )
                .await?;
                txn.execute(
                    "INSERT INTO collections (id, org_id, name, slug, owner_user_id, visibility)
                     VALUES ($1, $2, 'Cross Collection', $3, $4, 'private')",
                    &[
                        &s.cross_collection_id,
                        &s.other_org_id,
                        &format!("cross-{}", s.cross_collection_id.simple()),
                        &s.other_user_id,
                    ],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("seed cross org");

    for (user_id, password) in [
        (seeded.owner_id, PASSWORD),
        (seeded.editor_id, PASSWORD),
        (seeded.viewer_id, PASSWORD),
        (seeded.no_acl_id, PASSWORD),
        (seeded.no_query_id, PASSWORD),
        (seeded.other_user_id, PASSWORD),
    ] {
        session::set_password_hash(&env.pool, user_id, password, &test_auth_config().argon2)
            .await
            .expect("set password");
    }

    seeded
}

async fn seed_roles(
    txn: &tokio_postgres::Transaction<'_>,
    org_id: Uuid,
) -> Result<(), fileconv_server::db::error::DbError> {
    for permission in ["doc.upload", "doc.delete", "doc.publish", "qa.query"] {
        txn.execute(
            "INSERT INTO permissions (id, code, description)
             VALUES ($1, $2, $2)
             ON CONFLICT (code) DO NOTHING",
            &[&Uuid::new_v4(), &permission],
        )
        .await?;
    }
    for (code, name) in [
        ("owner", "Owner"),
        ("admin", "Admin"),
        ("editor", "Editor"),
        ("viewer", "Viewer"),
    ] {
        txn.execute(
            "INSERT INTO roles (id, org_id, code, name, is_system)
             VALUES ($1, $2, $3, $4, true)
             ON CONFLICT (org_id, code) DO NOTHING",
            &[&Uuid::new_v4(), &org_id, &code, &name],
        )
        .await?;
    }
    for permission in ["doc.upload", "doc.delete", "doc.publish", "qa.query"] {
        grant_permission(txn, org_id, "owner", permission).await?;
    }
    grant_permission(txn, org_id, "admin", "qa.query").await?;
    grant_permission(txn, org_id, "admin", "doc.delete").await?;
    grant_permission(txn, org_id, "editor", "doc.upload").await?;
    grant_permission(txn, org_id, "viewer", "qa.query").await?;
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

pub struct LlmEnvGuard {
    saved: Vec<(&'static str, Option<String>)>,
}

impl LlmEnvGuard {
    pub fn unset_all() -> Self {
        let saved = LLM_ENV_KEYS
            .iter()
            .map(|key| (*key, std::env::var(key).ok()))
            .collect::<Vec<_>>();
        for key in LLM_ENV_KEYS {
            std::env::remove_var(key);
        }
        Self { saved }
    }
}

impl Drop for LlmEnvGuard {
    fn drop(&mut self) {
        for (key, value) in &self.saved {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

pub struct HttpResponse {
    pub status: StatusCode,
    pub bytes: Bytes,
}

pub async fn send_request(
    app: axum::Router,
    method: &str,
    uri: &str,
    body: Body,
    bearer: Option<&str>,
    content_type: Option<String>,
) -> HttpResponse {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-request-id", Uuid::new_v4().to_string());
    if let Some(token) = bearer {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    if let Some(content_type) = content_type {
        builder = builder.header("content-type", content_type);
    }
    let response = app
        .oneshot(builder.body(body).expect("request"))
        .await
        .expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("response body")
        .to_bytes();
    HttpResponse { status, bytes }
}

pub async fn json_request(
    app: axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
    bearer: &str,
) -> (StatusCode, Value) {
    let body = body.map(|value| serde_json::to_vec(&value).expect("serialize body"));
    let response = send_request(
        app,
        method,
        uri,
        body.map(Body::from).unwrap_or_else(Body::empty),
        Some(bearer),
        Some("application/json".into()),
    )
    .await;
    let value = if response.bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&response.bytes).unwrap_or_else(|_| {
            Value::String(String::from_utf8_lossy(&response.bytes).into_owned())
        })
    };
    (response.status, value)
}

pub async fn upload(
    env: &LiveEnv,
    bearer: &str,
    filename: &str,
    content_type: &str,
    content: &[u8],
) -> (StatusCode, Value) {
    let response = send_request(
        env.app.clone(),
        "POST",
        "/api/v1/uploads",
        Body::from(multipart_body(filename, content_type, content)),
        Some(bearer),
        Some(format!("multipart/form-data; boundary={BOUNDARY}")),
    )
    .await;
    let value = if response.bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&response.bytes).unwrap_or_else(|_| {
            Value::String(String::from_utf8_lossy(&response.bytes).into_owned())
        })
    };
    (response.status, value)
}

pub fn multipart_body(filename: &str, content_type: &str, content: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; \
             filename=\"{filename}\"\r\nContent-Type: {content_type}\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(content);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    body
}

pub fn many_part_body(parts: usize) -> Vec<u8> {
    let mut body = Vec::new();
    for index in 0..parts {
        body.extend_from_slice(
            format!(
                "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"field{index}\"\r\n\r\nx\r\n"
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    body
}

pub async fn create_document(
    env: &LiveEnv,
    token: &str,
    collection_id: Uuid,
    title: &str,
    object_key: &str,
) -> (Uuid, Uuid, Uuid) {
    let (status, created) = json_request(
        env.app.clone(),
        "POST",
        &format!("/api/v1/collections/{collection_id}/documents"),
        Some(json!({ "objectKey": object_key, "title": title })),
        token,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    (
        parse_uuid_value(&created["document"]["id"]),
        parse_uuid_value(&created["version"]["id"]),
        parse_uuid_value(&created["jobId"]),
    )
}

pub async fn ingest_document(
    env: &LiveEnv,
    seeded: &SeededOrg,
    token: &str,
    case: FixtureCase,
) -> Option<IngestedDoc> {
    let ctx = seeded.worker_ctx();
    ingest_document_in_collection(env, token, seeded.collection_id, &ctx, case).await
}

pub async fn ingest_document_in_collection(
    env: &LiveEnv,
    token: &str,
    collection_id: Uuid,
    ctx: &OrgContext,
    case: FixtureCase,
) -> Option<IngestedDoc> {
    let (status, uploaded) = upload(env, token, case.filename, case.content_type, case.bytes).await;
    assert_eq!(status, StatusCode::CREATED, "{uploaded}");
    assert_eq!(uploaded["disposition"], "accepted");
    let object_key = uploaded["objectKey"].as_str().expect("object key");
    let (document_id, source_version_id, convert_job_id) =
        create_document(env, token, collection_id, case.title, object_key).await;
    let convert = run_convert_once(env, ctx).await?;
    if !matches!(
        convert,
        ConvertWorkerRun::Completed { job_id, .. } if job_id == convert_job_id
    ) {
        let (status, last_error) = job_status_and_error(env, ctx, convert_job_id).await;
        panic!(
            "unexpected convert outcome for {}: {convert:?}; job_status={status}; last_error={last_error:?}",
            case.name
        );
    }
    relay_outbox(env, ctx).await;
    let index = run_index_once(env, ctx)
        .await
        .expect("index worker available");
    assert!(matches!(index, IndexWorkerRun::Completed { chunks, .. } if chunks > 0));
    let current_version_id = current_version(env, ctx, document_id)
        .await
        .expect("current version");
    assert_ne!(current_version_id, source_version_id);
    Some(IngestedDoc {
        document_id,
        version_id: current_version_id,
        convert_job_id,
        collection_id,
        marker: case.marker.to_string(),
        title: case.title.to_string(),
    })
}

#[derive(Clone, Copy)]
pub struct FixtureCase {
    pub name: &'static str,
    pub filename: &'static str,
    pub content_type: &'static str,
    pub title: &'static str,
    pub bytes: &'static [u8],
    pub marker: &'static str,
}

#[derive(Clone)]
pub struct IngestedDoc {
    pub document_id: Uuid,
    pub version_id: Uuid,
    pub convert_job_id: Uuid,
    pub collection_id: Uuid,
    pub marker: String,
    pub title: String,
}

pub fn fixture_cases() -> Vec<FixtureCase> {
    vec![
        FixtureCase {
            name: "txt",
            filename: "e2e-alpha.txt",
            content_type: "text/plain",
            title: "E2E TXT",
            bytes: b"E2E-TXT-ALPHA-2026 is the unique text marker.\n",
            marker: "E2E-TXT-ALPHA-2026",
        },
        FixtureCase {
            name: "csv",
            filename: "e2e-beta.csv",
            content_type: "text/csv",
            title: "E2E CSV",
            bytes: b"name,value\nmarker,E2E-CSV-BETA-2026\n",
            marker: "E2E-CSV-BETA-2026",
        },
        FixtureCase {
            name: "html",
            filename: "e2e-gamma.html",
            content_type: "text/html",
            title: "E2E HTML",
            bytes: b"<!doctype html><html><body><h1>Policy</h1><p>E2E-HTML-GAMMA-2026 appears here.</p></body></html>",
            marker: "E2E-HTML-GAMMA-2026",
        },
        FixtureCase {
            name: "markdown",
            filename: "e2e-delta.md",
            content_type: "text/markdown",
            title: "E2E Markdown",
            bytes: b"# Markdown\n\nE2E-MD-DELTA-2026 appears in markdown body.\n",
            marker: "E2E-MD-DELTA-2026",
        },
    ]
}

pub async fn relay_outbox(env: &LiveEnv, ctx: &OrgContext) {
    let sink = Arc::new(OutboxJobSink::new());
    jobs::relay_outbox_with_sink(&env.pool, ctx, 64, &sink)
        .await
        .expect("relay outbox");
}

pub fn real_convert_config(worker_id: impl Into<String>) -> Option<ConvertWorkerConfig> {
    let Some(binary) = fileconv_binary() else {
        eprintln!("skipped: fileconv binary is not built");
        return None;
    };
    if let Err(error) = sandbox::preflight() {
        eprintln!("skipped: sandbox isolation unavailable: {error}");
        return None;
    }
    let mut config = ConvertWorkerConfig::new(
        worker_id,
        SandboxConfig {
            argv_template: vec![binary.display().to_string(), "one".into(), "{input}".into()],
            limits: ResourceLimits {
                wall_timeout: Duration::from_secs(20),
                memory_bytes: 512 * 1024 * 1024,
                cpu_seconds: 10,
                file_size_bytes: 16 * 1024 * 1024,
                max_processes: 512,
                max_open_files: 256,
                stdout_stderr_bytes: 8 * 1024 * 1024,
            },
        },
    );
    config.heartbeat_interval = Duration::from_millis(100);
    config.lease_ttl = Duration::from_secs(5);
    Some(config)
}

pub async fn run_convert_once(env: &LiveEnv, ctx: &OrgContext) -> Option<ConvertWorkerRun> {
    let config = real_convert_config(format!("e2e-convert-{}", Uuid::new_v4()))?;
    Some(
        ConvertWorker::new(env.pool.clone(), env.storage.clone(), config)
            .expect("convert worker")
            .run_once(ctx)
            .await
            .expect("convert run"),
    )
}

pub fn fileconv_binary() -> Option<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    [
        manifest.join("../../target/debug/fileconv"),
        manifest.join("../../target/release/fileconv"),
        PathBuf::from("/usr/local/bin/fileconv"),
    ]
    .into_iter()
    .find(|path| path.exists())
}

pub fn index_worker(env: &LiveEnv) -> IndexWorker {
    let mut config = IndexWorkerConfig::new(format!("e2e-index-{}", Uuid::new_v4()));
    config.lease_ttl = Duration::from_secs(30);
    config.heartbeat_interval = Duration::from_secs(5);
    config.max_job_duration = Duration::from_secs(60);
    config.embedding_batch_size = 2;
    IndexWorker::new(
        env.pool.clone(),
        env.storage.clone(),
        env.qdrant.clone(),
        config,
        None,
    )
    .expect("index worker")
}

pub async fn run_index_once(env: &LiveEnv, ctx: &OrgContext) -> Option<IndexWorkerRun> {
    Some(
        index_worker(env)
            .run_once(ctx)
            .await
            .expect("index worker run"),
    )
}

pub async fn run_delete_once(env: &LiveEnv, ctx: &OrgContext) -> DeleteWorkerRun {
    let mut config = DeleteWorkerConfig::new(format!("e2e-delete-{}", Uuid::new_v4()));
    config.lease_ttl = Duration::from_secs(30);
    config.heartbeat_interval = Duration::from_secs(5);
    config.max_job_duration = Duration::from_secs(60);
    DeleteWorker::new(
        env.pool.clone(),
        env.storage.clone(),
        env.qdrant.clone(),
        config,
    )
    .expect("delete worker")
    .run_once(ctx)
    .await
    .expect("delete worker run")
}

pub async fn current_version(env: &LiveEnv, ctx: &OrgContext, document_id: Uuid) -> Option<Uuid> {
    with_org_txn(&env.pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT current_version_id
                         FROM documents
                         WHERE org_id = $1 AND id = $2",
                        &[&ctx.org_id(), &document_id],
                    )
                    .await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .expect("current version")
}

pub async fn get_job_status(env: &LiveEnv, ctx: &OrgContext, job_id: Uuid) -> JobStatus {
    with_org_txn(&env.pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let status: String = txn
                    .query_one(
                        "SELECT status FROM jobs WHERE org_id = $1 AND id = $2",
                        &[&ctx.org_id(), &job_id],
                    )
                    .await?
                    .get(0);
                JobStatus::parse(&status).map_err(fileconv_server::db::error::DbError::Config)
            })
        }
    })
    .await
    .expect("job status")
}

pub async fn job_status_and_error(
    env: &LiveEnv,
    ctx: &OrgContext,
    job_id: Uuid,
) -> (String, Option<String>) {
    with_org_txn(&env.pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT status, last_error FROM jobs WHERE org_id = $1 AND id = $2",
                        &[&ctx.org_id(), &job_id],
                    )
                    .await?;
                Ok((row.get(0), row.get(1)))
            })
        }
    })
    .await
    .expect("job status and error")
}

pub async fn make_job_available(env: &LiveEnv, ctx: &OrgContext, job_id: Uuid) {
    with_org_txn(&env.pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "UPDATE jobs
                     SET status = 'pending', lease_owner = NULL, lease_expires_at = NULL,
                         heartbeat_at = NULL, available_at = clock_timestamp(),
                         updated_at = clock_timestamp()
                     WHERE org_id = $1 AND id = $2",
                    &[&ctx.org_id(), &job_id],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("make job available");
}

pub async fn expire_leases(env: &LiveEnv, ctx: &OrgContext) {
    jobs::reclaim_expired(&env.pool, ctx, 10, Duration::from_secs(1))
        .await
        .expect("reclaim expired");
}

pub async fn chunk_count(env: &LiveEnv, ctx: &OrgContext, document_id: Uuid) -> i64 {
    with_org_txn(&env.pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT count(*)::bigint
                         FROM chunks
                         WHERE org_id = $1 AND document_id = $2",
                        &[&ctx.org_id(), &document_id],
                    )
                    .await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .expect("chunk count")
}

pub async fn document_count(env: &LiveEnv, ctx: &OrgContext) -> i64 {
    with_org_txn(&env.pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT count(*)::bigint FROM documents WHERE org_id = $1",
                        &[&ctx.org_id()],
                    )
                    .await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .expect("document count")
}

pub async fn active_signature(env: &LiveEnv, ctx: &OrgContext, collection_id: Uuid) -> String {
    with_org_txn(&env.pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let metadata = fileconv_server::db::index_metadata::find_active(
                    txn,
                    &ctx,
                    Some(collection_id),
                )
                .await?;
                metadata
                    .map(|value| value.index_signature_sha256)
                    .ok_or(fileconv_server::db::error::DbError::NotFound)
            })
        }
    })
    .await
    .expect("active signature")
}

pub async fn qdrant_points_for_doc(
    env: &LiveEnv,
    ctx: &OrgContext,
    collection_id: Uuid,
    document_id: Uuid,
) -> usize {
    let signature = active_signature(env, ctx, collection_id).await;
    let collection = collection_name_for_digest(&signature).expect("collection name");
    env.qdrant
        .scroll_points(
            &collection,
            &VectorScope::new(ctx.org_id(), [collection_id]),
            &[json!({
                "key": "document_id",
                "match": { "value": document_id.to_string() }
            })],
            1000,
        )
        .await
        .expect("scroll points")
        .len()
}

pub async fn chunk_identities_for_doc(
    env: &LiveEnv,
    ctx: &OrgContext,
    document_id: Uuid,
) -> Vec<String> {
    with_org_txn(&env.pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let rows = txn
                    .query(
                        "SELECT chunk_identity_sha256
                         FROM chunks
                         WHERE org_id = $1 AND document_id = $2
                         ORDER BY ordinal, id",
                        &[&ctx.org_id(), &document_id],
                    )
                    .await?;
                Ok(rows.into_iter().map(|row| row.get(0)).collect())
            })
        }
    })
    .await
    .expect("chunk identities")
}

pub struct DirectQdrantDoc<'a> {
    pub ctx: &'a OrgContext,
    pub collection_id: Uuid,
    pub doc: &'a IngestedDoc,
}

pub async fn assert_direct_qdrant_tenant_filter(
    env: &LiveEnv,
    org_a: DirectQdrantDoc<'_>,
    org_b: DirectQdrantDoc<'_>,
    shared_query: &str,
) {
    let query_vector = local_vector(shared_query);
    let org_a_signature = active_signature(env, org_a.ctx, org_a.collection_id).await;
    let org_a_collection = collection_name_for_digest(&org_a_signature).expect("collection name");
    let org_a_hits = env
        .qdrant
        .search(
            &org_a_collection,
            &VectorScope::new(org_a.ctx.org_id(), [org_a.collection_id]),
            query_vector.values(),
            100,
        )
        .await
        .expect("direct qdrant org-a search");
    assert!(
        org_a_hits.iter().any(|hit| {
            hit.payload.org_id == org_a.ctx.org_id()
                && hit.payload.collection_id == org_a.collection_id
                && hit.payload.document_id == org_a.doc.document_id
        }),
        "direct Qdrant org-A control search did not return indexed org-A content"
    );
    let org_a_chunks = chunk_identities_for_doc(env, org_a.ctx, org_a.doc.document_id).await;
    assert!(!org_a_chunks.is_empty(), "org-A control doc has no chunks");

    let org_filter_only_hits = env
        .qdrant
        .search(
            &org_a_collection,
            &VectorScope::new(org_b.ctx.org_id(), [org_a.collection_id]),
            query_vector.values(),
            100,
        )
        .await
        .expect("direct qdrant org-filter-only search");
    assert!(
        org_filter_only_hits.is_empty(),
        "direct Qdrant org-B/org-A-collection search returned points"
    );

    let org_b_signature = active_signature(env, org_b.ctx, org_b.collection_id).await;
    let org_b_collection = collection_name_for_digest(&org_b_signature).expect("collection name");
    let org_b_hits = env
        .qdrant
        .search(
            &org_b_collection,
            &VectorScope::new(org_b.ctx.org_id(), [org_b.collection_id]),
            query_vector.values(),
            100,
        )
        .await
        .expect("direct qdrant org-b search");
    assert!(
        org_b_hits.iter().any(|hit| {
            hit.payload.org_id == org_b.ctx.org_id()
                && hit.payload.collection_id == org_b.collection_id
                && hit.payload.document_id == org_b.doc.document_id
        }),
        "direct Qdrant org-B search did not return indexed org-B control content"
    );
    assert!(
        org_b_hits.iter().all(|hit| {
            hit.payload.org_id != org_a.ctx.org_id()
                && hit.payload.collection_id != org_a.collection_id
                && hit.payload.document_id != org_a.doc.document_id
        }),
        "direct Qdrant org-B search leaked org-A payloads"
    );

    for chunk_identity in &org_a_chunks {
        assert!(
            org_b_hits
                .iter()
                .all(|hit| hit.payload.chunk_id.as_str() != chunk_identity.as_str()),
            "direct Qdrant org-B search leaked org-A chunk identity {chunk_identity}"
        );
    }
}

pub async fn assert_chunk_point_identity(
    env: &LiveEnv,
    ctx: &OrgContext,
    collection_id: Uuid,
    document_id: Uuid,
) {
    let chunk_identities = chunk_identities_for_doc(env, ctx, document_id).await;
    assert!(
        !chunk_identities.is_empty(),
        "document must have at least one PG chunk"
    );

    let mut unique_chunks = HashSet::with_capacity(chunk_identities.len());
    for identity in &chunk_identities {
        assert!(
            unique_chunks.insert(identity.clone()),
            "duplicated PG logical chunk identity {identity}"
        );
    }

    let expected_by_identity: HashMap<String, Uuid> = chunk_identities
        .iter()
        .map(|identity| {
            (
                identity.clone(),
                point_id_from_org_collection_and_chunk(ctx.org_id(), collection_id, identity)
                    .expect("deterministic point id"),
            )
        })
        .collect();
    let expected_ids = expected_by_identity.values().copied().collect::<Vec<_>>();
    let signature = active_signature(env, ctx, collection_id).await;
    let collection = collection_name_for_digest(&signature).expect("collection name");
    let scope = VectorScope::new(ctx.org_id(), [collection_id]);

    let fetched = env
        .qdrant
        .get_points(&collection, &scope, &expected_ids)
        .await
        .expect("fetch expected qdrant points");
    assert_eq!(
        fetched.len(),
        expected_ids.len(),
        "missing Qdrant point for at least one PG chunk"
    );
    let fetched_by_id = fetched.iter().cloned().collect::<HashMap<_, _>>();
    for (identity, expected_id) in &expected_by_identity {
        let payload = fetched_by_id
            .get(expected_id)
            .unwrap_or_else(|| panic!("missing Qdrant point for chunk {identity}"));
        assert_eq!(
            payload.chunk_id, *identity,
            "Qdrant point payload chunk identity diverged from PG"
        );
        assert_eq!(payload.document_id, document_id);
        assert_eq!(payload.collection_id, collection_id);
        assert_eq!(payload.org_id, ctx.org_id());
    }

    let scrolled = env
        .qdrant
        .scroll_points(
            &collection,
            &scope,
            &[json!({
                "key": "document_id",
                "match": { "value": document_id.to_string() }
            })],
            1000,
        )
        .await
        .expect("scroll document qdrant points");
    assert_eq!(
        scrolled.len(),
        expected_ids.len(),
        "document has orphan or duplicate Qdrant points"
    );
    let expected_id_set = expected_ids.iter().copied().collect::<HashSet<_>>();
    let mut seen_payload_chunks = HashSet::with_capacity(scrolled.len());
    for (point_id, payload) in scrolled {
        assert!(
            expected_id_set.contains(&point_id),
            "orphan Qdrant point {point_id} has no matching PG chunk"
        );
        assert!(
            seen_payload_chunks.insert(payload.chunk_id.clone()),
            "duplicated Qdrant logical chunk identity {}",
            payload.chunk_id
        );
        assert!(
            unique_chunks.contains(&payload.chunk_id),
            "Qdrant payload chunk {} has no matching PG chunk",
            payload.chunk_id
        );
    }
}

pub async fn revoke_collection_access(env: &LiveEnv, seeded: &SeededOrg, user_id: Uuid) {
    let ctx = seeded.worker_ctx();
    with_org_txn(&env.pool, &ctx, {
        let s = seeded.clone();
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "DELETE FROM collection_user_access
                     WHERE org_id = $1 AND collection_id = $2 AND user_id = $3",
                    &[&s.org_id, &s.collection_id, &user_id],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("revoke collection access");
}

pub fn parse_uuid_value(value: &Value) -> Uuid {
    Uuid::parse_str(value.as_str().expect("uuid string")).expect("uuid")
}

pub fn citation_pin_from(citation: &Value, quote: &str) -> Value {
    json!({
        "documentId": citation["documentId"],
        "versionId": citation["versionId"],
        "versionNumber": citation["versionNumber"],
        "contentSha256": citation["contentSha256"],
        "chunkId": citation["chunkId"],
        "spanStart": Value::Null,
        "spanEnd": Value::Null,
        "quote": quote,
    })
}

pub fn assert_body_lacks(bytes: &[u8], forbidden: &str) {
    let rendered = String::from_utf8_lossy(bytes);
    assert!(
        !rendered.contains(forbidden),
        "response body leaked forbidden marker"
    );
}

pub fn assert_value_lacks(value: &Value, forbidden: &str) {
    assert!(
        !value.to_string().contains(forbidden),
        "response body leaked forbidden marker"
    );
}

#[allow(dead_code)]
pub fn approved_signature() -> String {
    approved_plan()
        .index_signature(LOCAL_VECTOR_DIMENSIONS)
        .expect("approved signature")
        .digest()
}
