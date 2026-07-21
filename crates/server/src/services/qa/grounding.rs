//! Version-aware citation validation, conflict notes, and extractive fallback (P1B-R03).
//!
//! Exact `[CITE-\d{4}]` markers only. Production providers return claim records;
//! the server alone renders answer text + citation markers. Server notes are a typed
//! enum (never claim paragraphs / heading bypass). No token-overlap or short bypass.

use std::collections::{BTreeSet, HashMap, HashSet};

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::db::models::{ConflictSeverity, ConflictStatus, ConflictType};
use crate::db::search::VersionTimelineRow;
use crate::services::citation::StableCitation;
use crate::services::qa::prompt::neutralize_citation_syntax;
use crate::services::retrieval::{RetrievalHit, VersionMode};

/// Deterministic citation label for passage index `n` (1-based).
pub fn cite_id(index: usize) -> String {
    format!("CITE-{:04}", index + 1)
}

/// Exact citation marker pattern: `[CITE-` + exactly 4 digits + `]`.
pub fn is_exact_cite_marker(marker: &str) -> bool {
    let bytes = marker.as_bytes();
    if bytes.len() != 11 {
        return false;
    }
    bytes.starts_with(b"[CITE-")
        && bytes[6].is_ascii_digit()
        && bytes[7].is_ascii_digit()
        && bytes[8].is_ascii_digit()
        && bytes[9].is_ascii_digit()
        && bytes[10] == b']'
}

/// Extracts exact `[CITE-NNNN]` markers; rejects bare/malformed forms as errors.
pub fn extract_exact_cite_ids(answer: &str) -> Result<BTreeSet<String>, GroundingError> {
    let mut cited = BTreeSet::new();
    let mut chars = answer.char_indices().peekable();
    while let Some((i, ch)) = chars.next() {
        if ch == '[' && answer[i..].starts_with("[CITE") {
            let rest = &answer[i..];
            let end = rest.find(']').map(|n| i + n + 1);
            let Some(end) = end else {
                return Err(GroundingError::MalformedCitation);
            };
            let marker = &answer[i..end];
            if !is_exact_cite_marker(marker) {
                return Err(GroundingError::MalformedCitation);
            }
            cited.insert(marker[1..marker.len() - 1].to_string());
            while chars.peek().is_some_and(|(idx, _)| *idx < end) {
                chars.next();
            }
            continue;
        }
        if ch == 'C' && answer[i..].starts_with("CITE-") {
            let tail = &answer[i + 5..];
            let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.len() == 4 {
                let after = tail[4..].chars().next();
                if after.is_none_or(|c| !c.is_ascii_alphanumeric()) {
                    return Err(GroundingError::MalformedCitation);
                }
            }
        }
    }
    Ok(cited)
}

/// Allowlisted passage ready for grounding / extractive fallback.
#[derive(Clone, PartialEq)]
pub struct GroundingPassage {
    pub cite_id: String,
    pub hit: RetrievalHit,
    /// Authoritative quote from R02 StableCitation (required for claim support).
    pub authoritative_quote: Option<String>,
}

impl std::fmt::Debug for GroundingPassage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GroundingPassage")
            .field("cite_id", &self.cite_id)
            .field("chunk_id", &self.hit.chunk_id)
            .field("version_id", &self.hit.version_id)
            .field("is_current", &self.hit.is_current)
            .field("authoritative_quote", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl GroundingPassage {
    pub fn from_hits(hits: &[RetrievalHit]) -> Vec<Self> {
        hits.iter()
            .enumerate()
            .map(|(index, hit)| Self {
                cite_id: cite_id(index),
                hit: hit.clone(),
                authoritative_quote: None,
            })
            .collect()
    }

    pub fn allowlist(passages: &[Self]) -> HashSet<String> {
        passages.iter().map(|p| p.cite_id.clone()).collect()
    }

    pub fn by_cite_id(passages: &[Self]) -> HashMap<String, &Self> {
        passages.iter().map(|p| (p.cite_id.clone(), p)).collect()
    }
}

/// One structured factual claim from the production provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuredClaim {
    pub text: String,
    pub cite_ids: Vec<String>,
    /// Optional typed kind (`"numeric"` requires normalized value+unit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

impl StructuredClaim {
    pub fn is_numeric_kind(&self) -> bool {
        self.kind
            .as_deref()
            .is_some_and(|k| k.eq_ignore_ascii_case("numeric"))
            || self.value.is_some()
            || self.unit.is_some()
    }

    pub fn validate_numeric_fields(&self) -> Result<(), GroundingError> {
        if !self.is_numeric_kind() {
            return Ok(());
        }
        match (
            self.value
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty()),
            self.unit
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty()),
        ) {
            (Some(_), Some(_)) => Ok(()),
            _ => Err(GroundingError::StructuredClaimsRequired),
        }
    }
}

/// Provider grounded payload: claim records only (server renders answer + markers).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderGroundedPayload {
    pub claims: Vec<StructuredClaim>,
    #[serde(default)]
    pub refusal: bool,
}

/// Typed server-authored notes — never treated as provider claims / heading bypass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerNoteKind {
    Change,
    History,
    ConflictOpen,
    ConflictResolved,
    ConflictAcceptedException,
    ConflictFalsePositive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerNote {
    pub kind: ServerNoteKind,
    pub message: String,
    pub pin_cite_ids: Vec<String>,
}

