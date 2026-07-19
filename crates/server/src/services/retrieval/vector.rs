//! Vector candidate retrieval fenced by Qdrant tenant filters.

use serde_json::{json, Value as JsonValue};
use uuid::Uuid;

use crate::services::index_signature::CollectionName;
use crate::services::retrieval::{VersionMode, VECTOR_CANDIDATES};
use crate::storage::qdrant::{QdrantClient, VectorScope};
use crate::storage::StorageError;

#[derive(Debug, Clone, PartialEq)]
pub struct VectorCandidate {
    pub chunk_identity: String,
    pub vector_score: f32,
    pub rank: usize,
}

pub async fn search(
    qdrant: &QdrantClient,
    collection_name: &CollectionName,
    org_id: Uuid,
    authorized_collection_ids: impl IntoIterator<Item = Uuid>,
    vector: &[f32],
    mode: &VersionMode,
) -> Result<Vec<VectorCandidate>, StorageError> {
    let scope = VectorScope::new(org_id, authorized_collection_ids);
    let extra_must = mode_extra_must(mode);
    let hits = qdrant
        .search_filtered(
            collection_name,
            &scope,
            vector,
            VECTOR_CANDIDATES,
            &extra_must,
        )
        .await?;
    Ok(hits
        .into_iter()
        .enumerate()
        .map(|(rank, hit)| VectorCandidate {
            chunk_identity: hit.payload.chunk_id,
            vector_score: hit.score,
            rank,
        })
        .collect())
}

pub fn mode_extra_must(mode: &VersionMode) -> Vec<JsonValue> {
    let mut filters = Vec::new();
    match mode {
        VersionMode::Current => {
            filters.push(json!({
                "key": "is_effective",
                "match": { "value": true }
            }));
            filters.push(json!({
                "key": "is_current",
                "match": { "value": true }
            }));
        }
        VersionMode::AsOf(_) => {
            filters.push(json!({
                "key": "is_effective",
                "match": { "value": true }
            }));
        }
        VersionMode::History { document_id } => {
            filters.push(json!({
                "key": "document_id",
                "match": { "value": document_id.to_string() }
            }));
        }
        VersionMode::Compare {
            document_id,
            version_ids,
        } => {
            filters.push(json!({
                "key": "document_id",
                "match": { "value": document_id.to_string() }
            }));
            let versions: Vec<String> = version_ids.iter().map(Uuid::to_string).collect();
            filters.push(json!({
                "key": "version_id",
                "match": { "any": versions }
            }));
        }
    }
    filters
}

#[cfg(test)]
mod tests {
    use super::mode_extra_must;
    use crate::services::retrieval::VersionMode;
    use uuid::Uuid;

    #[test]
    fn current_filter_requires_effective_current_points() {
        let filters = mode_extra_must(&VersionMode::Current);
        assert_eq!(filters.len(), 2);
        assert!(filters.iter().any(|filter| filter["key"] == "is_effective"));
        assert!(filters.iter().any(|filter| filter["key"] == "is_current"));
    }

    #[test]
    fn compare_filter_fences_document_and_versions() {
        let document_id = Uuid::new_v4();
        let version_id = Uuid::new_v4();
        let filters = mode_extra_must(&VersionMode::Compare {
            document_id,
            version_ids: vec![version_id],
        });
        assert_eq!(filters.len(), 2);
        assert_eq!(filters[0]["key"], "document_id");
        assert_eq!(filters[1]["key"], "version_id");
    }
}
