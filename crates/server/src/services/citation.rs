//! Version-aware citation anchors and fresh-authorized resolve (P1B-R02 / ADR 0002).
//!
//! Quotes are sliced from authoritative trusted Markdown (not chunk.body). Span
//! offsets are byte offsets into that Markdown. Exact resolve does not require an
//! active index generation, but always rechecks ACL/membership/document state.

use chrono::{DateTime, Utc};
use deadpool_postgres::Pool;
use thiserror::Error;
use uuid::Uuid;

use crate::auth::permissions::{resolve_org_context_in_txn, ResolveError};
use crate::db::error::DbError;
use crate::db::pool::with_org_txn;
use crate::db::search::{self, HydratedChunkRow, TrustedMarkdownArtifact};
use crate::services::deletion::document_reads_suppressed;
use crate::services::retrieval::{PERMISSION_QA_HISTORY, PERMISSION_QA_QUERY};
use crate::storage::blob::{BlobStore, ObjectExpectation};
use crate::storage::keys::{authorize_key_for_version, parse_key_for_org, ObjectNamespace};
use crate::storage::StorageError;

/// Default bound for loading Markdown while verifying a citation quote.
pub const CITATION_MARKDOWN_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// Stable citation pin required by ADR 0002.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StableCitation {
    pub org_id: Uuid,
    pub logical_document_id: Uuid,
    pub version_id: Uuid,
    pub version_number: i32,
    pub content_sha256: String,
    pub chunk_id: Uuid,
    pub chunk_identity_sha256: String,
    pub page: Option<u32>,
    pub slide: Option<u32>,
    pub sheet: Option<String>,
    pub span_start: usize,
    pub span_end: usize,
    pub quote: String,
    pub effective_from: DateTime<Utc>,
    pub effective_to: Option<DateTime<Utc>>,
    pub is_current: bool,
    pub heading: String,
}

/// Client/server resolve request. Locators are IDs only — never object keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CitationResolveRequest {
    pub chunk_id: Uuid,
    pub expected_version_id: Option<Uuid>,
    pub expected_document_id: Option<Uuid>,
    pub expected_content_sha256: Option<String>,
    pub expected_quote: Option<String>,
    pub expected_span_start: Option<usize>,
    pub expected_span_end: Option<usize>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CitationError {
    #[error("permission denied")]
    PermissionDenied,
    #[error("citation not found")]
    NotFound,
    #[error("citation quote or span mismatch")]
    QuoteMismatch,
    #[error("citation version or hash mismatch")]
    VersionMismatch,
    #[error("citation source location is invalid")]
    InvalidAnchor,
    #[error("invalid citation request")]
    InvalidRequest,
    #[error("trusted markdown is missing")]
    MarkdownMissing,
    #[error("citation integrity check failed")]
    Integrity,
    #[error("storage error")]
    Storage,
    #[error("database error")]
    Database,
}

impl CitationError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::PermissionDenied => "citation_permission_denied",
            Self::NotFound => "citation_not_found",
            Self::QuoteMismatch => "citation_quote_mismatch",
            Self::VersionMismatch => "citation_version_mismatch",
            Self::InvalidAnchor => "citation_invalid_anchor",
            Self::InvalidRequest => "citation_invalid_request",
            Self::MarkdownMissing => "citation_markdown_missing",
            Self::Integrity => "citation_integrity",
            Self::Storage => "citation_storage",
            Self::Database => "citation_database",
        }
    }
}

impl From<ResolveError> for CitationError {
    fn from(_: ResolveError) -> Self {
        Self::PermissionDenied
    }
}

impl From<DbError> for CitationError {
    fn from(value: DbError) -> Self {
        match value {
            DbError::Config(ref msg) if msg.starts_with("markdown_artifact_") => {
                Self::MarkdownMissing
            }
            _ => Self::Database,
        }
    }
}

impl From<StorageError> for CitationError {
    fn from(value: StorageError) -> Self {
        match value {
            StorageError::NotFound => Self::NotFound,
            StorageError::KeyOrgMismatch
            | StorageError::MissingScope
            | StorageError::InvalidKey => Self::PermissionDenied,
            StorageError::PreconditionFailed | StorageError::ObjectTooLarge => Self::Integrity,
            _ => Self::Storage,
        }
    }
}