/// Conflict evidence for Q&A — cite ids are required (never `CITE-?`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QaConflict {
    pub conflict_id: Uuid,
    pub status: ConflictStatus,
    pub conflict_type: ConflictType,
    pub severity: ConflictSeverity,
    pub claim_a_id: Uuid,
    pub claim_b_id: Uuid,
    pub claim_a_key: String,
    pub claim_b_key: String,
    pub claim_a_scope: String,
    pub claim_b_scope: String,
    pub claim_a_unit: Option<String>,
    pub claim_b_unit: Option<String>,
    pub claim_a_version_id: Uuid,
    pub claim_b_version_id: Uuid,
    pub claim_a_document_id: Uuid,
    pub claim_b_document_id: Uuid,
    pub claim_a_is_current: bool,
    pub claim_b_is_current: bool,
    pub claim_a_quote: Option<String>,
    pub claim_b_quote: Option<String>,
    pub claim_a_cite_id: String,
    pub claim_b_cite_id: String,
    pub claim_a_numeric: Option<Decimal>,
    pub claim_b_numeric: Option<Decimal>,
    pub resolution_note: Option<String>,
    pub resolution_version_a_id: Option<Uuid>,
    pub resolution_version_b_id: Option<Uuid>,
    /// Resolution-side cite pins (required for Resolved lifecycle).
    pub resolution_a_cite_id: Option<String>,
    pub resolution_b_cite_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictWarning {
    pub conflict_id: Uuid,
    pub status: ConflictStatus,
    pub message: String,
    /// Citation IDs that must be present in returned StableCitation pins.
    pub pin_cite_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryVersionMeta {
    pub version_id: Uuid,
    pub version_number: i32,
    pub is_current: bool,
    pub effective_from: DateTime<Utc>,
    pub effective_to: Option<DateTime<Utc>>,
    pub content_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryPageMeta {
    pub truncated: bool,
    pub limit: usize,
    pub returned: usize,
    /// Request cursor echoed unchanged (keyset consistency).
    pub before_version_no: Option<i32>,
    /// Cursor for the next older page (`None` when not truncated).
    pub next_before_version_no: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionContext {
    pub mode: &'static str,
    pub current_version_ids: Vec<Uuid>,
    pub cited_version_ids: Vec<Uuid>,
    pub change_note: Option<String>,
    pub history: Vec<HistoryVersionMeta>,
    pub history_page: Option<HistoryPageMeta>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum GroundingError {
    #[error("answer cites unknown or fabricated citation")]
    FabricatedCitation,
    #[error("malformed citation marker")]
    MalformedCitation,
    #[error("factual paragraph missing citation")]
    MissingCitation,
    #[error("citation lacks structured claim support")]
    UnsupportedClaim,
    #[error("current answer cites a superseded version")]
    SupersededCitation,
    #[error("compare/history answer mixes versions without required citations")]
    MixedVersionCitation,
    #[error("conflict citation is invalid for the active mode")]
    ConflictCitation,
    #[error("answer failed grounding validation")]
    InvalidGrounding,
    #[error("structured provider claims missing or invalid")]
    StructuredClaimsRequired,
    #[error("current version pointer race")]
    CurrentPointerRace,
    #[error("citation pin snapshot race")]
    PinSnapshotRace,
}

impl GroundingError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::FabricatedCitation => "qa_fabricated_citation",
            Self::MalformedCitation => "qa_malformed_citation",
            Self::MissingCitation => "qa_missing_citation",
            Self::UnsupportedClaim => "qa_unsupported_claim",
            Self::SupersededCitation => "qa_superseded_citation",
            Self::MixedVersionCitation => "qa_mixed_version_citation",
            Self::ConflictCitation => "qa_conflict_citation",
            Self::InvalidGrounding => "qa_invalid_grounding",
            Self::StructuredClaimsRequired => "qa_structured_claims_required",
            Self::CurrentPointerRace => "qa_current_pointer_race",
            Self::PinSnapshotRace => "qa_pin_snapshot_race",
        }
    }
}

fn normalize_claim_text(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn is_refusal_sentence(text: &str) -> bool {
    let trimmed = text.trim();
    let lower = trimmed.to_ascii_lowercase();
    lower.contains("không đủ dữ liệu")
        || lower.contains("không tìm thấy bằng chứng")
        || lower.contains("không có bằng chứng")
        || lower.contains("not enough evidence")
        || lower.contains("i cannot answer")
        || trimmed.starts_with('#')
        // Extractive framing / non-claim meta lines.
        || lower.starts_with("câu hỏi đã được tiếp nhận")
        || lower == "## trả lời trích xuất"
        // Server-generated notes (validated separately via structured trusted evidence).
        || trimmed.starts_with("Thay đổi:")
        || trimmed.starts_with("Lịch sử:")
        || trimmed.starts_with("Cảnh báo xung đột")
        || trimmed.starts_with("Ghi chú lịch sử conflict")
}

/// Split answer into sentences for structured-claim coverage checks.
pub fn split_factual_sentences(answer: &str) -> Vec<String> {
    let mut out = Vec::new();
    for block in answer.split("\n\n") {
        let trimmed = block.trim();
        if trimmed.is_empty() || is_refusal_sentence(trimmed) {
            continue;
        }
        // Strip trailing citation markers for sentence identity.
        let mut body = trimmed.to_string();
        while let Some(pos) = body.rfind("[CITE-") {
            if is_exact_cite_marker(&body[pos..]) {
                body.truncate(pos);
                body = body.trim_end().to_string();
            } else {
                break;
            }
        }
        if body.is_empty() || is_refusal_sentence(&body) {
            continue;
        }
        out.push(body);
    }
    out
}

/// True when claim text is a contiguous normalized substring of the authoritative quote.
pub fn claim_supported_by_quote(claim_text: &str, quote: &str) -> bool {
    let claim_n = normalize_claim_text(claim_text);
    let quote_n = normalize_claim_text(quote);
    if claim_n.is_empty() || quote_n.is_empty() {
        return false;
    }
    quote_n.contains(&claim_n)
}

fn find_claim_for_sentence<'a>(
    sentence: &str,
    claims: &'a [StructuredClaim],
) -> Option<&'a StructuredClaim> {
    let sentence_n = normalize_claim_text(sentence);
    claims.iter().find(|claim| {
        let claim_n = normalize_claim_text(&claim.text);
        !claim_n.is_empty() && (sentence_n.contains(&claim_n) || claim_n.contains(&sentence_n))
    })
}

