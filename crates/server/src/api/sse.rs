//! Versioned SSE wire helpers and closed-snapshot delivery (P1B-R05).
//!
//! Envelope shape matches `openapi/fixtures/sse.event-stream` / [`SseEnvelope`].
//! Application events are sequenced and resumable; heartbeats are transport-only
//! comments and are never persisted or sequenced.
//!
//! HTTP delivery streams an already-closed durable snapshot. Backpressure or
//! cancel ends the connection only — the DB snapshot remains available for
//! reconnect. Already-sent bytes are not recalled.

use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use futures::Stream;
use serde_json::Value as JsonValue;
use tokio::sync::mpsc;
use tokio::time::{timeout, Instant};

use super::types::SseEnvelope;
use crate::services::qa::stream::{AuthProbeDecision, StreamCancel, DEFAULT_OVERALL_TIMEOUT};

/// Canonical envelope version for `/api/v1` SSE routes.
pub const SSE_ENVELOPE_VERSION: u16 = 1;
/// Default keep-alive / heartbeat interval (transport comment only).
pub const SSE_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
/// Bounded live channel capacity for slow clients.
pub const SSE_LIVE_BUFFER: usize = 32;
/// Max wait to enqueue one event to a slow HTTP client.
pub const SSE_SEND_TIMEOUT: Duration = Duration::from_secs(2);
/// Max wait for one DB auth probe during delivery.
pub const SSE_AUTH_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
/// Hard maximum accepted Last-Event-ID (fits PostgreSQL `bigint`).
pub const MAX_LAST_EVENT_ID: u64 = i64::MAX as u64;

/// Delivery-time bounds (send/probe timeouts + expiry/deadline stop).
#[derive(Debug, Clone, Copy)]
pub struct DeliveryBounds {
    pub send_timeout: Duration,
    pub probe_timeout: Duration,
    pub overall_timeout: Duration,
    pub expires_at: Option<DateTime<Utc>>,
}

impl DeliveryBounds {
    pub fn for_snapshot(expires_at: DateTime<Utc>) -> Self {
        Self {
            send_timeout: SSE_SEND_TIMEOUT,
            probe_timeout: SSE_AUTH_PROBE_TIMEOUT,
            overall_timeout: DEFAULT_OVERALL_TIMEOUT,
            expires_at: Some(expires_at),
        }
    }
}

/// Application event names persisted for resume.
pub const EVENT_METADATA: &str = "metadata";
pub const EVENT_TOKEN: &str = "token";
pub const EVENT_CLOSE: &str = "close";
pub const EVENT_ERROR: &str = "error";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LastEventIdError {
    InvalidSyntax,
    OutOfRange,
}

impl LastEventIdError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidSyntax => "invalid_last_event_id",
            Self::OutOfRange => "last_event_id_out_of_range",
        }
    }

    pub const fn message(self) -> &'static str {
        match self {
            Self::InvalidSyntax => "Last-Event-ID must be a non-negative decimal integer",
            Self::OutOfRange => "Last-Event-ID is out of range for this stream",
        }
    }
}

/// Read `Last-Event-ID` from headers.
///
/// Missing header → `Ok(None)`. Present but invalid UTF-8 or non-decimal → 400.
pub fn last_event_id_from_headers(headers: &HeaderMap) -> Result<Option<u64>, LastEventIdError> {
    match headers.get("last-event-id") {
        None => Ok(None),
        Some(value) => {
            let raw = value
                .to_str()
                .map_err(|_| LastEventIdError::InvalidSyntax)?;
            parse_last_event_id(Some(raw))
        }
    }
}

