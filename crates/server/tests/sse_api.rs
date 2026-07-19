//! Live HTTP integration tests for P1B-R05 search/ask/SSE APIs.
//!
//! These tests self-skip unless PostgreSQL and Qdrant test endpoints are provided.
//! They are intentionally not run by the normal library test gate.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use deadpool_postgres::Pool;
use fileconv_knowledge::embedding::{local_vector, LOCAL_VECTOR_DIMENSIONS};
use fileconv_knowledge::identity::chunk_identity;
use fileconv_server::auth::context::OrgContext;
use fileconv_server::auth::jwt::JwtKeys;
use fileconv_server::auth::provider::{AuthProvider, AuthRequestMeta, PasswordAuthProvider};
use fileconv_server::auth::session;
use fileconv_server::config::{
    Argon2Config, AuthConfig, JwtAlgorithm, RuntimeEndpoints, SecretString, ServerConfig,
};
use fileconv_server::database::apply_migrations;
use fileconv_server::db::chunks::{self, NewChunk};
use fileconv_server::db::collections::{self, NewCollection};
use fileconv_server::db::index_metadata::{self, EnsureGeneration};
use fileconv_server::db::models::{CollectionVisibility, EmbeddingRuntimePath};
use fileconv_server::db::orgs;
use fileconv_server::db::pool::{create_pool, with_org_txn};
use fileconv_server::http::{router, AppState};
use fileconv_server::services::embedding::approved_plan;
use fileconv_server::state::RuntimeState;
use fileconv_server::storage::qdrant::{ChunkPointPayload, QdrantClient, UpsertPoint, VectorScope};
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

fn test_qdrant_client() -> Option<(String, QdrantClient)> {
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
    let client = QdrantClient::with_api_key(url.clone(), api_key).expect("qdrant client");
    Some((url, client))
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
        issuer: Some("https://issuer.r05.markhand.test".into()),
        audience: Some("markhand-api".into()),
        signing_key: Some(SecretString::new("r05-integration-signing-key-abcdef")),
        alg: JwtAlgorithm::Hs256,
        kid: Some("r05-test-kid".into()),
        access_token_ttl_secs: 900,
        refresh_token_ttl_secs: 3_600,
        argon2: Argon2Config {
            memory_kib: 8_192,
            time_cost: 1,
            parallelism: 1,
        },
    }
}

fn test_runtime(database_url: &str, qdrant_url: String) -> RuntimeState {
    RuntimeState::from_config(ServerConfig::test_with_endpoints_and_auth(
        RuntimeEndpoints {
            database_url: SecretString::new(database_url),
            qdrant_url,
            minio_url: "http://127.0.0.1:1".into(),
        },
        test_auth_config(),
    ))
    .expect("runtime")
}

struct LiveEnv {
    db: EphemeralDb,
    pool: Pool,
    provider: PasswordAuthProvider,
    qdrant: QdrantClient,
    app: axum::Router,
}

impl LiveEnv {
    async fn boot() -> Option<Self> {
        let base_url = test_database_url()?;
        let (qdrant_url, qdrant) = test_qdrant_client()?;
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
            AppState::from_parts_with_clients(
                test_runtime(&db.url, qdrant_url),
                pool.clone(),
                Some(PasswordAuthProvider::new(
                    pool.clone(),
                    auth.clone(),
                    JwtKeys::from_auth(&auth).expect("jwt keys"),
                )),
                None,
                qdrant.clone(),
            )
            .expect("app state"),
        );
        Some(Self {
            db,
            pool,
            provider,
            qdrant,
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
    allowed_job_id: Uuid,
    denied_job_id: Uuid,
}

async fn seed(env: &LiveEnv) -> Seeded {
    let org_id = Uuid::new_v4();
    let owner_id = Uuid::new_v4();
    let other_user_id = Uuid::new_v4();
    let no_query_id = Uuid::new_v4();
    let collection_id = Uuid::new_v4();
    let denied_collection_id = Uuid::new_v4();
    let ctx = OrgContext::try_new(org_id, owner_id, ["qa.query"], [collection_id])
        .expect("owner context");
    let allowed = seed_indexed_document(
        env,
        &ctx,
        collection_id,
        "Allowed R05",
        "R05 Heading",
        "Markhand R05 answers cite this authorized Vietnamese content.",
    )
    .await;
    let denied_document_id = Uuid::new_v4();
    let denied_job_id = Uuid::new_v4();
    let allowed_job_id = Uuid::new_v4();
    with_org_txn(&env.pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                seed_user_and_roles(txn, org_id, owner_id, other_user_id, no_query_id).await?;
                collections::insert(
                    txn,
                    &ctx,
                    NewCollection {
                        id: denied_collection_id,
                        name: "Denied R05",
                        slug: "denied-r05",
                        description: None,
                        visibility: CollectionVisibility::Private,
                    },
                )
                .await?;
                txn.execute(
                    "UPDATE collections SET owner_user_id = $3 WHERE org_id = $1 AND id = $2",
                    &[&org_id, &denied_collection_id, &other_user_id],
                )
                .await?;
                txn.execute(
                    "INSERT INTO documents (id, org_id, collection_id, title, state, created_by_user_id)
                     VALUES ($1, $2, $3, 'Denied job doc', 'indexed', $4)",
                    &[&denied_document_id, &org_id, &denied_collection_id, &other_user_id],
                )
                .await?;
                insert_job(txn, org_id, allowed_job_id, allowed.document_id).await?;
                insert_job(txn, org_id, denied_job_id, denied_document_id).await
            })
        }
    })
    .await
    .expect("seed r05 rows");
    for (user_id, password) in [
        (owner_id, "owner-password"),
        (other_user_id, "other-password"),
        (no_query_id, "no-query-password"),
    ] {
        session::set_password_hash(&env.pool, user_id, password, &test_auth_config().argon2)
            .await
            .expect("password hash");
    }
    Seeded {
        collection_id,
        denied_collection_id,
        allowed_job_id,
        denied_job_id,
    }
}

