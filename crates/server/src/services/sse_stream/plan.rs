//! Pre-DB closed-snapshot event planning (R03-equivalent bounds).

use serde_json::{json, Value as JsonValue};

use crate::api::sse::{
    EVENT_CLOSE, EVENT_ERROR, EVENT_METADATA, EVENT_TOKEN, SSE_ENVELOPE_VERSION,
};
use crate::db::sse_streams::{PlannedSseEvent, MAX_EVENT_PAYLOAD_BYTES, TERMINAL_EVENT_RESERVE};
use crate::services::qa::stream::{
    tokenize_for_stream, DEFAULT_MAX_STREAM_BYTES, DEFAULT_MAX_STREAM_TOKENS,
};
use crate::services::qa::{QaAnswer, QaCitation};

/// R03-equivalent + migration hard caps applied before any DB write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotPlanBounds {
    pub max_events: i32,
    pub max_bytes: i64,
    pub max_event_payload_bytes: i32,
    pub max_token_events: usize,
    pub max_token_bytes: usize,
}

impl Default for SnapshotPlanBounds {
    fn default() -> Self {
        Self {
            max_events: crate::db::sse_streams::DEFAULT_MAX_EVENTS,
            max_bytes: crate::db::sse_streams::DEFAULT_MAX_BYTES,
            max_event_payload_bytes: MAX_EVENT_PAYLOAD_BYTES,
            max_token_events: DEFAULT_MAX_STREAM_TOKENS,
            max_token_bytes: DEFAULT_MAX_STREAM_BYTES,
        }
    }
}

pub fn citation_to_json(citation: &QaCitation) -> JsonValue {
    json!({
        "citeId": citation.cite_id,
        "documentId": citation.document_id,
        "versionId": citation.version_id,
        "versionNumber": citation.version_number,
        "contentSha256": citation.content_sha256,
        "chunkId": citation.chunk_id,
        "isCurrent": citation.is_current,
        "heading": citation.heading,
        "quote": citation.quote
    })
}

pub fn metadata_data(answer: &QaAnswer) -> JsonValue {
    json!({
        "mode": answer.mode.as_str(),
        "grounded": answer.grounded,
        "citationCount": answer.citations.len(),
        "citations": answer.citations.iter().map(citation_to_json).collect::<Vec<_>>(),
        "warnings": answer.warnings,
        "versionContext": {
            "mode": answer.version_context.mode,
            "currentVersionIds": answer.version_context.current_version_ids,
            "citedVersionIds": answer.version_context.cited_version_ids,
            "changeNote": answer.version_context.change_note
        },
        "answerMode": answer.mode.as_str(),
        "fallbackReason": answer.audit.fallback_reason,
        "envelopeVersion": SSE_ENVELOPE_VERSION
    })
}

fn metadata_data_slim(answer: &QaAnswer) -> JsonValue {
    json!({
        "mode": answer.mode.as_str(),
        "grounded": answer.grounded,
        "citationCount": answer.citations.len(),
        "warnings": answer.warnings,
        "versionContext": {
            "mode": answer.version_context.mode,
            "currentVersionIds": answer.version_context.current_version_ids,
            "citedVersionIds": answer.version_context.cited_version_ids,
            "changeNote": answer.version_context.change_note
        },
        "answerMode": answer.mode.as_str(),
        "fallbackReason": answer.audit.fallback_reason,
        "envelopeVersion": SSE_ENVELOPE_VERSION,
        "citationsTruncated": true
    })
}

fn json_payload_bytes(value: &JsonValue) -> i32 {
    i32::try_from(serde_json::to_vec(value).unwrap_or_default().len()).unwrap_or(i32::MAX)
}

fn fits_event(data: &JsonValue, bounds: SnapshotPlanBounds) -> bool {
    let bytes = json_payload_bytes(data);
    bytes >= 0 && bytes <= bounds.max_event_payload_bytes
}

fn safe_truncated_snapshot() -> (Vec<PlannedSseEvent>, &'static str) {
    (
        vec![PlannedSseEvent {
            event_type: EVENT_ERROR,
            data: json!({ "reason": "truncated" }),
        }],
        "truncated",
    )
}

