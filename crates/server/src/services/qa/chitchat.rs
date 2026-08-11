//! Ask routing: assistant chat vs grounded knowledge retrieval.
//!
//! "Xin chào" is only one example — turns that are *not* about org documents
//! should use the assistant system prompt. Document-related questions keep the
//! hybrid retrieval + citation path.
//!
//! Clear cases use heuristics. Ambiguous cases may be classified by the chat
//! provider (`KNOWLEDGE` | `ASSISTANT`); callers must reserve/settle token quota
//! around that `complete()` call themselves.

use crate::services::qa::prompt::{build_assistant_messages, GroundedMessages};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AskRoute {
    /// General/assistant turn — no retrieval, no citations.
    Assistant,
    /// Document / knowledge-base question — hybrid retrieval + grounding.
    Knowledge,
}

/// Fast heuristic. `None` means ambiguous → caller may ask the LLM router.
pub fn heuristic_ask_route(question: &str) -> Option<AskRoute> {
    let normalized = normalize_ask(question);
    if normalized.is_empty() {
        return Some(AskRoute::Knowledge);
    }

    if has_knowledge_cues(&normalized) {
        return Some(AskRoute::Knowledge);
    }
    if is_clear_assistant(&normalized) {
        return Some(AskRoute::Assistant);
    }
    None
}

/// System/user messages for the one-token ask router (caller runs `complete`).
pub fn router_messages(question: &str) -> GroundedMessages {
    GroundedMessages {
        system: ROUTER_SYSTEM_PROMPT.to_string(),
        user: question.trim().to_string(),
    }
}

const ROUTER_SYSTEM_PROMPT: &str = "Bạn là bộ phân loại một nhãn. \
Chỉ trả lời đúng một từ: KNOWLEDGE hoặc ASSISTANT (không giải thích thêm). \
KNOWLEDGE = câu hỏi về nội dung tài liệu / chính sách / quy trình / dữ liệu trong kho tri thức tổ chức, cần tra cứu và trích dẫn nguồn. \
ASSISTANT = chào hỏi, cảm ơn, hỏi về trợ lý, trò chuyện chung, hoặc chủ đề không liên quan tài liệu nội bộ.";

pub fn parse_router_label(raw: &str) -> Option<AskRoute> {
    let token = raw
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_matches(|c: char| !c.is_ascii_alphabetic())
        .to_ascii_uppercase();
    match token.as_str() {
        "KNOWLEDGE" => Some(AskRoute::Knowledge),
        "ASSISTANT" => Some(AskRoute::Assistant),
        _ => None,
    }
}

fn has_knowledge_cues(normalized: &str) -> bool {
    const CUES: &[&str] = &[
        "tài liệu",
        "tai lieu",
        "văn bản",
        "van ban",
        "chính sách",
        "chinh sach",
        "quy trình",
        "quy trinh",
        "quy định",
        "quy dinh",
        "hướng dẫn",
        "huong dan",
        "biên bản",
        "bien ban",
        "hợp đồng",
        "hop dong",
        "báo cáo",
        "bao cao",
        "thư viện",
        "thu vien",
        "bộ sưu tập",
        "bo suu tap",
        "trích dẫn",
        "trich dan",
        "nguồn",
        "nguon",
        "phiên bản",
        "phien ban",
        "lập chỉ mục",
        "lap chi muc",
        "upload",
        "casan",
        "orchid",
        "uat-",
        "theo tài liệu",
        "trong tài liệu",
        "trong kho",
        "document",
        "policy",
        "procedure",
        "citation",
        "knowledge base",
    ];
    CUES.iter().any(|cue| normalized.contains(cue))
}