struct SeededDocument {
    document_id: Uuid,
}

async fn seed_indexed_document(
    env: &LiveEnv,
    ctx: &OrgContext,
    collection_id: Uuid,
    title: &str,
    heading: &str,
    body: &str,
) -> SeededDocument {
    let plan = approved_plan();
    let signature = plan
        .index_signature(LOCAL_VECTOR_DIMENSIONS)
        .expect("index signature");
    let signature_digest = signature.digest();
    let runtime_path = signature.runtime_path.to_string();
    let embedding_family = signature.embedding_family.to_string();
    let embedding_revision = signature.embedding_revision.to_string();
    let dimensions = signature.dimensions;
    let normalized = signature.normalized;
    let chunking_version = signature.chunking_version.to_string();
    let body_text_version = signature.body_text_version.to_string();
    let query_normalization_version = signature.query_normalization_version.to_string();
    let collection_name = env
        .qdrant
        .ensure_collection_for_signature(&signature)
        .await
        .expect("ensure qdrant collection");
    let document_id = Uuid::new_v4();
    let version_id = Uuid::new_v4();
    let chunk_id = Uuid::new_v4();
    let heading_path = vec![heading.to_string()];
    let content_sha256 = hex::encode(Sha256::digest(body.as_bytes()));
    let chunk_identity = chunk_identity(
        &document_id.to_string(),
        &version_id.to_string(),
        0,
        &heading_path.join(" > "),
        body,
        &body_text_version,
    );
    let metadata = with_org_txn(&env.pool, ctx, {
        let ctx = ctx.clone();
        let title = title.to_string();
        let heading_path = heading_path.clone();
        let body = body.to_string();
        let content_sha256 = content_sha256.clone();
        let chunk_identity = chunk_identity.clone();
        move |txn| {
            Box::pin(async move {
                orgs::ensure_exists(txn, &ctx, "r05-org", "R05 Org").await?;
                orgs::ensure_user(txn, &ctx, ctx.user_id(), "owner-r05@example.test", "Owner")
                    .await?;
                orgs::ensure_membership(txn, &ctx).await?;
                collections::insert(
                    txn,
                    &ctx,
                    NewCollection {
                        id: collection_id,
                        name: "Allowed R05",
                        slug: "allowed-r05",
                        description: None,
                        visibility: CollectionVisibility::Org,
                    },
                )
                .await?;
                txn.execute(
                    "INSERT INTO documents (id, org_id, collection_id, title, state, created_by_user_id)
                     VALUES ($1, $2, $3, $4, 'indexed', $5)",
                    &[&document_id, &ctx.org_id(), &collection_id, &title, &ctx.user_id()],
                )
                .await?;
                let object_key = format!("r05/{version_id}.md");
                txn.execute(
                    "INSERT INTO document_versions (
                        id, org_id, document_id, version_number, publication_state,
                        is_current, content_sha256, original_object_key, markdown_object_key,
                        source_content_type, byte_size, created_by_user_id
                     )
                     VALUES ($1, $2, $3, 1, 'published', true, $4, $5, $5,
                             'text/markdown', $6, $7)",
                    &[
                        &version_id,
                        &ctx.org_id(),
                        &document_id,
                        &content_sha256,
                        &object_key,
                        &(body.len() as i64),
                        &ctx.user_id(),
                    ],
                )
                .await?;
                txn.execute(
                    "UPDATE documents SET current_version_id = $3 WHERE org_id = $1 AND id = $2",
                    &[&ctx.org_id(), &document_id, &version_id],
                )
                .await?;
                let metadata = index_metadata::ensure_active_generation(
                    txn,
                    &ctx,
                    EnsureGeneration {
                        collection_id: Some(collection_id),
                        signature_sha256: &signature_digest,
                        chunking_version: &chunking_version,
                        body_text_version: &body_text_version,
                        query_normalization_version: &query_normalization_version,
                        embedding_family: &embedding_family,
                        embedding_revision: &embedding_revision,
                        dimensions: i32::try_from(dimensions).expect("dimensions"),
                        normalized,
                        runtime_path: EmbeddingRuntimePath::parse(&runtime_path).expect("runtime"),
                    },
                )
                .await?;
                chunks::insert(
                    txn,
                    &ctx,
                    NewChunk {
                        id: chunk_id,
                        document_id,
                        version_id,
                        ordinal: 0,
                        heading_path: &heading_path,
                        body: &body,
                        body_text_version: &body_text_version,
                        chunk_identity_sha256: &chunk_identity,
                        index_metadata_id: metadata.id,
                        index_signature: &signature_digest,
                    },
                )
                .await?;
                Ok(metadata)
            })
        }
    })
    .await
    .expect("seed indexed document");
    env.qdrant
        .upsert_points(
            &collection_name,
            &VectorScope::new(ctx.org_id(), [collection_id]),
            &[UpsertPoint {
                chunk_identity: chunk_identity.clone(),
                vector: local_vector(body).into_values(),
                payload: ChunkPointPayload {
                    org_id: ctx.org_id(),
                    collection_id,
                    document_id,
                    version_id,
                    chunk_id: chunk_identity,
                    ordinal: 0,
                    is_current: true,
                    is_effective: true,
                    index_generation: u32::try_from(metadata.generation).expect("generation"),
                },
            }],
        )
        .await
        .expect("upsert point");
    SeededDocument { document_id }
}