/// Validates citation subset + version-mode rules + required structured claim support.
pub fn validate_grounded_answer(
    answer: &str,
    passages: &[GroundingPassage],
    mode: &VersionMode,
    conflicts: &[QaConflict],
    structured: &[StructuredClaim],
    require_structured: bool,
) -> Result<Vec<String>, GroundingError> {
    let allowlist = GroundingPassage::allowlist(passages);
    let by_id = GroundingPassage::by_cite_id(passages);
    let cited = extract_exact_cite_ids(answer)?;
    let sentences = split_factual_sentences(answer);

    if require_structured {
        if structured.is_empty() && !sentences.is_empty() {
            return Err(GroundingError::StructuredClaimsRequired);
        }
        for sentence in &sentences {
            let Some(claim) = find_claim_for_sentence(sentence, structured) else {
                return Err(GroundingError::StructuredClaimsRequired);
            };
            if claim.cite_ids.is_empty() {
                return Err(GroundingError::MissingCitation);
            }
            for cite in &claim.cite_ids {
                if !allowlist.contains(cite) {
                    return Err(GroundingError::FabricatedCitation);
                }
                let passage = by_id.get(cite).ok_or(GroundingError::FabricatedCitation)?;
                let quote = passage
                    .authoritative_quote
                    .as_deref()
                    .ok_or(GroundingError::UnsupportedClaim)?;
                if !claim_supported_by_quote(&claim.text, quote) {
                    return Err(GroundingError::UnsupportedClaim);
                }
                if let (Some(value), Some(unit)) = (claim.value.as_deref(), claim.unit.as_deref()) {
                    let quote_l = quote.to_lowercase();
                    if !quote_l.contains(&value.to_lowercase())
                        || !quote_l.contains(&unit.to_lowercase())
                    {
                        return Err(GroundingError::UnsupportedClaim);
                    }
                }
            }
        }
    } else {
        // Offline/extractive path: every factual sentence still needs an exact cite.
        for sentence_block in answer.split("\n\n") {
            let trimmed = sentence_block.trim();
            if trimmed.is_empty() || is_refusal_sentence(trimmed) {
                continue;
            }
            if extract_exact_cite_ids(trimmed)?.is_empty() {
                return Err(GroundingError::MissingCitation);
            }
        }
    }

    if cited.is_empty() {
        if !sentences.is_empty() {
            return Err(GroundingError::InvalidGrounding);
        }
        return Ok(Vec::new());
    }

    for cite in &cited {
        if !allowlist.contains(cite) {
            return Err(GroundingError::FabricatedCitation);
        }
        let passage = by_id.get(cite).ok_or(GroundingError::FabricatedCitation)?;
        if passage.authoritative_quote.is_none() {
            return Err(GroundingError::UnsupportedClaim);
        }
        if require_structured {
            let supported = structured.iter().any(|claim| {
                claim.cite_ids.iter().any(|c| c == cite)
                    && claim_supported_by_quote(
                        &claim.text,
                        passage.authoritative_quote.as_deref().unwrap_or(""),
                    )
            });
            if !supported {
                return Err(GroundingError::UnsupportedClaim);
            }
        }
    }

    match mode {
        VersionMode::Current => {
            for cite in &cited {
                let passage = by_id.get(cite).ok_or(GroundingError::FabricatedCitation)?;
                if !passage.hit.is_current {
                    return Err(GroundingError::SupersededCitation);
                }
            }
        }
        VersionMode::Compare {
            version_a,
            version_b,
            ..
        } => {
            let mut saw_a = false;
            let mut saw_b = false;
            let mut foreign = false;
            for cite in &cited {
                let passage = by_id.get(cite).ok_or(GroundingError::FabricatedCitation)?;
                if passage.hit.version_id == *version_a {
                    saw_a = true;
                } else if passage.hit.version_id == *version_b {
                    saw_b = true;
                } else {
                    foreign = true;
                }
            }
            if foreign || !saw_a || !saw_b {
                return Err(GroundingError::MixedVersionCitation);
            }
            let cited_versions: BTreeSet<Uuid> = cited
                .iter()
                .filter_map(|c| by_id.get(c).map(|p| p.hit.version_id))
                .collect();
            if cited_versions.len() != 2
                || !cited_versions.contains(version_a)
                || !cited_versions.contains(version_b)
            {
                return Err(GroundingError::MixedVersionCitation);
            }
        }
        VersionMode::History { document_id, .. } => {
            let versions: BTreeSet<Uuid> = cited
                .iter()
                .filter_map(|c| by_id.get(c).map(|p| p.hit.version_id))
                .collect();
            for cite in &cited {
                let passage = by_id.get(cite).ok_or(GroundingError::FabricatedCitation)?;
                if passage.hit.document_id != *document_id
                    && !cite_is_authorized_conflict_pin(cite, conflicts)
                {
                    return Err(GroundingError::MixedVersionCitation);
                }
            }
            // Terminal 1-version history is valid when current is the context anchor.
            if versions.is_empty() {
                return Err(GroundingError::MixedVersionCitation);
            }
        }
        VersionMode::AsOf { at } => {
            for cite in &cited {
                let passage = by_id.get(cite).ok_or(GroundingError::FabricatedCitation)?;
                if !version_effective_at(&passage.hit, *at) {
                    return Err(GroundingError::SupersededCitation);
                }
            }
        }
    }

    validate_conflict_citations(answer, &cited, mode, conflicts)?;
    Ok(cited.into_iter().collect())
}

fn cite_is_authorized_conflict_pin(cite: &str, conflicts: &[QaConflict]) -> bool {
    conflicts.iter().any(|c| {
        c.claim_a_cite_id == cite
            || c.claim_b_cite_id == cite
            || c.resolution_a_cite_id.as_deref() == Some(cite)
            || c.resolution_b_cite_id.as_deref() == Some(cite)
    })
}

fn version_effective_at(hit: &RetrievalHit, at: DateTime<Utc>) -> bool {
    hit.effective_from <= at && hit.effective_to.is_none_or(|t| t > at)
}

/// Inputs for final answer revalidation after server notes/pins.
pub struct FinalAnswerCheck<'a> {
    pub answer: &'a str,
    pub passages: &'a [GroundingPassage],
    pub mode: &'a VersionMode,
    pub conflicts: &'a [QaConflict],
    pub structured: &'a [StructuredClaim],
    pub require_structured: bool,
    pub returned_citations: &'a [StableCitation],
    /// Pins from provider claims only.
    pub claim_cite_ids: &'a [String],
    /// Pins from typed server notes only.
    pub server_note_pin_cites: &'a [String],
}

/// Re-validates claim body + returned pins. Claim cites and server-note pins
/// are checked separately; the returned citation set must cover their union.
pub fn revalidate_final_answer(input: FinalAnswerCheck<'_>) -> Result<(), GroundingError> {
    for claim in input.structured {
        claim.validate_numeric_fields()?;
    }
    let _ = validate_grounded_answer(
        input.answer,
        input.passages,
        input.mode,
        input.conflicts,
        input.structured,
        input.require_structured,
    )?;
    let by_id = GroundingPassage::by_cite_id(input.passages);
    let returned_chunks: HashSet<Uuid> = input
        .returned_citations
        .iter()
        .map(|c| c.chunk_id)
        .collect();
    // Canonical union: every claim cite and every server-note pin must resolve.
    let mut union: BTreeSet<&str> = BTreeSet::new();
    for cite in input.claim_cite_ids {
        union.insert(cite.as_str());
    }
    for cite in input.server_note_pin_cites {
        union.insert(cite.as_str());
    }
    for cite in input.claim_cite_ids {
        let passage = by_id.get(cite).ok_or(GroundingError::FabricatedCitation)?;
        if !returned_chunks.contains(&passage.hit.chunk_id) {
            return Err(GroundingError::InvalidGrounding);
        }
        let pin = input
            .returned_citations
            .iter()
            .find(|c| c.chunk_id == passage.hit.chunk_id)
            .ok_or(GroundingError::InvalidGrounding)?;
        if pin.version_id != passage.hit.version_id {
            return Err(GroundingError::InvalidGrounding);
        }
        if let Some(quote) = passage.authoritative_quote.as_deref() {
            if pin.quote != quote {
                return Err(GroundingError::InvalidGrounding);
            }
        }
        if matches!(input.mode, VersionMode::Current) && !pin.is_current {
            return Err(GroundingError::PinSnapshotRace);
        }
        if let VersionMode::AsOf { at } = input.mode {
            if pin.effective_from > *at || pin.effective_to.is_some_and(|t| t <= *at) {
                return Err(GroundingError::PinSnapshotRace);
            }
        }
    }
    for pin_cite in input.server_note_pin_cites {
        let passage = by_id
            .get(pin_cite)
            .ok_or(GroundingError::ConflictCitation)?;
        if !returned_chunks.contains(&passage.hit.chunk_id) {
            return Err(GroundingError::ConflictCitation);
        }
        let pin = input
            .returned_citations
            .iter()
            .find(|c| c.chunk_id == passage.hit.chunk_id)
            .ok_or(GroundingError::ConflictCitation)?;
        if pin.version_id != passage.hit.version_id {
            return Err(GroundingError::ConflictCitation);
        }
    }
    // Returned set must equal the canonical union (no extras from other paths).
    if input.returned_citations.len() < union.len() {
        return Err(GroundingError::InvalidGrounding);
    }
    Ok(())
}

