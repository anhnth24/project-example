//! Grounded Q&A with version-aware citations and extractive fallback (P1B-R03).

pub mod ask_stream;
pub mod chitchat;
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
use crate::db::models::ResourceKind;
use crate::services::citation::{pins_cited_in_answer, pins_from_hits, CitationPin};
use crate::services::embedding::ApprovedEmbeddingRuntime;
use crate::services::qa::grounding::{
    conflict_resolution_notes_for_history, conflict_warnings_for_current,
    validate_answer_citations, version_context_note, VersionContext,
};
use crate::services::qa::prompt::{build_grounded_messages, GroundedMessages};
use crate::services::qa::provider::{ChatProvider, ProviderError, MAX_ANSWER_CHARS};
use crate::services::quota::{self, QuotaError, DEFAULT_RESERVATION_TTL};
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

/// Prefix for an optional UAT-only warning that carries the discarded LLM draft
/// when claim validation fails under `MARKHAND_QA_ALLOW_UNVERIFIED_LLM`. Must
/// stay in sync with `DISCARDED_LLM_DRAFT_PREFIX` in the web chat UI.
pub const DISCARDED_LLM_DRAFT_WARNING_PREFIX: &str = "Discarded LLM draft (UAT):\n";

const DISCARDED_LLM_DRAFT_MAX_CHARS: usize = 4_000;

fn discarded_llm_draft_warning(llm_answer: &str) -> String {
    let truncated: String = llm_answer
        .chars()
        .take(DISCARDED_LLM_DRAFT_MAX_CHARS)
        .collect();
    format!("{DISCARDED_LLM_DRAFT_WARNING_PREFIX}{truncated}")
}

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
    // Câu từ chối đúng theo prompt ("Nếu nguồn thiếu, chỉ trả lời: …") không
    // chứa claim nào — trung thực hơn là đổ extractive không liên quan.
    let trimmed_answer = llm_answer.trim();
    if trimmed_answer.starts_with("Không đủ dữ liệu trong nguồn")
        && trimmed_answer.chars().count() <= 80
    {
        if force_extractive_only() {
            warnings.push(UNVERIFIED_LLM_WARNING.into());
            return (
                trimmed_answer.to_string(),
                AnswerMode::LlmUnverified,
                warnings,
            );
        }
        return (trimmed_answer.to_string(), real_mode, warnings);
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
            // Dev-gate + Current: model thường viết 3–4 câu đúng và MỘT câu
            // cite nhầm nguồn — vứt cả draft vì một câu là mất toàn bộ giá trị
            // (eval 2026-08-16: BERT/ResNet). Cắt riêng các dòng không kiểm
            // chứng được; phần còn lại phải tự pass lại validator toàn văn.
            if allow_unverified_llm() && matches!(mode, VersionMode::Current) {
                // P0.2: thử cứu câu thiếu citation bằng auto-attach TRƯỚC khi
                // prune — toàn văn đã gắn phải pass lại validator, nếu không
                // rơi xuống nhánh prune như cũ.
                if let Some(saved) =
                    attach_citations_to_uncited_lines(&llm_answer, valid_ids, citations, mode)
                {
                    warnings.push(citation_auto_attach_warning(saved.attached));
                    warnings.push(UNVERIFIED_LLM_WARNING.into());
                    return (saved.answer, AnswerMode::LlmUnverified, warnings);
                }
                if let Some(pruned) =
                    prune_unverifiable_lines(&llm_answer, valid_ids, citations, mode)
                {
                    warnings.push(format!(
                        "Removed {} unverifiable sentence(s) from LLM draft; remainder passed claim checks.",
                        pruned.removed
                    ));
                    warnings.push(UNVERIFIED_LLM_WARNING.into());
                    return (pruned.answer, AnswerMode::LlmUnverified, warnings);
                }
            }
            warnings.extend(failure.warnings);
            warnings.push(
                if failure.unverifiable {
                    "Unverifiable claim-level grounding; using extractive fallback."
                } else {
                    "LLM grounding failed validation; using extractive fallback."
                }
                .into(),
            );
            // UAT-only: surface the discarded draft so operators can inspect
            // what the model said without promoting it to the primary answer.
            if allow_unverified_llm() {
                warnings.push(discarded_llm_draft_warning(&llm_answer));
            }
            (
                extractive.to_string(),
                AnswerMode::FallbackExtractive,
                warnings,
            )
        }
    }
}

