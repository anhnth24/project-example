//! Freshly authorized version-aware citation resolution.

use chrono::{DateTime, Utc};
use deadpool_postgres::Pool;
use fileconv_knowledge::citation::extract_snippet;
use fileconv_knowledge::query::PreparedQuery;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::error::DbError;
use crate::db::pool::with_org_txn_typed;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CitationPin {
    pub document_id: Uuid,
    pub version_id: Uuid,
    pub version_number: i32,
    pub content_sha256: String,
    pub chunk_id: Uuid,
    pub span_start: Option<i32>,
    pub span_end: Option<i32>,
    pub quote: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedCitation {
    pub org_id: Uuid,
    pub document_id: Uuid,
    pub version_id: Uuid,
    pub version_number: i32,
    pub content_sha256: String,
    pub chunk_id: Uuid,
    pub collection_id: Uuid,
    pub heading_path: Vec<String>,
    pub snippet: String,
    pub page: Option<i32>,
    pub slide: Option<i32>,
    pub sheet: Option<String>,
    pub span_start: Option<i32>,
    pub span_end: Option<i32>,
    pub quote: Option<String>,
    pub effective_from: DateTime<Utc>,
    pub effective_to: Option<DateTime<Utc>>,
    pub is_current: bool,
}

#[derive(Debug, Error)]
pub enum CitationError {
    #[error("citation source was not found")]
    NotFound,
    #[error("database error")]
    Db(#[from] DbError),
}

#[derive(Debug)]
struct CitationRow {
    document_id: Uuid,
    version_id: Uuid,
    version_number: i32,
    content_sha256: String,
    collection_id: Uuid,
    heading_path: Vec<String>,
    body: String,
    page: Option<i32>,
    slide: Option<i32>,
    sheet: Option<String>,
    span_start: Option<i32>,
    span_end: Option<i32>,
    effective_from: DateTime<Utc>,
    effective_to: Option<DateTime<Utc>>,
    is_current: bool,
}

pub async fn resolve_citation(
    pool: &Pool,
    ctx: &OrgContext,
    pin: CitationPin,
) -> Result<ResolvedCitation, CitationError> {
    if ctx.org_id().is_nil() || ctx.allowed_collection_ids().is_empty() {
        return Err(CitationError::NotFound);
    }

    let authorized = ctx
        .allowed_collection_ids()
        .iter()
        .copied()
        .collect::<Vec<_>>();
    let txn_ctx = ctx.clone();
    let query_ctx = txn_ctx.clone();
    let chunk_id = pin.chunk_id;
    let row = with_org_txn_typed(pool, &txn_ctx, move |txn| {
        Box::pin(async move {
            let row = txn
                .query_opt(
                    "SELECT c.document_id,
                            c.version_id,
                            d.collection_id,
                            v.version_number,
                            v.content_sha256,
                            c.heading_path,
                            c.body,
                            c.page,
                            c.slide,
                            c.sheet,
                            c.span_start,
                            c.span_end,
                            v.effective_from,
                            v.effective_to,
                            v.is_current
                     FROM chunks c
                     JOIN document_versions v
                       ON v.org_id = c.org_id
                      AND v.document_id = c.document_id
                      AND v.id = c.version_id
                     JOIN documents d
                       ON d.org_id = c.org_id
                      AND d.id = c.document_id
                     WHERE c.org_id = $1
                       AND c.id = $3
                       AND d.collection_id = ANY($2::uuid[])
                       AND d.state = 'indexed'
                       AND d.deleted_at IS NULL",
                    &[&query_ctx.org_id(), &authorized, &chunk_id],
                )
                .await
                .map_err(DbError::from)?
                .ok_or(CitationError::NotFound)?;
            Ok::<_, CitationError>(CitationRow {
                document_id: row.get("document_id"),
                version_id: row.get("version_id"),
                version_number: row.get("version_number"),
                content_sha256: row.get("content_sha256"),
                collection_id: row.get("collection_id"),
                heading_path: row.get("heading_path"),
                body: row.get("body"),
                page: row.get("page"),
                slide: row.get("slide"),
                sheet: row.get("sheet"),
                span_start: row.get("span_start"),
                span_end: row.get("span_end"),
                effective_from: row.get("effective_from"),
                effective_to: row.get("effective_to"),
                is_current: row.get("is_current"),
            })
        })
    })
    .await?;

    verify_pin(&row, &pin)?;
    let tokens = pin
        .quote
        .as_deref()
        .map(PreparedQuery::new)
        .unwrap_or_else(|| PreparedQuery::new(&row.body))
        .tokens;
    let snippet = extract_snippet(&row.body, &tokens);

    Ok(ResolvedCitation {
        org_id: ctx.org_id(),
        document_id: row.document_id,
        version_id: row.version_id,
        version_number: row.version_number,
        content_sha256: row.content_sha256,
        chunk_id: pin.chunk_id,
        collection_id: row.collection_id,
        heading_path: row.heading_path,
        snippet,
        page: row.page,
        slide: row.slide,
        sheet: row.sheet,
        span_start: row.span_start,
        span_end: row.span_end,
        quote: pin.quote,
        effective_from: row.effective_from,
        effective_to: row.effective_to,
        is_current: row.is_current,
    })
}

fn verify_pin(row: &CitationRow, pin: &CitationPin) -> Result<(), CitationError> {
    if row.document_id != pin.document_id
        || row.version_id != pin.version_id
        || row.version_number != pin.version_number
        || row.content_sha256 != pin.content_sha256
    {
        return Err(CitationError::NotFound);
    }
    if let Some(quote) = pin.quote.as_deref() {
        let body = fileconv_core::normalize_nfc_text(&row.body);
        let quote = fileconv_core::normalize_nfc_text(quote);
        if !body.contains(&quote) {
            return Err(CitationError::NotFound);
        }
    }
    // When both the pin and the stored chunk carry a span, they must match.
    // TODO(R-followup): tighten to exact Option equality once chunk anchor
    // extraction populates spans, so a pin cannot claim a span the chunk lacks.
    if let (Some(pin_start), Some(stored_start)) = (pin.span_start, row.span_start) {
        if pin_start != stored_start {
            return Err(CitationError::NotFound);
        }
    }
    if let (Some(pin_end), Some(stored_end)) = (pin.span_end, row.span_end) {
        if pin_end != stored_end {
            return Err(CitationError::NotFound);
        }
    }
    Ok(())
}
