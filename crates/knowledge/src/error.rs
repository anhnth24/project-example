#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum KnowledgeError {
    #[error("invalid knowledge input: {0}")]
    InvalidInput(&'static str),
    #[error("knowledge index is incompatible: {0}")]
    IncompatibleIndex(&'static str),
    #[error("embedding count mismatch: expected {expected}, received {actual}")]
    EmbeddingCountMismatch { expected: usize, actual: usize },
    #[error("embedding dimension mismatch: expected {expected}, received {actual}")]
    EmbeddingDimensionMismatch { expected: usize, actual: usize },
    #[error("knowledge adapter is unavailable: {0}")]
    AdapterUnavailable(&'static str),
    #[error("knowledge adapter failed: {0}")]
    AdapterFailure(String),
}

pub type Result<T> = std::result::Result<T, KnowledgeError>;
