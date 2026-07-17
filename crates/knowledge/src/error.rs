#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum KnowledgeError {
    #[error("invalid knowledge input: {0}")]
    InvalidInput(&'static str),
    #[error("knowledge index is incompatible: {0}")]
    IncompatibleIndex(&'static str),
    #[error("knowledge adapter is unavailable: {0}")]
    AdapterUnavailable(&'static str),
}

pub type Result<T> = std::result::Result<T, KnowledgeError>;
