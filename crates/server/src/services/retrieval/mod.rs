//! Tenant-scoped hybrid retrieval engine.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use deadpool_postgres::Pool;
use fileconv_knowledge::embedding::{local_vector, LOCAL_VECTOR_DIMENSIONS};
use fileconv_knowledge::query::PreparedQuery;
use thiserror::Error;
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db;
use crate::services::embedding;
use crate::services::index_signature::collection_name_for_digest;
use crate::storage::qdrant::QdrantClient;
use crate::storage::StorageError;

pub(crate) mod fts;
pub(crate) mod hydrate;
pub(crate) mod vector;

pub const VECTOR_CANDIDATES: usize = 500;
pub const LEXICAL_CANDIDATES: usize = 250;
pub const MAX_RETRIEVAL_LIMIT: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetrievalRequest {
    pub query: String,
    pub limit: usize,
    pub mode: VersionMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionMode {
    Current,
    AsOf(DateTime<Utc>),
    History {
        document_id: Uuid,
    },
    Compare {
        document_id: Uuid,
        version_ids: Vec<Uuid>,
    },
}

impl Default for VersionMode {
    fn default() -> Self {
        Self::Current
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RetrievalResponse {
    pub hits: Vec<GroundedHit>,
    pub degraded: Option<Degradation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Degradation {
    VectorUnavailable,
    LexicalUnavailable,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GroundedHit {
    pub chunk_id: Uuid,
    pub chunk_identity: String,
    pub document_id: Uuid,
    pub version_id: Uuid,
    pub collection_id: Uuid,
    pub version_number: i32,
    pub content_sha256: String,
    pub heading_path: Vec<String>,
    pub snippet: String,
    pub page: Option<i32>,
    pub slide: Option<i32>,
    pub sheet: Option<String>,
    pub span_start: Option<i32>,
    pub span_end: Option<i32>,
    pub lexical_score: f32,
    pub vector_score: f32,
    pub rerank_score: f32,
    pub is_current: bool,
    pub effective_from: DateTime<Utc>,
    pub effective_to: Option<DateTime<Utc>>,
}

#[derive(Debug, Error)]
pub enum RetrievalError {
    #[error("retrieval scope is empty")]
    EmptyScope,
    #[error("database error")]
    Db(#[from] db::error::DbError),
    #[error("storage error")]
    Storage(#[from] StorageError),
    #[error("knowledge error")]
    Knowledge(#[from] fileconv_knowledge::KnowledgeError),
    #[error("embedding task failed")]
    EmbeddingJoin,
    #[error("both retrieval candidate legs are unavailable")]
    BothLegsUnavailable,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Candidate {
    pub chunk_identity: String,
    pub lexical_score: f32,
    pub vector_score: f32,
    pub lexical_rank: Option<usize>,
    pub vector_rank: Option<usize>,
}

pub async fn retrieve(
    pool: &Pool,
    qdrant: &QdrantClient,
    ctx: &OrgContext,
    request: RetrievalRequest,
) -> Result<RetrievalResponse, RetrievalError> {
    if ctx.org_id().is_nil() || ctx.allowed_collection_ids().is_empty() {
        return Err(RetrievalError::EmptyScope);
    }

    let limit = request.limit.clamp(1, MAX_RETRIEVAL_LIMIT);
    let prepared = PreparedQuery::new(&request.query);
    if prepared.is_empty() {
        return Ok(RetrievalResponse {
            hits: Vec::new(),
            degraded: None,
        });
    }

    let index_signature = active_index_signature_digest()?;
    let collection_name = collection_name_for_digest(&index_signature)?;
    let query_for_embed = request.query.clone();
    let query_vector = tokio::task::spawn_blocking(move || local_vector(&query_for_embed))
        .await
        .map_err(|_| RetrievalError::EmbeddingJoin)?
        .into_values();

    let vector_future = vector::search(
        qdrant,
        &collection_name,
        ctx.org_id(),
        ctx.allowed_collection_ids().iter().copied(),
        &query_vector,
        &request.mode,
    );
    let fts_future = fts::search(pool, ctx, &request.query, &request.mode, &index_signature);
    let (vector_result, lexical_result) = tokio::join!(vector_future, fts_future);

    let (candidates, degraded) = merge_leg_results(vector_result, lexical_result)?;
    if candidates.is_empty() {
        return Ok(RetrievalResponse {
            hits: Vec::new(),
            degraded,
        });
    }

    let mut hits = hydrate::hydrate(
        pool,
        ctx,
        &candidates,
        &request.mode,
        &index_signature,
        &prepared,
    )
    .await?;
    sort_hits_deterministically(&mut hits);
    hits.truncate(limit);

    Ok(RetrievalResponse { hits, degraded })
}

fn active_index_signature_digest() -> Result<String, RetrievalError> {
    Ok(embedding::approved_plan()
        .index_signature(LOCAL_VECTOR_DIMENSIONS)?
        .digest())
}

fn merge_leg_results(
    vector_result: Result<Vec<vector::VectorCandidate>, StorageError>,
    lexical_result: Result<Vec<db::search::LexicalCandidate>, RetrievalError>,
) -> Result<(Vec<Candidate>, Option<Degradation>), RetrievalError> {
    let degraded = match (&vector_result, &lexical_result) {
        (Ok(_), Ok(_)) => None,
        (Err(_), Ok(_)) => Some(Degradation::VectorUnavailable),
        (Ok(_), Err(_)) => Some(Degradation::LexicalUnavailable),
        (Err(_), Err(_)) => return Err(RetrievalError::BothLegsUnavailable),
    };

    let mut merged: BTreeMap<String, Candidate> = BTreeMap::new();
    if let Ok(vector_candidates) = vector_result {
        for candidate in vector_candidates {
            let entry = merged
                .entry(candidate.chunk_identity.clone())
                .or_insert_with(|| Candidate {
                    chunk_identity: candidate.chunk_identity,
                    lexical_score: 0.0,
                    vector_score: 0.0,
                    lexical_rank: None,
                    vector_rank: None,
                });
            entry.vector_score = candidate.vector_score;
            entry.vector_rank = Some(candidate.rank);
        }
    }
    if let Ok(lexical_candidates) = lexical_result {
        for (rank, candidate) in lexical_candidates.into_iter().enumerate() {
            let entry = merged
                .entry(candidate.chunk_identity.clone())
                .or_insert_with(|| Candidate {
                    chunk_identity: candidate.chunk_identity,
                    lexical_score: 0.0,
                    vector_score: 0.0,
                    lexical_rank: None,
                    vector_rank: None,
                });
            entry.lexical_score = candidate.lexical_score;
            entry.lexical_rank = Some(rank);
        }
    }

    Ok((merged.into_values().collect(), degraded))
}

pub(crate) fn sort_hits_deterministically(hits: &mut [GroundedHit]) {
    hits.sort_by(|left, right| {
        right
            .rerank_score
            .partial_cmp(&left.rerank_score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.chunk_identity.cmp(&right.chunk_identity))
    });
}

impl From<&VersionMode> for db::search::SearchVersionMode {
    fn from(value: &VersionMode) -> Self {
        match value {
            VersionMode::Current => Self::Current,
            VersionMode::AsOf(timestamp) => Self::AsOf(*timestamp),
            VersionMode::History { document_id } => Self::History {
                document_id: *document_id,
            },
            VersionMode::Compare {
                document_id,
                version_ids,
            } => Self::Compare {
                document_id: *document_id,
                version_ids: version_ids.clone(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_hit(identity: &str, score: f32) -> GroundedHit {
        let id = Uuid::new_v4();
        GroundedHit {
            chunk_id: id,
            chunk_identity: identity.to_string(),
            document_id: Uuid::new_v4(),
            version_id: Uuid::new_v4(),
            collection_id: Uuid::new_v4(),
            version_number: 1,
            content_sha256: "a".repeat(64),
            heading_path: Vec::new(),
            snippet: String::new(),
            page: None,
            slide: None,
            sheet: None,
            span_start: None,
            span_end: None,
            lexical_score: 0.0,
            vector_score: 0.0,
            rerank_score: score,
            is_current: true,
            effective_from: Utc::now(),
            effective_to: None,
        }
    }

    #[test]
    fn deterministic_sort_ties_by_chunk_identity() {
        let mut hits = vec![
            test_hit("bbbb", 1.0),
            test_hit("aaaa", 1.0),
            test_hit("cccc", 0.5),
        ];
        sort_hits_deterministically(&mut hits);
        let identities = hits
            .iter()
            .map(|hit| hit.chunk_identity.as_str())
            .collect::<Vec<_>>();
        assert_eq!(identities, ["aaaa", "bbbb", "cccc"]);
    }

    #[test]
    fn prepared_empty_token_query_is_empty() {
        let prepared = PreparedQuery::new("!? .");
        assert!(prepared.is_empty());
    }
}
