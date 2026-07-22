//! Version-aware citation pins and authorized resolve (P1B-R02 / ADR 0002).
//!
//! Spans:
//! - `source_span_*` — UTF-8 byte offsets into the trusted canonical Markdown
//! - `quote_local_*` — UTF-8 byte offsets into the immutable chunk body
//!
//! Resolve fetches trusted Markdown, verifies `canonical_markdown_sha256` and
//! `source_content_sha256`, then checks both spans + recomputed chunk identity.

use chrono::{DateTime, Utc};
use deadpool_postgres::Pool;
use fileconv_core::chunk::{locate_chunk_text, normalize_newlines};
use fileconv_knowledge::identity::chunk_identity;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::auth::permissions::require_permission;
use crate::db::error::DbError;
use crate::db::models::PublicationState;
use crate::db::pool::with_org_txn;
use crate::db::{document_versions, documents};
use crate::services::deletion::document_reads_suppressed;
use crate::services::retrieval::{RetrievalHit, PERMISSION_QA_HISTORY, PERMISSION_QA_QUERY};
use crate::storage::keys::parse_key_for_org;
use crate::storage::minio::MinioClient;
use crate::storage::StorageError;

/// Stable citation label used in grounded answers (`CITE-0001`, …).
pub fn cite_label(index: usize) -> String {
    format!("CITE-{:04}", index.saturating_add(1))
}

/// Immutable citation pin (ADR 0002).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CitationPin {
    pub cite_id: String,
    pub org_id: Uuid,
    pub logical_document_id: Uuid,
    pub version_id: Uuid,
    pub version_number: i32,
    /// Original uploaded/source object SHA-256 (`document_versions.content_sha256`).
    pub source_content_sha256: String,
    /// Canonical Markdown artifact SHA-256 (`derived_artifacts` markdown row).
    pub canonical_markdown_sha256: String,
    /// SHA-256 of the exact UTF-8 quote bytes.
    pub quote_sha256: String,
    pub chunk_id: Uuid,
    pub chunk_identity_sha256: String,
    pub collection_id: Uuid,
    pub heading: String,
    pub quote: String,
    pub page: Option<u32>,
    pub slide: Option<u32>,
    pub sheet: Option<String>,
    /// Absolute UTF-8 offsets into canonical Markdown.
    pub source_span_start: usize,
    pub source_span_end: usize,
    /// UTF-8 offsets into the chunk body (quote-local).
    pub quote_local_start: usize,
    pub quote_local_end: usize,
    pub effective_at: DateTime<Utc>,
    pub effective_to: Option<DateTime<Utc>>,
    pub is_current: bool,
    pub anchor: String,
}

/// Request to re-authorize and hydrate a stored citation pin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveCitationRequest {
    pub logical_document_id: Uuid,
    pub version_id: Uuid,
    pub source_content_sha256: String,
    pub canonical_markdown_sha256: String,
    pub chunk_id: Uuid,
    pub source_span_start: usize,
    pub source_span_end: usize,
    pub quote_local_start: usize,
    pub quote_local_end: usize,
    pub quote: String,
    pub require_current: bool,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CitationError {
    #[error("permission denied")]
    PermissionDenied,
    #[error("citation not found or unauthorized")]
    NotFound,
    #[error("document deleted or suspended")]
    Suppressed,
    #[error("citation quote/hash/span mismatch")]
    IntegrityMismatch,
    #[error("historical citation requires qa.history")]
    HistoryDenied,
    #[error("invalid citation request")]
    InvalidRequest,
    #[error("markdown artifact unavailable")]
    ArtifactUnavailable,
    #[error("database error")]
    Database,
    #[error("storage error")]
    Storage,
}

impl CitationError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::PermissionDenied => "citation_permission_denied",
            Self::NotFound => "citation_not_found",
            Self::Suppressed => "citation_suppressed",
            Self::IntegrityMismatch => "citation_integrity_mismatch",
            Self::HistoryDenied => "citation_history_denied",
            Self::InvalidRequest => "citation_invalid_request",
            Self::ArtifactUnavailable => "citation_artifact_unavailable",
            Self::Database => "citation_database",
            Self::Storage => "citation_storage",
        }
    }
}