struct PrunedDraft {
    answer: String,
    removed: usize,
}

pub(crate) struct AttachedDraft {
    pub(crate) answer: String,
    pub(crate) attached: usize,
}

/// Warning khi P0.2 auto-attach cứu được câu thiếu citation (đếm được trong
/// eval; web dịch theo regex — giữ nguyên khuôn "Auto-attached citations to N
/// sentence(s)").
pub(crate) fn citation_auto_attach_warning(attached: usize) -> String {
    format!("Auto-attached citations to {attached} sentence(s); full draft passed claim checks.")
}

/// P0.2 — đề xuất trước, validate sau: với mỗi dòng CÓ nội dung nhưng THIẾU
/// `[CITE-]`, tìm pin ứng viên theo token-overlap (trên pins/hybrid hits sẵn
/// có — không gọi embed lại) và gắn thử marker. Sau đó chạy **nguyên bộ**
/// `validate_answer_citations` (claim-check + negation + date/unit) trên toàn
/// văn đã gắn; fail → trả `None` để caller prune/fallback như cũ. Fail-closed
/// nguyên vẹn: không câu nào được giữ mà chưa qua validator.
pub(crate) fn attach_citations_to_uncited_lines(
    answer: &str,
    valid_ids: &HashSet<String>,
    pins: &[CitationPin],
    mode: &VersionMode,
) -> Option<AttachedDraft> {
    if pins.is_empty() {
        return None;
    }
    let lines: Vec<&str> = answer
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if lines.is_empty() {
        return None;
    }
    let mut attached = 0usize;
    let mut rewritten: Vec<String> = Vec::with_capacity(lines.len());
    for line in &lines {
        if line.contains("[CITE-") || !line_has_prose(line) {
            rewritten.push((*line).to_string());
            continue;
        }
        match crate::services::qa::grounding::propose_citation_for_sentence(line, pins) {
            Some(cite_id) => {
                // Prompt yêu cầu marker ngay TRƯỚC dấu chấm cuối câu.
                let base = line.trim_end();
                let (body, dot) = match base.strip_suffix('.') {
                    Some(without_dot) => (without_dot.trim_end(), "."),
                    None => (base, ""),
                };
                rewritten.push(format!("{body} [{cite_id}]{dot}"));
                attached += 1;
            }
            None => rewritten.push((*line).to_string()),
        }
    }
    if attached == 0 {
        return None;
    }
    let joined = rewritten.join("\n");
    validate_answer_citations(&joined, valid_ids, pins, mode).ok()?;
    Some(AttachedDraft {
        answer: joined,
        attached,
    })
}

/// Warning gắn kèm khi phải nhắc model bổ sung citation (đếm được trong eval).
pub(crate) const CITATION_RETRY_WARNING: &str =
    "LLM draft lacked citations; retried once with a citation reminder.";

/// Draft không có bất kỳ `[CITE-` nào thì chắc chắn rớt validation toàn văn →
/// rơi thẳng về extractive dù nội dung thường ĐÚNG (model nhanh/reasoning-off
/// hay quên định dạng; eval 2026-08-17: 3/22 câu mất điểm kiểu này). Câu từ
/// chối hợp lệ thì không cần retry.
pub(crate) fn draft_needs_citation_retry(draft: &str) -> bool {
    let trimmed = draft.trim();
    !trimmed.contains("[CITE-") && !trimmed.starts_with("Không đủ dữ liệu trong nguồn")
}

/// Lượt nhắc lại DUY NHẤT: giữ nguyên system + context, nối thêm chỉ thị sửa
/// bản nháp. Không lặp vô hạn — call site chỉ gọi một lần và chỉ nhận kết quả
/// mới nếu nó thực sự có `[CITE-`.
pub(crate) fn citation_retry_messages(
    original: &GroundedMessages,
    draft: &str,
) -> GroundedMessages {
    GroundedMessages {
        system: original.system.clone(),
        user: format!(
            "{}\n\nBản nháp dưới đây KHÔNG có [CITE-xxxx] nên bị loại. Viết lại \
             câu trả lời với đúng nội dung đó, MỌI câu kết thúc bằng [CITE-xxxx] \
             đúng id nguồn ngay trước dấu chấm.\n\nBản nháp:\n{draft}",
            original.user
        ),
    }
}