async fn seed_user_and_roles(
    txn: &tokio_postgres::Transaction<'_>,
    org_id: Uuid,
    owner_id: Uuid,
    other_user_id: Uuid,
    no_query_id: Uuid,
) -> Result<(), fileconv_server::db::error::DbError> {
    for (user_id, email, name) in [
        (other_user_id, "other-r05@example.test", "Other"),
        (no_query_id, "no-query-r05@example.test", "No Query"),
    ] {
        txn.execute(
            "INSERT INTO users (id, email, display_name) VALUES ($1, $2, $3)",
            &[&user_id, &email, &name],
        )
        .await?;
    }
    txn.execute(
        "INSERT INTO org_memberships (org_id, user_id, role)
         VALUES ($1, $2, 'owner'), ($1, $3, 'viewer'), ($1, $4, 'editor')
         ON CONFLICT DO NOTHING",
        &[&org_id, &owner_id, &other_user_id, &no_query_id],
    )
    .await?;
    for (role, permission) in [
        ("owner", "qa.query"),
        ("viewer", "qa.query"),
        ("editor", "doc.upload"),
    ] {
        grant_permission(txn, org_id, role, permission).await?;
    }
    Ok(())
}

async fn grant_permission(
    txn: &tokio_postgres::Transaction<'_>,
    org_id: Uuid,
    role: &str,
    permission: &str,
) -> Result<(), fileconv_server::db::error::DbError> {
    txn.execute(
        "INSERT INTO roles (id, org_id, code, name, is_system)
         VALUES ($1, $2, $3, $3, true)
         ON CONFLICT (org_id, code) DO NOTHING",
        &[&Uuid::new_v4(), &org_id, &role],
    )
    .await?;
    txn.execute(
        "INSERT INTO permissions (id, code, description)
         VALUES ($1, $2, $2)
         ON CONFLICT (code) DO NOTHING",
        &[&Uuid::new_v4(), &permission],
    )
    .await?;
    let role_id: Uuid = txn
        .query_one(
            "SELECT id FROM roles WHERE org_id = $1 AND code = $2",
            &[&org_id, &role],
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

async fn insert_job(
    txn: &tokio_postgres::Transaction<'_>,
    org_id: Uuid,
    job_id: Uuid,
    document_id: Uuid,
) -> Result<(), fileconv_server::db::error::DbError> {
    let payload = json!({ "document_id": document_id });
    txn.execute(
        "INSERT INTO jobs (
            id, org_id, job_type, status, payload_version, payload, attempts,
            max_attempts, idempotency_key, document_id, available_at, finished_at
         )
         VALUES ($1, $2, 'convert', 'succeeded', 1, $3, 1, 1, $4, $5,
                 clock_timestamp(), clock_timestamp())",
        &[
            &job_id,
            &org_id,
            &payload,
            &format!("r05-job-{job_id}"),
            &document_id,
        ],
    )
    .await?;
    Ok(())
}

async fn post_json(app: axum::Router, token: &str, uri: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({}));
    (status, body)
}

