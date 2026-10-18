// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Who is driving the guest pointer, and whether it is idle.
//!
//! Every path that injects input (ComputerService, media `interactive_input`,
//! cua-driver tools through `/mcp` and the Driver service) announces itself
//! here first and holds the pointer lock while it injects. Presence uses the
//! record two ways:
//!
//! - **Ownership:** the participant whose input was injected last, recently,
//!   owns the real pointer, so its cursor shape is the OS's real cursor.
//! - **Idleness:** the cursor-shape probe may move the real pointer only while
//!   nothing is pending and nothing was injected recently, and it holds the
//!   same lock, so a probe and an injection never interleave. An injection
//!   that arrives mid-probe is visible as `pending() > 0` before it waits for
//!   the lock, which makes the probe abort and restore first.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::sync::OwnedMutexGuard;

/// The last injected input.
#[derive(Debug, Clone, PartialEq)]
pub struct LastInput {
    /// `Principal.id` of whoever injected it (empty when unknown).
    pub principal: String,
    /// When it finished.
    pub at: Instant,
    /// Where it left the pointer, in global logical points, when known.
    pub position: Option<(f64, f64)>,
}

/// Input bookkeeping shared by every injection path. See the module docs.
#[derive(Debug)]
pub struct InputActivity {
    lock: Arc<tokio::sync::Mutex<()>>,
    pending: AtomicUsize,
    last: Mutex<Option<LastInput>>,
}

impl Default for InputActivity {
    fn default() -> Self {
        Self {
            lock: Arc::new(tokio::sync::Mutex::new(())),
            pending: AtomicUsize::new(0),
            last: Mutex::new(None),
        }
    }
}

/// The process-wide record every production injection path uses.
pub fn global() -> &'static Arc<InputActivity> {
    static GLOBAL: OnceLock<Arc<InputActivity>> = OnceLock::new();
    GLOBAL.get_or_init(|| Arc::new(InputActivity::default()))
}

impl InputActivity {
    /// A fresh record (tests; production uses [`global`]).
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Announce an injection: counted as pending at once, before the caller
    /// waits for the pointer lock.
    pub fn announce(self: &Arc<Self>, principal: &str) -> Announced {
        self.pending.fetch_add(1, Ordering::SeqCst);
        Announced {
            activity: self.clone(),
            principal: principal.to_owned(),
            position: None,
            done: false,
        }
    }

    /// Record activity that does not inject through a locked path (for
    /// example the driver's cursor hook reporting an agent move).
    pub fn note(&self, principal: &str, position: Option<(f64, f64)>) {
        *self.last.lock().unwrap() = Some(LastInput {
            principal: principal.to_owned(),
            at: Instant::now(),
            position,
        });
    }

    /// Injections announced and not finished.
    pub fn pending(&self) -> usize {
        self.pending.load(Ordering::SeqCst)
    }

    /// The last injected input.
    pub fn last(&self) -> Option<LastInput> {
        self.last.lock().unwrap().clone()
    }

    /// Nothing pending and nothing injected within `quiet`.
    pub fn idle_for(&self, quiet: Duration) -> bool {
        self.pending() == 0 && self.last().is_none_or(|last| last.at.elapsed() >= quiet)
    }

    /// The pointer lock, if nobody holds it (the probe never waits).
    pub fn try_lock_pointer(&self) -> Option<OwnedMutexGuard<()>> {
        self.lock.clone().try_lock_owned().ok()
    }
}

/// An announced injection. Acquire the pointer lock before injecting; the
/// record is written when the guard (or this, if never acquired) drops.
#[derive(Debug)]
pub struct Announced {
    activity: Arc<InputActivity>,
    principal: String,
    position: Option<(f64, f64)>,
    done: bool,
}

impl Announced {
    /// Where the injection leaves the pointer, when known.
    pub fn at(mut self, position: Option<(f64, f64)>) -> Self {
        self.position = position;
        self
    }

    /// Wait for the pointer lock (async paths).
    pub async fn acquire(self) -> InputGuard {
        let guard = self.activity.lock.clone().lock_owned().await;
        InputGuard {
            announced: self,
            _guard: guard,
        }
    }

    /// Wait for the pointer lock from a blocking thread.
    pub fn acquire_blocking(self) -> InputGuard {
        let guard = self.activity.lock.clone().blocking_lock_owned();
        InputGuard {
            announced: self,
            _guard: guard,
        }
    }

    fn finish(&mut self) {
        if self.done {
            return;
        }
        self.done = true;
        self.activity.note(&self.principal, self.position);
        self.activity.pending.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Drop for Announced {
    fn drop(&mut self) {
        self.finish();
    }
}

/// Holds the pointer lock while input is injected.
#[derive(Debug)]
pub struct InputGuard {
    announced: Announced,
    _guard: OwnedMutexGuard<()>,
}

impl InputGuard {
    /// Correct the recorded end position (known only after injecting).
    pub fn set_position(&mut self, position: Option<(f64, f64)>) {
        self.announced.position = position;
    }
}

impl Drop for InputGuard {
    fn drop(&mut self) {
        self.announced.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn announcing_is_pending_until_the_guard_drops_and_records_the_owner() {
        let a = InputActivity::new();
        assert!(a.idle_for(Duration::from_millis(0)));
        let announced = a.announce("alice").at(Some((1.0, 2.0)));
        assert_eq!(a.pending(), 1);
        assert!(!a.idle_for(Duration::from_millis(0)));
        let guard = announced.acquire().await;
        assert!(a.try_lock_pointer().is_none(), "injection holds the lock");
        drop(guard);
        assert_eq!(a.pending(), 0);
        let last = a.last().unwrap();
        assert_eq!(last.principal, "alice");
        assert_eq!(last.position, Some((1.0, 2.0)));
        assert!(
            !a.idle_for(Duration::from_secs(5)),
            "recent input is not idle"
        );
        assert!(a.try_lock_pointer().is_some());
    }

    #[tokio::test]
    async fn a_pending_injection_waits_for_the_probe_lock() {
        let a = InputActivity::new();
        let probe = a.try_lock_pointer().unwrap();
        let announced = a.announce("bob");
        assert_eq!(
            a.pending(),
            1,
            "the probe sees the injection before it waits"
        );
        let waiter = tokio::spawn(async move { announced.acquire().await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!waiter.is_finished());
        drop(probe);
        let guard = tokio::time::timeout(Duration::from_secs(2), waiter)
            .await
            .unwrap()
            .unwrap();
        drop(guard);
        assert_eq!(a.pending(), 0);
    }
}
