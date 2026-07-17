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
