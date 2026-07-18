use crate::{KnowledgeError, Result};

pub const MIN_QUERY_TOKEN_CHARS: usize = 2;

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

/// Query representation shared by lexical and local-vector retrieval.
///
/// The normalized form intentionally delegates to the established core
/// accent-folding implementation so extraction does not change desktop search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedQuery {
    pub normalized: String,
    pub tokens: Vec<String>,
    pub fts5: String,
}

impl PreparedQuery {
    pub fn new(text: &str) -> Self {
        let normalized = fileconv_core::intelligence::normalize_search_text(text);
        let tokens = tokens_from_normalized(&normalized);
        let fts5 = tokens
            .iter()
            .map(|token| format!("\"{token}\"*"))
            .collect::<Vec<_>>()
            .join(" OR ");
        Self {
            normalized,
            tokens,
            fts5,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }
}

pub fn normalized_tokens(text: &str) -> Vec<String> {
    PreparedQuery::new(text).tokens
}

pub fn fts5_prefix_query(text: &str) -> String {
    PreparedQuery::new(text).fts5
}

fn tokens_from_normalized(normalized: &str) -> Vec<String> {
    normalized
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| token.chars().count() >= MIN_QUERY_TOKEN_CHARS)
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{fts5_prefix_query, normalized_tokens, PreparedQuery, SearchQuery};

    #[test]
    fn prepares_vietnamese_query_with_desktop_parity() {
        let query = PreparedQuery::new("Đối soát GIAO DỊCH");
        assert_eq!(query.normalized, "doi soat giao dich");
        assert_eq!(query.tokens, ["doi", "soat", "giao", "dich"]);
        assert_eq!(query.fts5, r#""doi"* OR "soat"* OR "giao"* OR "dich"*"#,);
    }

    #[test]
    fn punctuation_and_fts_operators_are_data_not_syntax() {
        assert_eq!(
            fts5_prefix_query(r#"API: "xác thực" OR (giao dịch) - NEAR"#),
            r#""api"* OR "xac"* OR "thuc"* OR "or"* OR "giao"* OR "dich"* OR "near"*"#,
        );
    }

    #[test]
    fn empty_and_punctuation_only_queries_are_safe() {
        for value in ["", " \n\t ", "\"'()[]{}:;!?.,-"] {
            let query = PreparedQuery::new(value);
            assert!(query.is_empty());
            assert!(query.fts5.is_empty());
            assert!(normalized_tokens(value).is_empty());
        }
        assert!(SearchQuery::new("...", vec![], 10).is_ok());
        assert!(SearchQuery::new(" ", vec![], 10).is_err());
    }
}
