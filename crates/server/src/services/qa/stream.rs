//! Bounded validated replay streaming for grounded Q&A (P1B-R03).
//!
//! Contract: the whole answer is validated first, then server-rendered UTF-8-safe
//! chunks are replayed through a bounded channel. A caller-provided async
//! authorization probe runs before each application chunk; deny/delete closes
//! before enqueueing further chunks.
//!
//! Overall deadline (required) and cancel wrap hanging auth probes, enqueue waits,
//! and close sends so producers cannot block indefinitely. Cancellation uses a
//! `watch` flag with pre/post atomic checks + `select` (no Notify lost-wake races).
//! This module does **not** claim recall of bytes already handed to HTTP/kernel.
//! Routes/SSE resume belong to R05.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tokio::time::{timeout, Instant};

/// Default max tokens accepted during replay before forced close.
pub const DEFAULT_MAX_STREAM_TOKENS: usize = 4_096;
/// Hard maximum tokens.
pub const MAX_STREAM_TOKENS: usize = 4_096;
/// Default max UTF-8 bytes across streamed answer body.
pub const DEFAULT_MAX_STREAM_BYTES: usize = 64 * 1024;
/// Hard maximum streamed answer bytes.
pub const MAX_STREAM_BYTES: usize = 64 * 1024;
/// Default bounded channel capacity (backpressure).
pub const DEFAULT_STREAM_BUFFER: usize = 32;
/// Hard maximum channel capacity.
pub const MAX_STREAM_BUFFER: usize = 256;
/// Max time waiting to enqueue one token under backpressure before closing.
pub const DEFAULT_BACKPRESSURE_WAIT: Duration = Duration::from_secs(2);
/// Hard maximum backpressure wait.
pub const MAX_BACKPRESSURE_WAIT: Duration = Duration::from_secs(30);
/// Default overall stream deadline.
pub const DEFAULT_OVERALL_TIMEOUT: Duration = Duration::from_secs(60);
/// Hard maximum overall stream deadline.
pub const MAX_OVERALL_TIMEOUT: Duration = Duration::from_secs(120);
/// Minimum overall stream deadline.
pub const MIN_OVERALL_TIMEOUT: Duration = Duration::from_millis(1);
/// Max wait for the terminal close event itself.
pub const DEFAULT_CLOSE_SEND_WAIT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamCloseReason {
    Completed,
    Cancelled,
    Timeout,
    Backpressure,
    AuthzDenied,
    DocumentDeleted,
    Truncated,
}

impl StreamCloseReason {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Timeout => "timeout",
            Self::Backpressure => "backpressure",
            Self::AuthzDenied => "authz_denied",
            Self::DocumentDeleted => "document_deleted",
            Self::Truncated => "truncated",
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum StreamEvent {
    Token(String),
    Closed { reason: StreamCloseReason },
}

impl std::fmt::Debug for StreamEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Token(_) => f.write_str("Token([REDACTED])"),
            Self::Closed { reason } => f.debug_struct("Closed").field("reason", reason).finish(),
        }
    }
}

/// Cooperative cancellation: watch flag + atomic mirror for race-free select.
#[derive(Clone)]
pub struct StreamCancel {
    flag: Arc<AtomicBool>,
    tx: watch::Sender<bool>,
    rx: watch::Receiver<bool>,
}

impl Default for StreamCancel {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamCancel {
    pub fn new() -> Self {
        let (tx, rx) = watch::channel(false);
        Self {
            flag: Arc::new(AtomicBool::new(false)),
            tx,
            rx,
        }
    }

    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
        let _ = self.tx.send(true);
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst) || *self.rx.borrow()
    }

    /// Resolves when cancelled. Safe against lost-wake races via watch + flag.
    pub async fn cancelled(&self) {
        if self.is_cancelled() {
            return;
        }
        let mut rx = self.rx.clone();
        loop {
            if self.flag.load(Ordering::SeqCst) || *rx.borrow_and_update() {
                return;
            }
            if rx.changed().await.is_err() {
                // Sender dropped: treat as cancelled so producers cannot hang.
                return;
            }
        }
    }
}