/// Rejects half-open expected span pairs (both-or-none).
pub fn validate_request_spans(request: &CitationResolveRequest) -> Result<(), CitationError> {
    match (request.expected_span_start, request.expected_span_end) {
        (None, None) | (Some(_), Some(_)) => Ok(()),
        _ => Err(CitationError::InvalidRequest),
    }
}

/// Validates PDF/PPTX/XLSX style location fields (page/slide/sheet).
pub fn validate_source_location(
    page: Option<u32>,
    slide: Option<&u32>,
    sheet: Option<&str>,
) -> Result<(), CitationError> {
    if let Some(page) = page {
        if page == 0 {
            return Err(CitationError::InvalidAnchor);
        }
    }
    if let Some(slide) = slide {
        if *slide == 0 {
            return Err(CitationError::InvalidAnchor);
        }
    }
    if let Some(sheet) = sheet {
        if sheet.trim().is_empty() || sheet.len() > 256 {
            return Err(CitationError::InvalidAnchor);
        }
    }
    Ok(())
}

/// Slices a UTF-8 quote from authoritative Markdown using production spans.
pub fn quote_from_markdown(
    markdown: &str,
    span_start: usize,
    span_end: usize,
) -> Result<&str, CitationError> {
    if span_start >= span_end || span_end > markdown.len() {
        return Err(CitationError::InvalidAnchor);
    }
    if !markdown.is_char_boundary(span_start) || !markdown.is_char_boundary(span_end) {
        return Err(CitationError::InvalidAnchor);
    }
    Ok(&markdown[span_start..span_end])
}

/// Builds a citation after Markdown quote verification (hermetic).
pub fn citation_from_markdown_quote(
    row: &HydratedChunkRow,
    markdown: &str,
    markdown_sha256: &str,
) -> Result<StableCitation, CitationError> {
    if document_reads_suppressed(row.document_state, row.deleted_at.is_some()) {
        return Err(CitationError::PermissionDenied);
    }
    // Production spans are offsets into the trusted Markdown, not chunk.body.
    let (span_start, span_end) = match (row.span_start, row.span_end) {
        (Some(start), Some(end)) if start >= 0 && end >= 0 => (start as usize, end as usize),
        (None, None) => (0, markdown.len()),
        _ => return Err(CitationError::InvalidRequest),
    };
    let quote = quote_from_markdown(markdown, span_start, span_end)?.to_string();
    // Chunk body must match the authoritative Markdown span (fail closed).
    if quote != row.body {
        return Err(CitationError::Integrity);
    }
    if markdown_sha256 != row.content_sha256 {
        // Published version content_sha256 is the Markdown hash after promotion.
        return Err(CitationError::Integrity);
    }
    let page = row.page.and_then(|value| u32::try_from(value).ok());
    let slide = row.slide.and_then(|value| u32::try_from(value).ok());
    validate_source_location(page, slide.as_ref(), row.sheet.as_deref())?;
    Ok(StableCitation {
        org_id: row.org_id,
        logical_document_id: row.document_id,
        version_id: row.version_id,
        version_number: row.version_number,
        content_sha256: row.content_sha256.clone(),
        chunk_id: row.chunk_id,
        chunk_identity_sha256: row.chunk_identity_sha256.clone(),
        page,
        slide,
        sheet: row.sheet.clone(),
        span_start,
        span_end,
        quote,
        effective_from: row.effective_from,
        effective_to: row.effective_to,
        is_current: row.is_current,
        heading: row.heading_path.join(" / "),
    })
}

/// Validates optional pins from a prior answer against the resolved citation.
pub fn validate_citation_pins(
    citation: &StableCitation,
    request: &CitationResolveRequest,
) -> Result<(), CitationError> {
    if let Some(version_id) = request.expected_version_id {
        if version_id != citation.version_id {
            return Err(CitationError::VersionMismatch);
        }
    }
    if let Some(document_id) = request.expected_document_id {
        if document_id != citation.logical_document_id {
            return Err(CitationError::VersionMismatch);
        }
    }
    if let Some(hash) = request.expected_content_sha256.as_deref() {
        if hash != citation.content_sha256 {
            return Err(CitationError::VersionMismatch);
        }
    }
    if let (Some(start), Some(end)) = (request.expected_span_start, request.expected_span_end) {
        if start != citation.span_start || end != citation.span_end {
            return Err(CitationError::QuoteMismatch);
        }
    }
    if let Some(expected_quote) = request.expected_quote.as_deref() {
        if expected_quote != citation.quote {
            return Err(CitationError::QuoteMismatch);
        }
    }
    Ok(())
}