fn answer_mentions_conflict(answer: &str) -> bool {
    let lower = answer.to_ascii_lowercase();
    lower.contains("conflict")
        || answer.contains("mâu thuẫn")
        || answer.contains("xung đột")
        || answer.contains("Cảnh báo xung đột")
        || answer.contains("Ghi chú lịch sử conflict")
        || answer.contains("accepted_exception")
        || answer.contains("false_positive")
}

fn validate_conflict_citations(
    answer: &str,
    cited: &BTreeSet<String>,
    mode: &VersionMode,
    conflicts: &[QaConflict],
) -> Result<(), GroundingError> {
    if conflicts.is_empty() || !answer_mentions_conflict(answer) {
        return Ok(());
    }
    for conflict in conflicts {
        if conflict.claim_a_cite_id.is_empty()
            || conflict.claim_b_cite_id.is_empty()
            || conflict.claim_a_cite_id.contains('?')
            || conflict.claim_b_cite_id.contains('?')
        {
            return Err(GroundingError::ConflictCitation);
        }
        validate_conflict_lifecycle(conflict)?;
        match (mode, conflict.status) {
            (VersionMode::Current, ConflictStatus::Open)
            | (
                VersionMode::History { .. } | VersionMode::Compare { .. },
                ConflictStatus::Resolved
                | ConflictStatus::AcceptedException
                | ConflictStatus::FalsePositive,
            ) => {
                if !cited.contains(&conflict.claim_a_cite_id)
                    || !cited.contains(&conflict.claim_b_cite_id)
                {
                    return Err(GroundingError::ConflictCitation);
                }
            }
            (VersionMode::Current, _) => {
                if answer.contains(&conflict.conflict_id.to_string()) {
                    return Err(GroundingError::ConflictCitation);
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Lifecycle invariants for conflict rows used in Q&A notes.
pub fn validate_conflict_lifecycle(conflict: &QaConflict) -> Result<(), GroundingError> {
    match conflict.status {
        ConflictStatus::Open => {
            if conflict.resolution_version_a_id.is_some()
                || conflict.resolution_version_b_id.is_some()
            {
                return Err(GroundingError::ConflictCitation);
            }
        }
        ConflictStatus::AcceptedException | ConflictStatus::FalsePositive => {
            if conflict.resolution_version_a_id.is_some()
                || conflict.resolution_version_b_id.is_some()
            {
                return Err(GroundingError::ConflictCitation);
            }
        }
        ConflictStatus::Resolved => {
            if conflict.resolution_version_a_id.is_none()
                || conflict.resolution_version_b_id.is_none()
                || conflict
                    .resolution_a_cite_id
                    .as_ref()
                    .is_none_or(|s| s.is_empty())
                || conflict
                    .resolution_b_cite_id
                    .as_ref()
                    .is_none_or(|s| s.is_empty())
            {
                return Err(GroundingError::ConflictCitation);
            }
        }
    }
    Ok(())
}

/// Deterministic extractive answer from authorized passages (provider outage path).
/// Source text is citation-neutralized before the server renders the fallback marker.
pub fn extractive_answer(question: &str, passages: &[GroundingPassage]) -> String {
    if passages.is_empty() {
        return "Không tìm thấy bằng chứng phù hợp trong kho tri thức.".into();
    }
    let _ = question;
    let mut answer = String::from("## Trả lời trích xuất\n\n");
    for (index, passage) in passages.iter().enumerate() {
        let quote = passage
            .authoritative_quote
            .as_deref()
            .unwrap_or(passage.hit.snippet.trim());
        let neutralized = neutralize_citation_syntax(quote);
        answer.push_str(&format!(
            "{}. {} [{}]\n\n",
            index + 1,
            neutralized,
            passage.cite_id
        ));
    }
    answer
}

/// Server-only answer renderer from validated claim records (one claim → one paragraph).
pub fn render_answer_from_claims(claims: &[StructuredClaim]) -> String {
    if claims.is_empty() {
        return "Không tìm thấy bằng chứng phù hợp trong kho tri thức.".into();
    }
    let mut out = String::new();
    for (index, claim) in claims.iter().enumerate() {
        if index > 0 {
            out.push_str("\n\n");
        }
        let text = neutralize_citation_syntax(claim.text.trim());
        out.push_str(&text);
        for cite in &claim.cite_ids {
            out.push_str(" [");
            out.push_str(cite);
            out.push(']');
        }
    }
    out
}

/// Append typed server notes (separated from claim paragraphs).
pub fn append_server_notes(answer: &str, notes: &[ServerNote]) -> String {
    let mut out = answer.to_string();
    for note in notes {
        if note.message.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&note.message);
    }
    out
}

/// Validate claim records one-to-one against passages (no free-paragraph answer path).
pub fn validate_structured_claims(
    claims: &[StructuredClaim],
    passages: &[GroundingPassage],
    mode: &VersionMode,
    require_non_empty: bool,
) -> Result<Vec<String>, GroundingError> {
    let allowlist = GroundingPassage::allowlist(passages);
    let by_id = GroundingPassage::by_cite_id(passages);
    if require_non_empty && claims.is_empty() {
        return Err(GroundingError::StructuredClaimsRequired);
    }
    let mut cited = BTreeSet::new();
    for claim in claims {
        let text = claim.text.trim();
        if text.is_empty() || is_refusal_sentence(text) {
            return Err(GroundingError::StructuredClaimsRequired);
        }
        // Headings / markdown structure cannot bypass claim validation.
        if text.starts_with('#') {
            return Err(GroundingError::StructuredClaimsRequired);
        }
        claim.validate_numeric_fields()?;
        if claim.cite_ids.is_empty() {
            return Err(GroundingError::MissingCitation);
        }
        for cite in &claim.cite_ids {
            if !allowlist.contains(cite) {
                return Err(GroundingError::FabricatedCitation);
            }
            let passage = by_id.get(cite).ok_or(GroundingError::FabricatedCitation)?;
            let quote = passage
                .authoritative_quote
                .as_deref()
                .ok_or(GroundingError::UnsupportedClaim)?;
            if !claim_supported_by_quote(&claim.text, quote) {
                return Err(GroundingError::UnsupportedClaim);
            }
            if let (Some(value), Some(unit)) = (claim.value.as_deref(), claim.unit.as_deref()) {
                let quote_l = quote.to_lowercase();
                if !quote_l.contains(&value.to_lowercase())
                    || !quote_l.contains(&unit.to_lowercase())
                {
                    return Err(GroundingError::UnsupportedClaim);
                }
            }
            cited.insert(cite.clone());
        }
    }
    // Mode checks over cited set.
    match mode {
        VersionMode::Current => {
            for cite in &cited {
                let passage = by_id.get(cite).ok_or(GroundingError::FabricatedCitation)?;
                if !passage.hit.is_current {
                    return Err(GroundingError::SupersededCitation);
                }
            }
        }
        VersionMode::Compare {
            version_a,
            version_b,
            ..
        } => {
            let mut saw_a = false;
            let mut saw_b = false;
            for cite in &cited {
                let passage = by_id.get(cite).ok_or(GroundingError::FabricatedCitation)?;
                if passage.hit.version_id == *version_a {
                    saw_a = true;
                } else if passage.hit.version_id == *version_b {
                    saw_b = true;
                } else {
                    return Err(GroundingError::MixedVersionCitation);
                }
            }
            if !saw_a || !saw_b {
                return Err(GroundingError::MixedVersionCitation);
            }
        }
        VersionMode::History { document_id, .. } => {
            // Terminal 1-version history is valid (context anchor = current pointer,
            // loaded separately). Empty cites fail elsewhere; do not require ≥2.
            for cite in &cited {
                let passage = by_id.get(cite).ok_or(GroundingError::FabricatedCitation)?;
                if passage.hit.document_id != *document_id {
                    // Cross-doc cites are only allowed as typed conflict-note pins.
                    return Err(GroundingError::MixedVersionCitation);
                }
            }
        }
        VersionMode::AsOf { at } => {
            for cite in &cited {
                let passage = by_id.get(cite).ok_or(GroundingError::FabricatedCitation)?;
                if !version_effective_at(&passage.hit, *at) {
                    return Err(GroundingError::SupersededCitation);
                }
            }
        }
    }
    Ok(cited.into_iter().collect())
}

/// Structured claims mirroring an extractive fallback answer.
pub fn extractive_structured_claims(passages: &[GroundingPassage]) -> Vec<StructuredClaim> {
    passages
        .iter()
        .map(|passage| {
            let quote = passage
                .authoritative_quote
                .as_deref()
                .unwrap_or(passage.hit.snippet.trim());
            StructuredClaim {
                text: neutralize_citation_syntax(quote),
                cite_ids: vec![passage.cite_id.clone()],
                kind: None,
                value: None,
                unit: None,
            }
        })
        .collect()
}

/// Builds version_context. History timeline must come from authoritative DB rows.
///
/// `current_pointer_ids`: when `Some`, used as `current_version_ids` (M5: AsOf loads
/// current pointers separately from as-of cited versions). `None` derives from
/// timeline/passages.
pub fn build_version_context(
    mode: &VersionMode,
    passages: &[GroundingPassage],
    cited_ids: &[String],
    timeline: &[VersionTimelineRow],
    typed_delta: Option<&TypedDeltaMatch>,
    history_page: Option<HistoryPageMeta>,
    current_pointer_ids: Option<Vec<Uuid>>,
) -> Result<VersionContext, GroundingError> {
    let by_id = GroundingPassage::by_cite_id(passages);
    let mut cited_version_ids = BTreeSet::new();
    for cite in cited_ids {
        if let Some(passage) = by_id.get(cite) {
            cited_version_ids.insert(passage.hit.version_id);
        }
    }
    let current_version_ids: Vec<Uuid> = if let Some(ids) = current_pointer_ids {
        ids.into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    } else {
        match mode {
            VersionMode::History { .. } => timeline
                .iter()
                .filter(|row| row.is_current)
                .map(|row| row.version_id)
                .collect(),
            _ => passages
                .iter()
                .filter(|p| p.hit.is_current)
                .map(|p| p.hit.version_id)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
        }
    };

    let history: Vec<HistoryVersionMeta> = if matches!(mode, VersionMode::History { .. }) {
        if timeline.is_empty() {
            return Err(GroundingError::MixedVersionCitation);
        }
        timeline
            .iter()
            .map(|row| HistoryVersionMeta {
                version_id: row.version_id,
                version_number: row.version_number,
                is_current: row.is_current,
                effective_from: row.effective_from,
                effective_to: row.effective_to,
                content_sha256: row.content_sha256.clone(),
            })
            .collect()
    } else {
        Vec::new()
    };

    let mode_label = match mode {
        VersionMode::Current => "current",
        VersionMode::AsOf { .. } => "as_of",
        VersionMode::Compare { .. } => "compare",
        VersionMode::History { .. } => "history",
    };

    let change_note = match mode {
        VersionMode::Compare {
            version_a,
            version_b,
            ..
        } => typed_change_note(passages, *version_a, *version_b, typed_delta),
        VersionMode::History { .. } => {
            let older = &history[0];
            let newer = history.last().unwrap();
            Some(format!(
                "Lịch sử: phiên bản {} → phiên bản {} (current={}).",
                older.version_number, newer.version_number, newer.is_current
            ))
        }
        _ => None,
    };

    Ok(VersionContext {
        mode: mode_label,
        current_version_ids,
        cited_version_ids: cited_version_ids.into_iter().collect(),
        change_note,
        history,
        history_page,
    })
}

/// Page a history timeline: fetch `page_size + 1` (newest-first), truncate.
///
/// Current is **not** spliced into the page — callers load the current pointer
/// separately every page (`current_version_ids`). Evidence must stay restricted
/// to the returned page version IDs.
pub fn page_history_timeline(
    rows: Vec<VersionTimelineRow>,
    page_size: usize,
    before_version_no: Option<i32>,
) -> Result<(Vec<VersionTimelineRow>, Option<HistoryPageMeta>), GroundingError> {
    let truncated = rows.len() > page_size;
    let mut page = rows;
    if truncated {
        page.truncate(page_size);
    }
    // Terminal single-version history is supported (context anchor = current
    // loaded separately). Empty page is invalid.
    if page.is_empty() {
        return Err(GroundingError::MixedVersionCitation);
    }
    let next_before_version_no = if truncated {
        page.last().map(|r| r.version_number)
    } else {
        None
    };
    // Chronological order for consumers.
    page.sort_by_key(|r| (r.version_number, r.version_id));
    let meta = HistoryPageMeta {
        truncated,
        limit: page_size,
        returned: page.len(),
        before_version_no,
        next_before_version_no,
    };
    Ok((page, Some(meta)))
}

/// Typed numeric pair for compare deltas (ordered by version_number lineage).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedVersionClaim {
    pub version_id: Uuid,
    pub version_number: i32,
    pub claim_key: String,
    pub scope: String,
    pub unit: Option<String>,
    pub value: Decimal,
    pub chunk_id: Option<Uuid>,
    pub cite_id: Option<String>,
}

/// Exact delta match result with lineage-ordered values and cite/chunk pins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedDeltaMatch {
    pub old_value: Decimal,
    pub new_value: Decimal,
    pub delta: Decimal,
    pub older_version_number: i32,
    pub newer_version_number: i32,
    pub older_cite_id: String,
    pub newer_cite_id: String,
    pub older_chunk_id: Uuid,
    pub newer_chunk_id: Uuid,
    pub claim_key: String,
    pub scope: String,
    pub unit: Option<String>,
}

/// Build a [`TypedDeltaMatch`] from a deterministic DB join row + passage cites.
pub fn typed_delta_from_pair_row(
    row: &crate::db::search::TypedDeltaPairRow,
    passages: &[GroundingPassage],
) -> Option<TypedDeltaMatch> {
    let older_cite = passages
        .iter()
        .find(|p| p.hit.chunk_id == row.older_chunk_id && p.hit.version_id == row.older_version_id)?
        .cite_id
        .clone();
    let newer_cite = passages
        .iter()
        .find(|p| p.hit.chunk_id == row.newer_chunk_id && p.hit.version_id == row.newer_version_id)?
        .cite_id
        .clone();
    Some(TypedDeltaMatch {
        old_value: row.older_value,
        new_value: row.newer_value,
        delta: row.newer_value - row.older_value,
        older_version_number: row.older_version_number,
        newer_version_number: row.newer_version_number,
        older_cite_id: older_cite,
        newer_cite_id: newer_cite,
        older_chunk_id: row.older_chunk_id,
        newer_chunk_id: row.newer_chunk_id,
        claim_key: row.claim_key.clone(),
        scope: row.scope.clone(),
        unit: row.unit.clone(),
    })
}

/// Numeric delta only for typed claims on exactly version_a/version_b with same
/// key/scope/unit and deterministic chunk/cite pins. Ordered by `version_number`.
pub fn typed_numeric_delta_for_versions(
    version_a: Uuid,
    version_b: Uuid,
    claims: &[TypedVersionClaim],
) -> Option<TypedDeltaMatch> {
    let a = claims.iter().find(|c| c.version_id == version_a)?;
    let b = claims.iter().find(|c| c.version_id == version_b)?;
    if a.claim_key != b.claim_key || a.scope != b.scope || a.unit != b.unit {
        return None;
    }
    let a_cite = a.cite_id.as_ref()?;
    let b_cite = b.cite_id.as_ref()?;
    let a_chunk = a.chunk_id?;
    let b_chunk = b.chunk_id?;
    let (older, newer) = if a.version_number <= b.version_number {
        (a, b)
    } else {
        (b, a)
    };
    let (older_cite, newer_cite, older_chunk, newer_chunk) = if a.version_number <= b.version_number
    {
        (a_cite.clone(), b_cite.clone(), a_chunk, b_chunk)
    } else {
        (b_cite.clone(), a_cite.clone(), b_chunk, a_chunk)
    };
    Some(TypedDeltaMatch {
        old_value: older.value,
        new_value: newer.value,
        delta: newer.value - older.value,
        older_version_number: older.version_number,
        newer_version_number: newer.version_number,
        older_cite_id: older_cite,
        newer_cite_id: newer_cite,
        older_chunk_id: older_chunk,
        newer_chunk_id: newer_chunk,
        claim_key: older.claim_key.clone(),
        scope: older.scope.clone(),
        unit: older.unit.clone(),
    })
}

fn typed_change_note(
    passages: &[GroundingPassage],
    version_a: Uuid,
    version_b: Uuid,
    typed_delta: Option<&TypedDeltaMatch>,
) -> Option<String> {
    let a = passages.iter().find(|p| p.hit.version_id == version_a)?;
    let b = passages.iter().find(|p| p.hit.version_id == version_b)?;
    let (older, newer) = if a.hit.version_number <= b.hit.version_number {
        (a, b)
    } else {
        (b, a)
    };
    match typed_delta {
        Some(delta) => {
            let direction = if delta.delta > Decimal::ZERO {
                "tăng"
            } else if delta.delta < Decimal::ZERO {
                "giảm"
            } else {
                "không đổi"
            };
            let unit = delta
                .unit
                .as_deref()
                .filter(|u| !u.is_empty())
                .map(|u| format!(" {u}"))
                .unwrap_or_default();
            Some(format!(
                "Thay đổi: phiên bản {} là {}{} [{}], phiên bản {} là {}{} [{}], {} {}{}.",
                delta.older_version_number,
                delta.old_value.normalize(),
                unit,
                delta.older_cite_id,
                delta.newer_version_number,
                delta.new_value.normalize(),
                unit,
                delta.newer_cite_id,
                direction,
                delta.delta.abs().normalize(),
                unit
            ))
        }
        None => Some(format!(
            "Thay đổi: phiên bản {} [{}] → phiên bản {} [{}].",
            older.hit.version_number, older.cite_id, newer.hit.version_number, newer.cite_id
        )),
    }
}

/// Current unresolved warnings + terminal-history notes (ADR 0003). Never emits `CITE-?`.
pub fn conflict_messages(mode: &VersionMode, conflicts: &[QaConflict]) -> Vec<ConflictWarning> {
    let mut out = Vec::new();
    for conflict in conflicts {
        if conflict.claim_a_cite_id.is_empty()
            || conflict.claim_b_cite_id.is_empty()
            || conflict.claim_a_cite_id.contains('?')
            || conflict.claim_b_cite_id.contains('?')
        {
            continue;
        }
        if validate_conflict_lifecycle(conflict).is_err() {
            continue;
        }
        match (mode, conflict.status) {
            (VersionMode::Current, ConflictStatus::Open)
            | (VersionMode::AsOf { .. }, ConflictStatus::Open) => {
                // M2: AsOf SQL already filters lifecycle-at-timestamp; render as open.
                out.push(ConflictWarning {
                    conflict_id: conflict.conflict_id,
                    status: ConflictStatus::Open,
                    message: unresolved_current_warning(conflict),
                    pin_cite_ids: vec![
                        conflict.claim_a_cite_id.clone(),
                        conflict.claim_b_cite_id.clone(),
                    ],
                });
            }
            // AsOf may surface rows whose DB status later became terminal, but the
            // evidence filter selected them as still-open at `at` — render open.
            (VersionMode::AsOf { .. }, _) => {
                out.push(ConflictWarning {
                    conflict_id: conflict.conflict_id,
                    status: ConflictStatus::Open,
                    message: unresolved_current_warning(conflict),
                    pin_cite_ids: vec![
                        conflict.claim_a_cite_id.clone(),
                        conflict.claim_b_cite_id.clone(),
                    ],
                });
            }
            (VersionMode::History { .. }, ConflictStatus::Resolved) => {
                let (Some(ra), Some(rb)) = (
                    conflict.resolution_a_cite_id.clone(),
                    conflict.resolution_b_cite_id.clone(),
                ) else {
                    continue;
                };
                out.push(ConflictWarning {
                    conflict_id: conflict.conflict_id,
                    status: conflict.status,
                    message: resolved_history_note(conflict, &ra, &rb),
                    pin_cite_ids: vec![
                        conflict.claim_a_cite_id.clone(),
                        conflict.claim_b_cite_id.clone(),
                        ra,
                        rb,
                    ],
                });
            }
            (VersionMode::History { .. }, ConflictStatus::AcceptedException) => {
                out.push(ConflictWarning {
                    conflict_id: conflict.conflict_id,
                    status: conflict.status,
                    message: accepted_exception_note(conflict),
                    pin_cite_ids: vec![
                        conflict.claim_a_cite_id.clone(),
                        conflict.claim_b_cite_id.clone(),
                    ],
                });
            }
            (VersionMode::History { .. }, ConflictStatus::FalsePositive) => {
                out.push(ConflictWarning {
                    conflict_id: conflict.conflict_id,
                    status: conflict.status,
                    message: false_positive_note(conflict),
                    pin_cite_ids: vec![
                        conflict.claim_a_cite_id.clone(),
                        conflict.claim_b_cite_id.clone(),
                    ],
                });
            }
            (VersionMode::Current, _) => {}
            _ => {}
        }
    }
    out.sort_by_key(|warning| warning.conflict_id);
    out
}

fn unresolved_current_warning(conflict: &QaConflict) -> String {
    match (
        conflict.conflict_type,
        conflict.claim_a_numeric,
        conflict.claim_b_numeric,
    ) {
        (ConflictType::Numeric, Some(a), Some(b))
            if conflict.claim_a_key == conflict.claim_b_key
                && conflict.claim_a_scope == conflict.claim_b_scope
                && conflict.claim_a_unit == conflict.claim_b_unit =>
        {
            let delta = (b - a).abs().normalize();
            format!(
                "Cảnh báo xung đột số (open): {} [{}] khác {} [{}], chênh {}.",
                a.normalize(),
                conflict.claim_a_cite_id,
                b.normalize(),
                conflict.claim_b_cite_id,
                delta
            )
        }
        _ => format!(
            "Cảnh báo xung đột chưa giải quyết (open) giữa [{}] và [{}].",
            conflict.claim_a_cite_id, conflict.claim_b_cite_id
        ),
    }
}

fn resolved_history_note(conflict: &QaConflict, res_a: &str, res_b: &str) -> String {
    let resolution = neutralize_citation_syntax(
        conflict
            .resolution_note
            .as_deref()
            .unwrap_or("đã căn chỉnh ở phiên bản sau"),
    );
    format!(
        "Ghi chú lịch sử conflict (resolved): trước đây [{}] vs [{}]; \
         đã căn chỉnh tại [{}] / [{}]; {}.",
        conflict.claim_a_cite_id, conflict.claim_b_cite_id, res_a, res_b, resolution
    )
}

fn accepted_exception_note(conflict: &QaConflict) -> String {
    let note = neutralize_citation_syntax(
        conflict
            .resolution_note
            .as_deref()
            .unwrap_or("được chấp nhận như ngoại lệ"),
    );
    format!(
        "Ghi chú lịch sử conflict (accepted_exception): [{}] vs [{}]; {}.",
        conflict.claim_a_cite_id, conflict.claim_b_cite_id, note
    )
}

fn false_positive_note(conflict: &QaConflict) -> String {
    let note = neutralize_citation_syntax(
        conflict
            .resolution_note
            .as_deref()
            .unwrap_or("đánh dấu false_positive"),
    );
    format!(
        "Ghi chú lịch sử conflict (false_positive): [{}] vs [{}]; {}.",
        conflict.claim_a_cite_id, conflict.claim_b_cite_id, note
    )
}

pub fn passages_for_mode<'a>(
    passages: &'a [GroundingPassage],
    mode: &VersionMode,
) -> Vec<&'a GroundingPassage> {
    match mode {
        VersionMode::Current => passages.iter().filter(|p| p.hit.is_current).collect(),
        VersionMode::Compare {
            version_a,
            version_b,
            ..
        } => passages
            .iter()
            .filter(|p| p.hit.version_id == *version_a || p.hit.version_id == *version_b)
            .collect(),
        // History claims stay on the requested document; conflict counterparts hydrate
        // separately into server-note pins (H11) and must not enter claim citations.
        VersionMode::History { document_id, .. } => passages
            .iter()
            .filter(|p| p.hit.document_id == *document_id)
            .collect(),
        VersionMode::AsOf { .. } => passages.iter().collect(),
    }
}

