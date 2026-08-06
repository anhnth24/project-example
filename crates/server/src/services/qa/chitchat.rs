//! Short social / assistant-only turns that must not run grounded retrieval.
//!
//! Greetings like "xin chào" otherwise still retrieve weakly related chunks and
//! dump extractive citations — which reads as a broken assistant. This classifier
//! is deliberately narrow (short + known social phrasings) so real knowledge
//! questions keep the grounded path.

/// Returns true when `question` is a short social/assistant turn (greeting,
/// thanks, identity, capability) that should use the assistant system prompt
/// instead of hybrid retrieval + citation grounding.
pub fn is_assistant_chitchat(question: &str) -> bool {
    let normalized = normalize_chitchat(question);
    if normalized.is_empty() {
        return false;
    }
    // Long questions are treated as knowledge asks even if they start politely.
    if normalized.chars().count() > 72 {
        return false;
    }

    const EXACT: &[&str] = &[
        "xin chào",
        "xin chao",
        "chào",
        "chao",
        "chào bạn",
        "chao ban",
        "chào trợ lý",
        "hello",
        "hi",
        "hey",
        "hola",
        "cảm ơn",
        "cam on",
        "cảm ơn bạn",
        "cam on ban",
        "cám ơn",
        "thanks",
        "thank you",
        "ty",
        "tạm biệt",
        "tam biet",
        "bye",
        "goodbye",
        "bạn là ai",
        "ban la ai",
        "bạn là gì",
        "ban la gi",
        "bạn tên gì",
        "ban ten gi",
        "who are you",
        "what can you do",
        "bạn làm được gì",
        "ban lam duoc gi",
        "giúp tôi được không",
        "help",
        "ok",
        "okay",
        "được",
        "vang",
        "vâng",
    ];
    if EXACT.iter().any(|phrase| normalized == *phrase) {
        return true;
    }

    const PREFIXES: &[&str] = &[
        "xin chào ",
        "xin chao ",
        "chào ",
        "chao ",
        "hello ",
        "hi ",
        "hey ",
        "cảm ơn ",
        "cam on ",
        "thanks ",
        "thank you ",
        "bạn là ai",
        "ban la ai",
        "who are you",
    ];
    PREFIXES
        .iter()
        .any(|prefix| normalized.starts_with(prefix) && normalized.chars().count() <= 48)
}

fn normalize_chitchat(question: &str) -> String {
    let lowered = question.trim().to_lowercase();
    let cleaned: String = lowered
        .chars()
        .map(|c| match c {
            '?' | '!' | '.' | ',' | ';' | ':' | '…' | '¿' | '¡' => ' ',
            _ => c,
        })
        .collect();
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Offline reply when the chat provider is unavailable for an assistant turn.
pub fn assistant_fallback_reply(question: &str) -> String {
    let normalized = normalize_chitchat(question);
    if normalized.starts_with("cảm ơn")
        || normalized.starts_with("cam on")
        || normalized.starts_with("thanks")
        || normalized.starts_with("thank you")
    {
        return "Không có chi. Bạn cứ hỏi thêm về tài liệu trong thư viện khi cần nhé.".into();
    }
    if normalized.starts_with("tạm biệt")
        || normalized.starts_with("tam biet")
        || normalized == "bye"
        || normalized.starts_with("goodbye")
    {
        return "Tạm biệt! Hẹn gặp lại khi bạn cần tra cứu tài liệu.".into();
    }
    if normalized.contains("là ai")
        || normalized.contains("la ai")
        || normalized.contains("who are you")
        || normalized.contains("làm được gì")
        || normalized.contains("lam duoc gi")
        || normalized.contains("what can you do")
    {
        return "Tôi là trợ lý kho tri thức Folyvo. Bạn có thể hỏi về nội dung tài liệu trong thư viện; khi trả lời từ tài liệu tôi sẽ kèm trích dẫn nguồn.".into();
    }
    "Xin chào! Tôi là trợ lý kho tri thức Folyvo. Bạn có thể hỏi về nội dung tài liệu trong thư viện — tôi sẽ trả lời kèm trích dẫn khi có bằng chứng trong nguồn.".into()
}

#[cfg(test)]
mod tests {
    use super::{assistant_fallback_reply, is_assistant_chitchat};

    #[test]
    fn greetings_are_chitchat() {
        assert!(is_assistant_chitchat("Xin chào"));
        assert!(is_assistant_chitchat("xin chào!"));
        assert!(is_assistant_chitchat("Hello"));
        assert!(is_assistant_chitchat("hi"));
        assert!(is_assistant_chitchat("Chào bạn"));
    }

    #[test]
    fn knowledge_questions_are_not_chitchat() {
        assert!(!is_assistant_chitchat(
            "CASAN là gì? Nêu năm bước và mã CASAN-UAT-20260805."
        ));
        assert!(!is_assistant_chitchat(
            "Chính sách nghỉ phép hiện tại là gì?"
        ));
        assert!(!is_assistant_chitchat(
            "Xin chào, cho tôi hỏi quy trình đối soát thanh toán tháng này"
        ));
    }

    #[test]
    fn fallback_is_polite_vietnamese() {
        let reply = assistant_fallback_reply("xin chào");
        assert!(reply.contains("Folyvo") || reply.contains("trợ lý"));
        assert!(!reply.contains("[CITE-"));
    }
}
