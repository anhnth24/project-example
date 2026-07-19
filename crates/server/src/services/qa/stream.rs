//! Grounded Q&A orchestration.
//!
//! LLM output follows a buffer-then-emit contract: provider deltas are accumulated
//! privately, then grounding and live authorization/citation validation must pass
//! before any LLM-origin `Token` is emitted. R05 can map these logical events to
//! SSE knowing fallback streams never contain unvalidated LLM text.

use deadpool_postgres::Pool;
use fileconv_knowledge::ask::extractive_answer;
use fileconv_knowledge::types::HybridSearchHit;
use futures::stream::BoxStream;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::auth::permissions::{require_permission, resolve_org_context_in_txn};
use crate::services::citation::{resolve_citation, CitationPin};
use crate::services::qa::grounding;
use crate::services::qa::prompt::{build_grounded_prompt, GroundedPrompt};
use crate::services::qa::provider::{stream_chat, LlmChatConfig};
use crate::services::retrieval::{
    retrieve, GroundedHit, RetrievalError, RetrievalRequest, VersionMode,
};
use crate::storage::qdrant::QdrantClient;

pub const DEFAULT_QA_LIMIT: usize = 8;
pub const MAX_QA_LIMIT: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QaRequest {
    pub question: String,
    pub limit: usize,
    pub mode: VersionMode,
}

