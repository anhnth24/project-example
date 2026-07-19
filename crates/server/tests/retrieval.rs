//! Live tests for tenant-scoped hybrid retrieval.
//!
//! These tests skip cleanly unless PostgreSQL, MinIO, and Qdrant test endpoints
//! are provided in the environment. They are intentionally not part of the normal
//! library test gate.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use deadpool_postgres::Pool;
use fileconv_knowledge::embedding::{local_vector, LOCAL_VECTOR_DIMENSIONS};
use fileconv_server::auth::context::OrgContext;
use fileconv_server::config::{MinioConfig, SecretString};
use fileconv_server::database::apply_migrations;
use fileconv_server::db::collections::{self, NewCollection};
use fileconv_server::db::documents::{self, NewDocument};
use fileconv_server::db::models::{ArtifactKind, CollectionVisibility};
use fileconv_server::db::orgs;
use fileconv_server::db::pool::{create_pool, with_org_txn};
use fileconv_server::jobs::{self, EventPayload, CURRENT_EVENT_PAYLOAD_VERSION};
use fileconv_server::services::deletion;
use fileconv_server::services::embedding::approved_plan;
use fileconv_server::services::indexing::OutboxJobSink;
use fileconv_server::services::retrieval::{
    retrieve, Degradation, RetrievalError, RetrievalRequest, VersionMode,
};
use fileconv_server::storage::minio::{MinioClient, ObjectIdentityMeta};
use fileconv_server::storage::qdrant::{ChunkPointPayload, QdrantClient, UpsertPoint, VectorScope};
use fileconv_server::storage::trusted_key;
use fileconv_server::workers::index::{IndexWorker, IndexWorkerConfig, IndexWorkerRun};
use sha2::{Digest, Sha256};
use tokio_postgres::NoTls;
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
    let bucket = format!("markhand-retrieval-{}", Uuid::new_v4().simple());
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

struct EphemeralDb {
    admin_url: String,
    db_name: String,
    url: String,
}