/// Compare must reference exactly the two requested versions among passages.
pub fn filter_compare_passages(
    passages: &[GroundingPassage],
    version_a: Uuid,
    version_b: Uuid,
) -> Result<Vec<GroundingPassage>, GroundingError> {
    let filtered: Vec<_> = passages
        .iter()
        .filter(|p| p.hit.version_id == version_a || p.hit.version_id == version_b)
        .cloned()
        .collect();
    let versions: BTreeSet<Uuid> = filtered.iter().map(|p| p.hit.version_id).collect();
    if versions.len() != 2 || !versions.contains(&version_a) || !versions.contains(&version_b) {
        return Err(GroundingError::MixedVersionCitation);
    }
    Ok(filtered)
}

/// Ensure passages cover every required version id (representative evidence).
pub fn ensure_versions_represented(
    passages: &[GroundingPassage],
    required_versions: &[Uuid],
) -> Result<(), GroundingError> {
    let present: BTreeSet<Uuid> = passages.iter().map(|p| p.hit.version_id).collect();
    for version in required_versions {
        if !present.contains(version) {
            return Err(GroundingError::MixedVersionCitation);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn hit(version_id: Uuid, version_number: i32, is_current: bool, snippet: &str) -> RetrievalHit {
        RetrievalHit {
            chunk_id: Uuid::new_v4(),
            chunk_identity_sha256: "a".repeat(64),
            collection_id: Uuid::new_v4(),
            document_id: Uuid::parse_str("66666666-6666-6666-6666-666666666601").unwrap(),
            version_id,
            version_number,
            content_sha256: "b".repeat(64),
            heading: "Kinh phí".into(),
            snippet: snippet.into(),
            body: snippet.into(),
            lexical_score: 1.0,
            vector_score: 0.5,
            rerank_score: 1.5,
            is_current,
            effective_from: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
            effective_to: None,
            page: Some(1),
            slide: None,
            sheet: None,
            span_start: 0,
            span_end: snippet.len(),
        }
    }

    fn with_quote(mut passage: GroundingPassage, quote: &str) -> GroundingPassage {
        passage.authoritative_quote = Some(quote.into());
        passage
    }

    #[test]
    fn rejects_malformed_and_bare_citation_markers() {
        assert_eq!(
            extract_exact_cite_ids("x [CITE-12] y"),
            Err(GroundingError::MalformedCitation)
        );
        assert_eq!(
            extract_exact_cite_ids("x CITE-0001 y"),
            Err(GroundingError::MalformedCitation)
        );
        assert_eq!(
            extract_exact_cite_ids("ok [CITE-0001]"),
            Ok(BTreeSet::from(["CITE-0001".into()]))
        );
    }

    #[test]
    fn requires_structured_claims_no_token_overlap_bypass() {
        let v2 = Uuid::parse_str("77777777-7777-7777-7777-777777777702").unwrap();
        let quote = "Kinh phí phê duyệt là 15 triệu đồng.";
        let passages = vec![with_quote(
            GroundingPassage::from_hits(&[hit(v2, 2, true, quote)])
                .into_iter()
                .next()
                .unwrap(),
            quote,
        )];
        let short = "Ok. [CITE-0001]";
        assert_eq!(
            validate_grounded_answer(short, &passages, &VersionMode::Current, &[], &[], true),
            Err(GroundingError::StructuredClaimsRequired)
        );
        let structured = [StructuredClaim {
            text: quote.into(),
            cite_ids: vec!["CITE-0001".into()],
            kind: Some("numeric".into()),
            value: Some("15".into()),
            unit: Some("triệu".into()),
        }];
        assert!(validate_grounded_answer(
            "Kinh phí phê duyệt là 15 triệu đồng. [CITE-0001]",
            &passages,
            &VersionMode::Current,
            &[],
            &structured,
            true
        )
        .is_ok());
    }

    #[test]
    fn extractive_neutralizes_source_before_fallback_marker() {
        let v2 = Uuid::parse_str("77777777-7777-7777-7777-777777777702").unwrap();
        let quote = "Giá trị [CITE-9999] trong nguồn.";
        let passages = vec![with_quote(
            GroundingPassage::from_hits(&[hit(v2, 2, true, quote)])
                .into_iter()
                .next()
                .unwrap(),
            quote,
        )];
        let answer = extractive_answer("q?", &passages);
        assert!(!answer.contains("[CITE-9999]"));
        assert!(answer.contains("[CITE-0001]"));
        assert!(answer.contains("CITE\u{2011}9999") || answer.contains("[CITE\u{2011}9999]"));
    }

    #[test]
    fn delta_orders_by_version_number_not_canonical_claim_order() {
        let v1 = Uuid::parse_str("77777777-7777-7777-7777-777777777701").unwrap();
        let v2 = Uuid::parse_str("77777777-7777-7777-7777-777777777702").unwrap();
        // Intentionally put higher value on older version id ordering vs claim order.
        let c1 = Uuid::parse_str("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaa1").unwrap();
        let c2 = Uuid::parse_str("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaa2").unwrap();
        let claims = vec![
            TypedVersionClaim {
                version_id: v2,
                version_number: 2,
                claim_key: "budget".into(),
                scope: "org".into(),
                unit: Some("VND".into()),
                value: Decimal::from(15),
                chunk_id: Some(c2),
                cite_id: Some("CITE-0002".into()),
            },
            TypedVersionClaim {
                version_id: v1,
                version_number: 1,
                claim_key: "budget".into(),
                scope: "org".into(),
                unit: Some("VND".into()),
                value: Decimal::from(10),
                chunk_id: Some(c1),
                cite_id: Some("CITE-0001".into()),
            },
        ];
        let delta = typed_numeric_delta_for_versions(v2, v1, &claims).unwrap();
        assert_eq!(delta.old_value, Decimal::from(10));
        assert_eq!(delta.new_value, Decimal::from(15));
        assert_eq!(delta.delta, Decimal::from(5));
        assert_eq!(
            (delta.older_version_number, delta.newer_version_number),
            (1, 2)
        );
        assert_eq!(delta.older_chunk_id, c1);
        assert_eq!(delta.newer_chunk_id, c2);
    }

    #[test]
    fn lifecycle_rejects_resolution_versions_on_accepted_exception() {
        let conflict = QaConflict {
            conflict_id: Uuid::new_v4(),
            status: ConflictStatus::AcceptedException,
            conflict_type: ConflictType::Numeric,
            severity: ConflictSeverity::Warning,
            claim_a_id: Uuid::new_v4(),
            claim_b_id: Uuid::new_v4(),
            claim_a_key: "budget".into(),
            claim_b_key: "budget".into(),
            claim_a_scope: "org".into(),
            claim_b_scope: "org".into(),
            claim_a_unit: Some("VND".into()),
            claim_b_unit: Some("VND".into()),
            claim_a_version_id: Uuid::new_v4(),
            claim_b_version_id: Uuid::new_v4(),
            claim_a_document_id: Uuid::new_v4(),
            claim_b_document_id: Uuid::new_v4(),
            claim_a_is_current: true,
            claim_b_is_current: true,
            claim_a_quote: None,
            claim_b_quote: None,
            claim_a_cite_id: "CITE-0001".into(),
            claim_b_cite_id: "CITE-0002".into(),
            claim_a_numeric: Some(Decimal::from(10)),
            claim_b_numeric: Some(Decimal::from(15)),
            resolution_note: Some("ok".into()),
            resolution_version_a_id: Some(Uuid::new_v4()),
            resolution_version_b_id: Some(Uuid::new_v4()),
            resolution_a_cite_id: None,
            resolution_b_cite_id: None,
        };
        assert_eq!(
            validate_conflict_lifecycle(&conflict),
            Err(GroundingError::ConflictCitation)
        );
    }

    #[test]
    fn server_renders_claims_not_provider_paragraphs() {
        let claims = vec![StructuredClaim {
            text: "Kinh phí phê duyệt là 15 triệu đồng.".into(),
            cite_ids: vec!["CITE-0001".into()],
            kind: Some("numeric".into()),
            value: Some("15".into()),
            unit: Some("triệu".into()),
        }];
        let rendered = render_answer_from_claims(&claims);
        assert!(rendered.contains("[CITE-0001]"));
        assert!(rendered.contains("15 triệu"));
    }

    #[test]
    fn numeric_kind_requires_normalized_value_and_unit() {
        let missing = StructuredClaim {
            text: "Kinh phí 15".into(),
            cite_ids: vec!["CITE-0001".into()],
            kind: Some("numeric".into()),
            value: None,
            unit: None,
        };
        assert_eq!(
            missing.validate_numeric_fields(),
            Err(GroundingError::StructuredClaimsRequired)
        );
        let unit_only = StructuredClaim {
            text: "Kinh phí".into(),
            cite_ids: vec!["CITE-0001".into()],
            kind: Some("numeric".into()),
            value: None,
            unit: Some("VND".into()),
        };
        assert_eq!(
            unit_only.validate_numeric_fields(),
            Err(GroundingError::StructuredClaimsRequired)
        );
        let ok = StructuredClaim {
            text: "Kinh phí 15 VND".into(),
            cite_ids: vec!["CITE-0001".into()],
            kind: Some("numeric".into()),
            value: Some("15".into()),
            unit: Some("VND".into()),
        };
        assert!(ok.validate_numeric_fields().is_ok());
    }
}