/// Parse `Last-Event-ID` as a non-negative decimal integer.
///
/// Empty / absent → `Ok(None)` (replay from the start). Leading zeros, signs,
/// whitespace, and non-decimal forms are rejected.
pub fn parse_last_event_id(raw: Option<&str>) -> Result<Option<u64>, LastEventIdError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    if raw.is_empty() {
        return Ok(None);
    }
    if !raw.bytes().all(|b| b.is_ascii_digit()) {
        return Err(LastEventIdError::InvalidSyntax);
    }
    if raw.len() > 1 && raw.starts_with('0') {
        return Err(LastEventIdError::InvalidSyntax);
    }
    let value = raw
        .parse::<u64>()
        .map_err(|_| LastEventIdError::InvalidSyntax)?;
    if value > MAX_LAST_EVENT_ID {
        return Err(LastEventIdError::OutOfRange);
    }
    Ok(Some(value))
}

/// Validate that an acknowledged id is not ahead of the stream high-water mark.
pub fn validate_last_event_id_range(
    last_event_id: u64,
    high_water: u64,
) -> Result<(), LastEventIdError> {
    if last_event_id > high_water {
        return Err(LastEventIdError::OutOfRange);
    }
    Ok(())
}

pub fn build_envelope(
    sequence: u64,
    event: impl Into<String>,
    request_id: impl Into<String>,
    data: JsonValue,
) -> SseEnvelope {
    SseEnvelope {
        version: SSE_ENVELOPE_VERSION,
        sequence,
        event: event.into(),
        request_id: request_id.into(),
        data,
    }
}

pub fn envelope_to_sse_event(envelope: &SseEnvelope) -> Result<Event, serde_json::Error> {
    let data = serde_json::to_string(envelope)?;
    Ok(Event::default()
        .id(envelope.sequence.to_string())
        .event(envelope.event.clone())
        .data(data))
}

/// Headers required on every SSE response (no cache; explicit event-stream type).
pub fn sse_response_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream; charset=utf-8"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache, no-store, must-revalidate"),
    );
    headers.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    headers.insert(
        header::HeaderName::from_static("x-accel-buffering"),
        HeaderValue::from_static("no"),
    );
    headers
}

/// One sequenced application event ready for the live SSE channel.
#[derive(Debug, Clone)]
pub struct LiveSseEvent {
    pub envelope: SseEnvelope,
}

/// Guard that cancels delivery when the HTTP body is dropped.
pub struct CancelOnDrop {
    cancel: StreamCancel,
}

