//! Approved OpenAI-compatible embedding runtime for server indexing.
//!
//! The server deliberately has no 256D hash fallback. A missing or invalid
//! approved runtime fails the durable embedding job; lexical retrieval remains
//! available independently while operators repair the runtime.

use std::env;

use fileconv_knowledge::embedding::{
    validate_embedding_batch, EmbeddingPlan, EmbeddingVector, ProviderDeployment,
    RUNTIME_LOCAL_HASH,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::config::Profile;

const ENV_BASE_URL: &str = "MARKHAND_EMBEDDING_BASE_URL";
const ENV_API_KEY: &str = "MARKHAND_EMBEDDING_API_KEY";
const ENV_PROVIDER: &str = "MARKHAND_EMBEDDING_PROVIDER";
const ENV_MODEL: &str = "MARKHAND_EMBEDDING_MODEL";
const ENV_REVISION: &str = "MARKHAND_EMBEDDING_REVISION";
const ENV_DIMENSIONS: &str = "MARKHAND_EMBEDDING_DIMENSIONS";
const ENV_RUNTIME_PATH: &str = "MARKHAND_EMBEDDING_RUNTIME_PATH";
const ENV_ALLOW_CLOUD_EMBEDDINGS: &str = "MARKHAND_ALLOW_CLOUD_EMBEDDINGS";

#[derive(Clone)]
pub struct ApprovedEmbeddingRuntime {
    client: reqwest::Client,
    endpoint: String,
    api_key: String,
    plan: EmbeddingPlan,
}

impl std::fmt::Debug for ApprovedEmbeddingRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ApprovedEmbeddingRuntime")
            .field("endpoint", &"[REDACTED_ENDPOINT]")
            .field("plan", &self.plan)
            .finish_non_exhaustive()
    }
}

impl ApprovedEmbeddingRuntime {
    /// Reads the explicitly approved worker runtime. No provider/model defaults
    /// are permitted because those values are part of the index signature.
    pub fn from_env(
        approved_signature: Option<&str>,
        profile: Profile,
    ) -> Result<Self, EmbeddingError> {
        let base_url = required_env(ENV_BASE_URL)?;
        let api_key = required_env(ENV_API_KEY)?;
        let provider = env::var(ENV_PROVIDER).unwrap_or_else(|_| "openai-compatible".into());
        let model = required_env(ENV_MODEL)?;
        let revision = required_env(ENV_REVISION)?;
        let dimensions = required_env(ENV_DIMENSIONS)?
            .parse::<usize>()
            .map_err(|_| EmbeddingError::InvalidConfiguration(ENV_DIMENSIONS))?;
        let runtime_path = required_env(ENV_RUNTIME_PATH)?;
        let allow_cloud_embeddings = allow_cloud_embeddings_from_env()?;
        Self::new(
            base_url,
            api_key,
            provider,
            model,
            revision,
            dimensions,
            runtime_path,
            profile,
            allow_cloud_embeddings,
            approved_signature,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        base_url: String,
        api_key: String,
        provider: String,
        model: String,
        revision: String,
        dimensions: usize,
        runtime_path: String,
        profile: Profile,
        allow_cloud_embeddings: bool,
        approved_signature: Option<&str>,
    ) -> Result<Self, EmbeddingError> {
        if api_key.trim().is_empty() {
            return Err(EmbeddingError::InvalidConfiguration(ENV_API_KEY));
        }
        if runtime_path == RUNTIME_LOCAL_HASH {
            return Err(EmbeddingError::UnapprovedRuntime);
        }
        validate_runtime_policy(&runtime_path, profile, allow_cloud_embeddings)?;
        let deployment = ProviderDeployment::from_base_url(Some(&base_url))?;
        let plan = EmbeddingPlan::provider(
            provider,
            model,
            revision,
            deployment,
            Some(dimensions),
            runtime_path,
        )?;
        let signature = plan.index_signature(dimensions)?.digest();
        if let Some(approved) = approved_signature {
            if approved != signature {
                return Err(EmbeddingError::SignatureMismatch);
            }
        }
        let endpoint = format!("{}/embeddings", base_url.trim_end_matches('/'));
        let client = reqwest::Client::builder()
            .build()
            .map_err(|_| EmbeddingError::Http)?;
        Ok(Self {
            client,
            endpoint,
            api_key,
            plan,
        })
    }

    pub fn plan(&self) -> &EmbeddingPlan {
        &self.plan
    }

    /// Canonical server/desktop-compatible input: `{heading}\n{body}`.
    pub fn canonical_input(heading: &str, body: &str) -> String {
        format!("{heading}\n{body}")
    }

    pub async fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        let request = ProviderRequest {
            model: self.plan.model(),
            input: inputs,
            encoding_format: "float",
        };
        let response = self
            .client
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .json(&request)
            .send()
            .await
            .map_err(|_| EmbeddingError::Http)?
            .error_for_status()
            .map_err(|_| EmbeddingError::Http)?
            .json::<ProviderResponse>()
            .await
            .map_err(|_| EmbeddingError::InvalidResponse)?;
        validate_response(response, inputs.len(), &self.plan)
    }

