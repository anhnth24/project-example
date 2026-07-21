//! In-process authorization epoch fence for zero-token-after-revoke delivery.
//!
//! Delivery acquires a send permit bound to the captured epoch. Revocation bumps
//! the epoch and waits until all in-flight sends complete before the mutation
//! path may commit/ack. Cross-process bumps arrive via [`sync_epoch`].

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::sync::Notify;

/// Shared fence for one protected stream session.
#[derive(Debug, Clone)]
pub struct AuthzEpochFence {
    inner: Arc<FenceInner>,
}

#[derive(Debug)]
struct FenceInner {
    /// Captured composite epoch at initial full auth (0 = unset / revoked).
    captured: AtomicU64,
    /// Live epoch; bump invalidates when ≠ captured.
    live: AtomicU64,
    in_flight: AtomicUsize,
    notify: Notify,
    revoked: AtomicU64, // 0 allow, 1 revoked, 2 deleted
}

const REASON_NONE: u64 = 0;
const REASON_REVOKED: u64 = 1;
const REASON_DELETED: u64 = 2;

impl Default for AuthzEpochFence {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthzEpochFence {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(FenceInner {
                captured: AtomicU64::new(0),
                live: AtomicU64::new(0),
                in_flight: AtomicUsize::new(0),
                notify: Notify::new(),
                revoked: AtomicU64::new(REASON_NONE),
            }),
        }
    }

    /// Initial synchronous capture after full auth succeeds.
    pub fn capture(&self, composite_epoch: u64) {
        self.inner.captured.store(composite_epoch, Ordering::SeqCst);
        self.inner.live.store(composite_epoch, Ordering::SeqCst);
        self.inner.revoked.store(REASON_NONE, Ordering::SeqCst);
    }

    pub fn captured_epoch(&self) -> u64 {
        self.inner.captured.load(Ordering::SeqCst)
    }

    pub fn is_valid(&self) -> bool {
        let captured = self.inner.captured.load(Ordering::SeqCst);
        let live = self.inner.live.load(Ordering::SeqCst);
        captured != 0
            && captured == live
            && self.inner.revoked.load(Ordering::SeqCst) == REASON_NONE
    }

    /// Cross-process epoch observation — invalidate when DB epoch advances.
    pub fn sync_epoch(&self, composite_epoch: u64) {
        let captured = self.inner.captured.load(Ordering::SeqCst);
        if captured != 0 && composite_epoch != captured {
            self.inner.live.store(composite_epoch, Ordering::SeqCst);
            self.inner.revoked.store(REASON_REVOKED, Ordering::SeqCst);
            self.inner.notify.notify_waiters();
        } else {
            self.inner.live.store(composite_epoch, Ordering::SeqCst);
        }
    }

    pub fn signal_revoked(&self) {
        let next = self.inner.live.load(Ordering::SeqCst).saturating_add(1);
        self.inner.live.store(next.max(1), Ordering::SeqCst);
        self.inner.revoked.store(REASON_REVOKED, Ordering::SeqCst);
        self.inner.notify.notify_waiters();
    }

    pub fn signal_deleted(&self) {
        let next = self.inner.live.load(Ordering::SeqCst).saturating_add(1);
        self.inner.live.store(next.max(1), Ordering::SeqCst);
        self.inner.revoked.store(REASON_DELETED, Ordering::SeqCst);
        self.inner.notify.notify_waiters();
    }

    pub fn close_reason(&self) -> Option<CloseKind> {
        match self.inner.revoked.load(Ordering::SeqCst) {
            REASON_REVOKED => Some(CloseKind::Revoked),
            REASON_DELETED => Some(CloseKind::Deleted),
            _ if !self.is_valid() => Some(CloseKind::Revoked),
            _ => None,
        }
    }

    /// Acquire a send permit; fails if epoch already invalidated.
    pub fn try_acquire_send(&self) -> Result<SendPermit, CloseKind> {
        if let Some(kind) = self.close_reason() {
            return Err(kind);
        }
        self.inner.in_flight.fetch_add(1, Ordering::SeqCst);
        // Re-check after increment so a concurrent revoke cannot slip a send.
        if let Some(kind) = self.close_reason() {
            self.release_send();
            return Err(kind);
        }
        Ok(SendPermit {
            fence: self.clone(),
        })
    }

    fn release_send(&self) {
        let prev = self.inner.in_flight.fetch_sub(1, Ordering::SeqCst);
        if prev == 1 {
            self.inner.notify.notify_waiters();
        }
    }

    pub fn in_flight(&self) -> usize {
        self.inner.in_flight.load(Ordering::SeqCst)
    }

    /// Invalidate epoch and wait until in-flight sends drain (mutation barrier).
    pub async fn revoke_and_drain(&self, kind: CloseKind) {
        match kind {
            CloseKind::Revoked => self.signal_revoked(),
            CloseKind::Deleted => self.signal_deleted(),
        }
        while self.inner.in_flight.load(Ordering::SeqCst) > 0 {
            let notified = self.inner.notify.notified();
            if self.inner.in_flight.load(Ordering::SeqCst) == 0 {
                break;
            }
            notified.await;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseKind {
    Revoked,
    Deleted,
}

/// RAII send permit — holding it means a token may be delivered.
pub struct SendPermit {
    fence: AuthzEpochFence,
}

impl Drop for SendPermit {
    fn drop(&mut self) {
        self.fence.release_send();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn revoke_drains_in_flight_before_returning() {
        let fence = AuthzEpochFence::new();
        fence.capture(10);
        let permit = fence.try_acquire_send().expect("permit");
        assert_eq!(fence.in_flight(), 1);
        let fence2 = fence.clone();
        let handle = tokio::spawn(async move {
            fence2.revoke_and_drain(CloseKind::Revoked).await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(fence.close_reason().is_some());
        // Drain cannot finish while permit held.
        assert!(!handle.is_finished());
        drop(permit);
        handle.await.unwrap();
        assert_eq!(fence.in_flight(), 0);
        assert!(fence.try_acquire_send().is_err());
    }
}