impl std::fmt::Debug for StreamCancel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamCancel")
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

/// Result of the caller-provided authorization probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthProbeDecision {
    Allow,
    Deny,
    Deleted,
}

/// Bounds for validated replay. `overall_timeout` is required (hard-capped).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamBounds {
    pub max_tokens: usize,
    pub max_bytes: usize,
    pub buffer: usize,
    pub backpressure_wait: Duration,
    pub overall_timeout: Duration,
}

impl Default for StreamBounds {
    fn default() -> Self {
        Self {
            max_tokens: DEFAULT_MAX_STREAM_TOKENS,
            max_bytes: DEFAULT_MAX_STREAM_BYTES,
            buffer: DEFAULT_STREAM_BUFFER,
            backpressure_wait: DEFAULT_BACKPRESSURE_WAIT,
            overall_timeout: DEFAULT_OVERALL_TIMEOUT,
        }
    }
}

impl StreamBounds {
    /// Fail closed when bounds cannot guarantee progress/termination within hard maxima.
    pub fn validate(&self) -> Result<(), StreamCloseReason> {
        if self.max_tokens == 0
            || self.max_tokens > MAX_STREAM_TOKENS
            || self.max_bytes == 0
            || self.max_bytes > MAX_STREAM_BYTES
            || self.buffer == 0
            || self.buffer > MAX_STREAM_BUFFER
            || self.backpressure_wait.is_zero()
            || self.backpressure_wait > MAX_BACKPRESSURE_WAIT
            || self.overall_timeout < MIN_OVERALL_TIMEOUT
            || self.overall_timeout > MAX_OVERALL_TIMEOUT
        {
            return Err(StreamCloseReason::Truncated);
        }
        Ok(())
    }
}

