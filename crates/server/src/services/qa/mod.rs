//! Grounded Q&A over already-authorized retrieval hits (P1B-R03).
//!
//! Operates only on [`RetrievalResponse`] evidence. No database queries, ACL
//! mutations, distributed locks, or route/SSE resume logic. Streaming is
//! validate-then-replay of server-rendered chunks.

pub mod grounding;
pub mod prompt;
pub mod provider;
pub mod stream;

use std::time::{Duration, Instant};

use serde_json::{json, Value as JsonValue};
use thiserror::Error;
use uuid::Uuid;

use crate::db::search::AuthorizedConflictEvidence;
use crate::services::qa::grounding::{
    append_server_notes, build_version_context, conflict_messages, conflicts_from_evidence,
    extractive_answer, extractive_structured_claims, passages_for_mode, render_answer_from_claims,
    validate_rendered_answer, validate_structured_claims, ConflictWarning, GroundingError,
    GroundingPassage, ProviderGroundedPayload, StructuredClaim, VersionContext,
};
use crate::services::qa::prompt::{
    frame_passages, frame_question, grounded_user_prompt, system_policy_is_separated,
    PromptPassage, GROUNDED_SYSTEM_POLICY,
};
use crate::services::qa::provider::{
    ChatCompletionRequest, ProviderError, QaChatProvider, QaProviderConfig,
};
use crate::services::qa::stream::{
    replay_validated_answer, AuthProbeDecision, StreamBounds, StreamCancel, StreamEvent,
};
use crate::services::retrieval::{RetrievalHit, RetrievalResponse, VersionMode};

pub use grounding::{
    cite_id, ConflictLifecycle, ConflictLifecycle as QaConflictLifecycle,
    GroundingPassage as QaPassage,
};
pub use prompt::GROUNDED_SYSTEM_POLICY as SYSTEM_POLICY;
pub use provider::{
    canonicalize_base_url, ConfiguredProvider, HangingProvider, ProviderAuth, ScriptedProvider,
    ENV_QA_ALLOWED_HOSTS, ENV_QA_ALLOW_LOCAL, ENV_QA_ALLOW_NO_AUTH, ENV_QA_API_KEY,
    ENV_QA_BASE_URL, ENV_QA_MODEL, ENV_QA_PROVIDER, ENV_QA_TIMEOUT_MS,
};
pub use stream::{
    collect_stream_text, tokenize_for_stream, AuthProbeDecision as QaAuthProbeDecision,
    StreamBounds as QaStreamBounds, StreamCancel as QaStreamCancel,
    StreamCloseReason as QaStreamCloseReason, StreamEvent as QaStreamEvent,
};

pub const MAX_QUESTION_CHARS: usize = 4_000;
pub const MAX_PASSAGES: usize = 32;
pub const MAX_PASSAGE_CHARS: usize = 8_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnswerMode {
    OfflineExtractive,
    FallbackExtractive,
    ProviderLlm,
}

impl AnswerMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OfflineExtractive => "offline_extractive",
            Self::FallbackExtractive => "fallback_extractive",
            Self::ProviderLlm => "provider_llm",
        }
    }
}

/// Q&A request over already-authorized retrieval evidence.
#[derive(Clone, PartialEq)]
pub struct QaRequest {
    pub question: String,
    pub mode: VersionMode,
    pub use_provider: bool,
    /// Optional lifecycle overlays for `retrieval.conflict_evidence` (never loaded here).
    pub conflict_lifecycle: Vec<ConflictLifecycle>,
}

impl std::fmt::Debug for QaRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QaRequest")
            .field("question", &"[REDACTED]")
            .field("mode", &self.mode)
            .field("use_provider", &self.use_provider)
            .field("conflict_lifecycle_count", &self.conflict_lifecycle.len())
            .finish()
    }
}

/// Citation pin derived from authorized retrieval hits (no R02/DB resolve in R03).
#[derive(Clone, PartialEq, Eq)]
pub struct QaCitation {
    pub cite_id: String,
    pub document_id: Uuid,
    pub version_id: Uuid,
    pub version_number: i32,
    pub content_sha256: String,
    pub chunk_id: Uuid,
    pub is_current: bool,
    pub heading: String,
    pub quote: String,
}

