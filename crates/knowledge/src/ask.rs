use std::collections::HashSet;

use crate::types::HybridSearchHit;

pub const GROUNDED_SYSTEM_PROMPT: &str =
    "Bạn là trợ lý kho tri thức trung thực. Không bịa và luôn trích citation.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnswerMode {
    OfflineExtractive,
    FallbackExtractive,
    LocalLlm,
    CloudLlm,
    SubscriptionCli,
}

impl AnswerMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OfflineExtractive => "offline_extractive",
            Self::FallbackExtractive => "fallback_extractive",
            Self::LocalLlm => "local_llm",
            Self::CloudLlm => "cloud_llm",
            Self::SubscriptionCli => "subscription_cli",
        }
    }
}

pub fn extractive_answer(question: &str, hits: &[HybridSearchHit]) -> String {
    if hits.is_empty() {
        return "Không tìm thấy bằng chứng phù hợp trong kho tri thức.".into();
    }
    let mut answer = format!(
        "## Trả lời trích xuất\n\nCâu hỏi: **{}**\n\n",
        question.trim()
    );
    for (index, hit) in hits.iter().enumerate() {
        answer.push_str(&format!(
            "{}. {} [CITE-{:04}]\n\n",
            index + 1,
            hit.snippet,
            index + 1
        ));
    }
    answer
}

pub fn retrieval_context(hits: &[HybridSearchHit]) -> String {
    hits.iter()
        .enumerate()
        .map(|(index, hit)| {
            format!(
                "[CITE-{:04}] {} > {}\n{}",
                index + 1,
                hit.source_rel,
                hit.heading,
                hit.snippet
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub fn grounded_user_prompt(question: &str, context: &str) -> String {
    format!(
        "Câu hỏi: {question}\n\nNguồn:\n{context}\n\n\
         Chỉ trả lời từ nguồn. Mỗi đoạn factual phải kết thúc bằng [CITE-xxxx]. \
         Nếu nguồn thiếu, nói rõ không đủ dữ liệu."
    )
}

pub fn valid_citation_ids(hit_count: usize) -> HashSet<String> {
    (0..hit_count)
        .map(|index| format!("CITE-{:04}", index + 1))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        extractive_answer, grounded_user_prompt, retrieval_context, valid_citation_ids,
        GROUNDED_SYSTEM_PROMPT,
    };
    use crate::types::{HybridSearchHit, SourceAnchor};

    fn hit() -> HybridSearchHit {
        HybridSearchHit {
            chunk_id: "chunk-1".into(),
            source_rel: "payments.pdf".into(),
            md_rel: "payments.pdf.md".into(),
            heading: "Đối soát".into(),
            snippet: "Đối soát giao dịch theo ngày.".into(),
            lexical_score: 1.0,
            vector_score: 0.8,
            rerank_score: 1.9,
            anchor: SourceAnchor {
                page: Some(7),
                slide: None,
                sheet: None,
                start: 0,
                end: 30,
            },
        }
    }

    #[test]
    fn extractive_answer_is_always_cited() {
        let answer = extractive_answer(" Khi nào? ", &[hit()]);
        assert!(answer.contains("Câu hỏi: **Khi nào?**"));
        assert!(answer.contains("[CITE-0001]"));
        assert_eq!(
            extractive_answer("Không có?", &[]),
            "Không tìm thấy bằng chứng phù hợp trong kho tri thức."
        );
    }

    #[test]
    fn context_keeps_sources_untrusted_in_user_prompt() {
        let context = retrieval_context(&[hit()]);
        let prompt = grounded_user_prompt("Khi nào?", &context);
        assert_eq!(
            context,
            "[CITE-0001] payments.pdf > Đối soát\nĐối soát giao dịch theo ngày."
        );
        assert!(prompt.contains("Nguồn:\n[CITE-0001]"));
        assert!(prompt.contains("Chỉ trả lời từ nguồn."));
        assert!(!GROUNDED_SYSTEM_PROMPT.contains("payments.pdf"));
        assert_eq!(
            valid_citation_ids(2),
            ["CITE-0001".to_string(), "CITE-0002".to_string()]
                .into_iter()
                .collect()
        );
    }
}
