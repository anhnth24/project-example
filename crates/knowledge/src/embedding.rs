use std::hash::{Hash, Hasher};

use crate::identity::IndexSignature;
use crate::query::PreparedQuery;
use crate::{KnowledgeError, Result};

pub const LOCAL_VECTOR_DIMENSIONS: usize = 256;
pub const LOCAL_EMBEDDING_MODE: &str = "local_hash_v1";
pub const PROVIDER_EMBEDDING_MODE: &str = "provider_v1";
pub const DEFAULT_CHUNKING_VERSION: &str = "heading-chunks-2000-v1";
pub const QUERY_NORMALIZATION_VERSION: &str = "accent-fold-v1";

#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddingVector {
    values: Vec<f32>,
}

impl EmbeddingVector {
    pub fn new(values: Vec<f32>) -> Result<Self> {
        if values.is_empty() {
            return Err(KnowledgeError::InvalidInput("embedding vector is empty"));
        }
        if values.iter().any(|value| !value.is_finite()) {
            return Err(KnowledgeError::InvalidInput(
                "embedding vector contains non-finite values",
            ));
        }
        Ok(Self { values })
    }

    pub fn values(&self) -> &[f32] {
        &self.values
    }

    pub fn dimensions(&self) -> usize {
        self.values.len()
    }

    pub fn into_values(self) -> Vec<f32> {
        self.values
    }
}

pub trait EmbeddingProvider {
    fn signature(&self) -> &str;
    fn embed(&self, inputs: &[String]) -> Result<Vec<EmbeddingVector>>;
}

/// Secret-free description of how vectors in an index are produced.
///
/// Transport URLs and credentials deliberately cannot be represented here.
/// HTTP clients remain in `fileconv-core`; callers map their configuration to
/// this plan before validating provider output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingPlan {
    mode: &'static str,
    provider: String,
    model: String,
    revision: String,
    expected_dimensions: Option<usize>,
    normalized: bool,
}

impl EmbeddingPlan {
    pub fn local_hash_v1() -> Self {
        Self {
            mode: LOCAL_EMBEDDING_MODE,
            provider: "local".into(),
            model: LOCAL_EMBEDDING_MODE.into(),
            revision: "1".into(),
            expected_dimensions: Some(LOCAL_VECTOR_DIMENSIONS),
            normalized: true,
        }
    }

    pub fn provider(
        provider: impl Into<String>,
        model: impl Into<String>,
        revision: impl Into<String>,
        expected_dimensions: Option<usize>,
    ) -> Result<Self> {
        let provider = provider.into();
        let model = model.into();
        let revision = revision.into();
        if provider.trim().is_empty() {
            return Err(KnowledgeError::InvalidInput("embedding provider is empty"));
        }
        if model.trim().is_empty() {
            return Err(KnowledgeError::InvalidInput("embedding model is empty"));
        }
        if revision.trim().is_empty() {
            return Err(KnowledgeError::InvalidInput("embedding revision is empty"));
        }
        if expected_dimensions == Some(0) {
            return Err(KnowledgeError::InvalidInput(
                "embedding dimensions must be positive",
            ));
        }
        Ok(Self {
            mode: PROVIDER_EMBEDDING_MODE,
            provider,
            model,
            revision,
            expected_dimensions,
            normalized: true,
        })
    }

    pub fn mode(&self) -> &'static str {
        self.mode
    }

    pub fn provider_name(&self) -> &str {
        &self.provider
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn expected_dimensions(&self) -> Option<usize> {
        self.expected_dimensions
    }

    pub fn signature(&self, actual_dimensions: usize) -> Result<String> {
        if actual_dimensions == 0 {
            return Err(KnowledgeError::InvalidInput(
                "embedding dimensions must be positive",
            ));
        }
        if let Some(expected) = self.expected_dimensions {
            if expected != actual_dimensions {
                return Err(KnowledgeError::EmbeddingDimensionMismatch {
                    expected,
                    actual: actual_dimensions,
                });
            }
        }
        Ok(self.signature_with_dimensions(actual_dimensions))
    }

    /// Planning-only signature used before a provider reports its dimensions.
    ///
    /// It must not be persisted as index metadata; once vectors arrive callers
    /// replace it with [`Self::signature`].
    pub fn provisional_signature(&self) -> String {
        self.signature_with_dimensions(self.expected_dimensions.unwrap_or(0))
    }

    fn signature_with_dimensions(&self, dimensions: usize) -> String {
        let family = format!("{}/{}", self.provider, self.model);
        IndexSignature {
            embedding_family: &family,
            embedding_revision: &self.revision,
            dimensions,
            normalized: self.normalized,
            chunking_version: DEFAULT_CHUNKING_VERSION,
            text_version: QUERY_NORMALIZATION_VERSION,
        }
        .digest()
    }
}

pub fn validate_embedding_batch(
    vectors: &[EmbeddingVector],
    expected_count: usize,
    expected_dimensions: Option<usize>,
) -> Result<usize> {
    if vectors.len() != expected_count {
        return Err(KnowledgeError::EmbeddingCountMismatch {
            expected: expected_count,
            actual: vectors.len(),
        });
    }
    if vectors.is_empty() {
        return Ok(expected_dimensions.unwrap_or(0));
    }
    let dimensions = vectors[0].dimensions();
    for vector in vectors.iter().skip(1) {
        if vector.dimensions() != dimensions {
            return Err(KnowledgeError::EmbeddingDimensionMismatch {
                expected: dimensions,
                actual: vector.dimensions(),
            });
        }
    }
    if let Some(expected) = expected_dimensions {
        if dimensions != expected {
            return Err(KnowledgeError::EmbeddingDimensionMismatch {
                expected,
                actual: dimensions,
            });
        }
    }
    Ok(dimensions)
}

