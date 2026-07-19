//! Live tests for the grounded Q&A engine.
//!
//! These tests skip cleanly unless PostgreSQL and Qdrant test endpoints are
//! provided. They are intentionally not part of the normal library test gate.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use deadpool_postgres::Pool;
use fileconv_knowledge::embedding::{local_vector, LOCAL_VECTOR_DIMENSIONS};
use fileconv_knowledge::identity::chunk_identity;
use fileconv_server::auth::context::OrgContext;
use fileconv_server::config::SecretString;
use fileconv_server::database::apply_migrations;
use fileconv_server::db::chunks::{self, NewChunk};
use fileconv_server::db::collections::{self, NewCollection};
use fileconv_server::db::index_metadata::{self, EnsureGeneration};
use fileconv_server::db::models::{CollectionVisibility, EmbeddingRuntimePath};
use fileconv_server::db::orgs;
use fileconv_server::db::pool::{create_pool, with_org_txn};
use fileconv_server::services::deletion;
use fileconv_server::services::embedding::approved_plan;
use fileconv_server::services::qa::grounding;
use fileconv_server::services::qa::{answer_question, QaAnswerMode, QaError, QaEvent, QaRequest};
use fileconv_server::services::retrieval::VersionMode;
use fileconv_server::storage::qdrant::{ChunkPointPayload, QdrantClient, UpsertPoint, VectorScope};
use futures::StreamExt;
use sha2::{Digest, Sha256};
use tokio_postgres::NoTls;
use uuid::Uuid;

static LLM_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const LLM_ENV_KEYS: &[&str] = &[
    "FILECONV_LLM_PROVIDER",
    "FILECONV_LLM_API_KEY",
    "FILECONV_LLM_MODEL",
    "FILECONV_LLM_BASE_URL",
];

fn test_database_url() -> Option<String> {
    match std::env::var("MARKHAND_TEST_DATABASE_URL") {
        Ok(url) if !url.trim().is_empty() => Some(url),
        _ => {
            eprintln!("skipped: MARKHAND_TEST_DATABASE_URL unset");
            None
        }
    }
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
        let db_name = format!("markhand_qa_{}", Uuid::new_v4().simple());
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
    qdrant: QdrantClient,
    base_ctx: OrgContext,
}

impl LiveEnv {
    async fn boot() -> Option<Self> {
        let base_url = test_database_url()?;
        let qdrant = test_qdrant_client()?;
        let db = EphemeralDb::create(&base_url).await;
        apply_migrations(&db.url).await.expect("apply migrations");
        let pool = create_pool(&db.url).expect("pool");
        let base_ctx = OrgContext::try_new(
            Uuid::new_v4(),
            Uuid::new_v4(),
            ["qa.query", "doc.delete"],
            [],
        )
        .expect("org context");
        Some(Self {
            db,
            pool,
            qdrant,
            base_ctx,
        })
    }

    fn ctx_for(&self, collection_ids: impl IntoIterator<Item = Uuid>) -> OrgContext {
        OrgContext::try_new(
            self.base_ctx.org_id(),
            self.base_ctx.user_id(),
            self.base_ctx.permissions().iter().cloned(),
            collection_ids,
        )
        .expect("scoped context")
    }

    async fn drop(self) {
        self.db.drop().await;
    }
}

#[derive(Debug, Clone)]
struct SeededDocument {
    document_id: Uuid,
}