/// Builds citation pins from authorized retrieval hits (1-based CITE labels).
pub fn pins_from_hits(org_id: Uuid, hits: &[RetrievalHit]) -> Vec<CitationPin> {
    hits.iter()
        .enumerate()
        .map(|(index, hit)| pin_from_hit(org_id, &cite_label(index), hit))
        .collect()
}

pub fn pin_from_hit(org_id: Uuid, cite_id: &str, hit: &RetrievalHit) -> CitationPin {
    // Default pin cites the full chunk body as quote-local; source span is the
    // absolute markdown span carried on the hydrated hit.
    let quote = hit.body.clone();
    let quote_local_end = quote.len();
    let quote_sha256 = hex::encode(Sha256::digest(quote.as_bytes()));
    let anchor = stable_anchor(&AnchorInput {
        org_id,
        document_id: hit.document_id,
        version_id: hit.version_id,
        version_number: hit.version_number,
        source_content_sha256: &hit.content_sha256,
        canonical_markdown_sha256: &hit.canonical_markdown_sha256,
        chunk_id: hit.chunk_id,
        source_span_start: hit.span_start,
        source_span_end: hit.span_end,
    });
    CitationPin {
        cite_id: cite_id.to_string(),
        org_id,
        logical_document_id: hit.document_id,
        version_id: hit.version_id,
        version_number: hit.version_number,
        source_content_sha256: hit.content_sha256.clone(),
        canonical_markdown_sha256: hit.canonical_markdown_sha256.clone(),
        quote_sha256,
        chunk_id: hit.chunk_id,
        chunk_identity_sha256: hit.chunk_identity_sha256.clone(),
        collection_id: hit.collection_id,
        heading: hit.heading.clone(),
        quote,
        page: hit.page,
        slide: hit.slide,
        sheet: hit.sheet.clone(),
        source_span_start: hit.span_start,
        source_span_end: hit.span_end,
        quote_local_start: 0,
        quote_local_end,
        effective_at: hit.effective_from,
        effective_to: hit.effective_to,
        is_current: hit.is_current,
        anchor,
    }
}

/// Inputs for deterministic citation anchors (no object keys).
#[derive(Debug, Clone, Copy)]
pub struct AnchorInput<'a> {
    pub org_id: Uuid,
    pub document_id: Uuid,
    pub version_id: Uuid,
    pub version_number: i32,
    pub source_content_sha256: &'a str,
    pub canonical_markdown_sha256: &'a str,
    pub chunk_id: Uuid,
    pub source_span_start: usize,
    pub source_span_end: usize,
}

