//! PostgreSQL FTS retrieval leg.
//!
//! Thin wrapper over [`crate::db::search::fts_search`] so the orchestrator can
//! treat lexical/vector legs symmetrically and degrade on one-leg outages.

use deadpool_postgres::Pool;
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::error::DbError;
use crate::db::pool::with_org_txn;
use crate::db::search::{self, FtsCandidate, VersionVisibility};

/// Lexical candidate identity + score (no body until hydration).
#[derive(Debug, Clone, PartialEq)]
pub struct LexicalCandidate {
    pub chunk_id: Uuid,
    pub chunk_identity_sha256: String,
    pub document_id: Uuid,
    pub version_id: Uuid,
    pub collection_id: Uuid,
    pub score: f32,
}

impl From<FtsCandidate> for LexicalCandidate {
    fn from(value: FtsCandidate) -> Self {
        Self {
            chunk_id: value.chunk_id,
            chunk_identity_sha256: value.chunk_identity_sha256,
            document_id: value.document_id,
            version_id: value.version_id,
            collection_id: value.collection_id,
            score: value.rank,
        }
    }
}

/// Runs tenant-scoped FTS under an org transaction.
pub async fn search_lexical(
    pool: &Pool,
    ctx: &OrgContext,
    collection_ids: &[Uuid],
    query: &str,
    visibility: &VersionVisibility,
    limit: usize,
) -> Result<Vec<LexicalCandidate>, DbError> {
    let visibility = visibility.clone();
    let collection_ids = collection_ids.to_vec();
    let query = query.to_string();
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let rows =
                    search::fts_search(txn, &ctx, &collection_ids, &query, &visibility, limit)
                        .await?;
                Ok(rows.into_iter().map(LexicalCandidate::from).collect())
            })
        }
    })
    .await
}

/// Keeps only candidates whose collection remains in the authorized scope.
pub fn filter_lexical_in_scope(
    allowed_collections: &[Uuid],
    candidates: Vec<LexicalCandidate>,
) -> Vec<LexicalCandidate> {
    let allowed: std::collections::BTreeSet<Uuid> = allowed_collections.iter().copied().collect();
    candidates
        .into_iter()
        .filter(|candidate| allowed.contains(&candidate.collection_id))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexical_scope_filter_drops_foreign_collection() {
        let allowed = Uuid::new_v4();
        let foreign = Uuid::new_v4();
        let kept = LexicalCandidate {
            chunk_id: Uuid::new_v4(),
            chunk_identity_sha256: "a".into(),
            document_id: Uuid::new_v4(),
            version_id: Uuid::new_v4(),
            collection_id: allowed,
            score: 1.0,
        };
        let dropped = LexicalCandidate {
            collection_id: foreign,
            chunk_identity_sha256: "b".into(),
            ..kept.clone()
        };
        let out = filter_lexical_in_scope(&[allowed], vec![kept.clone(), dropped]);
        assert_eq!(out, vec![kept]);
    }
}