/// Build contiguous planned events with R03-equivalent hard caps before DB.
///
/// Always returns a persistable snapshot (terminal `truncated` or `completed`).
/// Never panics; oversized metadata yields a safe one-event error snapshot.
pub fn plan_closed_events(
    answer: &QaAnswer,
    bounds: SnapshotPlanBounds,
) -> (Vec<PlannedSseEvent>, &'static str) {
    if bounds.max_events <= TERMINAL_EVENT_RESERVE
        || bounds.max_bytes <= 0
        || bounds.max_event_payload_bytes <= 0
        || bounds.max_event_payload_bytes > MAX_EVENT_PAYLOAD_BYTES
        || bounds.max_token_events == 0
        || bounds.max_token_bytes == 0
    {
        return safe_truncated_snapshot();
    }

    let capacity = (bounds.max_events - TERMINAL_EVENT_RESERVE).max(1) as usize;
    let mut events = Vec::new();

    let metadata = {
        let full = metadata_data(answer);
        if fits_event(&full, bounds) && i64::from(json_payload_bytes(&full)) <= bounds.max_bytes {
            full
        } else {
            let slim = metadata_data_slim(answer);
            if fits_event(&slim, bounds) && i64::from(json_payload_bytes(&slim)) <= bounds.max_bytes
            {
                slim
            } else {
                return safe_truncated_snapshot();
            }
        }
    };
    let mut byte_count = i64::from(json_payload_bytes(&metadata));
    events.push(PlannedSseEvent {
        event_type: EVENT_METADATA,
        data: metadata,
    });

    let tokens = tokenize_for_stream(&answer.answer);
    let mut truncated = false;
    let mut token_events = 0usize;
    let mut token_bytes = 0usize;
    for token in tokens {
        if token.is_empty() {
            continue;
        }
        if events.len() >= capacity
            || token_events >= bounds.max_token_events
            || token_bytes.saturating_add(token.len()) > bounds.max_token_bytes
        {
            truncated = true;
            break;
        }
        let data = json!({ "text": token });
        if !fits_event(&data, bounds) {
            truncated = true;
            break;
        }
        let payload = i64::from(json_payload_bytes(&data));
        let terminal_reserve = 64i64;
        if byte_count
            .saturating_add(payload)
            .saturating_add(terminal_reserve)
            > bounds.max_bytes
        {
            truncated = true;
            break;
        }
        byte_count = byte_count.saturating_add(payload);
        token_bytes = token_bytes.saturating_add(token.len());
        token_events = token_events.saturating_add(1);
        events.push(PlannedSseEvent {
            event_type: EVENT_TOKEN,
            data,
        });
    }

    let (terminal_type, reason) = if truncated {
        (EVENT_ERROR, "truncated")
    } else {
        (EVENT_CLOSE, "completed")
    };
    let terminal_data = json!({ "reason": reason });
    if !fits_event(&terminal_data, bounds)
        || byte_count.saturating_add(i64::from(json_payload_bytes(&terminal_data)))
            > bounds.max_bytes
        || events.len() as i32 >= bounds.max_events
    {
        return safe_truncated_snapshot();
    }
    events.push(PlannedSseEvent {
        event_type: terminal_type,
        data: terminal_data,
    });

    let mut total = 0i64;
    for event in &events {
        let bytes = json_payload_bytes(&event.data);
        if bytes > bounds.max_event_payload_bytes {
            return safe_truncated_snapshot();
        }
        total = total.saturating_add(i64::from(bytes));
        if total > bounds.max_bytes {
            return safe_truncated_snapshot();
        }
    }
    if events.len() as i32 > bounds.max_events {
        return safe_truncated_snapshot();
    }

    (events, reason)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::qa::grounding::VersionContext;
    use crate::services::qa::{AnswerMode, QaAuditMetadata};
    use uuid::Uuid;

    fn sample_answer(answer: &str, quote: &str) -> QaAnswer {
        let doc = Uuid::new_v4();
        let version = Uuid::new_v4();
        QaAnswer {
            answer: answer.to_string(),
            citations: vec![QaCitation {
                cite_id: "c1".into(),
                document_id: doc,
                version_id: version,
                version_number: 1,
                content_sha256: "a".repeat(64),
                chunk_id: Uuid::new_v4(),
                is_current: true,
                heading: "H".into(),
                quote: quote.to_string(),
            }],
            mode: AnswerMode::OfflineExtractive,
            grounded: true,
            warnings: vec![],
            version_context: VersionContext {
                mode: "current",
                current_version_ids: vec![version],
                cited_version_ids: vec![version],
                change_note: None,
            },
            conflict_warnings: vec![],
            audit: QaAuditMetadata {
                action: "ask",
                outcome: "ok",
                answer_mode: AnswerMode::OfflineExtractive.as_str(),
                citation_count: 1,
                conflict_warning_count: 0,
                version_mode: "current",
                provider_configured: false,
                fallback_reason: None,
                request_id: "test".into(),
                grounded: true,
                latency_ms: 1,
                error: None,
            },
        }
    }

    #[test]
    fn oversized_metadata_is_bounded_or_truncated() {
        let huge = "x".repeat(70_000);
        let answer = sample_answer("short", &huge);
        let (events, reason) = plan_closed_events(&answer, SnapshotPlanBounds::default());
        assert!(matches!(reason, "completed" | "truncated"));
        assert!(!events.is_empty());
        assert!(events
            .iter()
            .all(|e| json_payload_bytes(&e.data) <= MAX_EVENT_PAYLOAD_BYTES));
        if reason == "completed" {
            assert_eq!(events[0].event_type, EVENT_METADATA);
            assert_eq!(events[0].data["citationsTruncated"], true);
        } else {
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].event_type, EVENT_ERROR);
            assert_eq!(events[0].data["reason"], "truncated");
        }

        let tiny = SnapshotPlanBounds {
            max_events: 8,
            max_bytes: 32,
            max_event_payload_bytes: MAX_EVENT_PAYLOAD_BYTES,
            max_token_events: DEFAULT_MAX_STREAM_TOKENS,
            max_token_bytes: DEFAULT_MAX_STREAM_BYTES,
        };
        let (events, reason) = plan_closed_events(&answer, tiny);
        assert_eq!(reason, "truncated");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, EVENT_ERROR);
        assert!(json_payload_bytes(&events[0].data) <= MAX_EVENT_PAYLOAD_BYTES);
    }

    #[test]
    fn token_caps_truncate_deterministically() {
        let answer = sample_answer(&"word ".repeat(5_000), "q");
        let bounds = SnapshotPlanBounds {
            max_events: 8,
            max_bytes: 256 * 1024,
            max_event_payload_bytes: MAX_EVENT_PAYLOAD_BYTES,
            max_token_events: 3,
            max_token_bytes: DEFAULT_MAX_STREAM_BYTES,
        };
        let (events, reason) = plan_closed_events(&answer, bounds);
        assert_eq!(reason, "truncated");
        assert_eq!(events.last().unwrap().event_type, EVENT_ERROR);
        assert_eq!(events.last().unwrap().data["reason"], "truncated");
        assert!(events.len() <= 8);
        let token_count = events
            .iter()
            .filter(|e| e.event_type == EVENT_TOKEN)
            .count();
        assert_eq!(token_count, 3);
    }
}