/// Deterministic anchor pin for deep-links (no object keys).
pub fn stable_anchor(input: &AnchorInput<'_>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"markhand-citation-anchor-v2\0");
    hasher.update(input.org_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(input.document_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(input.version_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(input.version_number.to_le_bytes());
    hasher.update(b"\0");
    hasher.update(input.source_content_sha256.as_bytes());
    hasher.update(b"\0");
    hasher.update(input.canonical_markdown_sha256.as_bytes());
    hasher.update(b"\0");
    hasher.update(input.chunk_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(input.source_span_start.to_le_bytes());
    hasher.update(input.source_span_end.to_le_bytes());
    format!("mhcite1.{}", hex::encode(hasher.finalize()))
}

/// Exact UTF-8 byte-span extraction; requires char boundaries.
pub fn exact_span_quote(body: &str, span_start: usize, span_end: usize) -> Option<String> {
    if span_start >= span_end || span_end > body.len() {
        return None;
    }
    if !body.is_char_boundary(span_start) || !body.is_char_boundary(span_end) {
        return None;
    }
    Some(body[span_start..span_end].to_string())
}

pub fn validate_exact_utf8_span(
    body: &str,
    span_start: usize,
    span_end: usize,
    quote: &str,
) -> bool {
    match exact_span_quote(body, span_start, span_end) {
        Some(span) => span == quote,
        None => false,
    }
}

/// Inputs for dual-span integrity checks (source Markdown + quote-local body).
#[derive(Debug, Clone, Copy)]
pub struct DualSpanCheck<'a> {
    pub markdown: &'a str,
    pub chunk_body: &'a str,
    pub chunk_source_start: usize,
    pub source_span_start: usize,
    pub source_span_end: usize,
    pub quote_local_start: usize,
    pub quote_local_end: usize,
    pub quote: &'a str,
}

/// Map a normalized (LF) byte span into offsets inside a source window that may
/// contain CRLF. `\r\n` and `\n` each advance the normalized cursor by one `\n`.
pub fn map_normalized_span_to_source(
    source_window: &str,
    norm_start: usize,
    norm_end: usize,
) -> Option<(usize, usize)> {
    if norm_start > norm_end {
        return None;
    }
    let haystack = source_window.as_bytes();
    let mut hi = 0usize;
    let mut ni = 0usize;
    let mut start = None;
    let mut end = None;
    loop {
        if ni == norm_start && start.is_none() {
            start = Some(hi);
        }
        if ni == norm_end {
            end = Some(hi);
            break;
        }
        if hi >= haystack.len() {
            break;
        }
        if hi + 1 < haystack.len() && haystack[hi] == b'\r' && haystack[hi + 1] == b'\n' {
            hi += 2;
            ni += 1;
        } else if haystack[hi] == b'\n' {
            hi += 1;
            ni += 1;
        } else {
            let ch_len = source_window[hi..].chars().next()?.len_utf8();
            hi += ch_len;
            ni += ch_len;
        }
    }
    Some((start?, end?))
}

/// Verifies quote-local span on the normalized chunk body, maps that span through
/// CRLF-aware offsets into the trusted source Markdown, and checks source_span_*.
pub fn verify_dual_spans(check: &DualSpanCheck<'_>) -> bool {
    if !validate_exact_utf8_span(
        check.chunk_body,
        check.quote_local_start,
        check.quote_local_end,
        check.quote,
    ) {
        return false;
    }
    // Full normalized chunk must locate exactly at the stored source start.
    let Some((win_start, win_end)) =
        locate_chunk_text(check.markdown, check.chunk_source_start, check.chunk_body)
    else {
        return false;
    };
    if win_start != check.chunk_source_start {
        return false;
    }
    let window = &check.markdown[win_start..win_end];
    let Some((rel_start, rel_end)) =
        map_normalized_span_to_source(window, check.quote_local_start, check.quote_local_end)
    else {
        return false;
    };
    let abs_start = win_start.saturating_add(rel_start);
    let abs_end = win_start.saturating_add(rel_end);
    if abs_start != check.source_span_start || abs_end != check.source_span_end {
        return false;
    }
    let Some(source_quote) = exact_span_quote(check.markdown, abs_start, abs_end) else {
        return false;
    };
    // Source may still contain CRLF; quote is LF-canonical chunk text.
    normalize_newlines(&source_quote).as_ref() == check.quote || source_quote == check.quote
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn hex64(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Fresh authorization + integrity check for a citation pin.
pub async fn resolve_citation(
    pool: &Pool,
    ctx: &OrgContext,
    store: &MinioClient,
    request: ResolveCitationRequest,
) -> Result<CitationPin, CitationError> {
    require_permission(ctx, PERMISSION_QA_QUERY).map_err(|_| CitationError::PermissionDenied)?;
    if !hex64(&request.source_content_sha256) || !hex64(&request.canonical_markdown_sha256) {
        return Err(CitationError::InvalidRequest);
    }
    if request.source_span_start >= request.source_span_end
        || request.quote_local_start >= request.quote_local_end
        || request.quote.is_empty()
    {
        return Err(CitationError::InvalidRequest);
    }

    let meta = with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        let owned = request.clone();
        move |txn| {
            Box::pin(async move {
                let document =
                    match documents::get_by_id(txn, &ctx, owned.logical_document_id).await {
                        Ok(document) => document,
                        Err(DbError::NotFound) => return Ok(Err(CitationError::NotFound)),
                        Err(_) => return Ok(Err(CitationError::Database)),
                    };
                if document_reads_suppressed(document.state, document.deleted_at.is_some()) {
                    return Ok(Err(CitationError::Suppressed));
                }
                if !ctx.allows_collection(document.collection_id) {
                    return Ok(Err(CitationError::NotFound));
                }
                let version = match document_versions::find_by_id(
                    txn,
                    &ctx,
                    owned.logical_document_id,
                    owned.version_id,
                )
                .await
                {
                    Ok(Some(version)) => version,
                    Ok(None) => return Ok(Err(CitationError::NotFound)),
                    Err(_) => return Ok(Err(CitationError::Database)),
                };
                if version.publication_state != PublicationState::Published {
                    return Ok(Err(CitationError::NotFound));
                }
                if version.content_sha256 != owned.source_content_sha256 {
                    return Ok(Err(CitationError::IntegrityMismatch));
                }
                if owned.require_current && !version.is_current {
                    return Ok(Err(CitationError::IntegrityMismatch));
                }
                if !version.is_current && !ctx.has_permission(PERMISSION_QA_HISTORY) {
                    return Ok(Err(CitationError::HistoryDenied));
                }
                let artifact =
                    match document_versions::find_markdown_artifact(txn, &ctx, owned.version_id)
                        .await
                    {
                        Ok(Some(artifact)) => artifact,
                        Ok(None) => return Ok(Err(CitationError::ArtifactUnavailable)),
                        Err(_) => return Ok(Err(CitationError::Database)),
                    };
                if artifact.content_sha256 != owned.canonical_markdown_sha256 {
                    return Ok(Err(CitationError::IntegrityMismatch));
                }
                let allowed: Vec<Uuid> = ctx.allowed_collection_ids().iter().copied().collect();
                let chunk_row = match txn
                    .query_opt(
                        "SELECT c.id, c.chunk_identity_sha256, c.heading_path, c.body,
                                c.ordinal, c.body_text_version,
                                c.span_start, c.span_end, c.page, c.slide, c.sheet
                         FROM chunks c
                         JOIN documents d
                           ON d.org_id = c.org_id AND d.id = c.document_id
                         WHERE c.org_id = $1
                           AND c.id = $2
                           AND c.document_id = $3
                           AND c.version_id = $4
                           AND d.collection_id = ANY($5::uuid[])
                           AND d.deleted_at IS NULL
                           AND d.state NOT IN ('tombstoned', 'purged')",
                        &[
                            &ctx.org_id(),
                            &owned.chunk_id,
                            &owned.logical_document_id,
                            &owned.version_id,
                            &allowed,
                        ],
                    )
                    .await
                {
                    Ok(row) => row,
                    Err(_) => return Ok(Err(CitationError::Database)),
                };
                let Some(row) = chunk_row else {
                    return Ok(Err(CitationError::NotFound));
                };
                Ok(Ok((
                    document.collection_id,
                    version,
                    artifact.object_key,
                    row,
                )))
            })
        }
    })
    .await
    .map_err(|_| CitationError::Database)??;

    let (collection_id, version, markdown_key, row) = meta;
    let key = parse_key_for_org(&markdown_key, ctx.org_id())
        .map_err(|_| CitationError::ArtifactUnavailable)?;
    let bytes = store
        .get_object(ctx.org_id(), &key)
        .await
        .map_err(|error| match error {
            StorageError::NotFound => CitationError::ArtifactUnavailable,
            StorageError::KeyOrgMismatch | StorageError::MissingScope => {
                CitationError::PermissionDenied
            }
            _ => CitationError::Storage,
        })?;
    let digest = sha256_hex(&bytes);
    if digest != request.canonical_markdown_sha256 {
        return Err(CitationError::IntegrityMismatch);
    }
    let markdown =
        String::from_utf8(bytes.to_vec()).map_err(|_| CitationError::IntegrityMismatch)?;

    let body: String = row.get("body");
    let ordinal: i32 = row.get("ordinal");
    let heading_path: Vec<String> = row.get("heading_path");
    let body_text_version: String = row.get("body_text_version");
    let stored_identity: String = row.get("chunk_identity_sha256");
    let heading = heading_path.join(" > ");
    let recomputed = chunk_identity(
        &request.logical_document_id.to_string(),
        &request.version_id.to_string(),
        u64::try_from(ordinal).unwrap_or(0),
        &heading,
        &body,
        &body_text_version,
    );
    if recomputed != stored_identity {
        return Err(CitationError::IntegrityMismatch);
    }
    let chunk_source_start =
        usize::try_from(row.get::<_, Option<i32>>("span_start").unwrap_or(0)).unwrap_or(0);
    if !verify_dual_spans(&DualSpanCheck {
        markdown: &markdown,
        chunk_body: &body,
        chunk_source_start,
        source_span_start: request.source_span_start,
        source_span_end: request.source_span_end,
        quote_local_start: request.quote_local_start,
        quote_local_end: request.quote_local_end,
        quote: &request.quote,
    }) {
        return Err(CitationError::IntegrityMismatch);
    }
    let page: Option<i32> = row.get("page");
    let slide: Option<i32> = row.get("slide");
    let sheet: Option<String> = row.get("sheet");
    let quote_sha256 = sha256_hex(request.quote.as_bytes());
    Ok(CitationPin {
        cite_id: "CITE-RESOLVED".into(),
        org_id: ctx.org_id(),
        logical_document_id: request.logical_document_id,
        version_id: request.version_id,
        version_number: version.version_number,
        source_content_sha256: version.content_sha256.clone(),
        canonical_markdown_sha256: request.canonical_markdown_sha256.clone(),
        quote_sha256,
        chunk_id: request.chunk_id,
        chunk_identity_sha256: stored_identity,
        collection_id,
        heading,
        quote: request.quote,
        page: page.and_then(|value| u32::try_from(value).ok()),
        slide: slide.and_then(|value| u32::try_from(value).ok()),
        sheet,
        source_span_start: request.source_span_start,
        source_span_end: request.source_span_end,
        quote_local_start: request.quote_local_start,
        quote_local_end: request.quote_local_end,
        effective_at: version.effective_from,
        effective_to: version.effective_to,
        is_current: version.is_current,
        anchor: stable_anchor(&AnchorInput {
            org_id: ctx.org_id(),
            document_id: request.logical_document_id,
            version_id: request.version_id,
            version_number: version.version_number,
            source_content_sha256: &version.content_sha256,
            canonical_markdown_sha256: &request.canonical_markdown_sha256,
            chunk_id: request.chunk_id,
            source_span_start: request.source_span_start,
            source_span_end: request.source_span_end,
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use fileconv_knowledge::identity::BODY_TEXT_VERSION;

    fn sample_hit(is_current: bool) -> RetrievalHit {
        RetrievalHit {
            chunk_id: Uuid::from_u128(1),
            chunk_identity_sha256: "a".repeat(64),
            collection_id: Uuid::from_u128(2),
            document_id: Uuid::from_u128(3),
            version_id: Uuid::from_u128(4),
            version_number: 2,
            content_sha256: "b".repeat(64),
            canonical_markdown_sha256: "c".repeat(64),
            heading: "Ngân sách".into(),
            snippet: "Kinh phí hiện tại là 15 triệu đồng.".into(),
            body: "Kinh phí hiện tại là 15 triệu đồng.".into(),
            lexical_score: 1.0,
            vector_score: 0.8,
            rerank_score: 1.5,
            is_current,
            effective_from: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            effective_to: None,
            page: Some(3),
            slide: None,
            sheet: None,
            span_start: 0,
            span_end: 34,
        }
    }

    #[test]
    fn pins_include_dual_hashes_and_stable_anchor() {
        let org = Uuid::from_u128(9);
        let pins = pins_from_hits(org, &[sample_hit(true)]);
        assert_eq!(pins[0].cite_id, "CITE-0001");
        assert_eq!(pins[0].source_content_sha256.len(), 64);
        assert_eq!(pins[0].canonical_markdown_sha256.len(), 64);
        assert!(pins[0].anchor.starts_with("mhcite1."));
        assert_eq!(pins[0].quote_local_start, 0);
        assert_eq!(pins[0].quote_local_end, pins[0].quote.len());
    }

    #[test]
    fn dual_spans_handle_second_chunk_crlf_and_utf8() {
        let markdown = "# H1\r\n\r\nChunk one body.\r\n\r\n## H2\r\n\r\nTiếng Việt chunk two.";
        let chunk1 = "Chunk one body.";
        let chunk2 = "Tiếng Việt chunk two.";
        let start1 = markdown.find(chunk1).unwrap();
        let start2 = markdown.find(chunk2).unwrap();
        assert!(verify_dual_spans(&DualSpanCheck {
            markdown,
            chunk_body: chunk1,
            chunk_source_start: start1,
            source_span_start: start1,
            source_span_end: start1 + chunk1.len(),
            quote_local_start: 0,
            quote_local_end: chunk1.len(),
            quote: chunk1,
        }));
        assert!(verify_dual_spans(&DualSpanCheck {
            markdown,
            chunk_body: chunk2,
            chunk_source_start: start2,
            source_span_start: start2,
            source_span_end: start2 + chunk2.len(),
            quote_local_start: 0,
            quote_local_end: chunk2.len(),
            quote: chunk2,
        }));
        // Quote-local subspan inside chunk 2.
        let local_start = chunk2.find("Việt").unwrap();
        let quote = "Việt";
        assert!(verify_dual_spans(&DualSpanCheck {
            markdown,
            chunk_body: chunk2,
            chunk_source_start: start2,
            source_span_start: start2 + local_start,
            source_span_end: start2 + local_start + quote.len(),
            quote_local_start: local_start,
            quote_local_end: local_start + quote.len(),
            quote,
        }));
        // Mid-codepoint source span fails.
        assert!(!validate_exact_utf8_span(chunk2, 1, 4, "iế"));
        // PDF/PPTX/XLSX-style anchors ride on pin page/slide/sheet fields.
        let mut hit = sample_hit(true);
        hit.page = Some(12);
        hit.slide = Some(3);
        hit.sheet = Some("Budget".into());
        let pin = pin_from_hit(Uuid::nil(), "CITE-0003", &hit);
        assert_eq!(
            (pin.page, pin.slide, pin.sheet.as_deref()),
            (Some(12), Some(3), Some("Budget"))
        );
    }

    #[test]
    fn dual_spans_map_multiline_crlf_utf8_after_crlf() {
        let markdown = "# Tiêu đề\r\n\r\nHệ thống phải giữ dấu.\r\nDòng hai: Tiếng Việt.\r\n";
        let body = "Hệ thống phải giữ dấu.\nDòng hai: Tiếng Việt.";
        let (win_start, win_end) =
            fileconv_core::chunk::locate_chunk_text(markdown, 0, body).unwrap();
        let window = &markdown[win_start..win_end];
        let local_start = body.find("Tiếng Việt").unwrap();
        let quote = "Tiếng Việt";
        let (rel_start, rel_end) =
            map_normalized_span_to_source(window, local_start, local_start + quote.len()).unwrap();
        assert!(verify_dual_spans(&DualSpanCheck {
            markdown,
            chunk_body: body,
            chunk_source_start: win_start,
            source_span_start: win_start + rel_start,
            source_span_end: win_start + rel_end,
            quote_local_start: local_start,
            quote_local_end: local_start + quote.len(),
            quote,
        }));
        // One-byte shift after CRLF must reject (not a char-boundary-safe quote).
        assert!(!verify_dual_spans(&DualSpanCheck {
            markdown,
            chunk_body: body,
            chunk_source_start: win_start,
            source_span_start: win_start + rel_start + 1,
            source_span_end: win_start + rel_end + 1,
            quote_local_start: local_start,
            quote_local_end: local_start + quote.len(),
            quote,
        }));
        // Mid-codepoint quote-local reject.
        assert!(!validate_exact_utf8_span(
            body,
            local_start + 1,
            local_start + 4,
            "iế"
        ));
        let _ = win_end;
    }

    #[test]
    fn chunk_identity_immutability_for_second_chunk() {
        let document_id = "11111111-1111-1111-1111-111111111111";
        let version_id = "22222222-2222-2222-2222-222222222222";
        let heading = "H1 > H2";
        let body = "Tiếng Việt chunk two.";
        let first = chunk_identity(document_id, version_id, 1, heading, body, BODY_TEXT_VERSION);
        let again = chunk_identity(document_id, version_id, 1, heading, body, BODY_TEXT_VERSION);
        assert_eq!(first, again);
        assert_ne!(
            first,
            chunk_identity(document_id, version_id, 0, heading, body, BODY_TEXT_VERSION)
        );
    }

    #[test]
    fn page_slide_sheet_anchors_preserved_on_pin() {
        let mut hit = sample_hit(true);
        hit.page = Some(9);
        hit.slide = Some(2);
        hit.sheet = Some("Sheet1".into());
        let pin = pin_from_hit(Uuid::nil(), "CITE-0001", &hit);
        assert_eq!(pin.page, Some(9));
        assert_eq!(pin.slide, Some(2));
        assert_eq!(pin.sheet.as_deref(), Some("Sheet1"));
    }
}
