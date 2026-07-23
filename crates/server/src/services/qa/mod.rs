//! Grounded Q&A with version-aware citations and extractive fallback (P1B-R03).

pub mod grounding;
pub mod prompt;
pub mod provider;
pub mod stream;

use std::time::Duration;

use deadpool_postgres::Pool;
use fileconv_knowledge::ask::{extractive_answer, valid_citation_ids, AnswerMode};
use fileconv_knowledge::types::{HybridSearchHit, SourceAnchor};
use thiserror::Error;
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::services::citation::{pins_from_hits, CitationPin};
use crate::services::embedding::ApprovedEmbeddingRuntime;
use crate::services::qa::grounding::{
    conflict_resolution_notes_for_history, conflict_warnings_for_current,
    validate_answer_citations, version_context_note, VersionContext,
};
use crate::services::qa::prompt::{build_grounded_messages, GroundedMessages};
use crate::services::qa::provider::{ChatProvider, ProviderError};
use crate::services::retrieval::{
    hybrid_search, RetrievalError, RetrievalHit, RetrievalRequest, VersionMode,
};
use crate::storage::qdrant::QdrantClient;

pub const ASK_TIMEOUT: Duration = Duration::from_secs(45);

/// Structured entailment is not yet an approved trusted verifier.
/// Until it is, grounded ask stays fail-closed on extractive-only answers.
const STRUCTURED_ENTAILMENT_AVAILABLE: bool = false;

/// Runtime probe used by tests/gates (avoids clippy `assertions_on_constants`).
pub fn structured_entailment_available() -> bool {
    STRUCTURED_ENTAILMENT_AVAILABLE
}

fn force_extractive_only() -> bool {
    if !STRUCTURED_ENTAILMENT_AVAILABLE {
        return true;
    }
    match std::env::var("MARKHAND_QA_EXTRACTIVE_ONLY") {
        Ok(value) => {
            let trimmed = value.trim();
            trimmed == "1" || trimmed.eq_ignore_ascii_case("true")
        }
        Err(_) => false,
    }
}

#[derive(Debug, Clone)]
pub struct AskRequest {
    pub question: String,
    pub collection_ids: Option<std::collections::BTreeSet<Uuid>>,
    pub mode: VersionMode,
    pub limit: usize,
    pub conflict_ids: Vec<Uuid>,
}

#[derive(Debug, Clone)]
pub struct AskResponse {
    pub answer: String,
    pub mode: AnswerMode,
    pub citations: Vec<CitationPin>,
    pub warnings: Vec<String>,
    pub version_context: VersionContext,
    pub embedding_mode: String,
}

#[derive(Debug, Error)]
pub enum AskError {
    #[error(transparent)]
    Retrieval(#[from] RetrievalError),
    #[error("invalid ask request: {0}")]
    InvalidRequest(&'static str),
    #[error("provider error")]
    Provider(#[from] ProviderError),
}

impl AskError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Retrieval(error) => error.code(),
            Self::InvalidRequest(_) => "ask_invalid_request",
            Self::Provider(_) => "ask_provider",
        }
    }
}

fn hits_to_hybrid(hits: &[RetrievalHit]) -> Vec<HybridSearchHit> {
    hits.iter()
        .map(|hit| HybridSearchHit {
            chunk_id: hit.chunk_identity_sha256.clone(),
            source_rel: hit.document_id.to_string(),
            md_rel: hit.version_id.to_string(),
            heading: hit.heading.clone(),
            snippet: hit.snippet.clone(),
            lexical_score: hit.lexical_score,
            vector_score: hit.vector_score,
            rerank_score: hit.rerank_score,
            anchor: SourceAnchor {
                page: hit.page,
                slide: hit.slide,
                sheet: hit.sheet.clone(),
                start: hit.span_start,
                end: hit.span_end,
            },
        })
        .collect()
}

