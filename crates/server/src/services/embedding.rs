//! Approved local embedding service for the index worker.

use fileconv_knowledge::embedding::{
    local_vector, validate_embedding_batch, EmbeddingPlan, LOCAL_VECTOR_DIMENSIONS,
};
use thiserror::Error;

pub fn approved_plan() -> EmbeddingPlan {
    EmbeddingPlan::local_hash_v1()
}

pub fn embed_bodies(bodies: &[String]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
    let vectors = bodies
        .iter()
        .map(|body| local_vector(body))
        .collect::<Vec<_>>();
    validate_embedding_batch(&vectors, bodies.len(), Some(LOCAL_VECTOR_DIMENSIONS))?;
    if vectors
        .iter()
        .any(|vector| !is_l2_normalized(vector.values()))
    {
        return Err(EmbeddingError::NotNormalized);
    }
    Ok(vectors
        .into_iter()
        .map(|vector| vector.into_values())
        .collect())
}

fn is_l2_normalized(values: &[f32]) -> bool {
    let norm = values.iter().map(|value| value * value).sum::<f32>().sqrt();
    norm.is_finite() && (norm - 1.0).abs() <= 0.001
}

#[derive(Debug, Error)]
pub enum EmbeddingError {
    #[error("embedding validation failed")]
    Knowledge(#[from] fileconv_knowledge::KnowledgeError),
    #[error("embedding vector is not L2-normalized")]
    NotNormalized,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_embedding_is_deterministic_and_256_dimensional() {
        let bodies = vec![
            "đối soát giao dịch".to_string(),
            "nội dung khác".to_string(),
        ];
        let first = embed_bodies(&bodies).unwrap();
        let second = embed_bodies(&bodies).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.len(), 2);
        assert!(first
            .iter()
            .all(|vector| vector.len() == LOCAL_VECTOR_DIMENSIONS));
    }
}
