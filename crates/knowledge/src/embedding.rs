use std::hash::{Hash, Hasher};

use sha2::{Digest, Sha256};
use siphasher::sip::SipHasher13;

use crate::identity::IndexSignature;
use crate::query::PreparedQuery;
use crate::{KnowledgeError, Result};

pub const LOCAL_VECTOR_DIMENSIONS: usize = 256;
pub const LOCAL_EMBEDDING_MODE: &str = "local_hash_v1";
pub const PROVIDER_EMBEDDING_MODE: &str = "provider_v1";
// Re-export identity pins (historical importers + local use).
pub use crate::identity::{
    BODY_TEXT_VERSION as EMBEDDING_BODY_TEXT_VERSION,
    DEFAULT_CHUNKING_VERSION as EMBEDDING_CHUNKING_VERSION,
    QUERY_NORMALIZATION_VERSION as EMBEDDING_QUERY_NORMALIZATION_VERSION,
};
pub use crate::identity::{
    BODY_TEXT_VERSION, DEFAULT_CHUNKING_VERSION, QUERY_NORMALIZATION_VERSION,
    RUNTIME_GLM_CLOUD_INTERIM, RUNTIME_LOCAL_HASH, RUNTIME_LOCAL_NEURAL, RUNTIME_PROVIDER_CLOUD,
    RUNTIME_VLLM_LOCAL,
};

/// Map endpoint metadata to a canonical runtime path (ADR 0006).
///
/// Re-export of always-on [`fileconv_core::embedding_runtime::infer_embedding_runtime_path`]
/// (not behind core `llm`). Fallback only — desktop presets carry an explicit
/// `runtime_path` because real vLLM hosts (`127.0.0.1:8000` + `BAAI/bge-m3`) do
/// not contain a `vllm` DNS label.
pub use fileconv_core::embedding_runtime::infer_embedding_runtime_path as infer_runtime_path;

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
    fn embed(&self, inputs: &[String]) -> Result<Vec<EmbeddingVector>>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDeployment {
    digest: String,
}

impl ProviderDeployment {
    pub fn from_base_url(base_url: Option<&str>) -> Result<Self> {
        let canonical = match base_url {
            Some(value) => {
                let mut url = url::Url::parse(value)
                    .map_err(|_| KnowledgeError::InvalidInput("embedding base URL is invalid"))?;
                if !matches!(url.scheme(), "http" | "https") {
                    return Err(KnowledgeError::InvalidInput(
                        "embedding base URL scheme is unsupported",
                    ));
                }
                url.set_username("").map_err(|_| {
                    KnowledgeError::InvalidInput("embedding base URL credentials are invalid")
                })?;
                url.set_password(None).map_err(|_| {
                    KnowledgeError::InvalidInput("embedding base URL credentials are invalid")
                })?;
                url.set_query(None);
                url.set_fragment(None);
                let normalized_path = url.path().trim_end_matches('/').to_string();
                url.set_path(if normalized_path.is_empty() {
                    "/"
                } else {
                    &normalized_path
                });
                url.to_string()
            }
            None => "provider-default".to_string(),
        };
        let digest = Sha256::digest(canonical.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        Ok(Self { digest })
    }
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
    family: String,
    revision: String,
    expected_dimensions: Option<usize>,
    normalized: bool,
    runtime_path: String,
}

impl EmbeddingPlan {
    pub fn local_hash_v1() -> Self {
        let deployment =
            ProviderDeployment::from_base_url(None).expect("default deployment identity is valid");
        let family = embedding_family("local", LOCAL_EMBEDDING_MODE, &deployment);
        Self {
            mode: LOCAL_EMBEDDING_MODE,
            provider: "local".into(),
            model: LOCAL_EMBEDDING_MODE.into(),
            family,
            revision: "1".into(),
            expected_dimensions: Some(LOCAL_VECTOR_DIMENSIONS),
            normalized: true,
            runtime_path: RUNTIME_LOCAL_HASH.into(),
        }
    }