    /// Fail-closed readiness probe: one tiny embedding request must succeed.
    pub async fn health_probe(&self) -> Result<(), EmbeddingError> {
        let vectors = self.embed(&[String::from("markhand-ready")]).await?;
        if vectors.len() != 1 {
            return Err(EmbeddingError::InvalidResponse);
        }
        Ok(())
    }
}

fn allow_cloud_embeddings_from_env() -> Result<bool, EmbeddingError> {
    match env::var(ENV_ALLOW_CLOUD_EMBEDDINGS) {
        Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Ok(true),
            "0" | "false" | "no" | "off" | "" => Ok(false),
            _ => Err(EmbeddingError::InvalidConfiguration(
                ENV_ALLOW_CLOUD_EMBEDDINGS,
            )),
        },
        Err(_) => Ok(false),
    }
}

fn validate_runtime_policy(
    runtime_path: &str,
    profile: Profile,
    allow_cloud_embeddings: bool,
) -> Result<(), EmbeddingError> {
    if matches!(runtime_path, "vllm-local" | "local-neural") {
        return Ok(());
    }
    if profile == Profile::Dev && allow_cloud_embeddings {
        return Ok(());
    }
    Err(EmbeddingError::CloudRuntimeNotAllowed)
}

/// Stable checksum persisted with an embedding-batch job. It covers input
/// boundaries, preventing concatenation ambiguity and detecting stale chunks.
pub fn canonical_inputs_sha256(inputs: &[String]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"markhand-embedding-input-v1");
    for input in inputs {
        hasher.update((input.len() as u64).to_be_bytes());
        hasher.update(input.as_bytes());
    }
    hex::encode(hasher.finalize())
}

fn required_env(name: &'static str) -> Result<String, EmbeddingError> {
    env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or(EmbeddingError::InvalidConfiguration(name))
}

#[derive(Serialize)]
struct ProviderRequest<'a> {
    model: &'a str,
    input: &'a [String],
    encoding_format: &'static str,
}

#[derive(Deserialize)]
struct ProviderResponse {
    data: Vec<ProviderVector>,
}

#[derive(Deserialize)]
struct ProviderVector {
    index: usize,
    embedding: Vec<f32>,
}

fn validate_response(
    response: ProviderResponse,
    expected_count: usize,
    plan: &EmbeddingPlan,
) -> Result<Vec<Vec<f32>>, EmbeddingError> {
    if response.data.len() != expected_count {
        return Err(EmbeddingError::ResponseCount {
            expected: expected_count,
            actual: response.data.len(),
        });
    }
    let mut ordered = vec![None; expected_count];
    for item in response.data {
        let slot = ordered
            .get_mut(item.index)
            .ok_or(EmbeddingError::InvalidResponse)?;
        if slot.replace(item.embedding).is_some() {
            return Err(EmbeddingError::InvalidResponse);
        }
    }
    let vectors = ordered
        .into_iter()
        .map(|vector| {
            vector
                .ok_or(EmbeddingError::InvalidResponse)
                .and_then(|values| EmbeddingVector::new(values).map_err(EmbeddingError::from))
        })
        .collect::<Result<Vec<_>, _>>()?;
    validate_embedding_batch(&vectors, expected_count, plan.expected_dimensions())?;
    if vectors
        .iter()
        .any(|vector| !is_l2_normalized(vector.values()))
    {
        return Err(EmbeddingError::NotNormalized);
    }
    Ok(vectors
        .into_iter()
        .map(EmbeddingVector::into_values)
        .collect())
}

fn is_l2_normalized(values: &[f32]) -> bool {
    let norm = values.iter().map(|value| value * value).sum::<f32>().sqrt();
    norm.is_finite() && (norm - 1.0).abs() <= 0.001
}

