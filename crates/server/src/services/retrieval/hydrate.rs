//! PG-authoritative candidate hydration and security recheck.

use deadpool_postgres::Pool;
use fileconv_knowledge::citation::extract_snippet;
use fileconv_knowledge::query::PreparedQuery;
use fileconv_knowledge::rank::hybrid_rerank_score;

use crate::auth::context::OrgContext;
use crate::db::pool::with_org_txn_typed;
use crate::db::search;
use crate::services::retrieval::{Candidate, GroundedHit, RetrievalError, VersionMode};

pub async fn hydrate(
    pool: &Pool,
    ctx: &OrgContext,
    candidates: &[Candidate],
    mode: &VersionMode,
    index_signature: &str,
    prepared: &PreparedQuery,
) -> Result<Vec<GroundedHit>, RetrievalError> {
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    let txn_ctx = ctx.clone();
    let query_ctx = txn_ctx.clone();
    let identities = candidates
        .iter()
        .map(|candidate| candidate.chunk_identity.clone())
        .collect::<Vec<_>>();
    let authorized = txn_ctx
        .allowed_collection_ids()
        .iter()
        .copied()
        .collect::<Vec<_>>();
    let mode = search::SearchVersionMode::from(mode);
    let index_signature = index_signature.to_string();
    let rows = with_org_txn_typed(pool, &txn_ctx, move |txn| {
        Box::pin(async move {
            search::hydrate_chunks(
                txn,
                &query_ctx,
                &authorized,
                &identities,
                &mode,
                &index_signature,
            )
            .await
            .map_err(RetrievalError::from)
        })
    })
    .await?;

    let mut hits = Vec::new();
    for row in rows {
        let Some(candidate) = candidates
            .iter()
            .find(|candidate| candidate.chunk_identity == row.chunk_identity)
        else {
            continue;
        };
        let heading_joined = row.heading_path.join(" ");
        let snippet = extract_snippet(&row.body, &prepared.tokens);
        let rerank_score = hybrid_rerank_score(
            candidate.lexical_rank,
            candidate.vector_rank,
            candidate.vector_score,
            &prepared.tokens,
            &heading_joined,
            &row.body,
        );
        hits.push(GroundedHit {
            chunk_id: row.chunk_id,
            chunk_identity: row.chunk_identity,
            document_id: row.document_id,
            version_id: row.version_id,
            collection_id: row.collection_id,
            version_number: row.version_number,
            content_sha256: row.content_sha256,
            heading_path: row.heading_path,
            snippet,
            page: row.page,
            slide: row.slide,
            sheet: row.sheet,
            span_start: row.span_start,
            span_end: row.span_end,
            lexical_score: candidate.lexical_score,
            vector_score: candidate.vector_score,
            rerank_score,
            is_current: row.is_current,
            effective_from: row.effective_from,
            effective_to: row.effective_to,
        });
    }
    Ok(hits)
}