/// Grounded ask: retrieve → optional GLM → citation validate → extractive fallback.
pub async fn ask(
    pool: &Pool,
    qdrant: &QdrantClient,
    embedder: Option<&ApprovedEmbeddingRuntime>,
    provider: Option<&ChatProvider>,
    ctx: &OrgContext,
    request: AskRequest,
) -> Result<AskResponse, AskError> {
    if request.question.trim().is_empty() {
        return Err(AskError::InvalidRequest("question is empty"));
    }
    if request.question.len() > 8_192 {
        return Err(AskError::InvalidRequest("question exceeds max length"));
    }
    let retrieval = hybrid_search(
        pool,
        qdrant,
        embedder,
        ctx,
        RetrievalRequest {
            query: request.question.clone(),
            collection_ids: request.collection_ids.clone(),
            mode: request.mode.clone(),
            limit: request.limit.clamp(1, 20),
            conflict_ids: request.conflict_ids.clone(),
        },
    )
    .await?;

    let citations = pins_from_hits(ctx.org_id(), &retrieval.hits);
    let hybrid = hits_to_hybrid(&retrieval.hits);
    let mut warnings = retrieval.warnings;
    warnings.extend(conflict_warnings_for_current(
        &request.mode,
        &retrieval.conflict_evidence,
    ));
    warnings.extend(conflict_resolution_notes_for_history(
        &request.mode,
        &retrieval.conflict_evidence,
    ));
    let version_context = version_context_note(&request.mode, &citations, &retrieval.hits);

    let extractive = extractive_answer(&request.question, &hybrid);
    let valid_ids = valid_citation_ids(hybrid.len());

    // Provider may be attempted for outage/timeout observability, but GLM answers are
    // never claimed grounded unless structured entailment is available AND validation passes.
    let (answer, mode) = match provider {
        Some(chat) if !hybrid.is_empty() => {
            let messages = build_grounded_messages(&request.question, &hybrid, &request.mode);
            match chat.complete(&messages).await {
                Ok(llm_answer) => {
                    if force_extractive_only() {
                        warnings.push(
                            "Structured entailment unavailable; fail-closed extractive-only grounding."
                                .into(),
                        );
                        (extractive, AnswerMode::OfflineExtractive)
                    } else {
                        match validate_answer_citations(
                            &llm_answer,
                            &valid_ids,
                            &citations,
                            &request.mode,
                        ) {
                            Ok(()) => (llm_answer, chat.answer_mode()),
                            Err(failure) => {
                                warnings.extend(failure.warnings);
                                warnings.push(
                                    if failure.unverifiable {
                                        "Unverifiable claim-level grounding; using extractive fallback."
                                    } else {
                                        "LLM grounding failed validation; using extractive fallback."
                                    }
                                    .into(),
                                );
                                (extractive, AnswerMode::FallbackExtractive)
                            }
                        }
                    }
                }
                Err(ProviderError::Timeout) => {
                    warnings.push("LLM provider timed out; using extractive fallback.".into());
                    (extractive, AnswerMode::FallbackExtractive)
                }
                Err(_) => {
                    warnings.push("LLM provider unavailable; using extractive fallback.".into());
                    (extractive, AnswerMode::FallbackExtractive)
                }
            }
        }
        _ => {
            if provider.is_none() {
                warnings.push("No chat provider configured; using extractive answer.".into());
            }
            if force_extractive_only() {
                warnings.push(
                    "Structured entailment unavailable; fail-closed extractive-only grounding."
                        .into(),
                );
            }
            (extractive, AnswerMode::OfflineExtractive)
        }
    };

    Ok(AskResponse {
        answer,
        mode,
        citations,
        warnings,
        version_context,
        embedding_mode: retrieval.embedding_mode,
    })
}

/// Build grounded prompt messages for streaming callers.
pub fn grounded_messages_for(
    question: &str,
    hits: &[RetrievalHit],
    mode: &VersionMode,
) -> GroundedMessages {
    let hybrid = hits_to_hybrid(hits);
    build_grounded_messages(question, &hybrid, mode)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use chrono::Utc;

    fn hit(is_current: bool, version_number: i32) -> RetrievalHit {
        RetrievalHit {
            chunk_id: Uuid::from_u128(version_number as u128),
            chunk_identity_sha256: format!("{version_number:0>64}"),
            collection_id: Uuid::from_u128(10),
            document_id: Uuid::from_u128(11),
            version_id: Uuid::from_u128(version_number as u128 + 100),
            version_number,
            content_sha256: format!("{:0>64}", version_number + 3),
            canonical_markdown_sha256: "".into(),
            heading: "Kinh phí".into(),
            snippet: format!("Version {version_number} budget value."),
            body: format!("Version {version_number} budget value."),
            lexical_score: 1.0,
            vector_score: 0.5,
            rerank_score: 1.2,
            is_current,
            effective_from: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            effective_to: None,
            page: Some(1),
            slide: None,
            sheet: None,
            span_start: 0,
            span_end: 20,
        }
    }

    #[test]
    fn hybrid_mapping_preserves_anchor_fields() {
        let mapped = hits_to_hybrid(&[hit(true, 2)]);
        assert_eq!(mapped[0].anchor.page, Some(1));
        assert_eq!(mapped[0].heading, "Kinh phí");
    }

    #[test]
    fn force_extractive_only_is_enabled_by_default() {
        // Fail-closed until a trusted structured entailment verifier ships.
        assert!(force_extractive_only());
    }
}