fn is_clear_assistant(normalized: &str) -> bool {
    // Long turns without knowledge cues stay ambiguous (LLM router / knowledge).
    if normalized.chars().count() > 96 {
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
        "bạn khỏe không",
        "ban khoe khong",
        "how are you",
    ];
    if EXACT.contains(&normalized) {
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
        "kể ",
        "ke ",
        "hát ",
        "hat ",
        "dịch giúp",
        "dich giup",
        "thời tiết",
        "thoi tiet",
        "bóng đá",
        "bong da",
        "kể chuyện",
        "ke chuyen",
        "đùa ",
        "dua ",
        "làm thơ",
        "lam tho",
    ];
    if PREFIXES
        .iter()
        .any(|prefix| normalized.starts_with(prefix) && normalized.chars().count() <= 64)
    {
        return true;
    }

    const OFFTOPIC: &[&str] = &[
        "thời tiết",
        "thoi tiet",
        "bóng đá",
        "bong da",
        "nói chuyện",
        "noi chuyen",
        "trò chuyện",
        "tro chuyen",
        "buồn",
        "buon",
        "vui quá",
        "vui qua",
        "1+1",
        "một cộng một",
    ];
    OFFTOPIC.iter().any(|p| normalized.contains(p)) && normalized.chars().count() <= 80
}

fn normalize_ask(question: &str) -> String {
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
    let normalized = normalize_ask(question);
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
    if is_clear_assistant(&normalized)
        && !normalized.contains("chào")
        && !normalized.contains("hello")
        && !normalized.contains("hi")
    {
        return "Tôi có thể trò chuyện ngắn hoặc tra cứu tài liệu trong thư viện kèm trích dẫn. Bạn muốn hỏi nội dung nào trong kho tri thức?".into();
    }
    "Xin chào! Tôi là trợ lý kho tri thức Folyvo. Hỏi tôi về tài liệu trong thư viện để được trả lời kèm nguồn; với câu chuyện chung tôi cũng có thể hỗ trợ ngắn gọn.".into()
}

/// Compatibility alias: clear heuristic assistant only (not LLM-routed).
pub fn is_assistant_chitchat(question: &str) -> bool {
    matches!(heuristic_ask_route(question), Some(AskRoute::Assistant))
}

pub fn assistant_messages(question: &str) -> GroundedMessages {
    build_assistant_messages(question)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greetings_route_assistant() {
        assert_eq!(heuristic_ask_route("Xin chào"), Some(AskRoute::Assistant));
        assert_eq!(heuristic_ask_route("hello"), Some(AskRoute::Assistant));
        assert_eq!(heuristic_ask_route("bạn là ai"), Some(AskRoute::Assistant));
        assert_eq!(
            heuristic_ask_route("thời tiết hôm nay thế nào"),
            Some(AskRoute::Assistant)
        );
    }

    #[test]
    fn document_questions_route_knowledge() {
        assert_eq!(
            heuristic_ask_route("CASAN là gì? Nêu năm bước và mã CASAN-UAT-20260805."),
            Some(AskRoute::Knowledge)
        );
        assert_eq!(
            heuristic_ask_route("Chính sách nghỉ phép hiện tại là gì?"),
            Some(AskRoute::Knowledge)
        );
        assert_eq!(
            heuristic_ask_route("Xin chào, cho tôi hỏi quy trình đối soát thanh toán tháng này"),
            Some(AskRoute::Knowledge)
        );
        assert_eq!(
            heuristic_ask_route("Tóm tắt tài liệu biên bản OCR UAT"),
            Some(AskRoute::Knowledge)
        );
    }

    #[test]
    fn ambiguous_returns_none_for_router() {
        assert_eq!(heuristic_ask_route("Công ty chúng ta làm gì?"), None);
    }

    #[test]
    fn parse_router_label_accepts_noise() {
        assert_eq!(parse_router_label("KNOWLEDGE"), Some(AskRoute::Knowledge));
        assert_eq!(parse_router_label("assistant\n"), Some(AskRoute::Assistant));
        assert_eq!(parse_router_label("ASSISTANT."), Some(AskRoute::Assistant));
        assert_eq!(parse_router_label("maybe later"), None);
    }

    #[test]
    fn fallback_is_polite_vietnamese() {
        let reply = assistant_fallback_reply("xin chào");
        assert!(reply.contains("Folyvo") || reply.contains("trợ lý"));
        assert!(!reply.contains("[CITE-"));
    }
}
