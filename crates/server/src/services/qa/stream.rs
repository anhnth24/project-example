//! Bounded token streaming with authorization-epoch + PG advisory fencing (P1B-R03).
//!
//! Egress is exposed only via [`GuardedSseBody`] / [`GuardedJsonBody`]: **one**
//! shared [`DeliveryGuard`] is acquired + exact-revalidated before any bytes leave,
//! held for the entire HTTP response lifetime, and released only on body end/drop
//! (or independent stall watchdog).
//!
//! # Emission guarantee (HTTP-honest)
//!
//! After revoke/delete, the **application** stops generating and enqueueing new
//! frames. Bytes already handed to Hyper / the kernel cannot be recalled. At most
//! **one small already-encoded frame** may still leave as a bounded transport tail
//! before the close event. Do **not** claim zero network bytes post-commit.

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use futures::StreamExt;
use http_body::{Body, Frame};
use thiserror::Error;
use tokio::sync::mpsc;
use tokio::time::{timeout, Instant};

use uuid::Uuid;

use crate::db::authz_epoch;
use crate::db::authz_lock::DeliveryGuard;
use crate::db::search::{self, StreamAuthzProbe};
use crate::services::qa::authz_fence::{AuthzEpochFence, CloseKind};

/// Default max tokens accepted from a provider stream before forced close.
pub const DEFAULT_MAX_STREAM_TOKENS: usize = 4_096;
/// Default max UTF-8 bytes across streamed answer body.
pub const DEFAULT_MAX_STREAM_BYTES: usize = 64 * 1024;
/// Default bounded channel capacity (backpressure).
pub const DEFAULT_STREAM_BUFFER: usize = 32;
/// Max time waiting to enqueue one token under backpressure before closing.
pub const DEFAULT_BACKPRESSURE_WAIT: Duration = Duration::from_secs(2);
/// Hard max HTTP response / write lifetime.
pub const DEFAULT_RESPONSE_LIFETIME: Duration = Duration::from_secs(60);
/// Independent stall watchdog: no body poll progress → cancel + release guard.
pub const DEFAULT_STALL_WATCHDOG: Duration = Duration::from_secs(15);
/// Max encoded SSE frame bytes retained as transport tail after revoke.
pub const MAX_TRANSPORT_TAIL_FRAME_BYTES: usize = 4_096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamCloseReason {
    Completed,
    Cancelled,
    Timeout,
    Backpressure,
    AuthzRevoked,
    DocumentDeleted,
    ProviderError,
    Truncated,
    StallWatchdog,
}