fn require_citation_permissions(
    ctx: &crate::auth::context::OrgContext,
    is_current: bool,
) -> Result<(), CitationError> {
    if !ctx.has_permission(PERMISSION_QA_QUERY) {
        return Err(CitationError::PermissionDenied);
    }
    if !is_current && !ctx.has_permission(PERMISSION_QA_HISTORY) {
        return Err(CitationError::PermissionDenied);
    }
    if ctx.allowed_collection_ids().is_empty() {
        return Err(CitationError::PermissionDenied);
    }
    Ok(())
}

async fn load_authorized_markdown<S: BlobStore>(
    pool: &Pool,
    storage: &S,
    ctx: &crate::auth::context::OrgContext,
    document_id: Uuid,
    version_id: Uuid,
) -> Result<(TrustedMarkdownArtifact, String), CitationError> {
    let row = with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                search::load_authorized_version_for_read(txn, &ctx, document_id, version_id).await
            })
        }
    })
    .await?
    .ok_or(CitationError::NotFound)?;
    let artifact = search::trusted_markdown_artifact(&row)?;
    let key = parse_key_for_org(&artifact.object_key, ctx.org_id())?;
    if key.namespace() != ObjectNamespace::Trusted {
        return Err(CitationError::PermissionDenied);
    }
    authorize_key_for_version(&key, version_id)?;
    if artifact.byte_size > CITATION_MARKDOWN_MAX_BYTES {
        return Err(CitationError::Integrity);
    }
    let fetched = storage
        .get_object_bounded(
            ctx.org_id(),
            &key,
            CITATION_MARKDOWN_MAX_BYTES,
            &ObjectExpectation {
                content_sha256: &artifact.content_sha256,
                content_length: artifact.byte_size,
                content_type: Some(artifact.content_type.as_str()),
            },
        )
        .await?;
    let markdown =
        String::from_utf8(fetched.bytes.to_vec()).map_err(|_| CitationError::Integrity)?;
    Ok((artifact, markdown))
}

/// Resolves one citation with fresh PostgreSQL authorization and Markdown quote verify.
pub async fn resolve_citation<S: BlobStore>(
    pool: &Pool,
    storage: &S,
    org_id: Uuid,
    user_id: Uuid,
    request: CitationResolveRequest,
) -> Result<StableCitation, CitationError> {
    validate_request_spans(&request)?;
    let ctx = resolve_org_context_in_txn(pool, org_id, user_id).await?;
    if !ctx.has_permission(PERMISSION_QA_QUERY) {
        return Err(CitationError::PermissionDenied);
    }
    let row = with_org_txn(pool, &ctx, {
        let ctx = ctx.clone();
        let chunk_id = request.chunk_id;
        move |txn| {
            Box::pin(async move { search::hydrate_chunk_for_citation(txn, &ctx, chunk_id).await })
        }
    })
    .await?
    .ok_or(CitationError::NotFound)?;

    if !ctx.allows_collection(row.collection_id) {
        return Err(CitationError::PermissionDenied);
    }
    require_citation_permissions(&ctx, row.is_current)?;

    let (artifact, markdown) =
        load_authorized_markdown(pool, storage, &ctx, row.document_id, row.version_id).await?;
    let citation = citation_from_markdown_quote(&row, &markdown, &artifact.content_sha256)?;
    if citation.org_id != org_id {
        return Err(CitationError::PermissionDenied);
    }
    validate_citation_pins(&citation, &request)?;
    Ok(citation)
}