impl std::fmt::Debug for QaCitation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QaCitation")
            .field("cite_id", &self.cite_id)
            .field("document_id", &self.document_id)
            .field("version_id", &self.version_id)
            .field("version_number", &self.version_number)
            .field("chunk_id", &self.chunk_id)
            .field("is_current", &self.is_current)
            .field("heading", &"[REDACTED]")
            .field("quote", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

#[derive(Clone, PartialEq)]
pub struct QaAnswer {
    pub answer: String,
    pub citations: Vec<QaCitation>,
    pub mode: AnswerMode,
    pub grounded: bool,
    pub warnings: Vec<String>,
    pub version_context: VersionContext,
    pub conflict_warnings: Vec<ConflictWarning>,
    pub audit: QaAuditMetadata,
}

impl std::fmt::Debug for QaAnswer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QaAnswer")
            .field("answer", &"[REDACTED]")
            .field("citations", &self.citations.len())
            .field("mode", &self.mode)
            .field("grounded", &self.grounded)
            .field("warnings_count", &self.warnings.len())
            .field("version_context", &self.version_context.mode)
            .field("conflict_warnings", &self.conflict_warnings.len())
            .field("audit", &self.audit)
            .finish()
    }
}

/// Audit metadata only — never question, prompt, passage, answer, token, or secret.
#[derive(Clone, PartialEq, Eq)]
pub struct QaAuditMetadata {
    pub action: &'static str,
    pub outcome: &'static str,
    pub answer_mode: &'static str,
    pub citation_count: usize,
    pub conflict_warning_count: usize,
    pub version_mode: &'static str,
    pub provider_configured: bool,
    pub fallback_reason: Option<&'static str>,
    pub request_id: String,
    pub grounded: bool,
    /// Wall-clock latency for the ask path (prompt → grounded answer), milliseconds.
    pub latency_ms: u64,
    /// Stable error/fallback code when grounding failed or fell back; never content.
    pub error: Option<&'static str>,
}

impl std::fmt::Debug for QaAuditMetadata {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QaAuditMetadata")
            .field("action", &self.action)
            .field("outcome", &self.outcome)
            .field("answer_mode", &self.answer_mode)
            .field("citation_count", &self.citation_count)
            .field("conflict_warning_count", &self.conflict_warning_count)
            .field("version_mode", &self.version_mode)
            .field("provider_configured", &self.provider_configured)
            .field("fallback_reason", &self.fallback_reason)
            .field("request_id", &self.request_id)
            .field("grounded", &self.grounded)
            .field("latency_ms", &self.latency_ms)
            .field("error", &self.error)
            .finish()
    }
}