impl StreamCloseReason {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Timeout => "timeout",
            Self::Backpressure => "backpressure",
            Self::AuthzRevoked => "authz_revoked",
            Self::DocumentDeleted => "document_deleted",
            Self::ProviderError => "provider_error",
            Self::Truncated => "truncated",
            Self::StallWatchdog => "stall_watchdog",
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

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Shared cell so the stall watchdog can release the dedicated guard without a body poll.
type SharedGuard = Arc<std::sync::Mutex<Option<DeliveryGuard>>>;

/// SSE body: owns one session [`DeliveryGuard`] until end/drop/watchdog.
pub struct GuardedSseBody {
    events: mpsc::Receiver<StreamEvent>,
    fence: Option<AuthzEpochFence>,
    _cancel: Option<StreamCancel>,
    guard: SharedGuard,
    deadline: Instant,
    /// At most one already-encoded frame may remain after revoke (transport tail).
    pending_frames: std::collections::VecDeque<Bytes>,
    close_reason: Option<StreamCloseReason>,
    done: bool,
    /// App-level: stop accepting new events after revoke/close.
    emission_closed: bool,
    last_progress_ms: Arc<AtomicU64>,
    watchdog_fired: Arc<AtomicBool>,
}

impl GuardedSseBody {
    pub fn new(
        events: mpsc::Receiver<StreamEvent>,
        guard: Option<DeliveryGuard>,
        fence: Option<AuthzEpochFence>,
        cancel: Option<StreamCancel>,
        metadata_json: Option<Bytes>,
        max_lifetime: Duration,
        stall_watchdog: Duration,
    ) -> Self {
        let mut pending_frames = std::collections::VecDeque::new();
        if let Some(meta) = metadata_json {
            // R9.5: protected metadata event before tokens/close.
            let encoded = String::from_utf8_lossy(&meta);
            pending_frames.push_back(Bytes::from(format!("event: metadata\ndata: {encoded}\n\n")));
        }
        let shared: SharedGuard = Arc::new(std::sync::Mutex::new(guard));
        let last_progress_ms = Arc::new(AtomicU64::new(now_ms()));
        let watchdog_fired = Arc::new(AtomicBool::new(false));
        // Independent watchdog: cancels session, signals fence, releases guard.
        {
            let last = last_progress_ms.clone();
            let fired = watchdog_fired.clone();
            let guard_w = shared.clone();
            let fence_w = fence.clone();
            let cancel_w = cancel.clone();
            let stall = stall_watchdog;
            tokio::spawn(async move {
                let tick = Duration::from_millis(100)
                    .min(stall / 4)
                    .max(Duration::from_millis(25));
                loop {
                    tokio::time::sleep(tick).await;
                    if fired.load(Ordering::SeqCst) {
                        break;
                    }
                    let last_ms = last.load(Ordering::SeqCst);
                    let age = now_ms().saturating_sub(last_ms);
                    if age >= stall.as_millis() as u64 {
                        fired.store(true, Ordering::SeqCst);
                        if let Some(c) = cancel_w.as_ref() {
                            c.cancel();
                        }
                        if let Some(f) = fence_w.as_ref() {
                            f.signal_revoked();
                        }
                        if let Ok(mut g) = guard_w.lock() {
                            let _ = g.take();
                        }
                        break;
                    }
                }
            });
        }
        Self {
            events,
            fence,
            _cancel: cancel,
            guard: shared,
            deadline: Instant::now() + max_lifetime,
            pending_frames,
            close_reason: None,
            done: false,
            emission_closed: false,
            last_progress_ms,
            watchdog_fired,
        }
    }

    pub fn close_reason(&self) -> Option<&StreamCloseReason> {
        self.close_reason.as_ref()
    }

    fn encode_token(token: &str) -> Bytes {
        let encoded = serde_json::to_string(token).unwrap_or_else(|_| "\"\"".into());
        Bytes::from(format!("data: {encoded}\n\n"))
    }

    fn encode_close(reason: &StreamCloseReason) -> Bytes {
        Bytes::from(format!("event: close\ndata: {}\n\n", reason.as_str()))
    }

    fn mark_progress(&self) {
        self.last_progress_ms.store(now_ms(), Ordering::SeqCst);
    }

    fn take_guard(&self) -> Option<DeliveryGuard> {
        self.guard.lock().ok().and_then(|mut g| g.take())
    }

    fn close_app_emission(&mut self, reason: StreamCloseReason) {
        if self.emission_closed {
            return;
        }
        self.emission_closed = true;
        self.close_reason = Some(reason.clone());
        // Bound transport tail: keep at most one small already-encoded frame.
        while self.pending_frames.len() > 1 {
            self.pending_frames.pop_front();
        }
        if let Some(front) = self.pending_frames.front() {
            if front.len() > MAX_TRANSPORT_TAIL_FRAME_BYTES {
                self.pending_frames.pop_front();
            }
        }
        self.pending_frames.push_back(Self::encode_close(&reason));
        self.done = true;
    }