/// Resolves many citations (multi-document / multi-version) with per-item auth.
pub async fn resolve_citations<S: BlobStore>(
    pool: &Pool,
    storage: &S,
    org_id: Uuid,
    user_id: Uuid,
    requests: &[CitationResolveRequest],
) -> Result<Vec<StableCitation>, CitationError> {
    let mut out = Vec::with_capacity(requests.len());
    for request in requests {
        out.push(resolve_citation(pool, storage, org_id, user_id, request.clone()).await?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::{DocumentState, IndexGenerationState, PublicationState};
    use chrono::TimeZone;

    fn sample_markdown() -> &'static str {
        "# Mục\n\nMở đầu.\n\nKinh phí phê duyệt là 15 triệu đồng.\n\nKết.\n"
    }

    fn sample_row(span_start: usize, span_end: usize, body: &str) -> HydratedChunkRow {
        let org = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
        let markdown = sample_markdown();
        let sha = {
            use sha2::{Digest, Sha256};
            hex::encode(Sha256::digest(markdown.as_bytes()))
        };
        HydratedChunkRow {
            chunk_id: Uuid::parse_str("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap(),
            chunk_identity_sha256: "a".repeat(64),
            org_id: org,
            collection_id: Uuid::parse_str("55555555-5555-5555-5555-555555555501").unwrap(),
            document_id: Uuid::parse_str("66666666-6666-6666-6666-666666666601").unwrap(),
            version_id: Uuid::parse_str("77777777-7777-7777-7777-777777777701").unwrap(),
            version_number: 2,
            content_sha256: sha,
            heading_path: vec!["Mục 1".into(), "Slide 3".into()],
            body: body.into(),
            page: Some(4),
            slide: Some(3),
            sheet: None,
            span_start: Some(span_start as i32),
            span_end: Some(span_end as i32),
            document_state: DocumentState::Indexed,
            deleted_at: None,
            publication_state: PublicationState::Published,
            is_current: true,
            effective_from: Utc.with_ymd_and_hms(2024, 8, 1, 0, 0, 0).unwrap(),
            effective_to: None,
            index_metadata_id: Uuid::new_v4(),
            index_generation_active: false,
            index_generation_state: IndexGenerationState::Retired,
        }
    }

    #[test]
    fn slices_nonzero_vietnamese_span_from_markdown_not_chunk_prefix() {
        let markdown = sample_markdown();
        let quote = "Kinh phí phê duyệt là 15 triệu đồng.";
        let start = markdown.find(quote).unwrap();
        let end = start + quote.len();
        assert!(start > 0, "production span must be nonzero into markdown");
        let row = sample_row(start, end, quote);
        // Retired generation still builds a citation (exact resolve ignores active).
        assert!(!row.index_generation_active);
        assert_eq!(row.index_generation_state, IndexGenerationState::Retired);
        let citation =
            citation_from_markdown_quote(&row, markdown, &row.content_sha256.clone()).unwrap();
        assert_eq!(citation.quote, quote);
        assert_eq!(citation.span_start, start);
        assert_eq!(citation.span_end, end);
    }

    #[test]
    fn second_chunk_span_does_not_use_first_chunk_body() {
        let markdown = sample_markdown();
        let first = "Mở đầu.";
        let second = "Kết.";
        let start = markdown.find(second).unwrap();
        let end = start + second.len();
        let mut row = sample_row(start, end, second);
        row.chunk_id = Uuid::parse_str("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb").unwrap();
        // Wrong body (first chunk) must fail closed against Markdown span.
        row.body = first.into();
        assert_eq!(
            citation_from_markdown_quote(&row, markdown, &row.content_sha256),
            Err(CitationError::Integrity)
        );
        row.body = second.into();
        let citation =
            citation_from_markdown_quote(&row, markdown, &row.content_sha256.clone()).unwrap();
        assert_eq!(citation.quote, second);
    }

    #[test]
    fn half_open_expected_spans_are_invalid_requests() {
        let request = CitationResolveRequest {
            chunk_id: Uuid::new_v4(),
            expected_version_id: None,
            expected_document_id: None,
            expected_content_sha256: None,
            expected_quote: None,
            expected_span_start: Some(1),
            expected_span_end: None,
        };
        assert_eq!(
            validate_request_spans(&request),
            Err(CitationError::InvalidRequest)
        );
    }

    #[test]
    fn rejects_non_utf8_boundary_spans() {
        let markdown = "áx"; // 'á' is two UTF-8 bytes; offset 1 is mid-character.
        assert!(!markdown.is_char_boundary(1));
        assert!(quote_from_markdown(markdown, 1, markdown.len()).is_err());
    }

    #[test]
    fn xlsx_sheet_and_page_anchors_still_validate() {
        assert!(validate_source_location(Some(2), None, None).is_ok());
        assert!(validate_source_location(None, Some(&3), None).is_ok());
        assert!(validate_source_location(None, None, Some("Quý I")).is_ok());
        assert!(validate_source_location(Some(0), None, None).is_err());
    }
}
