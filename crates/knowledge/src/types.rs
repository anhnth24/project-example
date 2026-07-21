use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentScope {
    source_rels: Vec<String>,
}

impl DocumentScope {
    pub fn new(source_rels: Vec<String>) -> Self {
        Self { source_rels }
    }

    pub fn source_rels(&self) -> &[String] {
        &self.source_rels
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexRequest {
    pub source_rels: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexBuildResult {
    pub documents: usize,
    pub chunks: usize,
    pub indexed: usize,
    pub skipped: usize,
    pub embedding_mode: String,
    pub embedding_provider: String,
    pub embedding_model: String,
    pub vector_dimensions: usize,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexStats {
    pub documents: usize,
    pub chunks: usize,
    pub database_bytes: u64,
    pub vector_dimensions: usize,
    pub embedding_mode: String,
    pub embedding_provider: String,
    pub embedding_model: String,
    pub ann_available: bool,
    pub ann_threshold: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HybridSearchRequest {
    pub source_rels: Vec<String>,
    pub query: String,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceAnchor {
    pub page: Option<u32>,
    pub slide: Option<u32>,
    pub sheet: Option<String>,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HybridSearchHit {
    pub chunk_id: String,
    pub source_rel: String,
    pub md_rel: String,
    pub heading: String,
    pub snippet: String,
    pub lexical_score: f32,
    pub vector_score: f32,
    pub rerank_score: f32,
    pub anchor: SourceAnchor,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HybridSearchResponse {
    pub hits: Vec<HybridSearchHit>,
    pub warnings: Vec<String>,
    pub embedding_mode: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HybridAskRequest {
    pub source_rels: Vec<String>,
    pub question: String,
    pub top_k: Option<usize>,
    pub use_llm: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroundedAnswer {
    pub answer: String,
    pub citations: Vec<HybridSearchHit>,
    pub mode: String,
    pub grounded: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexMetadata {
    pub mode: String,
    pub provider: String,
    pub model: String,
    pub dimensions: usize,
    pub signature: String,
    /// Core intelligence durable ID scheme (`sha256-v1`). Empty/missing = legacy → rebuild.
    #[serde(default)]
    pub id_scheme: String,
}

#[cfg(test)]
mod tests {
    use super::{HybridAskRequest, IndexBuildResult};

    #[test]
    fn serializes_public_contract_as_camel_case() {
        let request = HybridAskRequest {
            source_rels: vec!["doc.pdf".into()],
            question: "Nội dung?".into(),
            top_k: Some(8),
            use_llm: Some(false),
        };
        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["sourceRels"][0], "doc.pdf");
        assert_eq!(value["topK"], 8);
        assert!(value.get("top_k").is_none());

        let result: IndexBuildResult = serde_json::from_value(serde_json::json!({
            "documents": 1,
            "chunks": 2,
            "indexed": 1,
            "skipped": 0,
            "embeddingMode": "local_hash_v1",
            "embeddingProvider": "local",
            "embeddingModel": "local_hash_v1",
            "vectorDimensions": 256,
            "warnings": []
        }))
        .unwrap();
        assert_eq!(result.vector_dimensions, 256);
    }
}