    /// Await unlock + connection close (preferred after body end).
    pub async fn finish(self) {
        if let Some(g) = self.take_guard() {
            g.release().await;
        }
    }
}

impl Drop for GuardedSseBody {
    fn drop(&mut self) {
        let _ = self.take_guard();
    }
}

impl Body for GuardedSseBody {
    type Data = Bytes;
    type Error = StreamError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        self.mark_progress();
        if self.watchdog_fired.load(Ordering::SeqCst) && !self.emission_closed {
            self.close_app_emission(StreamCloseReason::StallWatchdog);
        }
        if self.done && self.pending_frames.is_empty() {
            return Poll::Ready(None);
        }
        if let Some(bytes) = self.pending_frames.pop_front() {
            return Poll::Ready(Some(Ok(Frame::data(bytes))));
        }
        if Instant::now() >= self.deadline {
            self.close_app_emission(StreamCloseReason::Timeout);
            let _ = self.take_guard();
            return self.poll_frame(cx);
        }
        if self.emission_closed {
            return Poll::Ready(None);
        }
        if let Some(kind) = self.fence.as_ref().and_then(|f| f.close_reason()) {
            // App-level: no new frames after revoke — close (bounded tail only).
            self.close_app_emission(close_from_kind(kind));
            return self.poll_frame(cx);
        }
        match Pin::new(&mut self.events).poll_recv(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(None) => {
                self.done = true;
                self.emission_closed = true;
                Poll::Ready(None)
            }
            Poll::Ready(Some(StreamEvent::Token(t))) => {
                if let Some(kind) = self.fence.as_ref().and_then(|f| f.close_reason()) {
                    self.close_app_emission(close_from_kind(kind));
                    return self.poll_frame(cx);
                }
                if self.emission_closed {
                    return self.poll_frame(cx);
                }
                self.pending_frames.push_back(Self::encode_token(&t));
                self.poll_frame(cx)
            }
            Poll::Ready(Some(StreamEvent::Closed { reason })) => {
                self.close_app_emission(reason);
                self.poll_frame(cx)
            }
        }
    }
}

/// JSON body holding one session guard until end/drop.
pub struct GuardedJsonBody {
    data: Option<Bytes>,
    guard: Option<DeliveryGuard>,
    deadline: Instant,
    started: bool,
}

impl GuardedJsonBody {
    pub fn new(json: Bytes, guard: DeliveryGuard, max_lifetime: Duration) -> Self {
        Self {
            data: Some(json),
            guard: Some(guard),
            deadline: Instant::now() + max_lifetime,
            started: false,
        }
    }

    /// Await unlock + connection close after the JSON body has ended.
    pub async fn finish(mut self) {
        if let Some(mut g) = self.guard.take() {
            g.mark_sent();
            g.release().await;
        }
    }
}

impl Drop for GuardedJsonBody {
    fn drop(&mut self) {
        let _ = self.guard.take();
    }
}

impl Body for GuardedJsonBody {
    type Data = Bytes;
    type Error = StreamError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        if Instant::now() >= self.deadline {
            self.data.take();
            let _ = self.guard.take();
            return Poll::Ready(Some(Err(StreamError::WriteTimeout)));
        }
        match self.data.take() {
            Some(bytes) => {
                self.started = true;
                Poll::Ready(Some(Ok(Frame::data(bytes))))
            }
            None => Poll::Ready(None),
        }
    }
}

/// Drive body to completion (tests / Tower adapters). No plaintext bypass API.
pub async fn drive_body_to_end<B>(body: &mut B) -> (Bytes, Option<StreamCloseReason>)
where
    B: Body<Data = Bytes> + Unpin,
{
    let mut acc = Vec::new();
    let mut close = None;
    loop {
        let frame = std::future::poll_fn(|cx| Pin::new(&mut *body).poll_frame(cx)).await;
        match frame {
            None => break,
            Some(Err(_)) => break,
            Some(Ok(frame)) => {
                if let Ok(data) = frame.into_data() {
                    let chunk = String::from_utf8_lossy(&data);
                    if chunk.contains("event: close") {
                        for line in chunk.lines() {
                            if let Some(rest) = line.strip_prefix("data: ") {
                                close = Some(match rest {
                                    "authz_revoked" => StreamCloseReason::AuthzRevoked,
                                    "document_deleted" => StreamCloseReason::DocumentDeleted,
                                    "timeout" => StreamCloseReason::Timeout,
                                    "cancelled" => StreamCloseReason::Cancelled,
                                    "backpressure" => StreamCloseReason::Backpressure,
                                    "provider_error" => StreamCloseReason::ProviderError,
                                    "truncated" => StreamCloseReason::Truncated,
                                    "stall_watchdog" => StreamCloseReason::StallWatchdog,
                                    _ => StreamCloseReason::Completed,
                                });
                            }
                        }
                        continue;
                    }
                    if chunk.contains("event: metadata") {
                        continue;
                    }
                    acc.extend_from_slice(&data);
                }
            }
        }
    }
    (Bytes::from(acc), close)
}