async fn seed_indexed_document(
    env: &LiveEnv,
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
    let content_sha256 = hex::encode(Sha256::digest(body.as_bytes()));
    let heading_path = vec![heading.to_string()];
    let chunk_identity = chunk_identity(
        &document_id.to_string(),
        &version_id.to_string(),
        0,
        &heading_path.join(" > "),
        body,
        &body_text_version,
    );
    let metadata = with_org_txn(&env.pool, &env.base_ctx, {
        let ctx = env.base_ctx.clone();
        let title = title.to_string();
        let heading_path = heading_path.clone();
        let body = body.to_string();
        let content_sha256 = content_sha256.clone();
        let chunk_identity = chunk_identity.clone();
        let signature_digest = signature_digest.clone();
        let runtime_path = runtime_path.clone();
        let embedding_family = embedding_family.clone();
        let embedding_revision = embedding_revision.clone();
        let chunking_version = chunking_version.clone();
        let body_text_version = body_text_version.clone();
        let query_normalization_version = query_normalization_version.clone();
        move |txn| {
            Box::pin(async move {
                orgs::ensure_exists(txn, &ctx, "qa-org", "QA Org").await?;
                orgs::ensure_user(txn, &ctx, ctx.user_id(), "qa@example.test", "QA").await?;
                orgs::ensure_membership(txn, &ctx).await?;
                seed_owner_permissions(txn, &ctx, &["qa.query", "doc.delete"]).await?;
                let collection_name = format!("QA {collection_id}");
                let collection_slug = format!("qa-{collection_id}");
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
                txn.execute(
                    "INSERT INTO documents (
                        id, org_id, collection_id, title, state, created_by_user_id
                     )
                     VALUES ($1, $2, $3, $4, 'indexed', $5)",
                    &[
                        &document_id,
                        &ctx.org_id(),
                        &collection_id,
                        &title,
                        &ctx.user_id(),
                    ],
                )
                .await?;
                let object_key = format!("qa/{version_id}.md");
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
                    "UPDATE documents
                     SET current_version_id = $3, updated_at = clock_timestamp()
                     WHERE org_id = $1 AND id = $2",
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
                        dimensions: i32::try_from(dimensions).expect("signature dimensions"),
                        normalized,
                        runtime_path: EmbeddingRuntimePath::parse(&runtime_path)
                            .expect("runtime path"),
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
            &VectorScope::new(env.base_ctx.org_id(), [collection_id]),
            &[UpsertPoint {
                chunk_identity: chunk_identity.clone(),
                vector: local_vector(body).into_values(),
                payload: ChunkPointPayload {
                    org_id: env.base_ctx.org_id(),
                    collection_id,
                    document_id,
                    version_id,
                    chunk_id: chunk_identity,
                    ordinal: 0,
                    is_current: true,
                    is_effective: true,
                    index_generation: u32::try_from(metadata.generation)
                        .expect("metadata generation"),
                },
            }],
        )
        .await
        .expect("upsert qdrant point");
    SeededDocument { document_id }
}

async fn seed_owner_permissions(
    txn: &tokio_postgres::Transaction<'_>,
    ctx: &OrgContext,
    codes: &[&str],
) -> Result<(), fileconv_server::db::error::DbError> {
    let role_id = Uuid::new_v4();
    txn.execute(
        "INSERT INTO roles (id, org_id, code, name, is_system)
         VALUES ($1, $2, 'owner', 'Owner', true)
         ON CONFLICT (org_id, code) DO NOTHING",
        &[&role_id, &ctx.org_id()],
    )
    .await?;
    let role_id: Uuid = txn
        .query_one(
            "SELECT id FROM roles WHERE org_id = $1 AND code = 'owner'",
            &[&ctx.org_id()],
        )
        .await?
        .get(0);
    for code in codes {
        txn.execute(
            "INSERT INTO permissions (id, code, description)
             VALUES ($1, $2, $2)
             ON CONFLICT (code) DO NOTHING",
            &[&Uuid::new_v4(), code],
        )
        .await?;
        let permission_id: Uuid = txn
            .query_one("SELECT id FROM permissions WHERE code = $1", &[code])
            .await?
            .get(0);
        txn.execute(
            "INSERT INTO role_permissions (org_id, role_id, permission_id)
             VALUES ($1, $2, $3)
             ON CONFLICT DO NOTHING",
            &[&ctx.org_id(), &role_id, &permission_id],
        )
        .await?;
    }
    Ok(())
}

struct LlmEnvGuard {
    _guard: tokio::sync::MutexGuard<'static, ()>,
    saved: Vec<(&'static str, Option<String>)>,
}

impl LlmEnvGuard {
    async fn lock() -> Self {
        let guard = LLM_ENV_LOCK.lock().await;
        let saved = LLM_ENV_KEYS
            .iter()
            .map(|key| (*key, std::env::var(key).ok()))
            .collect();
        Self {
            _guard: guard,
            saved,
        }
    }

    fn unset_all(&self) {
        for key in LLM_ENV_KEYS {
            std::env::remove_var(key);
        }
    }

    fn set_openai(&self, api_key: &str, base_url: &str) {
        std::env::set_var("FILECONV_LLM_PROVIDER", "openai");
        std::env::set_var("FILECONV_LLM_API_KEY", api_key);
        std::env::set_var("FILECONV_LLM_MODEL", "gpt-4o");
        std::env::set_var("FILECONV_LLM_BASE_URL", base_url);
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

async fn collect_events(mut stream: futures::stream::BoxStream<'static, QaEvent>) -> Vec<QaEvent> {
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event);
    }
    events
}

fn token_text(events: &[QaEvent]) -> String {
    events
        .iter()
        .filter_map(|event| match event {
            QaEvent::Token(token) => Some(token.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

fn done_mode(events: &[QaEvent]) -> Option<QaAnswerMode> {
    events.iter().rev().find_map(|event| match event {
        QaEvent::Done { mode } => Some(*mode),
        _ => None,
    })
}

fn citation_count(events: &[QaEvent]) -> usize {
    events
        .iter()
        .filter_map(|event| match event {
            QaEvent::Citations(citations) => Some(citations.len()),
            _ => None,
        })
        .sum()
}

fn qa_request(question: &str) -> QaRequest {
    QaRequest {
        question: question.into(),
        limit: 5,
        mode: VersionMode::Current,
    }
}

#[tokio::test]
async fn live_qa_offline_extractive() {
    let env_guard = LlmEnvGuard::lock().await;
    env_guard.unset_all();
    let Some(env) = LiveEnv::boot().await else {
        return;
    };
    let collection_id = Uuid::new_v4();
    seed_indexed_document(
        &env,
        collection_id,
        "Offline",
        "Chính sách",
        "Tài liệu nói mã QA-OFFLINE-2026 cần xử lý đối soát trong ngày.",
    )
    .await;
    let ctx = env.ctx_for([collection_id]);
    let stream = answer_question(&env.pool, &env.qdrant, &ctx, qa_request("QA-OFFLINE-2026"))
        .await
        .expect("qa answer");
    let events = collect_events(stream).await;
    let answer = token_text(&events);
    assert!(answer.contains("QA-OFFLINE-2026"));
    assert!(answer.contains("[CITE-0001]"));
    assert_eq!(done_mode(&events), Some(QaAnswerMode::OfflineExtractive));
    assert_eq!(citation_count(&events), 1);
    assert!(grounding::validate(&answer, 1).is_ok());
    env.drop().await;
}

#[tokio::test]
async fn live_qa_empty_scope_denied() {
    let env_guard = LlmEnvGuard::lock().await;
    env_guard.unset_all();
    let Some(env) = LiveEnv::boot().await else {
        return;
    };
    let empty = env.ctx_for([]);
    let result = answer_question(&env.pool, &env.qdrant, &empty, qa_request("anything")).await;
    assert!(matches!(result, Err(QaError::EmptyScope)));
    env.drop().await;
}

#[tokio::test]
async fn live_qa_cross_collection_excluded() {
    let env_guard = LlmEnvGuard::lock().await;
    env_guard.unset_all();
    let Some(env) = LiveEnv::boot().await else {
        return;
    };
    let allowed = Uuid::new_v4();
    let denied = Uuid::new_v4();
    seed_indexed_document(
        &env,
        allowed,
        "Allowed",
        "A",
        "Nội dung được phép chứa mã QA-CROSS-2026.",
    )
    .await;
    let denied_doc = seed_indexed_document(
        &env,
        denied,
        "Denied",
        "B",
        "Nội dung bị chặn chứa mã QA-CROSS-2026.",
    )
    .await;
    let ctx = env.ctx_for([allowed]);
    let stream = answer_question(&env.pool, &env.qdrant, &ctx, qa_request("QA-CROSS-2026"))
        .await
        .expect("qa answer");
    let events = collect_events(stream).await;
    let answer = token_text(&events);
    assert!(answer.contains("được phép"));
    assert!(!answer.contains("bị chặn"));
    assert!(events.iter().all(|event| match event {
        QaEvent::Citations(citations) => citations
            .iter()
            .all(|citation| citation.document_id != denied_doc.document_id),
        _ => true,
    }));
    env.drop().await;
}

#[tokio::test]
async fn live_qa_provider_error_falls_back() {
    let env_guard = LlmEnvGuard::lock().await;
    env_guard.set_openai("dummy-key", "http://127.0.0.1:1/v1");
    let Some(env) = LiveEnv::boot().await else {
        return;
    };
    let collection_id = Uuid::new_v4();
    seed_indexed_document(
        &env,
        collection_id,
        "Provider Error",
        "Fallback",
        "QA-PROVIDER-ERR-2026 là mã dùng để kiểm tra fallback.",
    )
    .await;
    let ctx = env.ctx_for([collection_id]);
    let stream = answer_question(
        &env.pool,
        &env.qdrant,
        &ctx,
        qa_request("QA-PROVIDER-ERR-2026"),
    )
    .await
    .expect("qa answer");
    let events = collect_events(stream).await;
    assert!(events
        .iter()
        .any(|event| matches!(event, QaEvent::Warning(_))));
    assert!(token_text(&events).contains("## Trả lời trích xuất"));
    assert_eq!(done_mode(&events), Some(QaAnswerMode::FallbackExtractive));
    env.drop().await;
}

fn sse_delta(content: &str) -> String {
    format!(
        "data: {}\n\n",
        serde_json::json!({"choices":[{"delta":{"content":content}}]})
    )
}

fn start_sse_stub(first_delta: &str, wait_before_done: bool) -> (String, Option<Sender<()>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind sse stub");
    let addr = listener.local_addr().expect("stub addr");
    let (gate_tx, gate_rx) = wait_before_done.then(mpsc::channel).unzip();
    let first = sse_delta(first_delta);
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept request");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("read timeout");
        let mut request = Vec::new();
        let mut buf = [0_u8; 512];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let read = stream.read(&mut buf).expect("read request");
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buf[..read]);
        }
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            )
            .expect("write headers");
        stream
            .write_all(first.as_bytes())
            .expect("write first delta");
        stream.flush().expect("flush first delta");
        if let Some(rx) = gate_rx {
            rx.recv_timeout(Duration::from_secs(10)).expect("wait gate");
        }
        stream.write_all(b"data: [DONE]\n\n").expect("write done");
    });
    (format!("http://{addr}/v1"), gate_tx)
}

#[tokio::test]
async fn live_qa_grounding_failure_falls_back() {
    let env_guard = LlmEnvGuard::lock().await;
    let Some(env) = LiveEnv::boot().await else {
        return;
    };
    let (base_url, _) = start_sse_stub("Câu trả lời dùng citation giả. [CITE-9999]", false);
    env_guard.set_openai("dummy-key", &base_url);
    let collection_id = Uuid::new_v4();
    seed_indexed_document(
        &env,
        collection_id,
        "Grounding",
        "G",
        "QA-GROUNDING-2026 là bằng chứng hợp lệ duy nhất.",
    )
    .await;
    let ctx = env.ctx_for([collection_id]);
    let stream = answer_question(
        &env.pool,
        &env.qdrant,
        &ctx,
        qa_request("QA-GROUNDING-2026"),
    )
    .await
    .expect("qa answer");
    let events = collect_events(stream).await;
    assert!(events
        .iter()
        .any(|event| matches!(event, QaEvent::Warning(_))));
    assert!(token_text(&events).contains("## Trả lời trích xuất"));
    assert_eq!(done_mode(&events), Some(QaAnswerMode::FallbackExtractive));
    env.drop().await;
}

#[tokio::test]
async fn live_qa_delete_during_finalization() {
    let env_guard = LlmEnvGuard::lock().await;
    let Some(env) = LiveEnv::boot().await else {
        return;
    };
    let (base_url, gate) = start_sse_stub("QA-DELETE-2026 có trong tài liệu. [CITE-0001]", true);
    env_guard.set_openai("dummy-key", &base_url);
    let collection_id = Uuid::new_v4();
    let doc = seed_indexed_document(
        &env,
        collection_id,
        "Delete During Finalization",
        "D",
        "QA-DELETE-2026 là nội dung sẽ bị tombstone trước khi finalize citation.",
    )
    .await;
    let ctx = env.ctx_for([collection_id]);
    let stream = answer_question(&env.pool, &env.qdrant, &ctx, qa_request("QA-DELETE-2026"))
        .await
        .expect("qa answer");
    deletion::request_delete(&env.pool, &ctx, doc.document_id)
        .await
        .expect("tombstone before finalization");
    gate.expect("gate sender").send(()).expect("release stub");
    let events = collect_events(stream).await;
    assert!(events
        .iter()
        .any(|event| matches!(event, QaEvent::Warning(_))));
    assert_eq!(done_mode(&events), Some(QaAnswerMode::FallbackExtractive));
    let answer = token_text(&events);
    assert!(!answer.contains("QA-DELETE-2026 có trong tài liệu."));
    assert!(answer.contains("Không tìm thấy bằng chứng phù hợp"));
    assert!(events.iter().all(|event| match event {
        QaEvent::Citations(citations) => citations
            .iter()
            .all(|citation| citation.document_id != doc.document_id),
        _ => true,
    }));
    env.drop().await;
}

#[tokio::test]
async fn live_qa_cloud_llm_gpt4o() {
    let env_guard = LlmEnvGuard::lock().await;
    let api_key = match std::env::var("FILECONV_LLM_API_KEY") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => {
            eprintln!("skipped: FILECONV_LLM_API_KEY unset");
            return;
        }
    };
    env_guard.set_openai(&api_key, "https://api.openai.com/v1");
    let Some(env) = LiveEnv::boot().await else {
        return;
    };
    let collection_id = Uuid::new_v4();
    let doc = seed_indexed_document(
        &env,
        collection_id,
        "Đối soát",
        "Đối soát",
        "Quy trình đối soát giao dịch được thực hiện tự động mỗi ngày một lần vào lúc 02:00 sáng.",
    )
    .await;
    let ctx = env.ctx_for([collection_id]);
    let stream = answer_question(
        &env.pool,
        &env.qdrant,
        &ctx,
        qa_request("Quy trình đối soát giao dịch được thực hiện bao lâu một lần?"),
    )
    .await
    .expect("qa answer");
    let events = collect_events(stream).await;
    assert!(events
        .iter()
        .any(|event| matches!(event, QaEvent::Token(_))));
    assert_eq!(done_mode(&events), Some(QaAnswerMode::CloudLlm));
    assert!(events.iter().all(|event| match event {
        QaEvent::Citations(citations) => citations
            .iter()
            .all(|citation| citation.document_id == doc.document_id),
        _ => true,
    }));
    env.drop().await;
}
