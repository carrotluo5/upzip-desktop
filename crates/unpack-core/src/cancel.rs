//! Cooperative cancellation token (cheap clone, wake-on-cancel).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::Notify;

#[derive(Clone, Default)]
pub struct CancelToken {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    flag: AtomicBool,
    notify: Notify,
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.inner.flag.store(true, Ordering::SeqCst);
        self.inner.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.flag.load(Ordering::SeqCst)
    }

    /// Resolves once the token is cancelled (immediately if it already is).
    pub async fn wait_cancelled(&self) {
        let notified = self.inner.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if self.is_cancelled() {
            return;
        }
        notified.await;
    }
}

/// Wait until the gate is open; returns `false` when the token was cancelled first.
pub async fn wait_pause_or_cancel(gate: &PauseGate, cancel: &CancelToken) -> bool {
    loop {
        if cancel.is_cancelled() {
            return false;
        }
        if !gate.is_paused() {
            return true;
        }
        tokio::select! {
            _ = gate.wait_if_paused() => {}
            _ = cancel.wait_cancelled() => return false,
        }
    }
}

/// Simple pause gate: tasks wait here while paused.
#[derive(Default)]
pub struct PauseGate {
    paused: AtomicBool,
    notify: Notify,
}

impl PauseGate {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
    }
    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
        self.notify.notify_waiters();
    }
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }
    pub async fn wait_if_paused(&self) {
        while self.is_paused() {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if !self.is_paused() {
                break;
            }
            notified.await;
        }
    }
}
