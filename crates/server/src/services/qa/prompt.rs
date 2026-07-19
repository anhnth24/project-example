//! Prompt construction from already-authorized retrieval hits.

use fileconv_knowledge::ask::{grounded_user_prompt, retrieval_context, GROUNDED_SYSTEM_PROMPT};
use fileconv_knowledge::types::{HybridSearchHit, SourceAnchor};

use crate::services::retrieval::GroundedHit;

pub const MAX_PROMPT_CITATIONS: usize = 40;
pub const MAX_PROMPT_SNIPPET_CHARS: usize = 600;

#[derive(Debug, Clone, PartialEq)]
pub struct GroundedPrompt {
    pub system: String,
    pub user: String,
    pub citations: Vec<PromptCitation>,
    pub context_hits: Vec<HybridSearchHit>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PromptCitation {
    pub id: String,
    pub hit: GroundedHit,
}

pub fn build_grounded_prompt(question: &str, hits: &[GroundedHit]) -> GroundedPrompt {
    let citations = hits
        .iter()
        .take(MAX_PROMPT_CITATIONS)
        .enumerate()
        .map(|(index, hit)| PromptCitation {
            id: format!("CITE-{:04}", index + 1),
            hit: capped_hit(hit),
        })
        .collect::<Vec<_>>();
    let context_hits = citations
        .iter()
        .map(|citation| hybrid_hit_from_grounded(&citation.hit))
        .collect::<Vec<_>>();
    let context = retrieval_context(&context_hits);
    GroundedPrompt {
        system: GROUNDED_SYSTEM_PROMPT.to_string(),
        user: grounded_user_prompt(question, &context),
        citations,
        context_hits,
    }
}

pub fn hybrid_hit_from_grounded(hit: &GroundedHit) -> HybridSearchHit {
    HybridSearchHit {
        chunk_id: hit.chunk_id.to_string(),
        source_rel: format!("document:{}", hit.document_id),
        md_rel: format!("version:{}", hit.version_id),
        heading: heading_for_prompt(hit),
        snippet: cap_chars(&hit.snippet, MAX_PROMPT_SNIPPET_CHARS),
        lexical_score: hit.lexical_score,
        vector_score: hit.vector_score,
        rerank_score: hit.rerank_score,
        anchor: SourceAnchor {
            page: non_negative_u32(hit.page),
            slide: non_negative_u32(hit.slide),
            sheet: hit.sheet.clone(),
            start: non_negative_usize(hit.span_start).unwrap_or(0),
            end: non_negative_usize(hit.span_end).unwrap_or(hit.snippet.len()),
        },
    }
}

fn capped_hit(hit: &GroundedHit) -> GroundedHit {
    let mut hit = hit.clone();
    hit.snippet = cap_chars(&hit.snippet, MAX_PROMPT_SNIPPET_CHARS);
    hit
}

fn heading_for_prompt(hit: &GroundedHit) -> String {
    if hit.heading_path.is_empty() {
        format!("chunk:{}", hit.chunk_id)
    } else {
        hit.heading_path.join(" > ")
    }
}

fn cap_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn non_negative_u32(value: Option<i32>) -> Option<u32> {
    value.and_then(|value| u32::try_from(value).ok())
}

fn non_negative_usize(value: Option<i32>) -> Option<usize> {
    value.and_then(|value| usize::try_from(value).ok())
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use uuid::Uuid;

    use super::{build_grounded_prompt, MAX_PROMPT_CITATIONS, MAX_PROMPT_SNIPPET_CHARS};
    use crate::services::retrieval::GroundedHit;

    fn hit(index: usize, snippet: String) -> GroundedHit {
        GroundedHit {
            chunk_id: Uuid::new_v4(),
            chunk_identity: format!("{index:064}"),
            document_id: Uuid::new_v4(),
            version_id: Uuid::new_v4(),
            collection_id: Uuid::new_v4(),
            version_number: 1,
            content_sha256: "a".repeat(64),
            heading_path: vec![format!("Heading {index}")],
            snippet,
            page: Some(1),
            slide: None,
            sheet: None,
            span_start: Some(0),
            span_end: Some(10),
            lexical_score: 1.0,
            vector_score: 0.5,
            rerank_score: 1.5,
            is_current: true,
            effective_from: Utc::now(),
            effective_to: None,
        }
    }

    #[test]
    fn prompt_caps_citation_count_and_snippet_length() {
        let long = "x".repeat(MAX_PROMPT_SNIPPET_CHARS + 20);
        let hits = (0..(MAX_PROMPT_CITATIONS + 5))
            .map(|index| hit(index, long.clone()))
            .collect::<Vec<_>>();
        let prompt = build_grounded_prompt("Nội dung?", &hits);
        assert_eq!(prompt.citations.len(), MAX_PROMPT_CITATIONS);
        assert_eq!(prompt.context_hits.len(), MAX_PROMPT_CITATIONS);
        assert!(prompt
            .context_hits
            .iter()
            .all(|hit| hit.snippet.chars().count() == MAX_PROMPT_SNIPPET_CHARS));
    }

    #[test]
    fn prompt_citation_ids_align_with_hit_order() {
        let hits = vec![hit(1, "Một".into()), hit(2, "Hai".into())];
        let prompt = build_grounded_prompt("Nội dung?", &hits);
        assert_eq!(prompt.citations[0].id, "CITE-0001");
        assert_eq!(prompt.citations[0].hit.chunk_id, hits[0].chunk_id);
        assert_eq!(prompt.citations[1].id, "CITE-0002");
        assert_eq!(prompt.citations[1].hit.chunk_id, hits[1].chunk_id);
        assert!(prompt.user.contains("id=\"CITE-0001\""));
        assert!(prompt.user.contains("id=\"CITE-0002\""));
    }
}