pub fn embed_checked(
    provider: &impl EmbeddingProvider,
    inputs: &[String],
    plan: &EmbeddingPlan,
) -> Result<Vec<EmbeddingVector>> {
    let vectors = provider.embed(inputs)?;
    validate_embedding_batch(&vectors, inputs.len(), plan.expected_dimensions())?;
    Ok(vectors)
}

/// Desktop-compatible, deterministic local feature-hashing fallback.
pub fn local_vector(text: &str) -> EmbeddingVector {
    let query = PreparedQuery::new(text);
    let mut vector = vec![0.0_f32; LOCAL_VECTOR_DIMENSIONS];
    for token in &query.tokens {
        add_feature(&mut vector, token, 1.0);
    }
    for pair in query.tokens.windows(2) {
        add_feature(&mut vector, &format!("{}:{}", pair[0], pair[1]), 0.65);
    }
    let compact: Vec<char> = query
        .normalized
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    for trigram in compact.windows(3) {
        add_feature(&mut vector, &trigram.iter().collect::<String>(), 0.15);
    }
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in &mut vector {
            *value /= norm;
        }
    }
    // The fixed-size, finite local vector always satisfies this invariant.
    EmbeddingVector::new(vector).expect("local embedding vector is valid")
}

fn add_feature(vector: &mut [f32], feature: &str, weight: f32) {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    feature.hash(&mut hasher);
    let hash = hasher.finish();
    let index = (hash as usize) % vector.len();
    let sign = if hash & (1 << 63) == 0 { 1.0 } else { -1.0 };
    vector[index] += sign * weight;
}

#[cfg(test)]
mod tests {
    use super::{
        embed_checked, local_vector, validate_embedding_batch, EmbeddingPlan, EmbeddingProvider,
        EmbeddingVector, LOCAL_VECTOR_DIMENSIONS,
    };
    use crate::{KnowledgeError, Result};

    struct MockProvider {
        signature: String,
        vectors: Vec<EmbeddingVector>,
    }

    impl EmbeddingProvider for MockProvider {
        fn signature(&self) -> &str {
            &self.signature
        }

        fn embed(&self, _inputs: &[String]) -> Result<Vec<EmbeddingVector>> {
            Ok(self.vectors.clone())
        }
    }

    #[test]
    fn rejects_empty_and_non_finite_vectors() {
        assert!(EmbeddingVector::new(vec![]).is_err());
        assert!(EmbeddingVector::new(vec![f32::NAN]).is_err());
        assert_eq!(
            EmbeddingVector::new(vec![1.0, 0.0]).unwrap().dimensions(),
            2
        );
    }

    #[test]
    fn local_vectors_preserve_desktop_normalization_and_determinism() {
        let first = local_vector("đối soát giao dịch");
        let second = local_vector("ĐỐI SOÁT GIAO DỊCH");
        assert_eq!(first, second);
        assert_eq!(first.dimensions(), LOCAL_VECTOR_DIMENSIONS);
        let norm = first
            .values()
            .iter()
            .map(|value| value * value)
            .sum::<f32>()
            .sqrt();
        assert!((norm - 1.0).abs() < 0.0001);

        let empty = local_vector("...");
        assert!(empty.values().iter().all(|value| *value == 0.0));
    }

    #[test]
    fn validates_provider_mock_count_and_dimensions() {
        let plan = EmbeddingPlan::provider("vllm", "vi-model", "r1", Some(3)).unwrap();
        let provider = MockProvider {
            signature: plan.signature(3).unwrap(),
            vectors: vec![
                EmbeddingVector::new(vec![1.0, 0.0, 0.0]).unwrap(),
                EmbeddingVector::new(vec![0.0, 1.0, 0.0]).unwrap(),
            ],
        };
        let inputs = vec!["một".into(), "hai".into()];
        assert_eq!(embed_checked(&provider, &inputs, &plan).unwrap().len(), 2);

        let error = validate_embedding_batch(&provider.vectors, 1, Some(3)).unwrap_err();
        assert_eq!(
            error,
            KnowledgeError::EmbeddingCountMismatch {
                expected: 1,
                actual: 2
            }
        );
        let error = validate_embedding_batch(&provider.vectors, 2, Some(4)).unwrap_err();
        assert_eq!(
            error,
            KnowledgeError::EmbeddingDimensionMismatch {
                expected: 4,
                actual: 3
            }
        );
    }

    #[test]
    fn provider_signature_is_secret_free_and_covers_compatibility_fields() {
        let first = EmbeddingPlan::provider("vllm", "vi-model", "r1", Some(768)).unwrap();
        let same = EmbeddingPlan::provider("vllm", "vi-model", "r1", Some(768)).unwrap();
        let changed_model =
            EmbeddingPlan::provider("vllm", "other-model", "r1", Some(768)).unwrap();
        let changed_dimensions =
            EmbeddingPlan::provider("vllm", "vi-model", "r1", Some(1024)).unwrap();
        assert_eq!(first.signature(768).unwrap(), same.signature(768).unwrap());
        assert_ne!(
            first.signature(768).unwrap(),
            changed_model.signature(768).unwrap()
        );
        assert_ne!(
            first.signature(768).unwrap(),
            changed_dimensions.signature(1024).unwrap()
        );
        assert!(!format!("{first:?}").contains("https://"));
    }
}
