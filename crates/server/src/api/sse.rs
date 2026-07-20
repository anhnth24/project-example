//! SSE helpers and bounded replay state for resumable API streams.

use std::collections::{BTreeSet, HashMap};
use std::convert::Infallible;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use axum::response::sse::Event;
use serde_json::Value;
use uuid::Uuid;

use crate::api::SseEnvelope;

const SSE_VERSION: u16 = 1;
const ASK_STREAM_TTL: Duration = Duration::from_secs(5 * 60);
const MAX_EVENTS_PER_STREAM: usize = 128;
/// Per-stream replay budget. Accounted bytes include stored collection-scope
/// UUIDs plus serialized envelope payload estimates; total retained replay
/// memory is bounded by `MAX_TOTAL_STREAMS * MAX_BYTES_PER_STREAM` plus O(1)
/// fixed overhead per retained record.
const MAX_BYTES_PER_STREAM: usize = 300 * 1024;
const MAX_CONCURRENT_STREAMS_PER_CALLER: usize = 4;
const MAX_RETAINED_STREAMS_PER_CALLER: usize = 8;
const MAX_TOTAL_STREAMS: usize = 512;

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
    collection_scope: Box<[Uuid]>,
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

    pub(crate) fn start_stream(
        &self,
        caller: StreamCaller,
        collection_scope: impl IntoIterator<Item = Uuid>,
    ) -> Result<Uuid, StreamRegistryError> {
        let collection_scope = normalized_scope(collection_scope);
        let scope_bytes = collection_scope_bytes(collection_scope.len());
        if scope_bytes > MAX_BYTES_PER_STREAM {
            return Err(StreamRegistryError::BufferLimitExceeded);
        }
        let mut inner = self.lock_inner();
        inner.evict_expired();
        if inner.active_count_for_caller(caller) >= MAX_CONCURRENT_STREAMS_PER_CALLER {
            return Err(StreamRegistryError::TooManyStreams);
        }
        if inner.total_count_for_caller(caller) >= MAX_RETAINED_STREAMS_PER_CALLER {
            inner.evict_oldest_done_for_caller(caller);
        }
        if inner.total_count_for_caller(caller) >= MAX_RETAINED_STREAMS_PER_CALLER {
            return Err(StreamRegistryError::TooManyStreams);
        }
        while inner.streams.len() >= MAX_TOTAL_STREAMS && inner.evict_oldest_done() {}
        if inner.streams.len() >= MAX_TOTAL_STREAMS {
            return Err(StreamRegistryError::TooManyStreams);
        }
        let stream_id = Uuid::new_v4();
        inner.streams.insert(
            stream_id,
            AskStreamRecord {
                caller,
                collection_scope,
                envelopes: Vec::new(),
                done: false,
                created_at: Instant::now(),
                bytes: scope_bytes,
            },
        );
        Ok(stream_id)
    }

    pub(crate) fn append(
        &self,
        stream_id: Uuid,
        envelope: SseEnvelope,
    ) -> Result<(), StreamRegistryError> {
        let mut inner = self.lock_inner();
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
        let mut inner = self.lock_inner();
        inner.evict_expired();
        if let Some(record) = inner.streams.get_mut(&stream_id) {
            record.done = true;
        }
    }

    pub(crate) fn remove(&self, stream_id: Uuid) {
        let mut inner = self.lock_inner();
        inner.evict_expired();
        inner.streams.remove(&stream_id);
    }

    pub(crate) fn replay_after(
        &self,
        stream_id: Uuid,
        after_sequence: u64,
        caller: StreamCaller,
        current_allowed: impl IntoIterator<Item = Uuid>,
    ) -> Option<Vec<SseEnvelope>> {
        let mut inner = self.lock_inner();
        inner.evict_expired();
        let record = inner.streams.get(&stream_id)?;
        if record.caller != caller {
            return None;
        }
        let current_allowed: BTreeSet<Uuid> = current_allowed.into_iter().collect();
        if !record
            .collection_scope
            .iter()
            .all(|collection_id| current_allowed.contains(collection_id))
        {
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

    fn lock_inner(&self) -> MutexGuard<'_, RegistryInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl RegistryInner {
    fn evict_expired(&mut self) {
        let now = Instant::now();
        self.streams
            .retain(|_, record| now.duration_since(record.created_at) <= ASK_STREAM_TTL);
    }

    fn active_count_for_caller(&self, caller: StreamCaller) -> usize {
        self.streams
            .values()
            .filter(|record| record.caller == caller && !record.done)
            .count()
    }

    fn total_count_for_caller(&self, caller: StreamCaller) -> usize {
        self.streams
            .values()
            .filter(|record| record.caller == caller)
            .count()
    }

    fn evict_oldest_done_for_caller(&mut self, caller: StreamCaller) -> bool {
        let oldest = self
            .streams
            .iter()
            .filter(|(_, record)| record.caller == caller && record.done)
            .min_by_key(|(_, record)| record.created_at)
            .map(|(stream_id, _)| *stream_id);
        if let Some(stream_id) = oldest {
            self.streams.remove(&stream_id);
            true
        } else {
            false
        }
    }

    fn evict_oldest_done(&mut self) -> bool {
        let oldest = self
            .streams
            .iter()
            .filter(|(_, record)| record.done)
            .min_by_key(|(_, record)| record.created_at)
            .map(|(stream_id, _)| *stream_id);
        if let Some(stream_id) = oldest {
            self.streams.remove(&stream_id);
            true
        } else {
            false
        }
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

fn normalized_scope(scope: impl IntoIterator<Item = Uuid>) -> Box<[Uuid]> {
    scope
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn collection_scope_bytes(len: usize) -> usize {
    len.saturating_mul(std::mem::size_of::<Uuid>())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn replay_is_bound_to_the_original_caller() {
        let registry = AskStreamRegistry::new();
        let collection = Uuid::new_v4();
        let caller = StreamCaller {
            org_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
        };
        let other = StreamCaller {
            org_id: caller.org_id,
            user_id: Uuid::new_v4(),
        };
        let stream_id = registry.start_stream(caller, [collection]).unwrap();
        registry
            .append(
                stream_id,
                envelope(1, "ask.token", "req", json!({"token":"a"})),
            )
            .unwrap();

        assert_eq!(
            registry
                .replay_after(stream_id, 0, caller, [collection])
                .unwrap()
                .len(),
            1
        );
        assert!(registry
            .replay_after(stream_id, 0, other, [collection])
            .is_none());
    }

    #[test]
    fn replay_requires_current_collection_scope() {
        let registry = AskStreamRegistry::new();
        let caller = StreamCaller {
            org_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
        };
        let collection = Uuid::new_v4();
        let stream_id = registry.start_stream(caller, [collection]).unwrap();
        registry
            .append(
                stream_id,
                envelope(1, "ask.token", "req", json!({"token":"a"})),
            )
            .unwrap();
        registry.mark_done(stream_id);

        assert!(registry
            .replay_after(stream_id, 0, caller, [collection])
            .is_some());
        assert!(registry.replay_after(stream_id, 0, caller, []).is_none());
    }

    #[test]
    fn record_accounted_bytes_include_collection_scope() {
        let registry = AskStreamRegistry::new();
        let caller = StreamCaller {
            org_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
        };
        let scope = [Uuid::new_v4(), Uuid::new_v4()];
        let stream_id = registry.start_stream(caller, scope).unwrap();

        let inner = registry.lock_inner();
        let record = inner.streams.get(&stream_id).expect("record");
        assert_eq!(record.bytes, collection_scope_bytes(scope.len()));
    }

    #[test]
    fn scope_larger_than_per_stream_budget_is_rejected() {
        let registry = AskStreamRegistry::new();
        let caller = StreamCaller {
            org_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
        };
        let oversized_scope =
            (0..=(MAX_BYTES_PER_STREAM / std::mem::size_of::<Uuid>())).map(|_| Uuid::new_v4());

        assert_eq!(
            registry.start_stream(caller, oversized_scope).unwrap_err(),
            StreamRegistryError::BufferLimitExceeded
        );
    }

    #[test]
    fn active_streams_are_capped_per_caller() {
        let registry = AskStreamRegistry::new();
        let caller = StreamCaller {
            org_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
        };
        let collection = Uuid::new_v4();
        for _ in 0..MAX_CONCURRENT_STREAMS_PER_CALLER {
            registry.start_stream(caller, [collection]).unwrap();
        }

        assert_eq!(
            registry.start_stream(caller, [collection]).unwrap_err(),
            StreamRegistryError::TooManyStreams
        );
    }

    #[test]
    fn retained_streams_are_capped_per_caller_by_evicting_done_records() {
        let registry = AskStreamRegistry::new();
        let caller = StreamCaller {
            org_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
        };
        let collection = Uuid::new_v4();
        let mut retained = Vec::new();
        for _ in 0..MAX_RETAINED_STREAMS_PER_CALLER {
            let stream_id = registry.start_stream(caller, [collection]).unwrap();
            registry.mark_done(stream_id);
            retained.push(stream_id);
        }

        let new_stream = registry.start_stream(caller, [collection]).unwrap();
        registry.mark_done(new_stream);

        assert!(registry
            .replay_after(retained[0], 0, caller, [collection])
            .is_none());
        assert!(registry
            .replay_after(new_stream, 0, caller, [collection])
            .is_some());
        assert_eq!(
            registry.lock_inner().total_count_for_caller(caller),
            MAX_RETAINED_STREAMS_PER_CALLER
        );
    }

    #[test]
    fn global_stream_cap_evicts_done_records() {
        let registry = AskStreamRegistry::new();
        let collection = Uuid::new_v4();
        for _ in 0..MAX_TOTAL_STREAMS {
            let caller = StreamCaller {
                org_id: Uuid::new_v4(),
                user_id: Uuid::new_v4(),
            };
            let stream_id = registry.start_stream(caller, [collection]).unwrap();
            registry.mark_done(stream_id);
        }
        let caller = StreamCaller {
            org_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
        };

        registry.start_stream(caller, [collection]).unwrap();

        assert_eq!(registry.lock_inner().streams.len(), MAX_TOTAL_STREAMS);
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