impl CancelOnDrop {
    pub fn new(cancel: StreamCancel) -> Self {
        Self { cancel }
    }
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// SSE body that forwards sequenced envelopes and cancels delivery on drop.
pub struct SseEventStream {
    rx: mpsc::Receiver<LiveSseEvent>,
    _guard: CancelOnDrop,
}

impl SseEventStream {
    pub fn new(rx: mpsc::Receiver<LiveSseEvent>, cancel: StreamCancel) -> Self {
        Self {
            rx,
            _guard: CancelOnDrop::new(cancel),
        }
    }
}

impl Stream for SseEventStream {
    type Item = Result<Event, Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.rx).poll_recv(cx) {
            Poll::Ready(Some(live)) => match envelope_to_sse_event(&live.envelope) {
                Ok(event) => Poll::Ready(Some(Ok(event))),
                Err(_) => Poll::Ready(None),
            },
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

fn remaining_until(deadline: Instant) -> Option<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
}

/// Deliver a durable closed snapshot with per-event auth probe + send timeout.
///
/// Deny/Deleted/cancel/probe-timeout/expiry/deadline stops further delivery only.
/// DB snapshot stays closed and reconnectable. Does not claim recall of bytes
/// already handed to the transport.
pub fn deliver_closed_snapshot<F, Fut>(
    envelopes: Vec<SseEnvelope>,
    cancel: StreamCancel,
    mut auth_probe: F,
    bounds: DeliveryBounds,
) -> SseEventStream
where
    F: FnMut() -> Fut + Send + 'static,
    Fut: Future<Output = AuthProbeDecision> + Send + 'static,
{
    let (tx, rx) = mpsc::channel(SSE_LIVE_BUFFER);
    let delivery_cancel = cancel.clone();
    tokio::spawn(async move {
        let deadline = Instant::now() + bounds.overall_timeout;
        for envelope in envelopes {
            if delivery_cancel.is_cancelled() {
                break;
            }
            let Some(remaining) = remaining_until(deadline) else {
                break;
            };
            if let Some(expires_at) = bounds.expires_at {
                if Utc::now() >= expires_at {
                    break;
                }
            }
            let probe_budget = remaining.min(bounds.probe_timeout);
            let decision = tokio::select! {
                biased;
                () = delivery_cancel.cancelled() => break,
                result = timeout(probe_budget, auth_probe()) => match result {
                    Ok(decision) => decision,
                    // Probe timeout closes delivery (HTTP only; DB remains).
                    Err(_) => break,
                },
            };
            // Deny/Deleted before each event: stop without enqueueing.
            if !matches!(decision, AuthProbeDecision::Allow) {
                break;
            }
            if delivery_cancel.is_cancelled() {
                break;
            }
            let Some(remaining) = remaining_until(deadline) else {
                break;
            };
            if let Some(expires_at) = bounds.expires_at {
                if Utc::now() >= expires_at {
                    break;
                }
            }
            let send_budget = remaining.min(bounds.send_timeout);
            let send = timeout(send_budget, tx.send(LiveSseEvent { envelope }));
            match tokio::select! {
                biased;
                () = delivery_cancel.cancelled() => None,
                result = send => Some(result),
            } {
                Some(Ok(Ok(()))) => {}
                Some(Ok(Err(_))) | Some(Err(_)) | None => break,
            }
        }
    });
    SseEventStream::new(rx, cancel)
}

/// Build a keep-alive SSE response with canonical cache/content-type headers.
pub fn sse_response(stream: SseEventStream) -> Response {
    let mut response = Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(SSE_HEARTBEAT_INTERVAL)
                .text("heartbeat"),
        )
        .into_response();
    let headers = response.headers_mut();
    for (key, value) in sse_response_headers().iter() {
        headers.insert(key.clone(), value.clone());
    }
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache, no-store, must-revalidate"),
    );
    *response.status_mut() = StatusCode::OK;
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn last_event_id_accepts_zero_and_decimal() {
        assert_eq!(parse_last_event_id(None).unwrap(), None);
        assert_eq!(parse_last_event_id(Some("")).unwrap(), None);
        assert_eq!(parse_last_event_id(Some("0")).unwrap(), Some(0));
        assert_eq!(parse_last_event_id(Some("42")).unwrap(), Some(42));
    }

    #[test]
    fn last_event_id_rejects_syntax() {
        assert_eq!(
            parse_last_event_id(Some("-1")).unwrap_err(),
            LastEventIdError::InvalidSyntax
        );
        assert_eq!(
            parse_last_event_id(Some("01")).unwrap_err(),
            LastEventIdError::InvalidSyntax
        );
        assert_eq!(
            parse_last_event_id(Some("1.5")).unwrap_err(),
            LastEventIdError::InvalidSyntax
        );
        assert_eq!(
            parse_last_event_id(Some(" 1")).unwrap_err(),
            LastEventIdError::InvalidSyntax
        );
        assert_eq!(
            parse_last_event_id(Some("abc")).unwrap_err(),
            LastEventIdError::InvalidSyntax
        );
    }

    #[test]
    fn last_event_id_from_headers_missing_vs_invalid() {
        let headers = HeaderMap::new();
        assert_eq!(last_event_id_from_headers(&headers).unwrap(), None);
        let mut headers = HeaderMap::new();
        headers.insert("last-event-id", HeaderValue::from_static("01"));
        assert_eq!(
            last_event_id_from_headers(&headers).unwrap_err(),
            LastEventIdError::InvalidSyntax
        );
        // Invalid UTF-8 header value → canonical 400 syntax error.
        let mut headers = HeaderMap::new();
        headers.insert(
            "last-event-id",
            HeaderValue::from_bytes(&[0xff, 0xfe]).unwrap(),
        );
        assert_eq!(
            last_event_id_from_headers(&headers).unwrap_err(),
            LastEventIdError::InvalidSyntax
        );
    }

    #[test]
    fn last_event_id_range_and_envelope_fixture() {
        assert!(validate_last_event_id_range(42, 42).is_ok());
        assert_eq!(
            validate_last_event_id_range(43, 42).unwrap_err(),
            LastEventIdError::OutOfRange
        );
        let envelope = build_envelope(42, "job.progress", "req", json!({"progress": 0.5}));
        let wire = serde_json::to_value(&envelope).unwrap();
        assert_eq!(wire["version"], 1);
        assert_eq!(wire["sequence"], 42);
        assert_eq!(wire["requestId"], "req");
        let _ = envelope_to_sse_event(&envelope).unwrap();
    }

    #[test]
    fn sse_headers_forbid_caching() {
        let headers = sse_response_headers();
        assert_eq!(
            headers.get(header::CONTENT_TYPE).unwrap(),
            "text/event-stream; charset=utf-8"
        );
        let cache = headers
            .get(header::CACHE_CONTROL)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(cache.contains("no-cache"));
        assert!(cache.contains("no-store"));
    }

    fn test_bounds() -> DeliveryBounds {
        DeliveryBounds {
            send_timeout: Duration::from_secs(1),
            probe_timeout: Duration::from_millis(200),
            overall_timeout: Duration::from_secs(5),
            expires_at: None,
        }
    }

    #[tokio::test]
    async fn delivery_probe_deny_stops_further_events_without_recall_claim() {
        use futures::StreamExt;
        let envelopes = vec![
            build_envelope(1, EVENT_METADATA, "req", json!({"n": 1})),
            build_envelope(2, EVENT_TOKEN, "req", json!({"text": "a"})),
            build_envelope(3, EVENT_TOKEN, "req", json!({"text": "b"})),
            build_envelope(4, EVENT_CLOSE, "req", json!({"reason": "completed"})),
        ];
        let cancel = StreamCancel::new();
        let mut calls = 0u32;
        let stream = deliver_closed_snapshot(
            envelopes,
            cancel,
            move || {
                calls += 1;
                let decision = if calls <= 2 {
                    AuthProbeDecision::Allow
                } else {
                    AuthProbeDecision::Deny
                };
                async move { decision }
            },
            test_bounds(),
        );
        let mut got = Vec::new();
        let mut pinned = std::pin::pin!(stream);
        while let Some(Ok(event)) = pinned.next().await {
            got.push(event);
        }
        // App-level: no further events after deny. Does not claim recalling prior bytes.
        assert!(got.len() <= 2);
        assert!(!got.is_empty());
    }

    #[tokio::test]
    async fn delivery_probe_deleted_stops_further_events_without_recall_claim() {
        use futures::StreamExt;
        let envelopes = vec![
            build_envelope(1, EVENT_METADATA, "req", json!({"n": 1})),
            build_envelope(2, EVENT_TOKEN, "req", json!({"text": "a"})),
            build_envelope(3, EVENT_TOKEN, "req", json!({"text": "b"})),
            build_envelope(4, EVENT_CLOSE, "req", json!({"reason": "completed"})),
        ];
        let cancel = StreamCancel::new();
        let mut calls = 0u32;
        let stream = deliver_closed_snapshot(
            envelopes,
            cancel,
            move || {
                calls += 1;
                let decision = if calls <= 2 {
                    AuthProbeDecision::Allow
                } else {
                    AuthProbeDecision::Deleted
                };
                async move { decision }
            },
            test_bounds(),
        );
        let mut got = Vec::new();
        let mut pinned = std::pin::pin!(stream);
        while let Some(Ok(event)) = pinned.next().await {
            got.push(event);
        }
        // Deletion is fail-closed for unsent bytes; already-sent events cannot be recalled.
        assert_eq!(got.len(), 2);
    }

    #[tokio::test]
    async fn hanging_probe_timeout_closes_delivery_hermetic() {
        use futures::StreamExt;
        let envelopes = vec![
            build_envelope(1, EVENT_METADATA, "req", json!({"n": 1})),
            build_envelope(2, EVENT_TOKEN, "req", json!({"text": "a"})),
            build_envelope(3, EVENT_CLOSE, "req", json!({"reason": "completed"})),
        ];
        let cancel = StreamCancel::new();
        let mut calls = 0u32;
        let stream = deliver_closed_snapshot(
            envelopes,
            cancel,
            move || {
                calls += 1;
                let hang = calls >= 2;
                async move {
                    if hang {
                        tokio::time::sleep(Duration::from_secs(30)).await;
                    }
                    AuthProbeDecision::Allow
                }
            },
            DeliveryBounds {
                probe_timeout: Duration::from_millis(50),
                ..test_bounds()
            },
        );
        let started = Instant::now();
        let mut got = Vec::new();
        let mut pinned = std::pin::pin!(stream);
        while let Some(Ok(event)) = pinned.next().await {
            got.push(event);
        }
        assert_eq!(got.len(), 1);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn expiry_during_delivery_stops_without_recall_claim() {
        use futures::StreamExt;
        let envelopes = vec![
            build_envelope(1, EVENT_METADATA, "req", json!({"n": 1})),
            build_envelope(2, EVENT_TOKEN, "req", json!({"text": "a"})),
            build_envelope(3, EVENT_TOKEN, "req", json!({"text": "b"})),
            build_envelope(4, EVENT_CLOSE, "req", json!({"reason": "completed"})),
        ];
        let cancel = StreamCancel::new();
        let mut calls = 0u32;
        let expires_at = Utc::now() + chrono::Duration::milliseconds(80);
        let stream = deliver_closed_snapshot(
            envelopes,
            cancel,
            move || {
                calls += 1;
                let delay = if calls == 1 {
                    Duration::from_millis(0)
                } else {
                    Duration::from_millis(120)
                };
                async move {
                    tokio::time::sleep(delay).await;
                    AuthProbeDecision::Allow
                }
            },
            DeliveryBounds {
                expires_at: Some(expires_at),
                probe_timeout: Duration::from_secs(1),
                ..test_bounds()
            },
        );
        let mut got = Vec::new();
        let mut pinned = std::pin::pin!(stream);
        while let Some(Ok(event)) = pinned.next().await {
            got.push(event);
        }
        assert!(got.len() < 4);
        assert!(!got.is_empty());
    }

    #[tokio::test]
    async fn drop_then_partial_consume_leaves_exact_resumable_tail() {
        use futures::StreamExt;
        let envelopes = vec![
            build_envelope(1, EVENT_METADATA, "req", json!({"n": 1})),
            build_envelope(2, EVENT_TOKEN, "req", json!({"text": "a"})),
            build_envelope(3, EVENT_TOKEN, "req", json!({"text": "b"})),
            build_envelope(4, EVENT_TOKEN, "req", json!({"text": "c"})),
            build_envelope(5, EVENT_CLOSE, "req", json!({"reason": "completed"})),
        ];
        let full_tail: Vec<_> = envelopes[2..].to_vec();
        let expected: Vec<String> = full_tail
            .iter()
            .map(|e| format!("{:?}", envelope_to_sse_event(e).unwrap()))
            .collect();
        let cancel = StreamCancel::new();
        let mut stream = deliver_closed_snapshot(
            envelopes,
            cancel.clone(),
            || async { AuthProbeDecision::Allow },
            test_bounds(),
        );
        {
            let mut pinned = std::pin::pin!(&mut stream);
            assert!(pinned.next().await.is_some());
            assert!(pinned.next().await.is_some());
        }
        // Body cancellation simulated by drop; DB snapshot remains reconnectable.
        drop(stream);
        cancel.cancel();

        let resume = deliver_closed_snapshot(
            full_tail,
            StreamCancel::new(),
            || async { AuthProbeDecision::Allow },
            test_bounds(),
        );
        let mut resumed = Vec::new();
        let mut pinned = std::pin::pin!(resume);
        while let Some(Ok(event)) = pinned.next().await {
            resumed.push(format!("{event:?}"));
        }
        assert_eq!(resumed, expected);
    }
}
