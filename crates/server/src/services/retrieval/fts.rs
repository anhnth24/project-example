//! PostgreSQL FTS candidate retrieval.

use deadpool_postgres::Pool;

use crate::auth::context::OrgContext;
use crate::db::pool::with_org_txn_typed;
use crate::db::search::{self, LexicalCandidate};
use crate::services::retrieval::{RetrievalError, VersionMode, LEXICAL_CANDIDATES};

pub async fn search(
    pool: &Pool,
    ctx: &OrgContext,
    query: &str,
    mode: &VersionMode,
    index_signature: &str,
) -> Result<Vec<LexicalCandidate>, RetrievalError> {
    let txn_ctx = ctx.clone();
    let query_ctx = txn_ctx.clone();
    let query = query.to_string();
    let index_signature = index_signature.to_string();
    let mode = search::SearchVersionMode::from(mode);
    let authorized = txn_ctx
        .allowed_collection_ids()
        .iter()
        .copied()
        .collect::<Vec<_>>();
    with_org_txn_typed(pool, &txn_ctx, move |txn| {
        Box::pin(async move {
            search::lexical_candidates(
                txn,
                &query_ctx,
                &authorized,
                &query,
                &mode,
                LEXICAL_CANDIDATES as i64,
                &index_signature,
            )
            .await
            .map_err(RetrievalError::from)
        })
    })
    .await
}
