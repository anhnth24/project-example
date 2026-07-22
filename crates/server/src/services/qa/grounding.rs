//! Version-aware citation validation, conflict notes, and extractive fallback (P1B-R03).
//!
//! Providers return structured claims only. The server alone renders answer text and
//! exact `[CITE-NNNN]` markers. R03 never queries or mutates the database.

use std::collections::{BTreeSet, HashMap, HashSet};

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::db::models::ConflictStatus;
use crate::db::search::AuthorizedConflictEvidence;
use crate::services::claims::{extract_typed_claims, ClaimValue};
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
    /// Trusted quote taken from authorized retrieval text (snippet/body).
    pub authoritative_quote: String,
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

/// True when a retrieval hit has no usable grounding text.
///
/// Heading alone does **not** ground — snippet or body must be nonempty.
pub fn is_blank_hit(hit: &RetrievalHit) -> bool {
    hit.snippet.trim().is_empty() && hit.body.trim().is_empty()
}

impl GroundingPassage {
    pub fn from_hits(hits: &[RetrievalHit]) -> Vec<Self> {
        hits.iter()
            .filter(|hit| !is_blank_hit(hit))
            .enumerate()
            .map(|(index, hit)| {
                let quote = if !hit.snippet.trim().is_empty() {
                    hit.snippet.trim().to_string()
                } else {
                    hit.body.trim().to_string()
                };
                Self {
                    cite_id: cite_id(index),
                    hit: hit.clone(),
                    authoritative_quote: quote,
                }
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
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuredClaim {
    pub text: String,
    pub cite_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

impl std::fmt::Debug for StructuredClaim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StructuredClaim")
            .field("text", &"[REDACTED]")
            .field("cite_ids", &self.cite_ids)
            .field("kind", &self.kind)
            .field("value", &self.value.as_ref().map(|_| "[REDACTED]"))
            .field("unit", &self.unit.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
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
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderGroundedPayload {
    pub claims: Vec<StructuredClaim>,
    #[serde(default)]
    pub refusal: bool,
}

impl std::fmt::Debug for ProviderGroundedPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderGroundedPayload")
            .field("claims_count", &self.claims.len())
            .field("refusal", &self.refusal)
            .finish()
    }
}

/// Optional lifecycle overlay for already-authorized conflict evidence.
///
/// R03 never loads lifecycle from the database. Callers/tests may attach status when
/// the retrieval path already knows it. Status-specific history notes require this
/// overlay; without it R03 emits only a generic authorized-conflict warning or omits.
#[derive(Clone, PartialEq, Eq)]
pub struct ConflictLifecycle {
    pub conflict_id: Uuid,
    pub status: ConflictStatus,
    pub resolution_note: Option<String>,
    pub resolution_version_a_id: Option<Uuid>,
    pub resolution_version_b_id: Option<Uuid>,
}

impl std::fmt::Debug for ConflictLifecycle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConflictLifecycle")
            .field("conflict_id", &self.conflict_id)
            .field("status", &self.status)
            .field(
                "resolution_note",
                &self.resolution_note.as_ref().map(|_| "[REDACTED]"),
            )
            .field("resolution_version_a_id", &self.resolution_version_a_id)
            .field("resolution_version_b_id", &self.resolution_version_b_id)
            .finish()
    }
}

/// Conflict view built only from authorized retrieval evidence (+ optional lifecycle).
#[derive(Clone, PartialEq, Eq)]
pub struct QaConflict {
    pub conflict_id: Uuid,
    /// Present only when the caller supplied lifecycle evidence for this conflict.
    pub lifecycle_status: Option<ConflictStatus>,
    pub claim_a_version_id: Uuid,
    pub claim_b_version_id: Uuid,
    pub claim_a_is_current: bool,
    pub claim_b_is_current: bool,
    pub claim_a_quote: Option<String>,
    pub claim_b_quote: Option<String>,
    pub claim_a_cite_id: String,
    pub claim_b_cite_id: String,
    pub resolution_note: Option<String>,
    pub resolution_a_cite_id: Option<String>,
    pub resolution_b_cite_id: Option<String>,
}

impl std::fmt::Debug for QaConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QaConflict")
            .field("conflict_id", &self.conflict_id)
            .field("lifecycle_status", &self.lifecycle_status)
            .field("claim_a_version_id", &self.claim_a_version_id)
            .field("claim_b_version_id", &self.claim_b_version_id)
            .field("claim_a_is_current", &self.claim_a_is_current)
            .field("claim_b_is_current", &self.claim_b_is_current)
            .field("claim_a_quote", &"[REDACTED]")
            .field("claim_b_quote", &"[REDACTED]")
            .field("claim_a_cite_id", &self.claim_a_cite_id)
            .field("claim_b_cite_id", &self.claim_b_cite_id)
            .field(
                "resolution_note",
                &self.resolution_note.as_ref().map(|_| "[REDACTED]"),
            )
            .field("resolution_a_cite_id", &self.resolution_a_cite_id)
            .field("resolution_b_cite_id", &self.resolution_b_cite_id)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ConflictWarning {
    pub conflict_id: Uuid,
    pub status: ConflictStatus,
    pub message: String,
    pub pin_cite_ids: Vec<String>,
}

impl std::fmt::Debug for ConflictWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConflictWarning")
            .field("conflict_id", &self.conflict_id)
            .field("status", &self.status)
            .field("message", &"[REDACTED]")
            .field("pin_cite_ids", &self.pin_cite_ids)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct VersionContext {
    pub mode: &'static str,
    pub current_version_ids: Vec<Uuid>,
    pub cited_version_ids: Vec<Uuid>,
    pub change_note: Option<String>,
}

impl std::fmt::Debug for VersionContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VersionContext")
            .field("mode", &self.mode)
            .field("current_version_ids", &self.current_version_ids)
            .field("cited_version_ids", &self.cited_version_ids)
            .field(
                "change_note",
                &self.change_note.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
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
    #[error("history mode requires at least two lineage versions")]
    HistoryVersionsRequired,
    #[error("conflict citation is invalid for the active mode")]
    ConflictCitation,
    #[error("answer failed grounding validation")]
    InvalidGrounding,
    #[error("structured provider claims missing or invalid")]
    StructuredClaimsRequired,
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
            Self::HistoryVersionsRequired => "qa_history_versions_required",
            Self::ConflictCitation => "qa_conflict_citation",
            Self::InvalidGrounding => "qa_invalid_grounding",
            Self::StructuredClaimsRequired => "qa_structured_claims_required",
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
        || lower.starts_with("câu hỏi đã được tiếp nhận")
        || lower == "## trả lời trích xuất"
        || trimmed.starts_with("Thay đổi:")
        || trimmed.starts_with("Lịch sử:")
        || trimmed.starts_with("Cảnh báo xung đột")
        || trimmed.starts_with("Ghi chú lịch sử conflict")
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

/// Filter passages for the active version mode before prompting/grounding.
pub fn passages_for_mode(
    passages: &[GroundingPassage],
    mode: &VersionMode,
) -> Result<Vec<GroundingPassage>, GroundingError> {
    let filtered: Vec<GroundingPassage> = match mode {
        VersionMode::Current => passages
            .iter()
            .filter(|p| p.hit.is_current)
            .cloned()
            .collect(),
        VersionMode::Compare {
            document_id,
            version_a,
            version_b,
        } => passages
            .iter()
            .filter(|p| {
                p.hit.document_id == *document_id
                    && (p.hit.version_id == *version_a || p.hit.version_id == *version_b)
            })
            .cloned()
            .collect(),
        VersionMode::History { document_id } => passages
            .iter()
            .filter(|p| p.hit.document_id == *document_id)
            .cloned()
            .collect(),
        VersionMode::AsOf { .. } => passages.to_vec(),
    };

    match mode {
        VersionMode::Compare {
            document_id,
            version_a,
            version_b,
        } => {
            if filtered.iter().any(|p| p.hit.document_id != *document_id) {
                return Err(GroundingError::MixedVersionCitation);
            }
            let versions: BTreeSet<Uuid> = filtered.iter().map(|p| p.hit.version_id).collect();
            if !versions.contains(version_a) || !versions.contains(version_b) {
                return Err(GroundingError::MixedVersionCitation);
            }
        }
        VersionMode::History { .. } => {
            let versions: BTreeSet<Uuid> = filtered.iter().map(|p| p.hit.version_id).collect();
            if versions.len() < 2 {
                return Err(GroundingError::HistoryVersionsRequired);
            }
        }
        _ => {}
    }
    Ok(filtered)
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
            if !claim_supported_by_quote(&claim.text, &passage.authoritative_quote) {
                return Err(GroundingError::UnsupportedClaim);
            }
            if let (Some(value), Some(unit)) = (claim.value.as_deref(), claim.unit.as_deref()) {
                let quote_l = passage.authoritative_quote.to_lowercase();
                if !quote_l.contains(&value.to_lowercase())
                    || !quote_l.contains(&unit.to_lowercase())
                {
                    return Err(GroundingError::UnsupportedClaim);
                }
            }
            cited.insert(cite.clone());
        }
    }
    enforce_mode_citation_rules(mode, &cited, &by_id)?;
    Ok(cited.into_iter().collect())
}

fn enforce_mode_citation_rules(
    mode: &VersionMode,
    cited: &BTreeSet<String>,
    by_id: &HashMap<String, &GroundingPassage>,
) -> Result<(), GroundingError> {
    match mode {
        VersionMode::Current => {
            for cite in cited {
                let passage = by_id.get(cite).ok_or(GroundingError::FabricatedCitation)?;
                if !passage.hit.is_current {
                    return Err(GroundingError::SupersededCitation);
                }
            }
        }
        VersionMode::Compare {
            document_id,
            version_a,
            version_b,
        } => {
            let mut saw_a = false;
            let mut saw_b = false;
            for cite in cited {
                let passage = by_id.get(cite).ok_or(GroundingError::FabricatedCitation)?;
                if passage.hit.document_id != *document_id {
                    return Err(GroundingError::MixedVersionCitation);
                }
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
        VersionMode::History { document_id } => {
            let mut versions = BTreeSet::new();
            for cite in cited {
                let passage = by_id.get(cite).ok_or(GroundingError::FabricatedCitation)?;
                if passage.hit.document_id != *document_id {
                    return Err(GroundingError::MixedVersionCitation);
                }
                versions.insert(passage.hit.version_id);
            }
            if versions.len() < 2 {
                return Err(GroundingError::HistoryVersionsRequired);
            }
        }
        VersionMode::AsOf { at } => {
            for cite in cited {
                let passage = by_id.get(cite).ok_or(GroundingError::FabricatedCitation)?;
                if !version_effective_at(&passage.hit, *at) {
                    return Err(GroundingError::SupersededCitation);
                }
            }
        }
    }
    Ok(())
}

fn version_effective_at(hit: &RetrievalHit, at: DateTime<Utc>) -> bool {
    hit.effective_from <= at && hit.effective_to.map(|to| at < to).unwrap_or(true)
}

/// Validate a fully server-rendered answer: exact cites + mode rules.
pub fn validate_rendered_answer(
    answer: &str,
    passages: &[GroundingPassage],
    mode: &VersionMode,
    structured: &[StructuredClaim],
    require_structured: bool,
) -> Result<Vec<String>, GroundingError> {
    if require_structured {
        validate_structured_claims(structured, passages, mode, true)?;
    }
    let allowlist = GroundingPassage::allowlist(passages);
    let by_id = GroundingPassage::by_cite_id(passages);
    let cited = extract_exact_cite_ids(answer)?;

    for block in answer.split("\n\n") {
        let trimmed = block.trim();
        if trimmed.is_empty() || is_refusal_sentence(trimmed) {
            continue;
        }
        if extract_exact_cite_ids(trimmed)?.is_empty() {
            return Err(GroundingError::MissingCitation);
        }
    }

    if cited.is_empty() {
        return Err(GroundingError::InvalidGrounding);
    }
    for cite in &cited {
        if !allowlist.contains(cite) {
            return Err(GroundingError::FabricatedCitation);
        }
    }
    enforce_mode_citation_rules(mode, &cited, &by_id)?;
    Ok(cited.into_iter().collect())
}

/// Deterministic extractive answer from authorized passages (provider outage path).
/// Source text is citation-neutralized before the server renders the fallback marker.
pub fn extractive_answer(passages: &[GroundingPassage]) -> String {
    if passages.is_empty() {
        return "Không tìm thấy bằng chứng phù hợp trong kho tri thức.".into();
    }
    let mut answer = String::from("## Trả lời trích xuất\n\n");
    for (index, passage) in passages.iter().enumerate() {
        let neutralized = neutralize_citation_syntax(passage.authoritative_quote.trim());
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

/// Structured claims mirroring an extractive fallback answer.
pub fn extractive_structured_claims(passages: &[GroundingPassage]) -> Vec<StructuredClaim> {
    passages
        .iter()
        .map(|passage| StructuredClaim {
            text: neutralize_citation_syntax(passage.authoritative_quote.trim()),
            cite_ids: vec![passage.cite_id.clone()],
            kind: None,
            value: None,
            unit: None,
        })
        .collect()
}

/// Map authorized conflict evidence onto passage cite IDs (no DB I/O).
///
/// Each side must match an allowlisted hit on exact version + document +
/// collection and a supporting quote. Incomplete or wrong mappings are rejected.
pub fn conflicts_from_evidence(
    evidence: &[AuthorizedConflictEvidence],
    passages: &[GroundingPassage],
    lifecycle: &[ConflictLifecycle],
    _mode: &VersionMode,
) -> Vec<QaConflict> {
    let lifecycle_by_id: HashMap<Uuid, &ConflictLifecycle> =
        lifecycle.iter().map(|l| (l.conflict_id, l)).collect();
    let mut out = Vec::new();
    for row in evidence {
        let Some(a_cite) = cite_for_conflict_side(
            passages,
            row.claim_a_version_id,
            row.claim_a_document_id,
            row.claim_a_collection_id,
            row.claim_a_quote.as_deref(),
        ) else {
            continue;
        };
        let Some(b_cite) = cite_for_conflict_side(
            passages,
            row.claim_b_version_id,
            row.claim_b_document_id,
            row.claim_b_collection_id,
            row.claim_b_quote.as_deref(),
        ) else {
            continue;
        };
        let life = lifecycle_by_id.get(&row.conflict_id).copied();
        let lifecycle_status = life.map(|l| l.status);
        let (resolution_a_cite_id, resolution_b_cite_id) =
            if lifecycle_status == Some(ConflictStatus::Resolved) {
                let ra = life
                    .and_then(|l| l.resolution_version_a_id)
                    .and_then(|vid| cite_for_version_only(passages, vid));
                let rb = life
                    .and_then(|l| l.resolution_version_b_id)
                    .and_then(|vid| cite_for_version_only(passages, vid));
                (ra, rb)
            } else {
                (None, None)
            };
        out.push(QaConflict {
            conflict_id: row.conflict_id,
            lifecycle_status,
            claim_a_version_id: row.claim_a_version_id,
            claim_b_version_id: row.claim_b_version_id,
            claim_a_is_current: row.claim_a_is_current,
            claim_b_is_current: row.claim_b_is_current,
            claim_a_quote: row.claim_a_quote.clone(),
            claim_b_quote: row.claim_b_quote.clone(),
            claim_a_cite_id: a_cite,
            claim_b_cite_id: b_cite,
            resolution_note: life.and_then(|l| l.resolution_note.clone()),
            resolution_a_cite_id,
            resolution_b_cite_id,
        });
    }
    out
}

fn hit_supports_quote(passage: &GroundingPassage, quote: &str) -> bool {
    let quote = quote.trim();
    if quote.is_empty() {
        return false;
    }
    claim_supported_by_quote(quote, &passage.authoritative_quote)
        || claim_supported_by_quote(quote, &passage.hit.snippet)
        || claim_supported_by_quote(quote, &passage.hit.body)
}

/// Exact side match: version + document + collection + supporting quote.
fn cite_for_conflict_side(
    passages: &[GroundingPassage],
    version_id: Uuid,
    document_id: Uuid,
    collection_id: Uuid,
    quote: Option<&str>,
) -> Option<String> {
    let quote = quote?;
    passages
        .iter()
        .find(|p| {
            p.hit.version_id == version_id
                && p.hit.document_id == document_id
                && p.hit.collection_id == collection_id
                && hit_supports_quote(p, quote)
        })
        .map(|p| p.cite_id.clone())
}

fn cite_for_version_only(passages: &[GroundingPassage], version_id: Uuid) -> Option<String> {
    passages
        .iter()
        .find(|p| p.hit.version_id == version_id)
        .map(|p| p.cite_id.clone())
}

/// Current unresolved warnings + history notes from authorized conflict evidence.
///
/// Status-specific Resolved / accepted_exception / false_positive notes are emitted
/// only when lifecycle evidence is present. Without lifecycle, Current/AsOf may warn
/// as open; History emits a generic authorized-conflict warning (or omits).
pub fn conflict_messages(mode: &VersionMode, conflicts: &[QaConflict]) -> Vec<ConflictWarning> {
    let mut out = Vec::new();
    for conflict in conflicts {
        if conflict.claim_a_cite_id.is_empty() || conflict.claim_b_cite_id.is_empty() {
            continue;
        }
        match (mode, conflict.lifecycle_status) {
            (VersionMode::Current, None)
            | (VersionMode::Current, Some(ConflictStatus::Open))
            | (VersionMode::AsOf { .. }, None)
            | (VersionMode::AsOf { .. }, Some(ConflictStatus::Open)) => {
                if !(conflict.claim_a_is_current && conflict.claim_b_is_current) {
                    continue;
                }
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
            (VersionMode::History { .. }, Some(ConflictStatus::Resolved)) => {
                let message = match (
                    conflict.resolution_a_cite_id.as_deref(),
                    conflict.resolution_b_cite_id.as_deref(),
                ) {
                    (Some(ra), Some(rb)) => resolved_history_note(conflict, ra, rb),
                    _ => format!(
                        "Ghi chú lịch sử conflict (resolved): trước đây [{}] vs [{}].",
                        conflict.claim_a_cite_id, conflict.claim_b_cite_id
                    ),
                };
                let mut pins = vec![
                    conflict.claim_a_cite_id.clone(),
                    conflict.claim_b_cite_id.clone(),
                ];
                if let Some(ra) = &conflict.resolution_a_cite_id {
                    pins.push(ra.clone());
                }
                if let Some(rb) = &conflict.resolution_b_cite_id {
                    pins.push(rb.clone());
                }
                out.push(ConflictWarning {
                    conflict_id: conflict.conflict_id,
                    status: ConflictStatus::Resolved,
                    message,
                    pin_cite_ids: pins,
                });
            }
            (VersionMode::History { .. }, Some(ConflictStatus::AcceptedException)) => {
                out.push(ConflictWarning {
                    conflict_id: conflict.conflict_id,
                    status: ConflictStatus::AcceptedException,
                    message: format!(
                        "Ghi chú lịch sử conflict (accepted_exception): [{}] vs [{}]; {}.",
                        conflict.claim_a_cite_id,
                        conflict.claim_b_cite_id,
                        neutralize_citation_syntax(
                            conflict
                                .resolution_note
                                .as_deref()
                                .unwrap_or("được chấp nhận như ngoại lệ")
                        )
                    ),
                    pin_cite_ids: vec![
                        conflict.claim_a_cite_id.clone(),
                        conflict.claim_b_cite_id.clone(),
                    ],
                });
            }
            (VersionMode::History { .. }, Some(ConflictStatus::FalsePositive)) => {
                out.push(ConflictWarning {
                    conflict_id: conflict.conflict_id,
                    status: ConflictStatus::FalsePositive,
                    message: format!(
                        "Ghi chú lịch sử conflict (false_positive): [{}] vs [{}]; {}.",
                        conflict.claim_a_cite_id,
                        conflict.claim_b_cite_id,
                        neutralize_citation_syntax(
                            conflict
                                .resolution_note
                                .as_deref()
                                .unwrap_or("đánh dấu false_positive")
                        )
                    ),
                    pin_cite_ids: vec![
                        conflict.claim_a_cite_id.clone(),
                        conflict.claim_b_cite_id.clone(),
                    ],
                });
            }
            (VersionMode::History { .. }, None) => {
                // Omit: do not invent Open/Resolved/accepted_exception/false_positive.
            }
            _ => {}
        }
    }
    out.sort_by_key(|warning| warning.conflict_id);
    out
}

fn unresolved_current_warning(conflict: &QaConflict) -> String {
    let a = conflict
        .claim_a_quote
        .as_deref()
        .map(neutralize_citation_syntax)
        .unwrap_or_else(|| "claim A".into());
    let b = conflict
        .claim_b_quote
        .as_deref()
        .map(neutralize_citation_syntax)
        .unwrap_or_else(|| "claim B".into());
    format!(
        "Cảnh báo xung đột chưa giải quyết (open) giữa [{}] ({a}) và [{}] ({b}).",
        conflict.claim_a_cite_id, conflict.claim_b_cite_id
    )
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
         đã căn chỉnh tại [{res_a}] / [{res_b}]; {resolution}.",
        conflict.claim_a_cite_id, conflict.claim_b_cite_id
    )
}

/// Typed numeric pair used only for deterministic compare change notes.
#[derive(Clone, PartialEq, Eq)]
pub struct TypedVersionClaim {
    pub version_id: Uuid,
    pub version_number: i32,
    pub claim_key: String,
    pub scope: String,
    pub unit: Option<String>,
    pub value: Decimal,
    pub cite_id: String,
}

impl std::fmt::Debug for TypedVersionClaim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TypedVersionClaim")
            .field("version_id", &self.version_id)
            .field("version_number", &self.version_number)
            .field("claim_key", &"[REDACTED]")
            .field("scope", &"[REDACTED]")
            .field("unit", &self.unit.as_ref().map(|_| "[REDACTED]"))
            .field("value", &"[REDACTED]")
            .field("cite_id", &self.cite_id)
            .finish()
    }
}

/// Extract trusted typed claims from authorized passage bodies (no DB).
pub fn typed_claims_from_passages(passages: &[GroundingPassage]) -> Vec<TypedVersionClaim> {
    let mut out = Vec::new();
    for passage in passages {
        let extracted = extract_typed_claims(
            &passage.hit.body,
            passage.hit.version_id,
            &passage.hit.chunk_identity_sha256,
        );
        for claim in extracted {
            let value = match claim.value {
                ClaimValue::Number(v) | ClaimValue::Money(v) => v,
                _ => continue,
            };
            out.push(TypedVersionClaim {
                version_id: passage.hit.version_id,
                version_number: passage.hit.version_number,
                claim_key: claim.claim_key,
                scope: claim.scope,
                unit: claim.unit,
                value,
                cite_id: passage.cite_id.clone(),
            });
        }
    }
    out
}

/// Deterministic change note only when same-key/unit typed data exists; otherwise omit.
pub fn deterministic_change_note(
    mode: &VersionMode,
    passages: &[GroundingPassage],
) -> Option<String> {
    let VersionMode::Compare {
        version_a,
        version_b,
        ..
    } = mode
    else {
        return None;
    };
    let claims = typed_claims_from_passages(passages);
    let a_claims: Vec<_> = claims
        .iter()
        .filter(|c| c.version_id == *version_a)
        .collect();
    let b_claims: Vec<_> = claims
        .iter()
        .filter(|c| c.version_id == *version_b)
        .collect();
    for older_src in &a_claims {
        for newer_src in &b_claims {
            if older_src.claim_key == newer_src.claim_key
                && older_src.scope == newer_src.scope
                && older_src.unit == newer_src.unit
            {
                let (older, newer) = if older_src.version_number <= newer_src.version_number {
                    (*older_src, *newer_src)
                } else {
                    (*newer_src, *older_src)
                };
                let delta = newer.value - older.value;
                let direction = if delta > Decimal::ZERO {
                    "tăng"
                } else if delta < Decimal::ZERO {
                    "giảm"
                } else {
                    "không đổi"
                };
                let unit = older
                    .unit
                    .as_deref()
                    .filter(|u| !u.is_empty())
                    .map(|u| format!(" {u}"))
                    .unwrap_or_default();
                return Some(format!(
                    "Thay đổi: phiên bản {} là {}{} [{}], phiên bản {} là {}{} [{}], {} {}{}.",
                    older.version_number,
                    older.value.normalize(),
                    unit,
                    older.cite_id,
                    newer.version_number,
                    newer.value.normalize(),
                    unit,
                    newer.cite_id,
                    direction,
                    delta.abs().normalize(),
                    unit
                ));
            }
        }
    }
    // Also try swapped pairing when only one side matched above loops (same sets).
    for older_src in &b_claims {
        for newer_src in &a_claims {
            if older_src.claim_key == newer_src.claim_key
                && older_src.scope == newer_src.scope
                && older_src.unit == newer_src.unit
            {
                // Already covered by a×b when both non-empty with matching keys.
                let _ = (older_src, newer_src);
            }
        }
    }
    let _ = passages;
    None
}

/// Build version context metadata (audit-safe; no passage/answer text).
pub fn build_version_context(
    mode: &VersionMode,
    passages: &[GroundingPassage],
    cited_ids: &[String],
) -> VersionContext {
    let by_id = GroundingPassage::by_cite_id(passages);
    let mut cited_version_ids = BTreeSet::new();
    for cite in cited_ids {
        if let Some(passage) = by_id.get(cite) {
            cited_version_ids.insert(passage.hit.version_id);
        }
    }
    let current_version_ids: Vec<Uuid> = passages
        .iter()
        .filter(|p| p.hit.is_current)
        .map(|p| p.hit.version_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mode_label = match mode {
        VersionMode::Current => "current",
        VersionMode::AsOf { .. } => "as_of",
        VersionMode::Compare { .. } => "compare",
        VersionMode::History { .. } => "history",
    };
    VersionContext {
        mode: mode_label,
        current_version_ids,
        cited_version_ids: cited_version_ids.into_iter().collect(),
        change_note: deterministic_change_note(mode, passages),
    }
}

/// Append server notes (change / conflict) after the grounded body.
pub fn append_server_notes(answer: &str, notes: &[String]) -> String {
    let mut out = answer.to_string();
    for note in notes {
        if note.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(note);
    }
    out
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use uuid::Uuid;

    use super::*;
    use crate::services::retrieval::RetrievalHit;

    fn hit(version: Uuid, number: i32, current: bool, snippet: &str) -> RetrievalHit {
        RetrievalHit {
            chunk_id: Uuid::new_v4(),
            chunk_identity_sha256: "a".repeat(64),
            collection_id: Uuid::new_v4(),
            document_id: Uuid::parse_str("66666666-6666-6666-6666-666666666601").unwrap(),
            version_id: version,
            version_number: number,
            content_sha256: "b".repeat(64),
            heading: "Kinh phí".into(),
            snippet: snippet.into(),
            body: snippet.into(),
            lexical_score: 1.0,
            vector_score: 0.8,
            rerank_score: 1.8,
            is_current: current,
            effective_from: Utc::now(),
            effective_to: None,
            page: Some(1),
            slide: None,
            sheet: None,
            span_start: 0,
            span_end: snippet.len(),
        }
    }

    #[test]
    fn rejects_fabricated_and_malformed_citations() {
        let v = Uuid::new_v4();
        let passages = GroundingPassage::from_hits(&[hit(v, 1, true, "Kinh phí 15 triệu.")]);
        assert!(matches!(
            validate_structured_claims(
                &[StructuredClaim {
                    text: "Kinh phí 15 triệu.".into(),
                    cite_ids: vec!["CITE-9999".into()],
                    kind: None,
                    value: None,
                    unit: None,
                }],
                &passages,
                &VersionMode::Current,
                true
            ),
            Err(GroundingError::FabricatedCitation)
        ));
        assert!(matches!(
            extract_exact_cite_ids("text [CITE-12]"),
            Err(GroundingError::MalformedCitation)
        ));
    }

    #[test]
    fn current_rejects_superseded_citation() {
        let v1 = Uuid::new_v4();
        let v2 = Uuid::new_v4();
        let passages = GroundingPassage::from_hits(&[
            hit(v1, 1, false, "Kinh phí 10 triệu."),
            hit(v2, 2, true, "Kinh phí 15 triệu."),
        ]);
        let err = validate_structured_claims(
            &[StructuredClaim {
                text: "Kinh phí 10 triệu.".into(),
                cite_ids: vec!["CITE-0001".into()],
                kind: None,
                value: None,
                unit: None,
            }],
            &passages,
            &VersionMode::Current,
            true,
        )
        .unwrap_err();
        assert_eq!(err, GroundingError::SupersededCitation);
    }

    #[test]
    fn extractive_neutralizes_source_cite_syntax() {
        let v = Uuid::new_v4();
        let passages =
            GroundingPassage::from_hits(&[hit(v, 1, true, "Nội dung [CITE-9999] còn lại.")]);
        let answer = extractive_answer(&passages);
        assert!(!answer.contains("[CITE-9999]"));
        assert!(answer.contains("[CITE-0001]"));
        assert!(answer.contains("CITE\u{2011}9999") || answer.contains("[CITE\u{2011}9999]"));
    }
}
