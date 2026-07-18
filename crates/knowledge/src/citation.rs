use std::collections::HashSet;

use crate::types::SourceAnchor;
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

pub fn infer_source_anchor(
    document_format: &str,
    heading: &str,
    page: Option<u32>,
    start: usize,
    end: usize,
) -> SourceAnchor {
    let normalized = fileconv_core::intelligence::normalize_search_text(heading);
    let slide = normalized
        .split("slide ")
        .nth(1)
        .and_then(|value| value.split_whitespace().next())
        .and_then(|value| value.parse().ok());
    let sheet = (document_format == "xlsx")
        .then(|| heading.split(" > ").last().unwrap_or("").to_string())
        .filter(|value| !value.is_empty());
    SourceAnchor {
        page,
        slide,
        sheet,
        start,
        end,
    }
}

pub fn extract_snippet(body: &str, query_tokens: &[String]) -> String {
    let words: Vec<&str> = body.split_whitespace().collect();
    let normalized_words: Vec<String> = words
        .iter()
        .map(|word| fileconv_core::intelligence::normalize_search_text(word))
        .collect();
    let match_index = normalized_words
        .iter()
        .position(|word| query_tokens.iter().any(|token| word.contains(token)))
        .unwrap_or(0);
    let start = match_index.saturating_sub(12);
    let end = (start + 56).min(words.len());
    words[start..end].join(" ")
}

pub fn validate_grounded_answer(
    answer: &str,
    valid_ids: &HashSet<String>,
) -> std::result::Result<(), Vec<String>> {
    let mut warnings = Vec::new();
    let cited: HashSet<String> = answer
        .split(|character: char| {
            character.is_whitespace() || matches!(character, '[' | ']' | '(' | ')' | ',' | '.')
        })
        .filter(|part| part.starts_with("CITE-"))
        .map(str::to_string)
        .collect();
    if cited.is_empty() {
        warnings.push("LLM không trả citation; đã fallback extractive.".into());
    }
    for citation in cited {
        if !valid_ids.contains(&citation) {
            warnings.push(format!("LLM dùng citation không tồn tại: {citation}"));
        }
    }
    for paragraph in answer.split("\n\n") {
        let factual = paragraph.chars().count() >= 60
            && !paragraph.starts_with('#')
            && !paragraph.starts_with("Câu hỏi:");
        if factual && !paragraph.contains("[CITE-") {
            warnings.push("Có đoạn trả lời dài không gắn citation.".into());
            break;
        }
    }
    if warnings.is_empty() {
        Ok(())
    } else {
        Err(warnings)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{extract_snippet, infer_source_anchor, validate_grounded_answer};

    #[test]
    fn infers_page_slide_and_sheet_without_filesystem_access() {
        let slide = infer_source_anchor("pptx", "Phụ lục > Slide 12", Some(7), 10, 20);
        assert_eq!(slide.page, Some(7));
        assert_eq!(slide.slide, Some(12));
        assert_eq!(slide.sheet, None);

        let sheet = infer_source_anchor("xlsx", "Báo cáo > Quý I", None, 30, 45);
        assert_eq!(sheet.sheet.as_deref(), Some("Quý I"));
        assert_eq!((sheet.start, sheet.end), (30, 45));
    }

    #[test]
    fn snippet_preserves_original_words_and_frozen_window() {
        let body = (0..80)
            .map(|index| {
                if index == 30 {
                    "ĐỐI-SOÁT".to_string()
                } else {
                    format!("w{index}")
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        let snippet = extract_snippet(&body, &["doi".into()]);
        let words = snippet.split_whitespace().collect::<Vec<_>>();
        assert_eq!(words.len(), 56);
        assert_eq!(words[0], "w18");
        assert_eq!(words[12], "ĐỐI-SOÁT");
    }

    #[test]
    fn grounding_rejects_missing_and_invented_citations() {
        let valid = HashSet::from(["CITE-0001".to_string()]);
        assert!(validate_grounded_answer(
            "Nội dung đủ dài nhưng không hề có citation nào ở cuối đoạn để kiểm tra.",
            &valid
        )
        .is_err());
        assert!(validate_grounded_answer(
            "Nội dung factual đủ dài và có citation giả không hợp lệ ở cuối. [CITE-9999]",
            &valid
        )
        .is_err());
        assert!(validate_grounded_answer(
            "Nội dung factual đủ dài, được hỗ trợ bởi nguồn đã retrieval. [CITE-0001]",
            &valid
        )
        .is_ok());
    }
}