    pub fn provider(
        provider: impl Into<String>,
        model: impl Into<String>,
        revision: impl Into<String>,
        deployment: ProviderDeployment,
        expected_dimensions: Option<usize>,
        runtime_path: impl Into<String>,
    ) -> Result<Self> {
        let provider = provider.into();
        let model = model.into();
        let revision = revision.into();
        let runtime_path = runtime_path.into();
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
        if !fileconv_core::embedding_runtime::is_allowed_embedding_runtime_path(&runtime_path) {
            return Err(KnowledgeError::InvalidInput(
                "embedding runtime_path is unsupported",
            ));
        }
        let family = embedding_family(&provider, &model, &deployment);
        Ok(Self {
            mode: PROVIDER_EMBEDDING_MODE,
            provider,
            model,
            family,
            revision,
            expected_dimensions,
            normalized: true,
            runtime_path,
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

    pub fn runtime_path(&self) -> &str {
        &self.runtime_path
    }

    pub fn expected_dimensions(&self) -> Option<usize> {
        self.expected_dimensions
    }

    pub fn signature(&self, actual_dimensions: usize) -> Result<String> {
        Ok(self.index_signature(actual_dimensions)?.digest())
    }

    pub fn index_signature(&self, actual_dimensions: usize) -> Result<IndexSignature<'_>> {
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
        Ok(self.index_signature_unchecked(actual_dimensions))
    }

    /// Planning-only signature used before a provider reports its dimensions.
    ///
    /// It must not be persisted as index metadata; once vectors arrive callers
    /// replace it with [`Self::signature`].
    pub fn provisional_signature(&self) -> String {
        self.index_signature_unchecked(self.expected_dimensions.unwrap_or(0))
            .digest()
    }

    fn index_signature_unchecked(&self, dimensions: usize) -> IndexSignature<'_> {
        IndexSignature {
            runtime_path: &self.runtime_path,
            embedding_family: &self.family,
            embedding_revision: &self.revision,
            dimensions,
            normalized: self.normalized,
            chunking_version: DEFAULT_CHUNKING_VERSION,
            body_text_version: BODY_TEXT_VERSION,
            query_normalization_version: QUERY_NORMALIZATION_VERSION,
        }
    }
}

fn embedding_family(provider: &str, model: &str, deployment: &ProviderDeployment) -> String {
    format!("{provider}/{model}/{}", deployment.digest)
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
    if plan.normalized
        && vectors.iter().any(|vector| {
            let norm = vector
                .values()
                .iter()
                .map(|value| value * value)
                .sum::<f32>()
                .sqrt();
            (norm - 1.0).abs() > 0.001
        })
    {
        return Err(KnowledgeError::InvalidInput(
            "embedding vector is not L2-normalized",
        ));
    }
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
    // `DefaultHasher::new()` used SipHash-1-3 with zero keys in the original
    // desktop implementation. Naming the algorithm makes local_hash_v1 stable.
    let mut hasher = SipHasher13::new();
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
        vectors: Vec<EmbeddingVector>,
    }

    impl EmbeddingProvider for MockProvider {
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

        let punctuation = local_vector("...");
        assert_eq!(punctuation, local_vector("..."));
        assert!(punctuation.values().iter().all(|value| value.is_finite()));
        let sparse_bits = first
            .values()
            .iter()
            .enumerate()
            .filter(|(_, value)| **value != 0.0)
            .map(|(index, value)| (index, value.to_bits()))
            .collect::<Vec<_>>();
        assert_eq!(
            sparse_bits,
            [
                (3, 1031655018),
                (15, 1049197177),
                (24, 1031655018),
                (26, 3179138666),
                (45, 3196680825),
                (46, 3179138666),
                (97, 1031655018),
                (111, 1031655018),
                (121, 1031655018),
                (132, 3179138666),
                (135, 3179138666),
                (141, 1054048600),
                (160, 3203611429),
                (170, 1049197177),
                (188, 1031655018),
                (191, 1054048600),
                (195, 3179138666),
                (212, 1031655018),
                (229, 1054048600),
            ]
        );
    }