impl QaAuditMetadata {
    pub fn to_json(&self) -> JsonValue {
        json!({
            "action": self.action,
            "outcome": self.outcome,
            "answer_mode": self.answer_mode,
            "citation_count": self.citation_count,
            "conflict_warning_count": self.conflict_warning_count,
            "version_mode": self.version_mode,
            "provider_configured": self.provider_configured,
            "fallback_reason": self.fallback_reason,
            "request_id": self.request_id,
            "grounded": self.grounded,
            "latency_ms": self.latency_ms,
            "error": self.error,
        })
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum QaError {
    #[error("invalid qa request")]
    InvalidRequest(&'static str),
    #[error("grounding error")]
    Grounding(#[from] GroundingError),
    #[error("provider error")]
    Provider(#[from] ProviderError),
}

impl QaError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidRequest(_) => "qa_invalid_request",
            Self::Grounding(error) => error.code(),
            Self::Provider(error) => error.code(),
        }
    }
}

fn new_request_id() -> String {
    hex::encode(Uuid::new_v4().as_bytes())
}

fn enforce_bounds(question: &str, hits: &[RetrievalHit]) -> Result<(), QaError> {
    if question.trim().is_empty() {
        return Err(QaError::InvalidRequest("question is empty"));
    }
    if question.chars().count() > MAX_QUESTION_CHARS {
        return Err(QaError::InvalidRequest("question too long"));
    }
    if hits.len() > MAX_PASSAGES {
        return Err(QaError::InvalidRequest("too many passages"));
    }
    for hit in hits {
        if hit.snippet.chars().count() > MAX_PASSAGE_CHARS
            || hit.body.chars().count() > MAX_PASSAGE_CHARS
        {
            return Err(QaError::InvalidRequest("passage too long"));
        }
    }
    Ok(())
}

fn prompt_passages(passages: &[GroundingPassage]) -> Vec<PromptPassage> {
    passages
        .iter()
        .map(|passage| PromptPassage {
            cite_id: passage.cite_id.clone(),
            source_label: format!("document:{}", passage.hit.document_id),
            heading: passage.hit.heading.clone(),
            snippet: passage.authoritative_quote.clone(),
            version_number: passage.hit.version_number,
            is_current: passage.hit.is_current,
        })
        .collect()
}

fn citations_for(passages: &[GroundingPassage], cite_ids: &[String]) -> Vec<QaCitation> {
    let by_id = GroundingPassage::by_cite_id(passages);
    cite_ids
        .iter()
        .filter_map(|id| {
            let passage = by_id.get(id)?;
            Some(QaCitation {
                cite_id: id.clone(),
                document_id: passage.hit.document_id,
                version_id: passage.hit.version_id,
                version_number: passage.hit.version_number,
                content_sha256: passage.hit.content_sha256.clone(),
                chunk_id: passage.hit.chunk_id,
                is_current: passage.hit.is_current,
                heading: passage.hit.heading.clone(),
                quote: passage.authoritative_quote.clone(),
            })
        })
        .collect()
}

fn version_mode_label(mode: &VersionMode) -> &'static str {
    match mode {
        VersionMode::Current => "current",
        VersionMode::AsOf { .. } => "as_of",
        VersionMode::Compare { .. } => "compare",
        VersionMode::History { .. } => "history",
    }
}

struct BuildAnswer {
    answer: String,
    citations: Vec<QaCitation>,
    mode: AnswerMode,
    grounded: bool,
    warnings: Vec<String>,
    version_context: VersionContext,
    conflict_warnings: Vec<ConflictWarning>,
    provider_configured: bool,
    fallback_reason: Option<&'static str>,
    request_id: String,
    latency_ms: u64,
}

fn finish_answer(built: BuildAnswer) -> QaAnswer {
    let BuildAnswer {
        answer,
        citations,
        mode,
        grounded,
        warnings,
        version_context,
        conflict_warnings,
        provider_configured,
        fallback_reason,
        request_id,
        latency_ms,
    } = built;
    let outcome = if grounded {
        if matches!(mode, AnswerMode::ProviderLlm) {
            "answered"
        } else if fallback_reason.is_some() {
            "fallback"
        } else {
            "extractive"
        }
    } else {
        "ungrounded"
    };
    QaAnswer {
        audit: QaAuditMetadata {
            action: "qa.ask",
            outcome,
            answer_mode: mode.as_str(),
            citation_count: citations.len(),
            conflict_warning_count: conflict_warnings.len(),
            version_mode: version_context.mode,
            provider_configured,
            fallback_reason,
            request_id,
            grounded,
            latency_ms,
            error: fallback_reason,
        },
        answer,
        citations,
        mode,
        grounded,
        warnings,
        version_context,
        conflict_warnings,
    }
}

/// Build a grounded answer from already-authorized retrieval evidence.
///
/// Does not query or mutate any database. Provider failures / invalid grounding
/// fall back to deterministic extractive answers with neutralized source cites.
pub async fn answer_question<P: QaChatProvider>(
    request: QaRequest,
    retrieval: RetrievalResponse,
    provider: Option<&P>,
    provider_config: Option<&QaProviderConfig>,
) -> Result<QaAnswer, QaError> {
    let started = Instant::now();
    let request_id = new_request_id();
    enforce_bounds(&request.question, &retrieval.hits)?;

    let all_passages = GroundingPassage::from_hits(&retrieval.hits);
    if all_passages.is_empty() {
        return Ok(finish_answer(BuildAnswer {
            answer: extractive_answer(&[]),
            citations: vec![],
            mode: AnswerMode::OfflineExtractive,
            grounded: false,
            warnings: vec!["Không có bằng chứng được ủy quyền.".into()],
            version_context: VersionContext {
                mode: version_mode_label(&request.mode),
                current_version_ids: vec![],
                cited_version_ids: vec![],
                change_note: None,
            },
            conflict_warnings: vec![],
            provider_configured: provider_config.is_some(),
            fallback_reason: Some("empty_evidence"),
            request_id,
            latency_ms: elapsed_ms(started),
        }));
    }

    let passages = passages_for_mode(&all_passages, &request.mode)?;
    if passages.is_empty() {
        return Ok(finish_answer(BuildAnswer {
            answer: extractive_answer(&[]),
            citations: vec![],
            mode: AnswerMode::OfflineExtractive,
            grounded: false,
            warnings: vec!["Không có bằng chứng phù hợp cho chế độ phiên bản.".into()],
            version_context: VersionContext {
                mode: version_mode_label(&request.mode),
                current_version_ids: vec![],
                cited_version_ids: vec![],
                change_note: None,
            },
            conflict_warnings: vec![],
            provider_configured: provider_config.is_some(),
            fallback_reason: Some("empty_evidence"),
            request_id,
            latency_ms: elapsed_ms(started),
        }));
    }

    let prompt_passages = prompt_passages(&passages);
    debug_assert!(
        system_policy_is_separated(),
        "system policy must remain structurally separated from untrusted framing tags"
    );
    // Keep framing helpers on every ask path (offline included) so injection
    // escapes remain wired even when no provider call is made.
    let framed_question = frame_question(&request.question);
    let framed_sources = frame_passages(&prompt_passages);
    let _ = (framed_question.len(), framed_sources.len());
    let provider_configured = provider_config.is_some();
    let mut warnings = retrieval.warnings;

    let (raw_answer, mode, structured, fallback_reason, require_structured) = if !request
        .use_provider
        || provider.is_none()
    {
        let reason = if provider.is_none() && request.use_provider {
            warnings.push("QA provider unavailable; fallback extractive.".into());
            Some("provider_unavailable")
        } else {
            None
        };
        let mode = if reason.is_some() {
            AnswerMode::FallbackExtractive
        } else {
            AnswerMode::OfflineExtractive
        };
        (
            extractive_answer(&passages),
            mode,
            extractive_structured_claims(&passages),
            reason,
            false,
        )
    } else {
        let provider = provider.expect("checked above");
        let chat_request = ChatCompletionRequest {
            system: GROUNDED_SYSTEM_POLICY.to_string(),
            user: grounded_user_prompt(&request.question, &prompt_passages),
        };
        let timeout_budget = provider_config
            .map(QaProviderConfig::timeout)
            .unwrap_or(Duration::from_secs(30));
        match tokio::time::timeout(timeout_budget, provider.complete_grounded(&chat_request)).await
        {
            Ok(Ok(payload)) if !payload.refusal => {
                match validate_structured_claims(&payload.claims, &passages, &request.mode, true) {
                    Ok(_) => (
                        render_answer_from_claims(&payload.claims),
                        AnswerMode::ProviderLlm,
                        payload.claims,
                        None,
                        true,
                    ),
                    Err(error) => {
                        warnings.push(format!(
                            "Grounding không hợp lệ ({}); fallback extractive.",
                            error.code()
                        ));
                        (
                            extractive_answer(&passages),
                            AnswerMode::FallbackExtractive,
                            extractive_structured_claims(&passages),
                            Some(error.code()),
                            false,
                        )
                    }
                }
            }
            Ok(Ok(_)) => {
                warnings.push("QA provider refused; fallback extractive.".into());
                (
                    extractive_answer(&passages),
                    AnswerMode::FallbackExtractive,
                    extractive_structured_claims(&passages),
                    Some("provider_refusal"),
                    false,
                )
            }
            Ok(Err(ProviderError::Timeout)) | Err(_) => {
                warnings.push("QA provider timeout; fallback extractive.".into());
                (
                    extractive_answer(&passages),
                    AnswerMode::FallbackExtractive,
                    extractive_structured_claims(&passages),
                    Some("provider_timeout"),
                    false,
                )
            }
            Ok(Err(ProviderError::Truncated)) => {
                warnings.push("QA provider response oversize; fallback extractive.".into());
                (
                    extractive_answer(&passages),
                    AnswerMode::FallbackExtractive,
                    extractive_structured_claims(&passages),
                    Some("provider_truncated"),
                    false,
                )
            }
            Ok(Err(ProviderError::InvalidResponse)) => {
                warnings.push("QA provider malformed response; fallback extractive.".into());
                (
                    extractive_answer(&passages),
                    AnswerMode::FallbackExtractive,
                    extractive_structured_claims(&passages),
                    Some("provider_invalid_response"),
                    false,
                )
            }
            Ok(Err(_)) => {
                warnings.push("QA provider outage; fallback extractive.".into());
                (
                    extractive_answer(&passages),
                    AnswerMode::FallbackExtractive,
                    extractive_structured_claims(&passages),
                    Some("provider_outage"),
                    false,
                )
            }
        }
    };

    let cited = match validate_rendered_answer(
        &raw_answer,
        &passages,
        &request.mode,
        &structured,
        require_structured,
    ) {
        Ok(cited) => cited,
        Err(error) if matches!(mode, AnswerMode::ProviderLlm) => {
            // Should be rare after pre-validation; still fail closed to extractive.
            warnings.push(format!(
                "Grounding không hợp lệ ({}); fallback extractive.",
                error.code()
            ));
            let answer = extractive_answer(&passages);
            let structured = extractive_structured_claims(&passages);
            let cited =
                validate_rendered_answer(&answer, &passages, &request.mode, &structured, false)?;
            return Ok(assemble_final(AssembleInput {
                answer,
                cited,
                mode: AnswerMode::FallbackExtractive,
                warnings,
                passages: &passages,
                request: &request,
                conflict_evidence: &retrieval.conflict_evidence,
                provider_configured,
                fallback_reason: Some(error.code()),
                request_id: &request_id,
                latency_ms: elapsed_ms(started),
            }));
        }
        Err(error) => return Err(QaError::Grounding(error)),
    };

    Ok(assemble_final(AssembleInput {
        answer: raw_answer,
        cited,
        mode,
        warnings,
        passages: &passages,
        request: &request,
        conflict_evidence: &retrieval.conflict_evidence,
        provider_configured,
        fallback_reason,
        request_id: &request_id,
        latency_ms: elapsed_ms(started),
    }))
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

struct AssembleInput<'a> {
    answer: String,
    cited: Vec<String>,
    mode: AnswerMode,
    warnings: Vec<String>,
    passages: &'a [GroundingPassage],
    request: &'a QaRequest,
    conflict_evidence: &'a [AuthorizedConflictEvidence],
    provider_configured: bool,
    fallback_reason: Option<&'static str>,
    request_id: &'a str,
    latency_ms: u64,
}

fn assemble_final(input: AssembleInput<'_>) -> QaAnswer {
    let AssembleInput {
        mut answer,
        cited,
        mode,
        mut warnings,
        passages,
        request,
        conflict_evidence,
        provider_configured,
        fallback_reason,
        request_id,
        latency_ms,
    } = input;
    let version_context = build_version_context(&request.mode, passages, &cited);
    let conflicts = conflicts_from_evidence(
        conflict_evidence,
        passages,
        &request.conflict_lifecycle,
        &request.mode,
    );
    let conflict_warnings = conflict_messages(&request.mode, &conflicts);

    let mut notes = Vec::new();
    if let Some(note) = version_context.change_note.clone() {
        notes.push(note);
    }
    for warning in &conflict_warnings {
        notes.push(warning.message.clone());
        if !warnings.iter().any(|w| w == &warning.message) {
            warnings.push(warning.message.clone());
        }
    }
    answer = append_server_notes(&answer, &notes);

    let mut hydrate_cites = cited;
    for warning in &conflict_warnings {
        for pin in &warning.pin_cite_ids {
            if !hydrate_cites.iter().any(|c| c == pin) {
                hydrate_cites.push(pin.clone());
            }
        }
    }
    let citations = citations_for(passages, &hydrate_cites);
    finish_answer(BuildAnswer {
        answer,
        citations,
        mode,
        grounded: !hydrate_cites.is_empty(),
        warnings,
        version_context,
        conflict_warnings,
        provider_configured,
        fallback_reason,
        request_id: request_id.to_string(),
        latency_ms,
    })
}

/// Inputs for bounded validated replay streaming.
pub struct StreamAskInput<'a, P: QaChatProvider> {
    pub request: QaRequest,
    pub retrieval: RetrievalResponse,
    pub provider: Option<&'a P>,
    pub provider_config: Option<&'a QaProviderConfig>,
    pub cancel: StreamCancel,
    pub bounds: StreamBounds,
}

/// Validate the whole answer, then replay UTF-8-safe chunks with an auth probe.
///
/// `auth_probe` runs before each application chunk. Deny/Deleted closes before
/// enqueueing further chunks. Already-emitted chunks are not recalled.
pub async fn stream_answer<P, F, Fut>(
    input: StreamAskInput<'_, P>,
    auth_probe: F,
) -> Result<(QaAnswer, tokio::sync::mpsc::Receiver<StreamEvent>), QaError>
where
    P: QaChatProvider,
    F: FnMut() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = AuthProbeDecision> + Send + 'static,
{
    let StreamAskInput {
        request,
        retrieval,
        provider,
        provider_config,
        cancel,
        bounds,
    } = input;
    let answer = answer_question(request, retrieval, provider, provider_config).await?;
    let rx = replay_validated_answer(answer.answer.clone(), bounds, cancel, auth_probe).await;
    Ok((answer, rx))
}

/// Helper to build a scripted grounded payload for tests.
pub fn scripted_claims(claims: Vec<StructuredClaim>) -> ProviderGroundedPayload {
    ProviderGroundedPayload {
        claims,
        refusal: false,
    }
}