#[derive(Debug, Error)]
pub enum EmbeddingError {
    #[error("embedding runtime configuration is missing or invalid: {0}")]
    InvalidConfiguration(&'static str),
    #[error("configured index signature does not match approved embedding runtime")]
    SignatureMismatch,
    #[error("the local hash embedding runtime is not approved for server indexing")]
    UnapprovedRuntime,
    #[error(
        "cloud embedding runtimes require MARKHAND_PROFILE=dev and MARKHAND_ALLOW_CLOUD_EMBEDDINGS=true"
    )]
    CloudRuntimeNotAllowed,
    #[error("embedding provider request failed")]
    Http,
    #[error("embedding provider returned an invalid response")]
    InvalidResponse,
    #[error("embedding provider returned {actual} vectors; expected {expected}")]
    ResponseCount { expected: usize, actual: usize },
    #[error("embedding validation failed")]
    Knowledge(#[from] fileconv_knowledge::KnowledgeError),
    #[error("embedding vector is not L2-normalized")]
    NotNormalized,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Profile;
    use fileconv_knowledge::embedding::{ProviderDeployment, RUNTIME_VLLM_LOCAL};

    #[test]
    fn canonical_input_includes_heading_and_body_with_fixed_boundary() {
        assert_eq!(
            ApprovedEmbeddingRuntime::canonical_input("Chương I > Điều 1", "Nội dung"),
            "Chương I > Điều 1\nNội dung"
        );
        assert_eq!(
            ApprovedEmbeddingRuntime::canonical_input("", "Nội dung"),
            "\nNội dung"
        );
    }

    #[test]
    fn response_validation_orders_provider_indices_and_rejects_duplicates() {
        let plan = EmbeddingPlan::provider(
            "mock",
            "model",
            "r1",
            ProviderDeployment::from_base_url(Some("http://embedding.test/v1")).unwrap(),
            Some(2),
            RUNTIME_VLLM_LOCAL,
        )
        .unwrap();
        let valid = ProviderResponse {
            data: vec![
                ProviderVector {
                    index: 1,
                    embedding: vec![0.0, 1.0],
                },
                ProviderVector {
                    index: 0,
                    embedding: vec![1.0, 0.0],
                },
            ],
        };
        assert_eq!(
            validate_response(valid, 2, &plan).unwrap(),
            vec![vec![1.0, 0.0], vec![0.0, 1.0]]
        );

        let duplicate = ProviderResponse {
            data: vec![
                ProviderVector {
                    index: 0,
                    embedding: vec![1.0, 0.0],
                },
                ProviderVector {
                    index: 0,
                    embedding: vec![0.0, 1.0],
                },
            ],
        };
        assert!(matches!(
            validate_response(duplicate, 2, &plan),
            Err(EmbeddingError::InvalidResponse)
        ));
    }

    #[test]
    fn input_checksum_is_boundary_sensitive() {
        assert_ne!(
            canonical_inputs_sha256(&["ab".into(), "c".into()]),
            canonical_inputs_sha256(&["a".into(), "bc".into()])
        );
    }

    #[test]
    fn rejects_the_hash_runtime_even_when_other_fields_are_present() {
        assert!(matches!(
            ApprovedEmbeddingRuntime::new(
                "http://embedding.test/v1".into(),
                "key".into(),
                "mock".into(),
                "model".into(),
                "r1".into(),
                8,
                RUNTIME_LOCAL_HASH.into(),
                Profile::Test,
                false,
                None,
            ),
            Err(EmbeddingError::UnapprovedRuntime)
        ));
    }

    #[test]
    fn cloud_embedding_runtime_requires_an_explicit_development_override() {
        let config = || {
            (
                "http://embedding.test/v1".into(),
                "key".into(),
                "mock".into(),
                "model".into(),
                "r1".into(),
                8,
                "provider-cloud".into(),
            )
        };
        let (base_url, api_key, provider, model, revision, dimensions, runtime_path) = config();
        assert!(matches!(
            ApprovedEmbeddingRuntime::new(
                base_url,
                api_key,
                provider,
                model,
                revision,
                dimensions,
                runtime_path,
                Profile::Prod,
                true,
                None,
            ),
            Err(EmbeddingError::CloudRuntimeNotAllowed)
        ));
        let (base_url, api_key, provider, model, revision, dimensions, runtime_path) = config();
        assert!(matches!(
            ApprovedEmbeddingRuntime::new(
                base_url,
                api_key,
                provider,
                model,
                revision,
                dimensions,
                runtime_path,
                Profile::Dev,
                false,
                None,
            ),
            Err(EmbeddingError::CloudRuntimeNotAllowed)
        ));
        let (base_url, api_key, provider, model, revision, dimensions, runtime_path) = config();
        assert!(ApprovedEmbeddingRuntime::new(
            base_url,
            api_key,
            provider,
            model,
            revision,
            dimensions,
            runtime_path,
            Profile::Dev,
            true,
            None,
        )
        .is_ok());
    }
}
