// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A process-wide single-flight gate for blocking platform calls that cannot
//! be cancelled once they start (COM, WinRT, and similar provider calls).
//!
//! A caller that gives up on a timed-out worker cannot stop it. The worker
//! therefore keeps the gate's permit until the blocking call actually returns,
//! so retries fail fast instead of stranding one more thread per attempt. When
//! a timed-out worker finally returns, an optional cooldown keeps a hot retry
//! loop from immediately re-entering the same unhealthy provider.
//!
//! The gate owns only admission. Each adapter keeps its own worker spawning,
//! deadline, and error type.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Where a [`SingleFlight`] gate is. Only [`SingleFlight::try_acquire`],
/// [`SingleFlightDeadline::expire`] and the permit's drop change it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Gate {
    /// No worker holds the gate.
    Open,
    /// One worker holds the gate. `timed_out` means its caller gave up on it,
    /// not that it stopped: it still holds the gate until its call returns.
    Busy { timed_out: bool },
    /// A timed-out worker returned; new work is refused until `until`.
    Cooling { until: Instant },
}

#[derive(Debug)]
pub struct SingleFlight {
    gate: Mutex<Gate>,
    cooldown: Duration,
}

impl SingleFlight {
    /// A gate that, after a timed-out worker returns, refuses new work for
    /// `cooldown`. Pass [`Duration::ZERO`] to reopen as soon as it returns.
    pub const fn new(cooldown: Duration) -> Self {
        Self {
            gate: Mutex::new(Gate::Open),
            cooldown,
        }
    }

    /// Admit one worker, or return `None` while another worker holds the gate
    /// or the recovery cooldown is running. Move the permit into the worker so
    /// the gate stays closed until the blocking call returns.
    pub fn try_acquire(self: &Arc<Self>) -> Option<SingleFlightPermit> {
        let mut gate = self.lock();
        match *gate {
            Gate::Busy { .. } => return None,
            Gate::Cooling { until } if Instant::now() < until => return None,
            Gate::Open | Gate::Cooling { .. } => {}
        }
        *gate = Gate::Busy { timed_out: false };
        Some(SingleFlightPermit {
            gate: Arc::clone(self),
            timed_out: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Whether a worker currently holds the gate.
    pub fn is_in_flight(&self) -> bool {
        matches!(*self.lock(), Gate::Busy { .. })
    }

    fn lock(&self) -> MutexGuard<'_, Gate> {
        self.gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The gate once a timed-out worker has returned.
    fn after_timeout(&self) -> Gate {
        if self.cooldown.is_zero() {
            Gate::Open
        } else {
            Gate::Cooling {
                until: Instant::now() + self.cooldown,
            }
        }
    }
}

/// Ownership of a [`SingleFlight`] gate. Dropping it reopens the gate, or
/// starts the cooldown if the caller's deadline expired first.
#[derive(Debug)]
pub struct SingleFlightPermit {
    gate: Arc<SingleFlight>,
    timed_out: Arc<AtomicBool>,
}

impl SingleFlightPermit {
    /// The flag set when the caller's deadline expires. Workers that can stop
    /// early poll it as their cancellation signal.
    pub fn timeout_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.timed_out)
    }

    /// Whether the caller gave up on this worker.
    pub fn timed_out(&self) -> bool {
        self.timed_out.load(Ordering::Acquire)
    }

    /// The caller's handle for giving up on this worker. Take it before
    /// moving the permit into the worker.
    pub fn deadline(&self) -> SingleFlightDeadline {
        SingleFlightDeadline {
            gate: Arc::clone(&self.gate),
            timed_out: Arc::clone(&self.timed_out),
        }
    }
}

impl Drop for SingleFlightPermit {
    fn drop(&mut self) {
        let mut gate = self.gate.lock();
        *gate = if *gate == (Gate::Busy { timed_out: true }) {
            self.gate.after_timeout()
        } else {
            Gate::Open
        };
    }
}

/// The caller's side of a [`SingleFlightPermit`].
#[derive(Debug)]
pub struct SingleFlightDeadline {
    gate: Arc<SingleFlight>,
    timed_out: Arc<AtomicBool>,
}

impl SingleFlightDeadline {
    /// The caller's deadline fired. Signal the worker, and mark the gate so
    /// the worker's return starts the cooldown. A worker that returned
    /// between the deadline and this call already reopened the gate, so the
    /// cooldown starts now instead.
    pub fn expire(&self) {
        self.timed_out.store(true, Ordering::Release);
        let mut gate = self.gate.lock();
        *gate = match *gate {
            Gate::Busy { .. } => Gate::Busy { timed_out: true },
            Gate::Open | Gate::Cooling { .. } => self.gate.after_timeout(),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(gate: &SingleFlight) -> Gate {
        *gate.lock()
    }

    #[test]
    fn a_worker_that_returns_in_time_reopens_the_gate_at_once() {
        let gate = Arc::new(SingleFlight::new(Duration::from_secs(60)));
        assert_eq!(state(&gate), Gate::Open);

        let permit = gate.try_acquire().expect("open gate admits");
        assert_eq!(state(&gate), Gate::Busy { timed_out: false });
        assert!(gate.is_in_flight());
        assert!(gate.try_acquire().is_none(), "second worker must wait");

        drop(permit);
        assert_eq!(state(&gate), Gate::Open);
        assert!(!gate.is_in_flight());
        assert!(gate.try_acquire().is_some(), "reopened without cooldown");
    }

    #[test]
    fn a_timed_out_worker_keeps_the_gate_then_cools_down_then_reopens() {
        let gate = Arc::new(SingleFlight::new(Duration::from_millis(50)));
        let permit = gate.try_acquire().expect("open gate admits");
        let deadline = permit.deadline();

        deadline.expire();
        assert_eq!(state(&gate), Gate::Busy { timed_out: true });
        assert!(permit.timed_out());
        assert!(permit.timeout_flag().load(Ordering::Acquire));
        assert!(gate.is_in_flight(), "a timeout does not stop the worker");
        assert!(gate.try_acquire().is_none());

        drop(permit);
        assert!(matches!(state(&gate), Gate::Cooling { .. }));
        assert!(!gate.is_in_flight());
        assert!(gate.try_acquire().is_none(), "late return starts cooldown");

        std::thread::sleep(Duration::from_millis(80));
        let permit = gate.try_acquire().expect("gate recovers after cooldown");
        assert_eq!(state(&gate), Gate::Busy { timed_out: false });
        drop(permit);
        assert_eq!(state(&gate), Gate::Open);
    }

    #[test]
    fn a_worker_that_returns_before_its_deadline_is_observed_still_cools_down() {
        let gate = Arc::new(SingleFlight::new(Duration::from_secs(60)));
        let permit = gate.try_acquire().unwrap();
        let deadline = permit.deadline();
        drop(permit);
        assert_eq!(state(&gate), Gate::Open);

        deadline.expire();
        assert!(matches!(state(&gate), Gate::Cooling { .. }));
        assert!(gate.try_acquire().is_none());
    }

    #[test]
    fn zero_cooldown_reopens_as_soon_as_the_timed_out_worker_returns() {
        let gate = Arc::new(SingleFlight::new(Duration::ZERO));
        let permit = gate.try_acquire().unwrap();
        permit.deadline().expire();
        assert_eq!(state(&gate), Gate::Busy { timed_out: true });
        assert!(gate.try_acquire().is_none());
        drop(permit);
        assert_eq!(state(&gate), Gate::Open);
        assert!(gate.try_acquire().is_some());
    }
}