/// Cắt các dòng không tự kiểm chứng được khỏi draft (prompt yêu cầu mỗi câu
/// một dòng). Trả `None` nếu không cắt được gì, không còn dòng nào có
/// citation, hoặc phần còn lại vẫn không pass validator toàn văn.
fn prune_unverifiable_lines(
    answer: &str,
    valid_ids: &HashSet<String>,
    pins: &[CitationPin],
    mode: &VersionMode,
) -> Option<PrunedDraft> {
    let lines: Vec<&str> = answer
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if lines.len() < 2 {
        return None;
    }
    let kept: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|line| validate_answer_citations(line, valid_ids, pins, mode).is_ok())
        // Dòng chỉ chứa marker ("[CITE-0001] [CITE-0006]") pass validation
        // nhưng không phải câu trả lời — giữ lại làm đáp án cuối chỉ tạo nhiễu
        // (eval 2026-08-17: 2 câu trả về đúng một marker trơ trọi).
        .filter(|line| line_has_prose(line))
        .collect();
    if kept.is_empty() || kept.len() == lines.len() {
        return None;
    }
    if !kept.iter().any(|line| line.contains("[CITE-")) {
        return None;
    }
    let joined = kept.join("\n");
    validate_answer_citations(&joined, valid_ids, pins, mode).ok()?;
    Some(PrunedDraft {
        answer: joined,
        removed: lines.len() - kept.len(),
    })
}

/// Còn nội dung chữ/số sau khi gỡ hết marker `[CITE-xxxx]`?
fn line_has_prose(line: &str) -> bool {
    let mut rest = line.to_string();
    while let Some(start) = rest.find("[CITE-") {
        let Some(offset) = rest[start..].find(']') else {
            break;
        };
        rest.replace_range(start..=start + offset, "");
    }
    rest.chars().any(char::is_alphanumeric)
}

// --- Token-quota lifecycle around the chat-provider call (1C-09 a) ---
//
// The only real token consumer on the ask path is the configured chat
// provider (`ChatProvider::complete` / `stream_tokens`); when no provider is
// configured (extractive-only MVP deployment) nothing is reserved. Neither
// the OpenAI-compatible non-streaming response nor provider streaming chunks are
// guaranteed to carry a `usage` block, so both admission and settlement use
// the same character heuristic (~4 chars/token) — reserve an upper bound
// (prompt + MAX_ANSWER_CHARS allowance), settle measured characters.

/// Heuristic chars→tokens divisor (BPE average; Vietnamese diacritics make
/// this conservative in the reserve direction).
const TOKEN_CHARS_PER_TOKEN: u64 = 4;

pub(crate) fn estimate_tokens_from_chars(chars: usize) -> u64 {
    (chars as u64).div_ceil(TOKEN_CHARS_PER_TOKEN).max(1)
}

/// An admitted token reservation covering one provider interaction.
#[derive(Debug, Clone)]
pub(crate) struct TokenLease {
    pub reservation_key: String,
    pub prompt_tokens: u64,
}

/// Reserve prompt + max-answer tokens before the provider call. Fail-closed:
/// `QuotaExceeded` surfaces as a distinguishable 429 to the caller.
pub(crate) async fn reserve_ask_tokens(
    pool: &Pool,
    ctx: &OrgContext,
    messages: &GroundedMessages,
) -> Result<TokenLease, QuotaError> {
    let prompt_chars = messages.system.chars().count() + messages.user.chars().count();
    let prompt_tokens = estimate_tokens_from_chars(prompt_chars);
    let reserve_amount = prompt_tokens
        .checked_add(estimate_tokens_from_chars(MAX_ANSWER_CHARS))
        .ok_or(QuotaError::ArithmeticOverflow)?;
    let reservation_key = format!("ask.tokens.{}", Uuid::new_v4());
    quota::reserve(
        pool,
        ctx,
        &reservation_key,
        ResourceKind::Tokens,
        reserve_amount,
        DEFAULT_RESERVATION_TTL,
        None,
    )
    .await?;
    Ok(TokenLease {
        reservation_key,
        prompt_tokens,
    })
}

