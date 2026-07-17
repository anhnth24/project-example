use crate::{KnowledgeError, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CitationAnchor {
    pub source_rel: String,
    pub start: usize,
    pub end: usize,
}

impl CitationAnchor {
    pub fn validate(&self) -> Result<()> {
        if self.source_rel.trim().is_empty() {
            return Err(KnowledgeError::InvalidInput("citation source is empty"));
        }
        if self.start >= self.end {
            return Err(KnowledgeError::InvalidInput(
                "citation range must be non-empty",
            ));
        }
        Ok(())
    }
}