async fn sse_body(
    app: axum::Router,
    token: &str,
    uri: &str,
    body: Option<Value>,
    last_event_id: Option<&str>,
) -> (StatusCode, String) {
    let mut builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"));
    if let Some(last_event_id) = last_event_id {
        builder = builder.header("last-event-id", last_event_id);
    }
    let body = body.map(|value| value.to_string()).unwrap_or_default();
    let response = app
        .oneshot(builder.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

fn parse_sse(body: &str) -> Vec<(String, Value)> {
    body.split("\n\n")
        .filter_map(|frame| {
            let id = frame
                .lines()
                .find_map(|line| line.strip_prefix("id: "))
                .map(str::to_string)?;
            let data = frame.lines().find_map(|line| line.strip_prefix("data: "))?;
            Some((id, serde_json::from_str(data).expect("json envelope")))
        })
        .collect()
}

#[tokio::test]
async fn search_enforces_scope_permission_and_query_bounds() {
    let Some(env) = LiveEnv::boot().await else {
        return;
    };
    let seeded = seed(&env).await;
    let owner = env.token("owner-r05@example.test", "owner-password").await;
    let no_query = env
        .token("no-query-r05@example.test", "no-query-password")
        .await;

    let (status, body) = post_json(
        env.app.clone(),
        &owner,
        "/api/v1/search",
        json!({ "query": "Markhand R05", "collectionIds": [seeded.collection_id] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["hits"][0]["collectionId"],
        seeded.collection_id.to_string()
    );

    let (status, body) = post_json(
        env.app.clone(),
        &owner,
        "/api/v1/search",
        json!({ "query": "Markhand", "collectionIds": [seeded.denied_collection_id] }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "empty_scope");

    let (status, _) = post_json(
        env.app.clone(),
        &no_query,
        "/api/v1/search",
        json!({ "query": "Markhand" }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = post_json(
        env.app.clone(),
        &owner,
        "/api/v1/search",
        json!({ "query": "x".repeat(4097) }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    env.drop().await;
}

#[tokio::test]
async fn ask_json_stream_and_resume_are_caller_bound() {
    let Some(env) = LiveEnv::boot().await else {
        return;
    };
    let seeded = seed(&env).await;
    let owner = env.token("owner-r05@example.test", "owner-password").await;
    let other = env.token("other-r05@example.test", "other-password").await;

    let (status, body) = post_json(
        env.app.clone(),
        &owner,
        "/api/v1/ask",
        json!({ "question": "What does R05 content say?", "collectionIds": [seeded.collection_id] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["answer"]
        .as_str()
        .is_some_and(|value| !value.is_empty()));

    let (status, body) = sse_body(
        env.app.clone(),
        &owner,
        "/api/v1/ask/stream",
        Some(json!({ "question": "What does R05 content say?", "collectionIds": [seeded.collection_id] })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let events = parse_sse(&body);
    assert!(!events.is_empty());
    assert_eq!(events.last().unwrap().1["event"].as_str(), Some("ask.done"));
    for (expected, (id, envelope)) in (1_u64..).zip(events.iter()) {
        assert!(id.ends_with(&format!(":{expected}")));
        assert_eq!(envelope["sequence"].as_u64(), Some(expected));
    }

    let resume_from = &events[0].0;
    let (status, replay) = sse_body(
        env.app.clone(),
        &owner,
        "/api/v1/ask/stream",
        None,
        Some(resume_from),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let replayed = parse_sse(&replay);
    assert_eq!(replayed.len(), events.len().saturating_sub(1));

    let (status, _) = sse_body(
        env.app.clone(),
        &other,
        "/api/v1/ask/stream",
        None,
        Some(resume_from),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    env.drop().await;
}

#[tokio::test]
async fn job_events_emit_safe_terminal_snapshot_and_hide_unauthorized_jobs() {
    let Some(env) = LiveEnv::boot().await else {
        return;
    };
    let seeded = seed(&env).await;
    let owner = env.token("owner-r05@example.test", "owner-password").await;

    let response = env
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/jobs/{}/events", seeded.allowed_job_id))
                .header("authorization", format!("Bearer {owner}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    let events = parse_sse(&body);
    assert_eq!(events.last().unwrap().1["event"], "job.done");
    let data = &events.last().unwrap().1["data"];
    assert_eq!(data["id"], seeded.allowed_job_id.to_string());
    assert!(data.get("payload").is_none());
    assert!(data.get("checkpoint").is_none());
    assert!(data.get("leaseOwner").is_none());

    let response = env
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/jobs/{}/events", seeded.denied_job_id))
                .header("authorization", format!("Bearer {owner}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    env.drop().await;
}