/// Settle a token lease after the provider interaction.
///
/// `answer_chars = Some(n)` commits prompt + measured answer tokens (the
/// provider really consumed them — even when the answer is later discarded by
/// fail-closed grounding). `None` means the request never got off the ground
/// (transport failure): refund. Settlement failures are logged, never bubbled
/// — the reservation then simply expires via the sweeper (capacity held for
/// at most the TTL, counters untouched).
pub(crate) async fn settle_ask_tokens(
    pool: &Pool,
    ctx: &OrgContext,
    lease: TokenLease,
    answer_chars: Option<usize>,
) {
    let result = match answer_chars {
        Some(chars) => {
            let actual = lease.prompt_tokens.saturating_add(if chars == 0 {
                0
            } else {
                estimate_tokens_from_chars(chars)
            });
            quota::finalize_actual(pool, ctx, &lease.reservation_key, actual).await
        }
        None => quota::refund(pool, ctx, &lease.reservation_key).await,
    };
    if let Err(error) = result {
        tracing::warn!(
            target: "quota",
            code = error.code(),
            "ask token settlement failed; reservation left for expiry sweeper"
        );
    }
}

/// Decide assistant vs knowledge for one ask turn.
///
/// Clear heuristic wins. Ambiguous turns may call the chat provider once with a
/// one-token router (`KNOWLEDGE`|`ASSISTANT`); quota is reserved/settled around
/// that call. Router failure defaults to knowledge (safer for org Q&A).
pub(crate) async fn decide_ask_route(
    pool: &Pool,
    ctx: &OrgContext,
    provider: Option<&ChatProvider>,
    question: &str,
) -> Result<(chitchat::AskRoute, Vec<String>), QuotaError> {
    if let Some(route) = chitchat::heuristic_ask_route(question) {
        return Ok((route, Vec::new()));
    }

    let Some(chat) = provider else {
        return Ok((chitchat::AskRoute::Knowledge, Vec::new()));
    };

    let messages = chitchat::router_messages(question);
    let lease = reserve_ask_tokens(pool, ctx, &messages).await?;
    match chat.complete(&messages).await {
        Ok(raw) => {
            settle_ask_tokens(pool, ctx, lease, Some(raw.chars().count())).await;
            match chitchat::parse_router_label(&raw) {
                Some(route) => Ok((route, Vec::new())),
                None => Ok((
                    chitchat::AskRoute::Knowledge,
                    vec![
                        "Ask router returned an unrecognized label; defaulting to knowledge."
                            .into(),
                    ],
                )),
            }
        }
        Err(ProviderError::Timeout) => {
            settle_ask_tokens(pool, ctx, lease, Some(0)).await;
            Ok((
                chitchat::AskRoute::Knowledge,
                vec!["Ask router timed out; defaulting to knowledge.".into()],
            ))
        }
        Err(_) => {
            settle_ask_tokens(pool, ctx, lease, None).await;
            Ok((
                chitchat::AskRoute::Knowledge,
                vec!["Ask router unavailable; defaulting to knowledge.".into()],
            ))
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
    /// Token-quota admission failed (1C-09 a). `QuotaExceeded` maps to the
    /// same distinguishable 429 contract as storage quota on upload.
    #[error("quota error")]
    Quota(QuotaError),
}

impl AskError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Retrieval(error) => error.code(),
            Self::InvalidRequest(_) => "ask_invalid_request",
            Self::Provider(_) => "ask_provider",
            Self::Quota(error) => error.code(),
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
            // Ask/extractive must see the ranked chunk, not the 240-char
            // preview window (`snippet` starts at the first query token —
            // often the circular title). Citation pins already use `body`.
            snippet: if hit.body.trim().is_empty() {
                hit.snippet.clone()
            } else {
                hit.body.clone()
            },
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

/// Grounded ask: retrieve → optional chat LLM → citation validate → extractive fallback.
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

    // Non-document turns: assistant system prompt, no retrieval/citations.
    // Document-related questions keep hybrid search + grounding below.
    let (route, mut route_warnings) = decide_ask_route(pool, ctx, provider, &request.question)
        .await
        .map_err(AskError::Quota)?;
    if matches!(route, chitchat::AskRoute::Assistant) {
        let fallback = chitchat::assistant_fallback_reply(&request.question);
        let version_context = version_context_note(&request.mode, &[], &[]);
        let (answer, mut warnings) = match provider {
            Some(chat) => {
                let messages = prompt::build_assistant_messages(&request.question);
                let lease = reserve_ask_tokens(pool, ctx, &messages)
                    .await
                    .map_err(AskError::Quota)?;
                match chat.complete(&messages).await {
                    Ok(llm_answer) => {
                        settle_ask_tokens(pool, ctx, lease, Some(llm_answer.chars().count())).await;
                        let trimmed = llm_answer.trim();
                        if trimmed.is_empty() {
                            (
                                fallback,
                                vec!["Assistant provider returned empty; using offline reply."
                                    .into()],
                            )
                        } else {
                            (trimmed.to_string(), Vec::new())
                        }
                    }
                    Err(ProviderError::Timeout) => {
                        settle_ask_tokens(pool, ctx, lease, Some(0)).await;
                        (
                            fallback,
                            vec!["LLM provider timed out; using offline assistant reply.".into()],
                        )
                    }
                    Err(_) => {
                        settle_ask_tokens(pool, ctx, lease, None).await;
                        (
                            fallback,
                            vec!["LLM provider unavailable; using offline assistant reply.".into()],
                        )
                    }
                }
            }
            None => (
                fallback,
                vec!["No chat provider configured; using offline assistant reply.".into()],
            ),
        };
        warnings.append(&mut route_warnings);
        return Ok(AskResponse {
            answer,
            mode: AnswerMode::Assistant,
            citations: Vec::new(),
            warnings,
            version_context,
            embedding_mode: "assistant".into(),
        });
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
    let mut warnings = route_warnings;
    warnings.extend(retrieval.warnings);
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

    // Provider may be attempted for outage/timeout observability, but LLM answers are
    // never claimed grounded unless structured entailment is available AND validation passes.
    let (answer, mode) = match provider {
        Some(chat) if !hybrid.is_empty() => {
            let messages = build_grounded_messages(&request.question, &hybrid, &request.mode);
            // Token quota (1C-09 a): reserve before the provider call — the
            // only real token consumer on this path — fail-closed on denial.
            let lease = reserve_ask_tokens(pool, ctx, &messages)
                .await
                .map_err(AskError::Quota)?;
            match chat.complete(&messages).await {
                Ok(mut llm_answer) => {
                    let mut consumed = llm_answer.chars().count();
                    if draft_needs_citation_retry(&llm_answer) {
                        // P0.2: auto-attach trước — cứu được toàn văn thì khỏi
                        // tốn lượt gọi LLM nhắc citation.
                        if let Some(saved) = attach_citations_to_uncited_lines(
                            &llm_answer,
                            &valid_ids,
                            &citations,
                            &request.mode,
                        ) {
                            warnings.push(citation_auto_attach_warning(saved.attached));
                            llm_answer = saved.answer;
                        } else {
                            warnings.push(CITATION_RETRY_WARNING.into());
                            let retry = citation_retry_messages(&messages, &llm_answer);
                            if let Ok(second) = chat.complete(&retry).await {
                                consumed += second.chars().count();
                                if second.contains("[CITE-") {
                                    llm_answer = second;
                                }
                            }
                        }
                    }
                    // The provider consumed prompt + answer tokens even when
                    // fail-closed grounding discards the answer below.
                    settle_ask_tokens(pool, ctx, lease, Some(consumed)).await;
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
                    // The request reached the provider; the prompt was very
                    // likely billed. Commit prompt tokens, no answer chars.
                    settle_ask_tokens(pool, ctx, lease, Some(0)).await;
                    warnings.push("LLM provider timed out; using extractive fallback.".into());
                    (extractive, AnswerMode::FallbackExtractive)
                }
                Err(_) => {
                    // Transport/parse failure before any usable exchange.
                    settle_ask_tokens(pool, ctx, lease, None).await;
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

    let citations = pins_cited_in_answer(&answer, citations);

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
        assert_eq!(mapped[0].snippet, "Version 2 budget value.");
    }

    #[test]
    fn hybrid_mapping_prefers_ranked_body_over_preview_snippet() {
        let mut long = hit(true, 2);
        long.snippet = "sửa đổi, bổ sung một số điều của Thông tư".into();
        long.body = "Điều 1. Sửa đổi khoản 2 Điều 3 như sau: Sản lượng điện năng bao tiêu.".into();
        let mapped = hits_to_hybrid(&[long]);
        assert_eq!(
            mapped[0].snippet,
            "Điều 1. Sửa đổi khoản 2 Điều 3 như sau: Sản lượng điện năng bao tiêu."
        );
    }

    #[test]
    fn force_extractive_only_is_enabled_by_default() {
        // Fail-closed until a trusted structured entailment verifier ships.
        assert!(force_extractive_only());
    }

    #[test]
    fn citation_retry_triggers_only_on_uncited_non_refusal_drafts() {
        assert!(draft_needs_citation_retry("Kinh phí là 10 triệu đồng."));
        assert!(!draft_needs_citation_retry(
            "Kinh phí là 10 triệu [CITE-0001]."
        ));
        assert!(!draft_needs_citation_retry(
            "Không đủ dữ liệu trong nguồn đã cung cấp."
        ));
    }

    #[test]
    fn citation_retry_messages_carry_original_context_and_draft() {
        let original = GroundedMessages {
            system: "system-prompt".into(),
            user: "câu hỏi + context".into(),
        };
        let retry = citation_retry_messages(&original, "bản nháp không cite");
        assert_eq!(retry.system, original.system);
        assert!(retry.user.starts_with("câu hỏi + context"));
        assert!(retry.user.contains("bản nháp không cite"));
        assert!(retry.user.contains("[CITE-xxxx]"));
    }

    #[test]
    fn line_has_prose_rejects_citation_marker_only_lines() {
        assert!(!line_has_prose("[CITE-0001]"));
        assert!(!line_has_prose("[CITE-0001] [CITE-0006]."));
        assert!(line_has_prose("Lưu 35 ngày [CITE-0001]."));
    }

    #[test]
    fn prune_never_returns_citation_marker_only_answer() {
        with_dev_gate(true, || {
            let pins = vec![test_pin("CITE-0001", 1, "Kinh phí là 10 triệu đồng")];
            let valid = HashSet::from(["CITE-0001".to_string()]);
            // Câu prose sai số liệu rớt validation; dòng sống sót duy nhất chỉ
            // là marker → prune phải chịu thua thay vì trả "[CITE-0001]" trơ.
            let (answer, mode, _warnings) = resolve_llm_answer(
                "Kinh phí là 99 triệu [CITE-0001].\n[CITE-0001]".into(),
                "extractive fallback text",
                &valid,
                &pins,
                &VersionMode::Current,
                AnswerMode::CloudLlm,
            );
            assert_eq!(answer, "extractive fallback text");
            assert_eq!(mode, AnswerMode::FallbackExtractive);
        });
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
            assert!(warnings
                .iter()
                .any(|w| w.starts_with(DISCARDED_LLM_DRAFT_WARNING_PREFIX)));
            assert!(warnings.iter().any(|w| w.contains("CITE-9999")));
        });
    }

    #[test]
    fn dev_gate_prunes_single_bad_sentence_and_keeps_rest() {
        with_dev_gate(true, || {
            let pins = vec![
                test_pin("CITE-0001", 1, "Kinh phí là 10 triệu đồng"),
                test_pin("CITE-0002", 2, "Thời hạn nộp báo cáo là ngày 15 hằng tháng"),
            ];
            let valid = HashSet::from(["CITE-0001".to_string(), "CITE-0002".to_string()]);
            // Câu 2 cite CITE-0002 nhưng mượn con số của CITE-0001 → chỉ câu đó hỏng.
            let (answer, mode, warnings) = resolve_llm_answer(
                "Kinh phí là 10 triệu đồng [CITE-0001].\nKinh phí là 99 triệu [CITE-0002].".into(),
                "extractive fallback text",
                &valid,
                &pins,
                &VersionMode::Current,
                AnswerMode::CloudLlm,
            );
            assert_eq!(answer, "Kinh phí là 10 triệu đồng [CITE-0001].");
            assert_eq!(mode, AnswerMode::LlmUnverified);
            assert!(warnings
                .iter()
                .any(|w| w.contains("Removed 1 unverifiable")));
            assert!(warnings.iter().any(|w| w == UNVERIFIED_LLM_WARNING));
        });
    }

    #[test]
    fn dev_gate_auto_attaches_citation_then_validates_before_prune() {
        with_dev_gate(true, || {
            let pins = vec![
                test_pin("CITE-0001", 1, "Kinh phí là 10 triệu đồng"),
                test_pin("CITE-0002", 2, "Thời hạn nộp báo cáo là ngày 15 hằng tháng"),
            ];
            let valid = HashSet::from(["CITE-0001".to_string(), "CITE-0002".to_string()]);
            // Câu 2 đúng nội dung passage CITE-0002 nhưng model quên marker.
            let (answer, mode, warnings) = resolve_llm_answer(
                "Kinh phí là 10 triệu đồng [CITE-0001].\nThời hạn nộp báo cáo là ngày 15 hằng tháng."
                    .into(),
                "extractive fallback text",
                &valid,
                &pins,
                &VersionMode::Current,
                AnswerMode::CloudLlm,
            );
            assert_eq!(mode, AnswerMode::LlmUnverified);
            assert!(
                answer.contains("Thời hạn nộp báo cáo là ngày 15 hằng tháng [CITE-0002]"),
                "expected auto-attached marker, got: {answer}"
            );
            assert!(warnings
                .iter()
                .any(|w| w.contains("Auto-attached citations to 1 sentence(s)")));
            assert!(warnings.iter().any(|w| w == UNVERIFIED_LLM_WARNING));
        });
    }

    #[test]
    fn auto_attach_rejects_low_overlap_sentence_and_falls_back_to_prune() {
        with_dev_gate(true, || {
            let pins = vec![test_pin("CITE-0001", 1, "Kinh phí là 10 triệu đồng")];
            let valid = HashSet::from(["CITE-0001".to_string()]);
            // Câu 2 không liên quan passage nào → không đề xuất được pin;
            // prune giữ câu 1, loại câu 2 (fail-closed nguyên vẹn).
            let (answer, mode, warnings) = resolve_llm_answer(
                "Kinh phí là 10 triệu đồng [CITE-0001].\nGiá vàng thế giới tăng 25% tuần qua."
                    .into(),
                "extractive fallback text",
                &valid,
                &pins,
                &VersionMode::Current,
                AnswerMode::CloudLlm,
            );
            assert_eq!(answer, "Kinh phí là 10 triệu đồng [CITE-0001].");
            assert_eq!(mode, AnswerMode::LlmUnverified);
            assert!(!warnings.iter().any(|w| w.contains("Auto-attached")));
            assert!(warnings
                .iter()
                .any(|w| w.contains("Removed 1 unverifiable")));
        });
    }

    #[test]
    fn auto_attach_returns_none_when_every_line_already_cited() {
        let pins = vec![test_pin("CITE-0001", 1, "Kinh phí là 10 triệu đồng")];
        let valid = HashSet::from(["CITE-0001".to_string()]);
        assert!(attach_citations_to_uncited_lines(
            "Kinh phí là 10 triệu đồng [CITE-0001].",
            &valid,
            &pins,
            &VersionMode::Current,
        )
        .is_none());
    }

    #[test]
    fn auto_attach_result_must_pass_full_validation_or_none() {
        // Đề xuất được pin (overlap chủ đề cao) nhưng giá trị số lệch passage
        // → validation từ chối → None (không được giữ câu sai).
        let pins = vec![test_pin("CITE-0001", 1, "Kinh phí là 10 triệu đồng")];
        let valid = HashSet::from(["CITE-0001".to_string()]);
        assert!(attach_citations_to_uncited_lines(
            "Kinh phí là 99 triệu đồng.",
            &valid,
            &pins,
            &VersionMode::Current,
        )
        .is_none());
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
    /// exercises the identical call path without a live provider endpoint.
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