/// Collect SSE token text from a driven body (tests), then await guard release.
///
/// Asserts **app emission** stop (no new tokens after revoke), not zero network bytes.
pub async fn collect_sse_token_text(mut body: GuardedSseBody) -> (String, StreamCloseReason) {
    let (raw, close) = drive_body_to_end(&mut body).await;
    body.finish().await;
    let mut acc = String::new();
    for line in String::from_utf8_lossy(&raw).lines() {
        if let Some(rest) = line.strip_prefix("data: ") {
            if let Ok(token) = serde_json::from_str::<String>(rest) {
                acc.push_str(&token);
            } else {
                acc.push_str(rest);
            }
        }
    }
    (acc, close.unwrap_or(StreamCloseReason::Completed))
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum StreamError {
    #[error("qa stream cancelled")]
    Cancelled,
    #[error("qa stream backpressure")]
    Backpressure,
    #[error("qa stream authz revoked")]
    AuthzRevoked,
    #[error("qa stream document deleted")]
    DocumentDeleted,
    #[error("qa stream socket write timeout")]
    WriteTimeout,
    #[error("qa stream stall watchdog")]
    StallWatchdog,
}

impl StreamError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Cancelled => "qa_stream_cancelled",
            Self::Backpressure => "qa_stream_backpressure",
            Self::AuthzRevoked => "qa_stream_authz_revoked",
            Self::DocumentDeleted => "qa_stream_document_deleted",
            Self::WriteTimeout => "qa_stream_write_timeout",
            Self::StallWatchdog => "qa_stream_stall_watchdog",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthzProbeResult {
    Allow,
    Revoked,
    Deleted,
}

#[derive(Debug, Clone, Default)]
pub struct StreamCancel {
    inner: Arc<AtomicBool>,
}

impl StreamCancel {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn cancel(&self) {
        self.inner.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.load(Ordering::SeqCst)
    }

    pub async fn cancelled(&self) {
        while !self.is_cancelled() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}

#[derive(Debug, Clone)]
pub struct AuthzWatch {
    fence: AuthzEpochFence,
}

impl Default for AuthzWatch {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthzWatch {
    pub fn new() -> Self {
        let fence = AuthzEpochFence::new();
        fence.capture(1);
        Self { fence }
    }

    pub fn fence(&self) -> &AuthzEpochFence {
        &self.fence
    }

    pub fn signal_revoked(&self) {
        self.fence.signal_revoked();
    }

    pub fn signal_deleted(&self) {
        self.fence.signal_deleted();
    }

    pub async fn revoke_and_drain(&self) {
        self.fence.revoke_and_drain(CloseKind::Revoked).await;
    }

    pub fn probe(&self) -> AuthzProbeResult {
        match self.fence.close_reason() {
            Some(CloseKind::Deleted) => AuthzProbeResult::Deleted,
            Some(CloseKind::Revoked) => AuthzProbeResult::Revoked,
            None => AuthzProbeResult::Allow,
        }
    }
}

#[derive(Debug, Clone)]
pub struct StreamBounds {
    pub max_tokens: usize,
    pub max_bytes: usize,
    pub buffer: usize,
    pub backpressure_wait: Duration,
    pub overall_timeout: Option<Duration>,
    pub source_wait: Duration,
}

impl Default for StreamBounds {
    fn default() -> Self {
        Self {
            max_tokens: DEFAULT_MAX_STREAM_TOKENS,
            max_bytes: DEFAULT_MAX_STREAM_BYTES,
            buffer: DEFAULT_STREAM_BUFFER,
            backpressure_wait: DEFAULT_BACKPRESSURE_WAIT,
            overall_timeout: Some(Duration::from_secs(60)),
            source_wait: Duration::from_secs(5),
        }
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

fn close_from_kind(kind: CloseKind) -> StreamCloseReason {
    match kind {
        CloseKind::Revoked => StreamCloseReason::AuthzRevoked,
        CloseKind::Deleted => StreamCloseReason::DocumentDeleted,
    }
}

/// Protected stream: each token delivery acquires an epoch send permit.
pub async fn run_bounded_stream<S>(
    mut source: S,
    bounds: StreamBounds,
    cancel: StreamCancel,
    fence: Option<AuthzEpochFence>,
) -> mpsc::Receiver<StreamEvent>
where
    S: futures::Stream<Item = Result<String, ()>> + Unpin + Send + 'static,
{
    let (tx, rx) = mpsc::channel(bounds.buffer.max(1));
    tokio::spawn(async move {
        let started = Instant::now();
        let mut emitted = 0usize;
        let mut emitted_bytes = 0usize;
        let close = |reason: StreamCloseReason| async {
            let _ = tx.send(StreamEvent::Closed { reason }).await;
        };

        loop {
            if cancel.is_cancelled() {
                close(StreamCloseReason::Cancelled).await;
                break;
            }
            if let Some(limit) = bounds.overall_timeout {
                if started.elapsed() > limit {
                    close(StreamCloseReason::Timeout).await;
                    break;
                }
            }
            if let Some(fence) = fence.as_ref() {
                if let Some(kind) = fence.close_reason() {
                    close(close_from_kind(kind)).await;
                    break;
                }
            }

            let next = match timeout(bounds.source_wait, async {
                loop {
                    tokio::select! {
                        biased;
                        item = source.next() => return item,
                        _ = tokio::time::sleep(Duration::from_millis(5)) => {
                            if cancel.is_cancelled() {
                                return None;
                            }
                            if fence.as_ref().is_some_and(|f| f.close_reason().is_some()) {
                                return None;
                            }
                        }
                    }
                }
            })
            .await
            {
                Ok(value) => value,
                Err(_) => {
                    close(StreamCloseReason::Timeout).await;
                    break;
                }
            };

            if cancel.is_cancelled() {
                close(StreamCloseReason::Cancelled).await;
                break;
            }

            let Some(item) = next else {
                close(StreamCloseReason::Completed).await;
                break;
            };
            let token = match item {
                Ok(token) => token,
                Err(()) => {
                    close(StreamCloseReason::ProviderError).await;
                    break;
                }
            };
            if token.is_empty() {
                continue;
            }
            if !token.is_char_boundary(0) || !token.is_char_boundary(token.len()) {
                close(StreamCloseReason::ProviderError).await;
                break;
            }
            let next_bytes = emitted_bytes.saturating_add(token.len());
            if next_bytes > bounds.max_bytes || emitted.saturating_add(1) > bounds.max_tokens {
                close(StreamCloseReason::Truncated).await;
                break;
            }

            let _permit = if let Some(fence) = fence.as_ref() {
                match fence.try_acquire_send() {
                    Ok(permit) => Some(permit),
                    Err(kind) => {
                        close(close_from_kind(kind)).await;
                        break;
                    }
                }
            } else {
                None
            };

            match timeout(bounds.backpressure_wait, tx.send(StreamEvent::Token(token))).await {
                Ok(Ok(())) => {
                    emitted = emitted.saturating_add(1);
                    emitted_bytes = next_bytes;
                }
                Ok(Err(_)) => break,
                Err(_) => {
                    close(StreamCloseReason::Backpressure).await;
                    break;
                }
            }
        }
    });
    rx
}

/// Optional DB identity retained for watchers / session metadata.
#[derive(Clone)]
pub struct DeliveryLockContext {
    pub lock_pool: crate::db::authz_lock::LockPool,
    pub org_id: Uuid,
    pub user_id: Uuid,
    pub document_ids: Vec<Uuid>,
    pub version_ids: Vec<Uuid>,
    pub collection_ids: Vec<Uuid>,
    pub require_history: bool,
    pub captured_epoch: u64,
}

/// Thin event receiver (no per-token locks — session guard lives on the body).
pub struct ProtectedStreamReceiver {
    inner: mpsc::Receiver<StreamEvent>,
    fence: Option<AuthzEpochFence>,
    cancel: StreamCancel,
}

impl ProtectedStreamReceiver {
    pub fn new(
        inner: mpsc::Receiver<StreamEvent>,
        fence: Option<AuthzEpochFence>,
        cancel: StreamCancel,
    ) -> Self {
        Self {
            inner,
            fence,
            cancel,
        }
    }

    pub fn fence(&self) -> Option<&AuthzEpochFence> {
        self.fence.as_ref()
    }

    pub fn into_parts(
        self,
    ) -> (
        mpsc::Receiver<StreamEvent>,
        Option<AuthzEpochFence>,
        StreamCancel,
    ) {
        (self.inner, self.fence, self.cancel)
    }
}

/// Acquire one session delivery guard + exact auth/epoch revalidation.
pub(crate) async fn acquire_session_guard(
    ctx: &DeliveryLockContext,
    fence: Option<&AuthzEpochFence>,
) -> Result<DeliveryGuard, StreamCloseReason> {
    let guard = DeliveryGuard::acquire_shared(
        &ctx.lock_pool,
        ctx.org_id,
        ctx.user_id,
        &ctx.document_ids,
        &ctx.collection_ids,
    )
    .await
    .map_err(|err| match err {
        crate::db::error::DbError::WriterIntent => StreamCloseReason::AuthzRevoked,
        _ => StreamCloseReason::AuthzRevoked,
    })?;
    let org_id = ctx.org_id;
    let user_id = ctx.user_id;
    let document_ids = ctx.document_ids.clone();
    let version_ids = ctx.version_ids.clone();
    let collection_ids = ctx.collection_ids.clone();
    let require_history = ctx.require_history;
    let captured = ctx.captured_epoch;
    let client = match guard.client() {
        Ok(c) => c,
        Err(_) => {
            guard.abandon().await;
            return Err(StreamCloseReason::AuthzRevoked);
        }
    };
    let probe = match search::probe_stream_authz_exact(
        client,
        org_id,
        user_id,
        &document_ids,
        &version_ids,
        &collection_ids,
        require_history,
    )
    .await
    {
        Ok(p) => p,
        Err(_) => {
            guard.abandon().await;
            return Err(StreamCloseReason::AuthzRevoked);
        }
    };
    if !matches!(probe, StreamAuthzProbe::Allow) {
        let reason = match probe {
            StreamAuthzProbe::Deleted => StreamCloseReason::DocumentDeleted,
            _ => StreamCloseReason::AuthzRevoked,
        };
        guard.abandon().await;
        return Err(reason);
    }
    let snap =
        match authz_epoch::read_epoch_snapshot_on_client(client, org_id, user_id, &document_ids)
            .await
        {
            Ok(s) => s,
            Err(_) => {
                guard.abandon().await;
                return Err(StreamCloseReason::AuthzRevoked);
            }
        };
    if snap.composite() != captured {
        guard.abandon().await;
        return Err(StreamCloseReason::AuthzRevoked);
    }
    if let Some(fence) = fence {
        fence.sync_epoch(snap.composite());
        if let Some(kind) = fence.close_reason() {
            guard.abandon().await;
            return Err(close_from_kind(kind));
        }
    }
    Ok(guard)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpochWatchResult {
    Allow { composite_epoch: u64 },
    Revoked,
    Deleted,
}

/// Cross-process fallback: watch DB epoch + full permission/doc probe.
pub fn spawn_epoch_watch<F, Fut>(cancel: StreamCancel, fence: AuthzEpochFence, mut probe: F)
where
    F: FnMut() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = EpochWatchResult> + Send + 'static,
{
    tokio::spawn(async move {
        loop {
            if cancel.is_cancelled() || fence.close_reason().is_some() {
                break;
            }
            match probe().await {
                EpochWatchResult::Allow { composite_epoch } => {
                    fence.sync_epoch(composite_epoch);
                }
                EpochWatchResult::Revoked => {
                    fence.signal_revoked();
                    break;
                }
                EpochWatchResult::Deleted => {
                    fence.signal_deleted();
                    break;
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(200)) => {}
                _ = cancel.cancelled() => break,
            }
        }
    });
}

/// Compatibility alias used by Q&A live watchers.
pub fn spawn_db_authz_watch<F, Fut>(cancel: StreamCancel, fence: AuthzEpochFence, probe: F)
where
    F: FnMut() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = EpochWatchResult> + Send + 'static,
{
    spawn_epoch_watch(cancel, fence, probe);
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream;

    async fn poll_one_frame(body: &mut GuardedSseBody) -> Option<Result<Bytes, StreamError>> {
        let frame = std::future::poll_fn(|cx| Pin::new(&mut *body).poll_frame(cx)).await?;
        Some(frame.map(|f| f.into_data().unwrap_or_else(|_| Bytes::new())))
    }

    fn body_from_rx(
        raw: mpsc::Receiver<StreamEvent>,
        fence: Option<AuthzEpochFence>,
    ) -> GuardedSseBody {
        GuardedSseBody::new(
            raw,
            None,
            fence,
            None,
            None,
            DEFAULT_RESPONSE_LIFETIME,
            DEFAULT_STALL_WATCHDOG,
        )
    }

    #[tokio::test]
    async fn revoke_barrier_stops_app_emission() {
        let fence = AuthzEpochFence::new();
        fence.capture(7);
        let tokens: Vec<Result<String, ()>> = (0..80).map(|i| Ok(format!("tok-{i} "))).collect();
        let bounds = StreamBounds {
            max_tokens: 1_000,
            max_bytes: 64 * 1024,
            buffer: 4,
            backpressure_wait: Duration::from_secs(1),
            overall_timeout: Some(Duration::from_secs(5)),
            source_wait: Duration::from_secs(1),
        };
        let raw = run_bounded_stream(
            stream::iter(tokens),
            bounds,
            StreamCancel::new(),
            Some(fence.clone()),
        )
        .await;
        let mut body = body_from_rx(raw, Some(fence.clone()));
        let mut before = 0usize;
        for _ in 0..3 {
            let frame = poll_one_frame(&mut body).await.expect("token frame");
            let data = frame.expect("ok frame");
            assert!(
                !String::from_utf8_lossy(&data).contains("event: close"),
                "expected token before revoke"
            );
            before += 1;
        }
        assert!(before >= 1);
        fence.revoke_and_drain(CloseKind::Revoked).await;
        let (later, reason) = collect_sse_token_text(body).await;
        assert_eq!(reason, StreamCloseReason::AuthzRevoked);
        assert!(
            later.is_empty(),
            "no new app tokens after revoke: before={before} later={later:?}"
        );
    }

    #[tokio::test]
    async fn cancel_wins_while_source_next_would_hang() {
        let cancel = StreamCancel::new();
        let cancel_flag = cancel.clone();
        let (_tx, rx) = tokio::sync::oneshot::channel::<()>();
        let source = futures::stream::once(async move {
            let _ = rx.await;
            Ok::<String, ()>("late".into())
        });
        let bounds = StreamBounds {
            max_tokens: 10,
            max_bytes: 1024,
            buffer: 1,
            backpressure_wait: Duration::from_secs(1),
            overall_timeout: Some(Duration::from_secs(5)),
            source_wait: Duration::from_secs(30),
        };
        let raw = run_bounded_stream(Box::pin(source), bounds, cancel, None).await;
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            cancel_flag.cancel();
        });
        let (body, reason) = collect_sse_token_text(body_from_rx(raw, None)).await;
        assert_eq!(reason, StreamCloseReason::Cancelled);
        assert!(body.is_empty() || !body.contains("late"));
    }

    #[tokio::test]
    async fn tokenize_preserves_full_text_on_char_boundaries() {
        let text = "Kinh phí phê duyệt là 15 triệu đồng.";
        let tokens = tokenize_for_stream(text);
        assert_eq!(tokens.concat(), text);
        for token in &tokens {
            assert!(token.is_char_boundary(0) && token.is_char_boundary(token.len()));
        }
    }

    #[tokio::test]
    async fn delete_during_stream_closes_without_leaking_further_text() {
        let fence = AuthzEpochFence::new();
        fence.capture(9);
        let tokens: Vec<Result<String, ()>> = (0..40).map(|i| Ok(format!("secret-{i} "))).collect();
        let bounds = StreamBounds::default();
        let raw = run_bounded_stream(
            stream::iter(tokens),
            bounds,
            StreamCancel::new(),
            Some(fence.clone()),
        )
        .await;
        let mut body = body_from_rx(raw, Some(fence.clone()));
        let _ = poll_one_frame(&mut body).await;
        tokio::time::sleep(Duration::from_millis(5)).await;
        fence.signal_deleted();
        let (body, reason) = collect_sse_token_text(body).await;
        assert_eq!(reason, StreamCloseReason::DocumentDeleted);
        assert!(
            body.is_empty() || !body.contains("secret-19"),
            "buffered tail must not leak: {body:?}"
        );
    }

    #[tokio::test]
    async fn sse_json_token_preserves_newlines() {
        let tokens: Vec<Result<String, ()>> = vec![Ok("line1\nline2".into())];
        let bounds = StreamBounds::default();
        let raw = run_bounded_stream(stream::iter(tokens), bounds, StreamCancel::new(), None).await;
        let (body, reason) = collect_sse_token_text(body_from_rx(raw, None)).await;
        assert_eq!(reason, StreamCloseReason::Completed);
        assert_eq!(body, "line1\nline2");
    }

    #[tokio::test]
    async fn stall_watchdog_closes_and_releases() {
        let (tx, rx) = mpsc::channel::<StreamEvent>(4);
        let _keep = tx; // never send — body stalls waiting
        let body = GuardedSseBody::new(
            rx,
            None,
            None,
            Some(StreamCancel::new()),
            None,
            DEFAULT_RESPONSE_LIFETIME,
            Duration::from_millis(80),
        );
        // Do not poll — watchdog must still fire independently.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let (rest, reason) = collect_sse_token_text(body).await;
        assert_eq!(reason, StreamCloseReason::StallWatchdog);
        assert!(rest.is_empty());
    }

    #[tokio::test]
    async fn tower_body_stalled_drop_end_and_mutation_ordering() {
        use axum::body::Body as AxumBody;
        use http_body_util::BodyExt;

        let fence = AuthzEpochFence::new();
        fence.capture(1);
        let tokens: Vec<Result<String, ()>> = (0..10).map(|i| Ok(format!("tok-{i} "))).collect();
        let raw = run_bounded_stream(
            stream::iter(tokens),
            StreamBounds::default(),
            StreamCancel::new(),
            Some(fence.clone()),
        )
        .await;
        let body = GuardedSseBody::new(
            raw,
            None,
            Some(fence.clone()),
            None,
            None,
            DEFAULT_RESPONSE_LIFETIME,
            DEFAULT_STALL_WATCHDOG,
        );

        // Stalled: poll one frame then drop body (guard released on drop).
        {
            let mut b = body;
            let _ = poll_one_frame(&mut b).await;
            drop(b);
        }

        // End: wrap in Axum Body and collect (Tower/Axum body path).
        let fence2 = AuthzEpochFence::new();
        fence2.capture(2);
        let tokens2: Vec<Result<String, ()>> = vec![Ok("hello ".into()), Ok("world".into())];
        let raw2 = run_bounded_stream(
            stream::iter(tokens2),
            StreamBounds::default(),
            StreamCancel::new(),
            Some(fence2.clone()),
        )
        .await;
        let sse = GuardedSseBody::new(
            raw2,
            None,
            Some(fence2),
            None,
            Some(Bytes::from(r#"{"grounded":true}"#)),
            DEFAULT_RESPONSE_LIFETIME,
            DEFAULT_STALL_WATCHDOG,
        );
        let axum_body = AxumBody::new(sse);
        let bytes = axum_body.collect().await.unwrap().to_bytes();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("event: metadata"));
        assert!(text.contains("hello"));
        assert!(text.contains("world"));

        // Mutation ordering: revoke_and_drain then remainder yields close only
        // (app emission stop — not a zero-network-byte claim).
        let fence3 = AuthzEpochFence::new();
        fence3.capture(3);
        let raw3 = run_bounded_stream(
            stream::iter(vec![
                Ok::<_, ()>("x".into()),
                Ok("y".into()),
                Ok("z".into()),
            ]),
            StreamBounds::default(),
            StreamCancel::new(),
            Some(fence3.clone()),
        )
        .await;
        let mut b3 = GuardedSseBody::new(
            raw3,
            None,
            Some(fence3.clone()),
            None,
            None,
            DEFAULT_RESPONSE_LIFETIME,
            DEFAULT_STALL_WATCHDOG,
        );
        let _ = poll_one_frame(&mut b3).await;
        fence3.revoke_and_drain(CloseKind::Revoked).await;
        let (rest, reason) = collect_sse_token_text(b3).await;
        assert_eq!(reason, StreamCloseReason::AuthzRevoked);
        assert!(rest.is_empty(), "no new app tokens after revoke: {rest:?}");
    }

    #[tokio::test]
    async fn guarded_body_holds_through_frame_poll() {
        let tokens: Vec<Result<String, ()>> = vec![Ok("a".into())];
        let raw = run_bounded_stream(
            stream::iter(tokens),
            StreamBounds::default(),
            StreamCancel::new(),
            None,
        )
        .await;
        let mut body = body_from_rx(raw, None);
        let frame = poll_one_frame(&mut body).await;
        assert!(frame.is_some());
    }
}
