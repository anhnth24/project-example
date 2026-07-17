use crate::{KnowledgeError, Result};

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
}

pub trait EmbeddingProvider {
    fn signature(&self) -> &str;
    fn embed(&self, inputs: &[String]) -> Result<Vec<EmbeddingVector>>;
}

#[cfg(test)]
mod tests {
    use super::EmbeddingVector;

    #[test]
    fn rejects_empty_and_non_finite_vectors() {
        assert!(EmbeddingVector::new(vec![]).is_err());
        assert!(EmbeddingVector::new(vec![f32::NAN]).is_err());
        assert_eq!(
            EmbeddingVector::new(vec![1.0, 0.0]).unwrap().dimensions(),
            2
        );
    }
}
