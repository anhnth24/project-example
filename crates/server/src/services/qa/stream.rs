//! Token stream helpers for grounded ask SSE (P1B-R03 / R05).

use std::time::Duration;

use futures::stream::{self, Stream};
use serde_json::{json, Value};

use crate::api::SseEnvelope;
use crate::services::qa::AskResponse;

pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);
/// Bound buffered SSE events for a slow consumer before disconnect.
pub const MAX_BUFFERED_EVENTS: usize = 256;
pub const SSE_ENVELOPE_VERSION: u16 = 1;

#[derive(Debug, Clone)]
pub struct StreamEvent {
    pub envelope: SseEnvelope,
}

/// Splits an answer into bounded token-like chunks for SSE.
pub fn tokenize_answer(answer: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for word in answer.split_whitespace() {
        if current.len() + word.len() + 1 > 48 && !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    if tokens.is_empty() {
        tokens.push(String::new());
    }
    tokens
}

pub fn ask_response_events(request_id: &str, response: &AskResponse) -> Vec<SseEnvelope> {
    let mut sequence = 1_u64;
    let mut events = Vec::new();
    events.push(envelope(
        sequence,
        request_id,
        "ask.started",
        json!({
            "mode": response.mode.as_str(),
            "embeddingMode": response.embedding_mode,
            "citationCount": response.citations.len(),
        }),
    ));
    sequence += 1;
    for warning in &response.warnings {
        events.push(envelope(
            sequence,
            request_id,
            "ask.warning",
            json!({ "message": warning }),
        ));
        sequence += 1;
    }
    for token in tokenize_answer(&response.answer) {
        events.push(envelope(
            sequence,
            request_id,
            "ask.token",
            json!({ "text": token }),
        ));
        sequence += 1;
    }
    events.push(envelope(
        sequence,
        request_id,
        "ask.citations",
        json!({ "citations": response.citations }),
    ));
    sequence += 1;
    events.push(envelope(
        sequence,
        request_id,
        "ask.version_context",
        json!(response.version_context),
    ));
    sequence += 1;
    events.push(envelope(
        sequence,
        request_id,
        "ask.completed",
        json!({
            "mode": response.mode.as_str(),
            "answerChars": response.answer.chars().count(),
        }),
    ));
    events
}

pub fn heartbeat_envelope(sequence: u64, request_id: &str) -> SseEnvelope {
    envelope(sequence, request_id, "heartbeat", json!({ "ok": true }))
}

pub fn auth_closed_envelope(sequence: u64, request_id: &str, reason: &str) -> SseEnvelope {
    envelope(
        sequence,
        request_id,
        "stream.closed",
        json!({ "reason": reason }),
    )
}

fn envelope(sequence: u64, request_id: &str, event: &str, data: Value) -> SseEnvelope {
    SseEnvelope {
        version: SSE_ENVELOPE_VERSION,
        sequence,
        event: event.into(),
        request_id: request_id.into(),
        data,
    }
}

/// Replay helper: drop events with sequence <= last_event_id.
pub fn replay_from(events: &[SseEnvelope], last_event_id: Option<u64>) -> Vec<SseEnvelope> {
    match last_event_id {
        Some(last) => events
            .iter()
            .filter(|event| event.sequence > last)
            .cloned()
            .collect(),
        None => events.to_vec(),
    }
}

pub fn into_sse_stream(
    events: Vec<SseEnvelope>,
) -> impl Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>> {
    stream::iter(events.into_iter().map(|envelope| {
        let data = serde_json::to_string(&envelope).unwrap_or_else(|_| "{}".into());
        Ok(axum::response::sse::Event::default()
            .id(envelope.sequence.to_string())
            .event(envelope.event)
            .data(data))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::citation::CitationPin;
    use crate::services::qa::grounding::VersionContext;
    use fileconv_knowledge::ask::AnswerMode;

    fn sample_response() -> AskResponse {
        let _ = std::mem::size_of::<CitationPin>();
        AskResponse {
            answer: "Kinh phí hiện tại là 15 triệu đồng theo phiên bản 2 [CITE-0001].".into(),
            mode: AnswerMode::OfflineExtractive,
            citations: Vec::new(),
            warnings: vec!["note".into()],
            version_context: VersionContext {
                mode: "current".into(),
                current_version_ids: vec![],
                cited_version_ids: vec![],
                change_note: None,
            },
            embedding_mode: "fts_only".into(),
        }
    }

    #[test]
    fn stream_events_are_sequenced_and_replayable() {
        let events = ask_response_events("req-1", &sample_response());
        assert!(events.len() >= 4);
        assert_eq!(events[0].sequence, 1);
        assert_eq!(events[0].event, "ask.started");
        let last = events[1].sequence;
        let replayed = replay_from(&events, Some(last));
        assert!(replayed.iter().all(|event| event.sequence > last));
        assert!(!events
            .iter()
            .any(|event| event.data.to_string().contains("password")));
    }

    #[test]
    fn tokenize_keeps_answer_coverage() {
        let tokens = tokenize_answer("Một hai ba bốn năm sáu bảy tám chín mười");
        let joined = tokens.join(" ");
        assert!(joined.contains("Một"));
        assert!(joined.contains("mười"));
    }
}
