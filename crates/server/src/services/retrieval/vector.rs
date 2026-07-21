//! Qdrant vector retrieval leg with tenant + version filters.
//!
//! Hits expose payload identity/scores only. Chunk text must come from
//! PostgreSQL hydration (`hydrate`).

use std::collections::BTreeSet;

use serde_json::{json, Value};
use uuid::Uuid;

use crate::db::search::VersionVisibility;
use crate::services::index_signature::CollectionName;
use crate::storage::error::StorageError;
use crate::storage::qdrant::{QdrantClient, SearchHit, VectorScope};

/// Candidate from the vector leg (identity + score; never authoritative text).
#[derive(Debug, Clone, PartialEq)]
pub struct VectorCandidate {
    pub chunk_identity_sha256: String,
    pub document_id: Uuid,
    pub version_id: Uuid,
    pub collection_id: Uuid,
    pub score: f32,
    pub payload_is_current: bool,
}

/// Builds Qdrant `must` clauses for the resolved version visibility.
pub fn version_filter_clauses(visibility: &VersionVisibility) -> Vec<Value> {
    match visibility {
        VersionVisibility::Current => vec![json!({
            "key": "is_current",
            "match": { "value": true }
        })],
        VersionVisibility::VersionIds(version_ids) => {
            if version_ids.is_empty() {
                return Vec::new();
            }
            let values: Vec<String> = version_ids.iter().map(Uuid::to_string).collect();
            vec![json!({
                "key": "version_id",
                "match": { "any": values }
            })]
        }
    }
}

/// Restricts vector hits to an optional document lineage (compare/history).
pub fn document_filter_clause(document_id: Option<Uuid>) -> Option<Value> {
    document_id.map(|id| {
        json!({
            "key": "document_id",
            "match": { "value": id.to_string() }
        })
    })
}

/// Runs a scoped vector search. Empty version visibility returns no hits.
pub async fn search_vectors(
    client: &QdrantClient,
    collection_name: &CollectionName,
    scope: &VectorScope,
    query_vector: &[f32],
    visibility: &VersionVisibility,
    document_id: Option<Uuid>,
    limit: usize,
) -> Result<Vec<VectorCandidate>, StorageError> {
    scope.validate()?;
    if limit == 0 {
        return Ok(Vec::new());
    }
    if matches!(visibility, VersionVisibility::VersionIds(ids) if ids.is_empty()) {
        return Ok(Vec::new());
    }
    let mut extra = version_filter_clauses(visibility);
    if let Some(clause) = document_filter_clause(document_id) {
        extra.push(clause);
    }
    let hits = client
        .search_filtered(collection_name, scope, query_vector, limit, &extra)
        .await?;
    Ok(hits.into_iter().filter_map(map_hit).collect())
}

/// Drops out-of-scope payload rows (defense in depth after Qdrant filter).
pub fn filter_candidates_in_scope(
    scope: &VectorScope,
    candidates: Vec<VectorCandidate>,
) -> Result<Vec<VectorCandidate>, StorageError> {
    let mut out = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        if candidate.collection_id.is_nil()
            || !scope.collection_ids.contains(&candidate.collection_id)
        {
            return Err(StorageError::OwnershipConflict);
        }
        out.push(candidate);
    }
    Ok(out)
}

/// Current-mode guard: ignore superseded payload markers before hydration.
pub fn suppress_non_current_for_mode(
    visibility: &VersionVisibility,
    candidates: Vec<VectorCandidate>,
) -> Vec<VectorCandidate> {
    match visibility {
        VersionVisibility::Current => candidates
            .into_iter()
            .filter(|candidate| candidate.payload_is_current)
            .collect(),
        VersionVisibility::VersionIds(_) => candidates,
    }
}

/// Restricts candidates to an explicit version allow-list (as_of/compare/history).
pub fn retain_version_ids(
    allowed: &BTreeSet<Uuid>,
    candidates: Vec<VectorCandidate>,
) -> Vec<VectorCandidate> {
    candidates
        .into_iter()
        .filter(|candidate| allowed.contains(&candidate.version_id))
        .collect()
}

fn map_hit(hit: SearchHit) -> Option<VectorCandidate> {
    if hit.payload.chunk_id.is_empty() {
        return None;
    }
    Some(VectorCandidate {
        chunk_identity_sha256: hit.payload.chunk_id,
        document_id: hit.payload.document_id,
        version_id: hit.payload.version_id,
        collection_id: hit.payload.collection_id,
        score: hit.score,
        payload_is_current: hit.payload.is_current,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn current_mode_filter_requires_is_current() {
        let clauses = version_filter_clauses(&VersionVisibility::Current);
        assert_eq!(clauses.len(), 1);
        assert_eq!(clauses[0]["key"], "is_current");
        assert_eq!(clauses[0]["match"]["value"], true);
    }

    #[test]
    fn version_ids_filter_uses_any_match() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let visibility = VersionVisibility::VersionIds(BTreeSet::from([a, b]));
        let clauses = version_filter_clauses(&visibility);
        assert_eq!(clauses.len(), 1);
        let any = clauses[0]["match"]["any"].as_array().unwrap();
        assert_eq!(any.len(), 2);
    }

    #[test]
    fn empty_version_ids_yield_no_filter_clauses() {
        let visibility = VersionVisibility::VersionIds(BTreeSet::new());
        assert!(version_filter_clauses(&visibility).is_empty());
    }

    #[test]
    fn suppress_non_current_drops_superseded_payload() {
        let current = VectorCandidate {
            chunk_identity_sha256: "a".into(),
            document_id: Uuid::new_v4(),
            version_id: Uuid::new_v4(),
            collection_id: Uuid::new_v4(),
            score: 0.9,
            payload_is_current: true,
        };
        let stale = VectorCandidate {
            chunk_identity_sha256: "b".into(),
            payload_is_current: false,
            ..current.clone()
        };
        let kept = suppress_non_current_for_mode(
            &VersionVisibility::Current,
            vec![current.clone(), stale],
        );
        assert_eq!(kept, vec![current]);
    }
}