/// UTF-8-safe chunks (char boundaries); prefers whitespace breaks ≤24 bytes.
pub fn tokenize_for_stream(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut tokens = Vec::new();
    let mut current = String::new();
    for character in text.chars() {
        current.push(character);
        if character.is_whitespace() || current.len() >= 24 {
            tokens.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn remaining_deadline(started: Instant, overall: Duration) -> Option<Duration> {
    overall
        .checked_sub(started.elapsed())
        .filter(|d| !d.is_zero())
}

async fn send_close(
    tx: &mpsc::Sender<StreamEvent>,
    reason: StreamCloseReason,
    started: Instant,
    overall: Duration,
) {
    let wait = remaining_deadline(started, overall)
        .unwrap_or(DEFAULT_CLOSE_SEND_WAIT)
        .min(DEFAULT_CLOSE_SEND_WAIT);
    let _ = timeout(wait, tx.send(StreamEvent::Closed { reason })).await;
}

/// Validate-then-replay: emit pre-validated chunks through a bounded channel.
///
/// Bounds are validated **before** the application channel is constructed.
/// `auth_probe` is awaited before each application chunk under the overall
/// deadline/cancel. Deny/Deleted closes without enqueueing that chunk.
pub async fn replay_validated_answer<F, Fut>(
    answer: String,
    bounds: StreamBounds,
    cancel: StreamCancel,
    mut auth_probe: F,
) -> mpsc::Receiver<StreamEvent>
where
    F: FnMut() -> Fut + Send + 'static,
    Fut: Future<Output = AuthProbeDecision> + Send + 'static,
{
    if let Err(reason) = bounds.validate() {
        let (tx, rx) = mpsc::channel(1);
        tokio::spawn(async move {
            let _ = timeout(
                DEFAULT_CLOSE_SEND_WAIT,
                tx.send(StreamEvent::Closed { reason }),
            )
            .await;
        });
        return rx;
    }

    let (tx, rx) = mpsc::channel(bounds.buffer);
    tokio::spawn(async move {
        let started = Instant::now();
        let overall = bounds.overall_timeout;
        let tokens = tokenize_for_stream(&answer);
        let mut emitted = 0usize;
        let mut emitted_bytes = 0usize;

        for token in tokens {
            if cancel.is_cancelled() {
                send_close(&tx, StreamCloseReason::Cancelled, started, overall).await;
                return;
            }
            let Some(probe_wait) = remaining_deadline(started, overall) else {
                send_close(&tx, StreamCloseReason::Timeout, started, overall).await;
                return;
            };
            if token.is_empty() {
                continue;
            }
            if !token.is_char_boundary(0) || !token.is_char_boundary(token.len()) {
                send_close(&tx, StreamCloseReason::Truncated, started, overall).await;
                return;
            }

            let decision = tokio::select! {
                biased;
                () = cancel.cancelled() => {
                    send_close(&tx, StreamCloseReason::Cancelled, started, overall).await;
                    return;
                }
                result = timeout(probe_wait, auth_probe()) => match result {
                    Ok(decision) => decision,
                    Err(_) => {
                        send_close(&tx, StreamCloseReason::Timeout, started, overall).await;
                        return;
                    }
                }
            };

            match decision {
                AuthProbeDecision::Allow => {}
                AuthProbeDecision::Deny => {
                    send_close(&tx, StreamCloseReason::AuthzDenied, started, overall).await;
                    return;
                }
                AuthProbeDecision::Deleted => {
                    send_close(&tx, StreamCloseReason::DocumentDeleted, started, overall).await;
                    return;
                }
            }

            if cancel.is_cancelled() {
                send_close(&tx, StreamCloseReason::Cancelled, started, overall).await;
                return;
            }

            let next_bytes = emitted_bytes.saturating_add(token.len());
            if next_bytes > bounds.max_bytes || emitted.saturating_add(1) > bounds.max_tokens {
                send_close(&tx, StreamCloseReason::Truncated, started, overall).await;
                return;
            }

            let Some(remaining) = remaining_deadline(started, overall) else {
                send_close(&tx, StreamCloseReason::Timeout, started, overall).await;
                return;
            };
            let enqueue_budget = remaining.min(bounds.backpressure_wait);

            let send_result = tokio::select! {
                biased;
                () = cancel.cancelled() => {
                    send_close(&tx, StreamCloseReason::Cancelled, started, overall).await;
                    return;
                }
                result = timeout(enqueue_budget, tx.send(StreamEvent::Token(token))) => result,
            };

            match send_result {
                Ok(Ok(())) => {
                    emitted = emitted.saturating_add(1);
                    emitted_bytes = next_bytes;
                }
                Ok(Err(_)) => return,
                Err(_) => {
                    if remaining_deadline(started, overall).is_none() {
                        send_close(&tx, StreamCloseReason::Timeout, started, overall).await;
                    } else {
                        send_close(&tx, StreamCloseReason::Backpressure, started, overall).await;
                    }
                    return;
                }
            }
        }

        send_close(&tx, StreamCloseReason::Completed, started, overall).await;
    });
    rx
}

/// Drain a stream receiver into concatenated token text + final close reason.
pub async fn collect_stream_text(
    mut rx: mpsc::Receiver<StreamEvent>,
) -> (String, Option<StreamCloseReason>) {
    let mut body = String::new();
    let mut reason = None;
    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::Token(token) => body.push_str(&token),
            StreamEvent::Closed { reason: close } => {
                reason = Some(close);
                break;
            }
        }
    }
    (body, reason)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_keeps_char_boundaries() {
        let tokens = tokenize_for_stream("Xin chào thế giới");
        assert!(tokens.iter().all(|t| t.is_char_boundary(0)));
        assert_eq!(tokens.concat(), "Xin chào thế giới");
    }

    #[test]
    fn invalid_and_over_max_bounds_are_rejected() {
        assert_eq!(
            StreamBounds {
                max_tokens: 0,
                ..StreamBounds::default()
            }
            .validate(),
            Err(StreamCloseReason::Truncated)
        );
        assert_eq!(
            StreamBounds {
                buffer: MAX_STREAM_BUFFER + 1,
                ..StreamBounds::default()
            }
            .validate(),
            Err(StreamCloseReason::Truncated)
        );
        assert_eq!(
            StreamBounds {
                overall_timeout: MAX_OVERALL_TIMEOUT + Duration::from_secs(1),
                ..StreamBounds::default()
            }
            .validate(),
            Err(StreamCloseReason::Truncated)
        );
    }

    #[tokio::test]
    async fn probe_deny_enqueues_no_further_tokens() {
        let cancel = StreamCancel::new();
        let bounds = StreamBounds {
            buffer: 8,
            backpressure_wait: Duration::from_secs(1),
            overall_timeout: Duration::from_secs(2),
            ..StreamBounds::default()
        };
        let mut calls = 0u32;
        let rx =
            replay_validated_answer("alpha beta gamma delta".into(), bounds, cancel, move || {
                calls += 1;
                let decision = if calls == 1 {
                    AuthProbeDecision::Allow
                } else {
                    AuthProbeDecision::Deny
                };
                async move { decision }
            })
            .await;
        let (body, reason) = collect_stream_text(rx).await;
        assert_eq!(reason, Some(StreamCloseReason::AuthzDenied));
        assert!(body.contains("alpha"));
        assert!(!body.contains("delta"));
    }

    #[tokio::test]
    async fn hanging_probe_hits_overall_deadline() {
        let cancel = StreamCancel::new();
        let bounds = StreamBounds {
            buffer: 4,
            backpressure_wait: Duration::from_secs(1),
            overall_timeout: Duration::from_millis(40),
            ..StreamBounds::default()
        };
        let rx = replay_validated_answer("alpha beta".into(), bounds, cancel, || async {
            tokio::time::sleep(Duration::from_secs(30)).await;
            AuthProbeDecision::Allow
        })
        .await;
        let (body, reason) = collect_stream_text(rx).await;
        assert_eq!(reason, Some(StreamCloseReason::Timeout));
        assert!(body.is_empty());
    }

    #[tokio::test]
    async fn cancel_interrupts_hanging_probe() {
        let cancel = StreamCancel::new();
        let cancel_signal = cancel.clone();
        let bounds = StreamBounds {
            buffer: 4,
            backpressure_wait: Duration::from_secs(1),
            overall_timeout: Duration::from_secs(5),
            ..StreamBounds::default()
        };
        let rx = replay_validated_answer("alpha beta".into(), bounds, cancel, || async {
            tokio::time::sleep(Duration::from_secs(30)).await;
            AuthProbeDecision::Allow
        })
        .await;
        tokio::time::sleep(Duration::from_millis(10)).await;
        cancel_signal.cancel();
        let (body, reason) = collect_stream_text(rx).await;
        assert_eq!(reason, Some(StreamCloseReason::Cancelled));
        assert!(body.is_empty());
    }

    #[tokio::test]
    async fn cancel_closes_without_completing() {
        let cancel = StreamCancel::new();
        let cancel_signal = cancel.clone();
        let bounds = StreamBounds {
            buffer: 1,
            backpressure_wait: Duration::from_millis(50),
            overall_timeout: Duration::from_secs(2),
            ..StreamBounds::default()
        };
        let long = "word ".repeat(200);
        let rx =
            replay_validated_answer(long, bounds, cancel, || async { AuthProbeDecision::Allow })
                .await;
        tokio::time::sleep(Duration::from_millis(10)).await;
        cancel_signal.cancel();
        let (_body, reason) = collect_stream_text(rx).await;
        assert!(matches!(
            reason,
            Some(StreamCloseReason::Cancelled) | Some(StreamCloseReason::Backpressure)
        ));
    }
}