    #[test]
    fn local_hash_v1_matches_legacy_desktop_hasher() {
        use std::hash::{Hash, Hasher};

        for feature in ["doi", "soat", "giao:dich", "gia"] {
            let mut legacy = std::collections::hash_map::DefaultHasher::new();
            feature.hash(&mut legacy);
            let mut stable = siphasher::sip::SipHasher13::new();
            feature.hash(&mut stable);
            assert_eq!(stable.finish(), legacy.finish());
        }
    }

    #[test]
    fn validates_provider_mock_count_and_dimensions() {
        let deployment =
            super::ProviderDeployment::from_base_url(Some("http://embedding.internal")).unwrap();
        let plan = EmbeddingPlan::provider(
            "vllm",
            "vi-model",
            "r1",
            deployment,
            Some(3),
            super::RUNTIME_VLLM_LOCAL,
        )
        .unwrap();
        let provider = MockProvider {
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
        let unnormalized = MockProvider {
            vectors: vec![EmbeddingVector::new(vec![2.0, 0.0, 0.0]).unwrap()],
        };
        assert_eq!(
            embed_checked(&unnormalized, &["một".into()], &plan).unwrap_err(),
            KnowledgeError::InvalidInput("embedding vector is not L2-normalized")
        );
    }

    #[test]
    fn provider_signature_is_secret_free_and_covers_compatibility_fields() {
        let deployment = super::ProviderDeployment::from_base_url(Some(
            "https://user:secret@embedding.internal/v1?token=hidden",
        ))
        .unwrap();
        let same_deployment =
            super::ProviderDeployment::from_base_url(Some("https://embedding.internal/v1"))
                .unwrap();
        let other_deployment =
            super::ProviderDeployment::from_base_url(Some("https://embedding.other/v1")).unwrap();
        let first = EmbeddingPlan::provider(
            "openai-compatible",
            "vi-model",
            "r1",
            deployment,
            Some(768),
            super::RUNTIME_VLLM_LOCAL,
        )
        .unwrap();
        let same = EmbeddingPlan::provider(
            "openai-compatible",
            "vi-model",
            "r1",
            same_deployment,
            Some(768),
            super::RUNTIME_VLLM_LOCAL,
        )
        .unwrap();
        let changed_endpoint = EmbeddingPlan::provider(
            "openai-compatible",
            "vi-model",
            "r1",
            other_deployment,
            Some(768),
            super::RUNTIME_VLLM_LOCAL,
        )
        .unwrap();
        let changed_model = EmbeddingPlan::provider(
            "openai-compatible",
            "other-model",
            "r1",
            super::ProviderDeployment::from_base_url(None).unwrap(),
            Some(768),
            super::RUNTIME_VLLM_LOCAL,
        )
        .unwrap();
        let changed_dimensions = EmbeddingPlan::provider(
            "openai-compatible",
            "vi-model",
            "r1",
            super::ProviderDeployment::from_base_url(None).unwrap(),
            Some(1024),
            super::RUNTIME_VLLM_LOCAL,
        )
        .unwrap();
        let changed_runtime = EmbeddingPlan::provider(
            "openai-compatible",
            "vi-model",
            "r1",
            super::ProviderDeployment::from_base_url(Some("https://embedding.internal/v1"))
                .unwrap(),
            Some(768),
            super::RUNTIME_GLM_CLOUD_INTERIM,
        )
        .unwrap();
        assert_eq!(first.signature(768).unwrap(), same.signature(768).unwrap());
        assert_ne!(
            first.signature(768).unwrap(),
            changed_endpoint.signature(768).unwrap()
        );
        assert_ne!(
            first.signature(768).unwrap(),
            changed_model.signature(768).unwrap()
        );
        assert_ne!(
            first.signature(768).unwrap(),
            changed_dimensions.signature(1024).unwrap()
        );
        assert_ne!(
            first.signature(768).unwrap(),
            changed_runtime.signature(768).unwrap()
        );
        assert_eq!(
            super::infer_runtime_path(Some("https://open.bigmodel.cn/api/paas/v4"), "embedding-3"),
            super::RUNTIME_GLM_CLOUD_INTERIM
        );
        assert_eq!(
            super::infer_runtime_path(Some("http://vllm.internal:8000/v1"), "bge-m3"),
            super::RUNTIME_VLLM_LOCAL
        );
        let debug = format!("{first:?}");
        assert!(!debug.contains("https://"));
        assert!(!debug.contains("secret"));
        assert!(!debug.contains("hidden"));
    }

    #[test]
    fn plan_index_signature_matches_digest() {
        let plan = EmbeddingPlan::local_hash_v1();
        let signature = plan.index_signature(LOCAL_VECTOR_DIMENSIONS).unwrap();
        assert_eq!(
            signature.digest(),
            plan.signature(LOCAL_VECTOR_DIMENSIONS).unwrap()
        );
        assert_eq!(signature.runtime_path, super::RUNTIME_LOCAL_HASH);
        assert_eq!(signature.dimensions, LOCAL_VECTOR_DIMENSIONS);
    }

    #[test]
    fn infer_runtime_path_literal_cases() {
        let cases: &[(Option<&str>, &str, &str)] = &[
            (None, "text-embedding-3-small", "provider-cloud"),
            (
                Some("http://127.0.0.1:8000"),
                "BAAI/bge-m3",
                "provider-cloud",
            ),
            (
                Some("https://open.bigmodel.cn/api/paas/v4"),
                "embedding-3",
                "glm-cloud-interim",
            ),
            (
                Some("open.bigmodel.cn/api/paas/v4"),
                "custom",
                "glm-cloud-interim",
            ),
            (Some("https://api.z.ai/v1"), "embed", "glm-cloud-interim"),
            (Some("https://modelz.ai/v1"), "embed", "provider-cloud"),
            (Some("http://vllm.internal:8000/v1"), "bge-m3", "vllm-local"),
            (
                Some("http://vllm.internal:8000/v1"),
                "glm-embedding",
                "vllm-local",
            ),
            (
                Some("http://vllm.bigmodel.cn/v1"),
                "bge-m3",
                "glm-cloud-interim",
            ),
            (Some("http://[vllm::1]:8000/v1"), "bge-m3", "provider-cloud"),
            (
                Some(r"https://evil.com\@open.bigmodel.cn/v1"),
                "bge-m3",
                "provider-cloud",
            ),
            (
                Some(r"https://evil\bigmodel.cn@127.0.0.1/v1"),
                "bge-m3",
                "provider-cloud",
            ),
            (None, "embedding-3", "glm-cloud-interim"),
            (None, "vllm-served-model", "vllm-local"),
            (Some("ftp://vllm.internal/v1"), "bge-m3", "provider-cloud"),
            (
                Some("https://api.openai.com/v1"),
                "embedding-3",
                "provider-cloud",
            ),
        ];
        for (base_url, model, expected) in cases {
            assert_eq!(
                super::infer_runtime_path(*base_url, model),
                *expected,
                "knowledge infer_runtime_path base_url={base_url:?} model={model}"
            );
        }
    }

    #[test]
    fn runtime_constants_alias_core_embedding_runtime() {
        use fileconv_core::embedding_runtime::{
            EMBEDDING_RUNTIME_GLM_CLOUD_INTERIM, EMBEDDING_RUNTIME_LOCAL_HASH,
            EMBEDDING_RUNTIME_LOCAL_NEURAL, EMBEDDING_RUNTIME_PROVIDER_CLOUD,
            EMBEDDING_RUNTIME_VLLM_LOCAL,
        };
        assert_eq!(super::RUNTIME_LOCAL_HASH, EMBEDDING_RUNTIME_LOCAL_HASH);
        assert_eq!(super::RUNTIME_LOCAL_NEURAL, EMBEDDING_RUNTIME_LOCAL_NEURAL);
        assert_eq!(
            super::RUNTIME_GLM_CLOUD_INTERIM,
            EMBEDDING_RUNTIME_GLM_CLOUD_INTERIM
        );
        assert_eq!(super::RUNTIME_VLLM_LOCAL, EMBEDDING_RUNTIME_VLLM_LOCAL);
        assert_eq!(
            super::RUNTIME_PROVIDER_CLOUD,
            EMBEDDING_RUNTIME_PROVIDER_CLOUD
        );
    }
}
