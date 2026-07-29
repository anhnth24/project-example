//! Grounded Q&A with version-aware citations and extractive fallback (P1B-R03).

pub mod ask_stream;
pub mod grounding;
pub mod prompt;
pub mod provider;
pub mod stream;

use std::collections::HashSet;
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
    force_extractive_only_runtime()
}

/// Shared by JSON ask and durable SSE producer (P1B-R05).
pub fn force_extractive_only_runtime() -> bool {
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

/// Dev-gate opt-in (default OFF): allow an LLM answer that passes citation/claim
/// validation to reach the caller labeled `AnswerMode::LlmUnverified` even while
/// structured entailment is unavailable (`force_extractive_only()` true). This
/// NEVER claims grounded — the response always carries a fixed warning saying
/// entailment is unverified. Off by default; every existing mode/warning/test
/// stays bit-for-bit identical when unset.
fn allow_unverified_llm() -> bool {
    allow_unverified_llm_runtime()
}

/// Shared by JSON ask and durable SSE producer (P1B-R05).
pub fn allow_unverified_llm_runtime() -> bool {
    match std::env::var("MARKHAND_QA_ALLOW_UNVERIFIED_LLM") {
        Ok(value) => {
            let trimmed = value.trim();
            trimmed == "1" || trimmed.eq_ignore_ascii_case("true")
        }
        Err(_) => false,
    }
}

/// Fixed warning attached whenever an `AnswerMode::LlmUnverified` answer is
/// returned — must always accompany that mode so callers never mistake it for
/// grounded/verified output.
pub const UNVERIFIED_LLM_WARNING: &str =
    "Dev-gate: LLM answer passed citation/claim checks but structured entailment \
is NOT available — this answer is unverified, not grounded.";

/// Decides the final (answer, mode) for a completed LLM response, applying the
/// fail-closed / dev-gate policy, plus any extra warnings the caller must
/// attach. Pure/DB-free by design so both `ask()` (JSON) and the SSE producer
/// (`ask_stream::run_producer`) apply byte-identical semantics and so the
/// policy is unit-testable without a live Postgres/Qdrant.
///
/// `real_mode` is only used once structured entailment ships and
/// `force_extractive_only()` can return `false` in production; today that arm
/// is unreachable (kept for forward-compat rather than deleted).
pub(crate) fn resolve_llm_answer(
    llm_answer: String,
    extractive: &str,
    valid_ids: &HashSet<String>,
    citations: &[CitationPin],
    mode: &VersionMode,
    real_mode: AnswerMode,
) -> (String, AnswerMode, Vec<String>) {
    let mut warnings = Vec::new();
    if force_extractive_only() && !allow_unverified_llm() {
        warnings.push(
            "Structured entailment unavailable; fail-closed extractive-only grounding.".into(),
        );
        return (
            extractive.to_string(),
            AnswerMode::OfflineExtractive,
            warnings,
        );
    }
    match validate_answer_citations(&llm_answer, valid_ids, citations, mode) {
        Ok(()) => {
            if force_extractive_only() {
                // Dev-gate path: passed validation, but structured entailment is
                // still unavailable — never claim grounded.
                warnings.push(UNVERIFIED_LLM_WARNING.into());
                (llm_answer, AnswerMode::LlmUnverified, warnings)
            } else {
                (llm_answer, real_mode, warnings)
            }
        }
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
            (
                extractive.to_string(),
                AnswerMode::FallbackExtractive,
                warnings,
            )
        }
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

pub(crate) fn hits_to_hybrid(hits: &[RetrievalHit]) -> Vec<HybridSearchHit> {
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
                    let (answer, resolved_mode, extra_warnings) = resolve_llm_answer(
                        llm_answer,
                        &extractive,
                        &valid_ids,
                        &citations,
                        &request.mode,
                        chat.answer_mode(),
                    );
                    warnings.extend(extra_warnings);
                    (answer, resolved_mode)
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
            document_title: "Ngân sách".into(),
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

    // --- Dev-gate (MARKHAND_QA_ALLOW_UNVERIFIED_LLM) ---
    //
    // `resolve_llm_answer` is pure/DB-free, so these exercise the exact policy
    // shared by ask() and the SSE producer without a live Postgres/Qdrant.
    // Env var mutation is process-global, so serialize with a mutex and always
    // restore to unset afterwards (the crate-wide default state).

    static ENV_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());
    const GATE_VAR: &str = "MARKHAND_QA_ALLOW_UNVERIFIED_LLM";

    fn with_dev_gate<T>(enabled: bool, run: impl FnOnce() -> T) -> T {
        let guard = ENV_GUARD
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if enabled {
            std::env::set_var(GATE_VAR, "1");
        } else {
            std::env::remove_var(GATE_VAR);
        }
        let result = run();
        std::env::remove_var(GATE_VAR);
        drop(guard);
        result
    }

    fn test_pin(cite_id: &str, version: u128, quote: &str) -> CitationPin {
        CitationPin {
            cite_id: cite_id.into(),
            org_id: Uuid::from_u128(1),
            logical_document_id: Uuid::from_u128(2),
            version_id: Uuid::from_u128(version),
            version_number: version as i32,
            source_content_sha256: "c".repeat(64),
            canonical_markdown_sha256: "f".repeat(64),
            quote_sha256: "e".repeat(64),
            chunk_id: Uuid::from_u128(4),
            chunk_identity_sha256: "d".repeat(64),
            collection_id: Uuid::from_u128(5),
            document_title: Some("Ngân sách".into()),
            heading: "Kinh phí".into(),
            quote: quote.into(),
            page: None,
            slide: None,
            sheet: None,
            source_span_start: 0,
            source_span_end: quote.len(),
            quote_local_start: 0,
            quote_local_end: quote.len(),
            effective_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            effective_to: None,
            is_current: true,
            anchor: "mhcite1.x".into(),
        }
    }

    #[test]
    fn dev_gate_off_is_bit_identical_fail_closed_regression() {
        with_dev_gate(false, || {
            assert!(!allow_unverified_llm_runtime());
            let pins = vec![test_pin("CITE-0001", 1, "Kinh phí là 10 triệu đồng")];
            let valid = HashSet::from(["CITE-0001".to_string()]);
            let (answer, mode, warnings) = resolve_llm_answer(
                "Kinh phí là 10 triệu [CITE-0001].".into(),
                "## Trả lời trích xuất\n\nextractive fallback text",
                &valid,
                &pins,
                &VersionMode::Current,
                AnswerMode::CloudLlm,
            );
            // Regression: with the gate unset, behavior must match the pre-dev-gate
            // code path exactly — extractive answer, OfflineExtractive, single
            // fail-closed warning, LLM answer never surfaced even though it would
            // have passed validation.
            assert_eq!(answer, "## Trả lời trích xuất\n\nextractive fallback text");
            assert_eq!(mode, AnswerMode::OfflineExtractive);
            assert_eq!(warnings.len(), 1);
            assert!(warnings[0].contains("fail-closed"));
        });
    }

    #[test]
    fn dev_gate_on_valid_citation_returns_llm_unverified_with_fixed_warning() {
        with_dev_gate(true, || {
            assert!(allow_unverified_llm_runtime());
            let pins = vec![test_pin("CITE-0001", 1, "Kinh phí là 10 triệu đồng")];
            let valid = HashSet::from(["CITE-0001".to_string()]);
            let (answer, mode, warnings) = resolve_llm_answer(
                "Kinh phí là 10 triệu [CITE-0001].".into(),
                "extractive fallback text",
                &valid,
                &pins,
                &VersionMode::Current,
                AnswerMode::CloudLlm,
            );
            assert_eq!(answer, "Kinh phí là 10 triệu [CITE-0001].");
            assert_eq!(mode, AnswerMode::LlmUnverified);
            assert_eq!(mode.as_str(), "llm_unverified");
            assert!(warnings.iter().any(|w| w == UNVERIFIED_LLM_WARNING));
            assert!(
                warnings
                    .iter()
                    .any(|w| w.to_lowercase().contains("not")
                        && w.to_lowercase().contains("grounded"))
            );
        });
    }

    #[test]
    fn dev_gate_on_fabricated_citation_still_falls_back_extractive() {
        with_dev_gate(true, || {
            let pins = vec![test_pin("CITE-0001", 1, "Kinh phí là 10 triệu đồng")];
            let valid = HashSet::from(["CITE-0001".to_string()]);
            let (answer, mode, warnings) = resolve_llm_answer(
                "Kinh phí là 99 triệu [CITE-9999].".into(),
                "extractive fallback text",
                &valid,
                &pins,
                &VersionMode::Current,
                AnswerMode::CloudLlm,
            );
            assert_eq!(answer, "extractive fallback text");
            assert_eq!(mode, AnswerMode::FallbackExtractive);
            assert!(!warnings.iter().any(|w| w == UNVERIFIED_LLM_WARNING));
            assert!(warnings
                .iter()
                .any(|w| w.contains("Fabricated") || w.contains("unknown")));
        });
    }

    #[test]
    fn dev_gate_on_wrong_compare_delta_falls_back_extractive() {
        with_dev_gate(true, || {
            let pins = vec![
                test_pin("CITE-0001", 11, "Kinh phí là 10 triệu đồng."),
                test_pin("CITE-0002", 12, "Kinh phí là 15 triệu đồng."),
            ];
            let mode = VersionMode::Compare {
                document_id: Uuid::from_u128(2),
                version_a: pins[0].version_id,
                version_b: pins[1].version_id,
            };
            let valid = HashSet::from(["CITE-0001".to_string(), "CITE-0002".to_string()]);
            // Wrong delta: newer value attributed to the older citation.
            let (answer, resolved_mode, warnings) = resolve_llm_answer(
                "Kinh phí cũ là 15 triệu [CITE-0001]. Kinh phí mới là 10 triệu [CITE-0002].".into(),
                "extractive fallback text",
                &valid,
                &pins,
                &mode,
                AnswerMode::CloudLlm,
            );
            assert_eq!(answer, "extractive fallback text");
            assert_eq!(resolved_mode, AnswerMode::FallbackExtractive);
            assert!(!warnings.iter().any(|w| w == UNVERIFIED_LLM_WARNING));
        });
    }

    /// End-to-end through the real `ChatProvider::complete` call (not just the
    /// pure policy function), same wiring `ask()` uses — a hermetic stand-in
    /// for an HTTP-mocked `OpenAiCompatibleChat` run, since `ChatProvider::Static`
    /// exercises the identical call path without a live GLM endpoint.
    /// Plain `#[test]` + `block_on` (not `#[tokio::test]`) so the env-var mutex
    /// guard is never held across an actual `.await` suspension point.
    #[test]
    fn dev_gate_on_wired_through_real_provider_complete_call() {
        use crate::services::qa::provider::StaticChatProvider;

        let guard = ENV_GUARD
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        std::env::set_var(GATE_VAR, "1");

        let provider = ChatProvider::Static(StaticChatProvider::new(
            "Kinh phí là 10 triệu [CITE-0001].",
            AnswerMode::LocalLlm,
        ));
        let messages = GroundedMessages {
            system: "s".into(),
            user: "u".into(),
        };
        let llm_answer = futures::executor::block_on(provider.complete(&messages)).unwrap();
        let pins = vec![test_pin("CITE-0001", 1, "Kinh phí là 10 triệu đồng")];
        let valid = HashSet::from(["CITE-0001".to_string()]);
        let (answer, mode, warnings) = resolve_llm_answer(
            llm_answer,
            "extractive fallback",
            &valid,
            &pins,
            &VersionMode::Current,
            provider.answer_mode(),
        );

        std::env::remove_var(GATE_VAR);
        drop(guard);

        assert_eq!(answer, "Kinh phí là 10 triệu [CITE-0001].");
        assert_eq!(mode, AnswerMode::LlmUnverified);
        assert!(warnings.iter().any(|w| w == UNVERIFIED_LLM_WARNING));
    }
}
