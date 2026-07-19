//! SSE helpers and bounded replay state for resumable API streams.

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::response::sse::Event;
use serde_json::Value;
use uuid::Uuid;

use crate::api::SseEnvelope;

const SSE_VERSION: u16 = 1;
const ASK_STREAM_TTL: Duration = Duration::from_secs(5 * 60);
const MAX_EVENTS_PER_STREAM: usize = 128;
const MAX_BYTES_PER_STREAM: usize = 300 * 1024;
const MAX_CONCURRENT_STREAMS_PER_CALLER: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StreamCaller {
    pub(crate) org_id: Uuid,
    pub(crate) user_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StreamRegistryError {
    TooManyStreams,
    BufferLimitExceeded,
}

#[derive(Debug, Clone)]
pub(crate) struct AskStreamRegistry {
    inner: Arc<Mutex<RegistryInner>>,
}

#[derive(Debug, Default)]
struct RegistryInner {
    streams: HashMap<Uuid, AskStreamRecord>,
}

#[derive(Debug, Clone)]
struct AskStreamRecord {
    caller: StreamCaller,
    envelopes: Vec<SseEnvelope>,
    done: bool,
    created_at: Instant,
    bytes: usize,
}

impl Default for AskStreamRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl AskStreamRegistry {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(RegistryInner::default())),
        }
    }

    pub(crate) fn start_stream(&self, caller: StreamCaller) -> Result<Uuid, StreamRegistryError> {
        let mut inner = self.inner.lock().expect("ask stream registry poisoned");
        inner.evict_expired();
        let active = inner
            .streams
            .values()
            .filter(|record| record.caller == caller && !record.done)
            .count();
        if active >= MAX_CONCURRENT_STREAMS_PER_CALLER {
            return Err(StreamRegistryError::TooManyStreams);
        }
        let stream_id = Uuid::new_v4();
        inner.streams.insert(
            stream_id,
            AskStreamRecord {
                caller,
                envelopes: Vec::new(),
                done: false,
                created_at: Instant::now(),
                bytes: 0,
            },
        );
        Ok(stream_id)
    }

    pub(crate) fn append(
        &self,
        stream_id: Uuid,
        envelope: SseEnvelope,
    ) -> Result<(), StreamRegistryError> {
        let mut inner = self.inner.lock().expect("ask stream registry poisoned");
        inner.evict_expired();
        let Some(record) = inner.streams.get_mut(&stream_id) else {
            return Ok(());
        };
        let bytes = envelope_size(&envelope);
        if record.envelopes.len() >= MAX_EVENTS_PER_STREAM
            || record.bytes.saturating_add(bytes) > MAX_BYTES_PER_STREAM
        {
            inner.streams.remove(&stream_id);
            return Err(StreamRegistryError::BufferLimitExceeded);
        }
        record.bytes += bytes;
        record.envelopes.push(envelope);
        Ok(())
    }

    pub(crate) fn mark_done(&self, stream_id: Uuid) {
        let mut inner = self.inner.lock().expect("ask stream registry poisoned");
        inner.evict_expired();
        if let Some(record) = inner.streams.get_mut(&stream_id) {
            record.done = true;
        }
    }

    pub(crate) fn replay_after(
        &self,
        stream_id: Uuid,
        after_sequence: u64,
        caller: StreamCaller,
    ) -> Option<Vec<SseEnvelope>> {
        let mut inner = self.inner.lock().expect("ask stream registry poisoned");
        inner.evict_expired();
        let record = inner.streams.get(&stream_id)?;
        if record.caller != caller {
            return None;
        }
        Some(
            record
                .envelopes
                .iter()
                .filter(|envelope| envelope.sequence > after_sequence)
                .cloned()
                .collect(),
        )
    }
}

impl RegistryInner {
    fn evict_expired(&mut self) {
        let now = Instant::now();
        self.streams
            .retain(|_, record| now.duration_since(record.created_at) <= ASK_STREAM_TTL);
    }
}

pub(crate) fn envelope(
    sequence: u64,
    event: impl Into<String>,
    request_id: &str,
    data: Value,
) -> SseEnvelope {
    SseEnvelope {
        version: SSE_VERSION,
        sequence,
        event: event.into(),
        request_id: request_id.to_string(),
        data,
    }
}

pub(crate) fn event_from_envelope(
    stream_id: Uuid,
    envelope: &SseEnvelope,
) -> Result<Event, Infallible> {
    let data = serde_json::to_string(envelope).expect("SSE envelope is serializable");
    Ok(Event::default()
        .id(format!("{stream_id}:{}", envelope.sequence))
        .event(envelope.event.clone())
        .data(data))
}

pub(crate) fn parse_last_event_id(value: &str) -> Option<(Uuid, u64)> {
    let (stream_id, sequence) = value.split_once(':')?;
    let stream_id = Uuid::parse_str(stream_id).ok()?;
    let sequence = sequence.parse::<u64>().ok()?;
    Some((stream_id, sequence))
}

fn envelope_size(envelope: &SseEnvelope) -> usize {
    envelope.request_id.len()
        + envelope.event.len()
        + envelope.data.to_string().len()
        + std::mem::size_of_val(&envelope.version)
        + std::mem::size_of_val(&envelope.sequence)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn replay_is_bound_to_the_original_caller() {
        let registry = AskStreamRegistry::new();
        let caller = StreamCaller {
            org_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
        };
        let other = StreamCaller {
            org_id: caller.org_id,
            user_id: Uuid::new_v4(),
        };
        let stream_id = registry.start_stream(caller).unwrap();
        registry
            .append(
                stream_id,
                envelope(1, "ask.token", "req", json!({"token":"a"})),
            )
            .unwrap();

        assert_eq!(
            registry.replay_after(stream_id, 0, caller).unwrap().len(),
            1
        );
        assert!(registry.replay_after(stream_id, 0, other).is_none());
    }

    #[test]
    fn last_event_id_parser_requires_stream_and_sequence() {
        let stream_id = Uuid::new_v4();
        assert_eq!(
            parse_last_event_id(&format!("{stream_id}:12")),
            Some((stream_id, 12))
        );
        assert!(parse_last_event_id("not-a-stream").is_none());
        assert!(parse_last_event_id(&format!("{stream_id}:nope")).is_none());
    }
}
