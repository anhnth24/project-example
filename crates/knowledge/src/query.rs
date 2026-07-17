use crate::{KnowledgeError, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchQuery {
    pub text: String,
    pub source_scope: Vec<String>,
    pub limit: usize,
}

impl SearchQuery {
    pub fn new(text: impl Into<String>, source_scope: Vec<String>, limit: usize) -> Result<Self> {
        let text = text.into();
        if text.trim().is_empty() {
            return Err(KnowledgeError::InvalidInput("query is empty"));
        }
        if !(1..=100).contains(&limit) {
            return Err(KnowledgeError::InvalidInput("query limit must be 1..=100"));
        }
        Ok(Self {
            text,
            source_scope,
            limit,
        })
    }
}