impl Default for QaRequest {
    fn default() -> Self {
        Self {
            question: String::new(),
            limit: DEFAULT_QA_LIMIT,
            mode: VersionMode::Current,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QaEvent {
    Token(String),
    Citations(Vec<QaCitation>),
    Warning(String),
    Done { mode: QaAnswerMode },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QaAnswerMode {
    CloudLlm,
    FallbackExtractive,
    OfflineExtractive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QaCitation {
    pub document_id: Uuid,
    pub version_id: Uuid,
    pub version_number: i32,
    pub content_sha256: String,
    pub chunk_id: Uuid,
    pub heading_path: Vec<String>,
    pub snippet: String,
    pub page: Option<i32>,
    pub slide: Option<i32>,
    pub sheet: Option<String>,
    pub is_current: bool,
}

impl QaCitation {
    fn from_hit(hit: &GroundedHit) -> Self {
        Self {
            document_id: hit.document_id,
            version_id: hit.version_id,
            version_number: hit.version_number,
            content_sha256: hit.content_sha256.clone(),
            chunk_id: hit.chunk_id,
            heading_path: hit.heading_path.clone(),
            snippet: hit.snippet.clone(),
            page: hit.page,
            slide: hit.slide,
            sheet: hit.sheet.clone(),
            is_current: hit.is_current,
        }
    }
}

#[derive(Debug, Error)]
pub enum QaError {
    #[error("retrieval scope is empty")]
    EmptyScope,
    #[error("retrieval failed")]
    Retrieval(RetrievalError),
}

impl From<RetrievalError> for QaError {
    fn from(error: RetrievalError) -> Self {
        match error {
            RetrievalError::EmptyScope => Self::EmptyScope,
            other => Self::Retrieval(other),
        }
    }
}

pub async fn answer_question(
    pool: &Pool,
    qdrant: &QdrantClient,
    ctx: &OrgContext,
    request: QaRequest,
) -> Result<BoxStream<'static, QaEvent>, QaError> {
    let limit = request.limit.clamp(1, MAX_QA_LIMIT);
    let response = retrieve(
        pool,
        qdrant,
        ctx,
        RetrievalRequest {
            query: request.question.clone(),
            limit,
            mode: request.mode,
        },
    )
    .await
    .map_err(QaError::from)?;
    if response.hits.is_empty() {
        let answer = extractive_answer(&request.question, &[]);
        return Ok(futures::stream::iter([
            QaEvent::Token(answer),
            QaEvent::Done {
                mode: QaAnswerMode::OfflineExtractive,
            },
        ])
        .boxed());
    }

    let prompt = build_grounded_prompt(&request.question, &response.hits);
    let pool = pool.clone();
    let ctx = ctx.clone();
    let question = request.question;
    let (tx, rx) = mpsc::channel(32);
    tokio::spawn(async move {
        run_answer_stream(pool, ctx, question, prompt, tx).await;
    });
    Ok(receiver_stream(rx))
}

fn receiver_stream(rx: mpsc::Receiver<QaEvent>) -> BoxStream<'static, QaEvent> {
    futures::stream::unfold(rx, |mut rx| async {
        rx.recv().await.map(|event| (event, rx))
    })
    .boxed()
}

async fn run_answer_stream(
    pool: Pool,
    ctx: OrgContext,
    question: String,
    prompt: GroundedPrompt,
    tx: mpsc::Sender<QaEvent>,
) {
    let Some(cfg) = LlmChatConfig::from_env() else {
        emit_extractive(
            &pool,
            &ctx,
            &question,
            &prompt,
            QaAnswerMode::OfflineExtractive,
            &tx,
        )
        .await;
        return;
    };

    let mut llm_stream = match stream_chat(&cfg, &prompt.system, &prompt.user).await {
        Ok(stream) => stream,
        Err(_) => {
            send_warning(&tx, "LLM provider unavailable; using extractive fallback.").await;
            emit_extractive(
                &pool,
                &ctx,
                &question,
                &prompt,
                QaAnswerMode::FallbackExtractive,
                &tx,
            )
            .await;
            return;
        }
    };

    let mut answer = String::new();
    while let Some(delta) = llm_stream.next().await {
        match delta {
            Ok(delta) => answer.push_str(&delta),
            Err(_) => {
                send_warning(&tx, "LLM stream failed; using extractive fallback.").await;
                emit_extractive(
                    &pool,
                    &ctx,
                    &question,
                    &prompt,
                    QaAnswerMode::FallbackExtractive,
                    &tx,
                )
                .await;
                return;
            }
        }
    }

    if let Err(warnings) = grounding::validate(&answer, prompt.citations.len()) {
        for warning in warnings {
            send_warning(&tx, warning).await;
        }
        emit_extractive(
            &pool,
            &ctx,
            &question,
            &prompt,
            QaAnswerMode::FallbackExtractive,
            &tx,
        )
        .await;
        return;
    }

    let cited_indices = grounding::cited_hit_indices(&answer, prompt.citations.len());
    let finalized = finalize_citation_indices(&pool, &ctx, &prompt, cited_indices).await;
    if finalized.has_unresolved() {
        for warning in finalized.warnings {
            send_warning(&tx, warning).await;
        }
        emit_extractive(
            &pool,
            &ctx,
            &question,
            &prompt,
            QaAnswerMode::FallbackExtractive,
            &tx,
        )
        .await;
        return;
    }
    if tx.send(QaEvent::Token(answer)).await.is_err() {
        return;
    }
    if !finalized.citations.is_empty()
        && tx
            .send(QaEvent::Citations(finalized.citations))
            .await
            .is_err()
    {
        return;
    }
    let _ = tx
        .send(QaEvent::Done {
            mode: QaAnswerMode::CloudLlm,
        })
        .await;
}

async fn emit_extractive(
    pool: &Pool,
    ctx: &OrgContext,
    question: &str,
    prompt: &GroundedPrompt,
    requested_mode: QaAnswerMode,
    tx: &mpsc::Sender<QaEvent>,
) {
    let all_indices = 0..prompt.citations.len();
    let finalized = finalize_citation_indices(pool, ctx, prompt, all_indices).await;
    let mode = if finalized.has_unresolved() {
        for warning in finalized.warnings {
            send_warning(tx, warning).await;
        }
        QaAnswerMode::FallbackExtractive
    } else {
        requested_mode
    };
    let answer = extractive_answer(question, &finalized.context_hits);
    if tx.send(QaEvent::Token(answer)).await.is_err() {
        return;
    }
    if !finalized.citations.is_empty()
        && tx
            .send(QaEvent::Citations(finalized.citations))
            .await
            .is_err()
    {
        return;
    }
    let _ = tx.send(QaEvent::Done { mode }).await;
}

async fn send_warning(tx: &mpsc::Sender<QaEvent>, warning: impl Into<String>) {
    let _ = tx.send(QaEvent::Warning(warning.into())).await;
}

#[derive(Debug, Default)]
struct FinalizedCitations {
    citations: Vec<QaCitation>,
    context_hits: Vec<HybridSearchHit>,
    warnings: Vec<String>,
}

impl FinalizedCitations {
    fn has_unresolved(&self) -> bool {
        !self.warnings.is_empty()
    }
}

async fn finalize_citation_indices(
    pool: &Pool,
    ctx: &OrgContext,
    prompt: &GroundedPrompt,
    indices: impl IntoIterator<Item = usize>,
) -> FinalizedCitations {
    let mut finalized = FinalizedCitations::default();
    let fresh_ctx = match refresh_authorized_context(pool, ctx).await {
        Ok(ctx) => ctx,
        Err(warning) => {
            finalized.warnings.push(warning);
            return finalized;
        }
    };
    for index in indices {
        let Some(citation) = prompt.citations.get(index) else {
            continue;
        };
        let pin = CitationPin {
            document_id: citation.hit.document_id,
            version_id: citation.hit.version_id,
            version_number: citation.hit.version_number,
            content_sha256: citation.hit.content_sha256.clone(),
            chunk_id: citation.hit.chunk_id,
            span_start: citation.hit.span_start,
            span_end: citation.hit.span_end,
            quote: None,
        };
        match resolve_citation(pool, &fresh_ctx, pin).await {
            Ok(_) => {
                finalized
                    .citations
                    .push(QaCitation::from_hit(&citation.hit));
                if let Some(hit) = prompt.context_hits.get(index) {
                    finalized.context_hits.push(hit.clone());
                }
            }
            Err(_) => finalized.warnings.push(format!(
                "Citation {} no longer resolves against live documents; using extractive fallback.",
                citation.id
            )),
        }
    }
    finalized
}

async fn refresh_authorized_context(pool: &Pool, ctx: &OrgContext) -> Result<OrgContext, String> {
    let fresh = resolve_org_context_in_txn(pool, ctx.org_id(), ctx.user_id())
        .await
        .map_err(|_| "Authorization changed during QA finalization; using extractive fallback.")?;
    require_permission(&fresh, "qa.query")
        .map_err(|_| "QA permission was revoked during finalization; using extractive fallback.")?;
    Ok(fresh)
}