impl EphemeralDb {
    async fn create(base_url: &str) -> Self {
        let db_name = format!("markhand_retrieval_{}", Uuid::new_v4().simple());
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

struct LiveEnv {
    db: EphemeralDb,
    pool: Pool,
    storage: MinioClient,
    qdrant: QdrantClient,
    base_ctx: OrgContext,
}

impl LiveEnv {
    async fn boot() -> Option<Self> {
        let base_url = test_database_url()?;
        let storage = test_minio_client()?;
        let qdrant = test_qdrant_client()?;
        storage.ensure_bucket().await.expect("ensure bucket");
        let db = EphemeralDb::create(&base_url).await;
        apply_migrations(&db.url).await.expect("apply migrations");
        let pool = create_pool(&db.url).expect("pool");
        let base_ctx = OrgContext::try_new(Uuid::new_v4(), Uuid::new_v4(), ["doc.upload"], [])
            .expect("org context");
        Some(Self {
            db,
            pool,
            storage,
            qdrant,
            base_ctx,
        })
    }

    fn retrieval_ctx(&self, collection_ids: impl IntoIterator<Item = Uuid>) -> OrgContext {
        OrgContext::try_new(
            self.base_ctx.org_id(),
            self.base_ctx.user_id(),
            self.base_ctx.permissions().iter().cloned(),
            collection_ids,
        )
        .expect("retrieval context")
    }

    async fn drop(self) {
        self.db.drop().await;
    }
}

async fn seed_converted_document(
    env: &LiveEnv,
    collection_id: Uuid,
    title: &str,
    markdown: &str,
) -> (Uuid, Uuid) {
    let document_id = Uuid::new_v4();
    let version_id = Uuid::new_v4();
    let artifact_id = Uuid::new_v4();
    let outbox_key = format!("index-request:{version_id}");
    let object_key =
        trusted_key(env.base_ctx.org_id(), version_id, Uuid::new_v4(), None).expect("trusted key");
    let object_key_string = object_key.as_str();
    let sha256 = hex::encode(Sha256::digest(markdown.as_bytes()));
    let markdown_len = markdown.len() as i64;
    env.storage
        .put_object(
            env.base_ctx.org_id(),
            &object_key,
            Bytes::copy_from_slice(markdown.as_bytes()),
            &ObjectIdentityMeta {
                org_id: env.base_ctx.org_id(),
                collection_id: Some(collection_id),
                document_id: Some(document_id),
                version_id: Some(version_id),
                original_filename: None,
                canonical_format: Some("md".into()),
                content_sha256: Some(sha256.clone()),
                content_length: Some(markdown.len() as u64),
                disposition: Some("trusted".into()),
            },
            "text/markdown; charset=utf-8",
        )
        .await
        .expect("put markdown");

    with_org_txn(&env.pool, &env.base_ctx, {
        let ctx = env.base_ctx.clone();
        let object_key_string = object_key_string.clone();
        let sha256 = sha256.clone();
        let title = title.to_string();
        move |txn| {
            Box::pin(async move {
                orgs::ensure_exists(txn, &ctx, "retrieval-org", "Retrieval Org").await?;
                orgs::ensure_user(
                    txn,
                    &ctx,
                    ctx.user_id(),
                    "retrieval@example.test",
                    "Retrieval",
                )
                .await?;
                orgs::ensure_membership(txn, &ctx).await?;
                txn.execute(
                    "INSERT INTO org_quotas (
                        org_id, max_storage_bytes, max_documents,
                        max_concurrent_jobs, max_monthly_tokens
                     )
                     VALUES ($1, 1000000000, 100000, 100, 1000000000)
                     ON CONFLICT (org_id) DO NOTHING",
                    &[&ctx.org_id()],
                )
                .await?;
                let collection_name = format!("Retrieval Collection {collection_id}");
                let collection_slug = format!("retrieval-{collection_id}");
                collections::insert(
                    txn,
                    &ctx,
                    NewCollection {
                        id: collection_id,
                        name: &collection_name,
                        slug: &collection_slug,
                        description: None,
                        visibility: CollectionVisibility::Private,
                    },
                )
                .await?;
                documents::insert(
                    txn,
                    &ctx,
                    NewDocument {
                        id: document_id,
                        collection_id,
                        title: &title,
                    },
                )
                .await?;
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
                        &sha256,
                        &object_key_string,
                        &markdown_len,
                        &ctx.user_id(),
                    ],
                )
                .await?;
                let kind = ArtifactKind::Markdown.as_str();
                txn.execute(
                    "INSERT INTO derived_artifacts (
                        id, org_id, document_id, version_id, artifact_kind,
                        object_key, content_sha256, content_type, byte_size
                     )
                     VALUES ($1, $2, $3, $4, $5, $6, $7,
                             'text/markdown; charset=utf-8', $8)",
                    &[
                        &artifact_id,
                        &ctx.org_id(),
                        &document_id,
                        &version_id,
                        &kind,
                        &object_key_string,
                        &sha256,
                        &markdown_len,
                    ],
                )
                .await?;
                txn.execute(
                    "UPDATE documents
                     SET state = 'converted', current_version_id = $3, updated_at = clock_timestamp()
                     WHERE org_id = $1 AND id = $2",
                    &[&ctx.org_id(), &document_id, &version_id],
                )
                .await?;
                let payload = EventPayload {
                    job_id: None,
                    document_id: Some(document_id),
                    version_id: Some(version_id),
                    outbox_event_id: None,
                }
                .to_json()
                .expect("event payload");
                txn.execute(
                    "INSERT INTO outbox_events (
                        org_id, event_type, payload_version, payload, idempotency_key
                     )
                     VALUES ($1, 'document.index_requested', $2, $3, $4)",
                    &[
                        &ctx.org_id(),
                        &CURRENT_EVENT_PAYLOAD_VERSION,
                        &payload,
                        &outbox_key,
                    ],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("seed converted document");
    (document_id, version_id)
}

async fn seed_promoted_current_version(
    env: &LiveEnv,
    document_id: Uuid,
    previous_version_id: Uuid,
    markdown: &str,
) -> Uuid {
    let version_id = Uuid::new_v4();
    let artifact_id = Uuid::new_v4();
    let outbox_key = format!("index-request:{version_id}");
    let object_key =
        trusted_key(env.base_ctx.org_id(), version_id, Uuid::new_v4(), None).expect("trusted key");
    let object_key_string = object_key.as_str();
    let sha256 = hex::encode(Sha256::digest(markdown.as_bytes()));
    let markdown_len = markdown.len() as i64;
    env.storage
        .put_object(
            env.base_ctx.org_id(),
            &object_key,
            Bytes::copy_from_slice(markdown.as_bytes()),
            &ObjectIdentityMeta {
                org_id: env.base_ctx.org_id(),
                collection_id: None,
                document_id: Some(document_id),
                version_id: Some(version_id),
                original_filename: None,
                canonical_format: Some("md".into()),
                content_sha256: Some(sha256.clone()),
                content_length: Some(markdown.len() as u64),
                disposition: Some("trusted".into()),
            },
            "text/markdown; charset=utf-8",
        )
        .await
        .expect("put promoted markdown");

    with_org_txn(&env.pool, &env.base_ctx, {
        let ctx = env.base_ctx.clone();
        let object_key_string = object_key_string.clone();
        let sha256 = sha256.clone();
        move |txn| {
            Box::pin(async move {
                let effective_to = txn
                    .query_one("SELECT clock_timestamp()", &[])
                    .await?
                    .get::<_, chrono::DateTime<chrono::Utc>>(0);
                txn.execute(
                    "UPDATE document_versions
                     SET is_current = false, effective_to = $4
                     WHERE org_id = $1 AND document_id = $2 AND id = $3",
                    &[
                        &ctx.org_id(),
                        &document_id,
                        &previous_version_id,
                        &effective_to,
                    ],
                )
                .await?;
                txn.execute(
                    "INSERT INTO document_versions (
                        id, org_id, document_id, version_number, publication_state,
                        is_current, content_sha256, original_object_key, markdown_object_key,
                        source_content_type, byte_size, created_by_user_id
                     )
                     SELECT $1, $2, $3, COALESCE(MAX(version_number), 0)::integer + 1,
                            'published', true, $4, $5, $5, 'text/markdown', $6, $7
                     FROM document_versions
                     WHERE org_id = $2 AND document_id = $3",
                    &[
                        &version_id,
                        &ctx.org_id(),
                        &document_id,
                        &sha256,
                        &object_key_string,
                        &markdown_len,
                        &ctx.user_id(),
                    ],
                )
                .await?;
                let kind = ArtifactKind::Markdown.as_str();
                txn.execute(
                    "INSERT INTO derived_artifacts (
                        id, org_id, document_id, version_id, artifact_kind,
                        object_key, content_sha256, content_type, byte_size
                     )
                     VALUES ($1, $2, $3, $4, $5, $6, $7,
                             'text/markdown; charset=utf-8', $8)",
                    &[
                        &artifact_id,
                        &ctx.org_id(),
                        &document_id,
                        &version_id,
                        &kind,
                        &object_key_string,
                        &sha256,
                        &markdown_len,
                    ],
                )
                .await?;
                txn.execute(
                    "UPDATE documents
                     SET current_version_id = $3, state = 'converted', updated_at = clock_timestamp()
                     WHERE org_id = $1 AND id = $2",
                    &[&ctx.org_id(), &document_id, &version_id],
                )
                .await?;
                let payload = EventPayload {
                    job_id: None,
                    document_id: Some(document_id),
                    version_id: Some(version_id),
                    outbox_event_id: None,
                }
                .to_json()
                .expect("event payload");
                txn.execute(
                    "INSERT INTO outbox_events (
                        org_id, event_type, payload_version, payload, idempotency_key
                     )
                     VALUES ($1, 'document.index_requested', $2, $3, $4)",
                    &[
                        &ctx.org_id(),
                        &CURRENT_EVENT_PAYLOAD_VERSION,
                        &payload,
                        &outbox_key,
                    ],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("seed promoted version");
    version_id
}

fn index_worker(env: &LiveEnv) -> IndexWorker {
    let mut config = IndexWorkerConfig::new(format!("retrieval-worker-{}", Uuid::new_v4()));
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
    .expect("worker")
}

async fn relay(env: &LiveEnv) {
    let sink = Arc::new(OutboxJobSink::new());
    jobs::relay_outbox_with_sink(&env.pool, &env.base_ctx, 32, &sink)
        .await
        .expect("relay outbox");
}

async fn run_next_index(env: &LiveEnv) {
    relay(env).await;
    let run = index_worker(env)
        .run_once(&env.base_ctx)
        .await
        .expect("index run");
    assert!(matches!(run, IndexWorkerRun::Completed { .. }));
}

async fn retrieve_once(
    env: &LiveEnv,
    ctx: &OrgContext,
    query: &str,
) -> fileconv_server::services::retrieval::RetrievalResponse {
    retrieve(
        &env.pool,
        &env.qdrant,
        ctx,
        RetrievalRequest {
            query: query.to_string(),
            limit: 10,
            mode: VersionMode::Current,
        },
    )
    .await
    .expect("retrieve")
}

#[tokio::test]
async fn live_hybrid_retrieval_ranks_and_grounds() {
    let Some(env) = LiveEnv::boot().await else {
        return;
    };
    let collection_id = Uuid::new_v4();
    let markdown = "# Đối soát\n\nNội dung thanh toán mã đặc biệt VANGANH-2026 cần đối soát theo ngày.\n\n## Phụ lục\n\nDòng khác về hóa đơn.\n";
    seed_converted_document(&env, collection_id, "Grounded", markdown).await;
    run_next_index(&env).await;
    let ctx = env.retrieval_ctx([collection_id]);

    let first = retrieve_once(&env, &ctx, "VANGANH-2026").await;
    let second = retrieve_once(&env, &ctx, "VANGANH-2026").await;
    assert!(!first.hits.is_empty());
    assert_eq!(
        first
            .hits
            .iter()
            .map(|hit| hit.chunk_identity.clone())
            .collect::<Vec<_>>(),
        second
            .hits
            .iter()
            .map(|hit| hit.chunk_identity.clone())
            .collect::<Vec<_>>()
    );
    let hit = &first.hits[0];
    assert!(hit.snippet.contains("VANGANH-2026"));
    assert!(hit.lexical_score >= 0.0);
    assert!(hit.vector_score >= 0.0);
    assert!(hit.rerank_score.is_finite());
    assert_eq!(hit.collection_id, collection_id);
    env.drop().await;
}

#[tokio::test]
async fn live_retrieval_denies_empty_scope() {
    let Some(env) = LiveEnv::boot().await else {
        return;
    };
    let empty = env.retrieval_ctx([]);
    let error = retrieve(
        &env.pool,
        &env.qdrant,
        &empty,
        RetrievalRequest {
            query: "anything".into(),
            limit: 10,
            mode: VersionMode::Current,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(error, RetrievalError::EmptyScope));
    env.drop().await;
}

#[tokio::test]
async fn live_retrieval_excludes_other_collection() {
    let Some(env) = LiveEnv::boot().await else {
        return;
    };
    let allowed_collection = Uuid::new_v4();
    let denied_collection = Uuid::new_v4();
    seed_converted_document(
        &env,
        allowed_collection,
        "Allowed",
        "# A\n\nNội dung được phép với KHOAPHAM-2026.\n",
    )
    .await;
    run_next_index(&env).await;
    seed_converted_document(
        &env,
        denied_collection,
        "Denied",
        "# B\n\nNội dung bị chặn với KHOAPHAM-2026.\n",
    )
    .await;
    run_next_index(&env).await;
    let ctx = env.retrieval_ctx([allowed_collection]);

    let response = retrieve_once(&env, &ctx, "KHOAPHAM-2026").await;
    assert!(!response.hits.is_empty());
    assert!(response
        .hits
        .iter()
        .all(|hit| hit.collection_id == allowed_collection));
    env.drop().await;
}

#[tokio::test]
async fn live_retrieval_excludes_tombstoned() {
    let Some(env) = LiveEnv::boot().await else {
        return;
    };
    let collection_id = Uuid::new_v4();
    let (document_id, _) = seed_converted_document(
        &env,
        collection_id,
        "Deleted",
        "# Xóa\n\nTài liệu có mã XOANG-2026 sẽ bị tombstone.\n",
    )
    .await;
    run_next_index(&env).await;
    deletion::request_delete(&env.pool, &env.base_ctx, document_id)
        .await
        .expect("request delete");
    let ctx = env.retrieval_ctx([collection_id]);

    let response = retrieve_once(&env, &ctx, "XOANG-2026").await;
    assert!(response.hits.is_empty());
    env.drop().await;
}

#[tokio::test]
async fn live_retrieval_current_excludes_superseded() {
    let Some(env) = LiveEnv::boot().await else {
        return;
    };
    let collection_id = Uuid::new_v4();
    let (document_id, old_version) = seed_converted_document(
        &env,
        collection_id,
        "Versions",
        "# Cũ\n\nPhiên bản cũ chứa mã CUONGOLD-2026.\n",
    )
    .await;
    run_next_index(&env).await;
    let new_version = seed_promoted_current_version(
        &env,
        document_id,
        old_version,
        "# Mới\n\nPhiên bản mới chứa mã CUONGNEW-2026.\n",
    )
    .await;
    run_next_index(&env).await;
    let ctx = env.retrieval_ctx([collection_id]);

    // Current mode must never surface the superseded version's chunks. (The
    // near-identical current-version chunk may still fuzzy-match the query via
    // the local-hash vector, so assert on version exclusion, not emptiness.)
    let old_response = retrieve_once(&env, &ctx, "CUONGOLD-2026").await;
    assert!(
        old_response
            .hits
            .iter()
            .all(|hit| hit.version_id != old_version),
        "current mode returned a superseded version chunk"
    );
    let new_response = retrieve_once(&env, &ctx, "CUONGNEW-2026").await;
    assert!(!new_response.hits.is_empty());
    assert!(new_response
        .hits
        .iter()
        .all(|hit| hit.version_id == new_version && hit.is_current));
    env.drop().await;
}

#[tokio::test]
async fn live_retrieval_as_of_returns_effective_version() {
    let Some(env) = LiveEnv::boot().await else {
        return;
    };
    let collection_id = Uuid::new_v4();
    let (document_id, old_version) = seed_converted_document(
        &env,
        collection_id,
        "As Of",
        "# Cũ\n\nPhiên bản hiệu lực cũ chứa mã ASOFOLD-2026.\n",
    )
    .await;
    run_next_index(&env).await;
    let as_of_old = chrono::Utc::now();
    let new_version = seed_promoted_current_version(
        &env,
        document_id,
        old_version,
        "# Mới\n\nPhiên bản hiệu lực mới chứa mã ASOFNEW-2026.\n",
    )
    .await;
    run_next_index(&env).await;
    let ctx = env.retrieval_ctx([collection_id]);

    let old_response = retrieve(
        &env.pool,
        &env.qdrant,
        &ctx,
        RetrievalRequest {
            query: "ASOFOLD-2026".into(),
            limit: 10,
            mode: VersionMode::AsOf(as_of_old),
        },
    )
    .await
    .expect("as-of old retrieve");
    assert!(!old_response.hits.is_empty());
    assert!(old_response
        .hits
        .iter()
        .all(|hit| hit.version_id == old_version && !hit.is_current));
    let current_response = retrieve_once(&env, &ctx, "ASOFNEW-2026").await;
    assert!(current_response
        .hits
        .iter()
        .all(|hit| hit.version_id == new_version));
    env.drop().await;
}

#[tokio::test]
async fn live_retrieval_stale_vector_no_text() {
    let Some(env) = LiveEnv::boot().await else {
        return;
    };
    let collection_id = Uuid::new_v4();
    let ctx = env.retrieval_ctx([collection_id]);
    let plan = approved_plan();
    let signature = plan
        .index_signature(LOCAL_VECTOR_DIMENSIONS)
        .expect("signature");
    let collection_name = env
        .qdrant
        .ensure_collection_for_signature(&signature)
        .await
        .expect("ensure collection");
    let query = "vector only stale PHANTOM-2026";
    env.qdrant
        .upsert_points(
            &collection_name,
            &VectorScope::new(env.base_ctx.org_id(), [collection_id]),
            &[UpsertPoint {
                chunk_identity: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    .into(),
                vector: local_vector(query).into_values(),
                payload: ChunkPointPayload {
                    org_id: env.base_ctx.org_id(),
                    collection_id,
                    document_id: Uuid::new_v4(),
                    version_id: Uuid::new_v4(),
                    chunk_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                        .into(),
                    ordinal: 0,
                    is_current: true,
                    is_effective: true,
                    index_generation: 1,
                },
            }],
        )
        .await
        .expect("upsert stale point");

    let response = retrieve_once(&env, &ctx, query).await;
    assert!(response.hits.is_empty());
    env.drop().await;
}

#[tokio::test]
async fn live_retrieval_one_leg_outage() {
    let Some(env) = LiveEnv::boot().await else {
        return;
    };
    let collection_id = Uuid::new_v4();
    seed_converted_document(
        &env,
        collection_id,
        "Outage",
        "# Sự cố\n\nFTS vẫn tìm thấy mã SONGHAN-2026 khi vector lỗi.\n",
    )
    .await;
    run_next_index(&env).await;
    let ctx = env.retrieval_ctx([collection_id]);
    let broken_qdrant = QdrantClient::new("http://127.0.0.1:1").expect("broken client");

    let response = retrieve(
        &env.pool,
        &broken_qdrant,
        &ctx,
        RetrievalRequest {
            query: "SONGHAN-2026".into(),
            limit: 10,
            mode: VersionMode::Current,
        },
    )
    .await
    .expect("retrieve with vector outage");
    assert_eq!(response.degraded, Some(Degradation::VectorUnavailable));
    assert!(!response.hits.is_empty());
    assert!(response
        .hits
        .iter()
        .all(|hit| hit.collection_id == collection_id));
    env.drop().await;
}
