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
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct SingleFlight {
    in_flight: AtomicBool,
    cooldown: Duration,
    cooldown_until: Mutex<Option<Instant>>,
}

impl SingleFlight {
    /// A gate that, after a timed-out worker returns, refuses new work for
    /// `cooldown`. Pass [`Duration::ZERO`] to reopen as soon as it returns.
    pub const fn new(cooldown: Duration) -> Self {
        Self {
            in_flight: AtomicBool::new(false),
            cooldown,
            cooldown_until: Mutex::new(None),
        }
    }

    /// Admit one worker, or return `None` while another worker holds the gate
    /// or the recovery cooldown is running. Move the permit into the worker so
    /// the gate stays closed until the blocking call returns.
    pub fn try_acquire(self: &Arc<Self>) -> Option<SingleFlightPermit> {
        if self.is_cooling_down()
            || self
                .in_flight
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return None;
        }
        Some(SingleFlightPermit {
            gate: Arc::clone(self),
            timed_out: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Whether a worker currently holds the gate.
    pub fn is_in_flight(&self) -> bool {
        self.in_flight.load(Ordering::Acquire)
    }

    /// Start the recovery cooldown now.
    ///
    /// The permit also arms it when a timed-out worker returns. Callers arm it
    /// when their deadline fires as well, because the worker can return between
    /// the deadline and observing the timeout flag.
    pub fn arm_cooldown(&self) {
        let until = Instant::now() + self.cooldown;
        *self
            .cooldown_until
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(until);
    }

    fn is_cooling_down(&self) -> bool {
        self.cooldown_until
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some_and(|until| Instant::now() < until)
    }
}

/// Ownership of a [`SingleFlight`] gate. Dropping it reopens the gate, after
/// arming the cooldown if the caller marked the work as timed out.
#[derive(Debug)]
pub struct SingleFlightPermit {
    gate: Arc<SingleFlight>,
    timed_out: Arc<AtomicBool>,
}

impl SingleFlightPermit {
    /// The flag the caller sets when its deadline fires. Workers that can
    /// stop early poll it as their cancellation signal.
    pub fn timeout_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.timed_out)
    }

    /// Whether the caller gave up on this worker.
    pub fn timed_out(&self) -> bool {
        self.timed_out.load(Ordering::Acquire)
    }
}

impl Drop for SingleFlightPermit {
    fn drop(&mut self) {
        if self.timed_out() {
            self.gate.arm_cooldown();
        }
        self.gate.in_flight.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_worker_at_a_time_and_cooldown_only_after_a_timed_out_worker() {
        let gate = Arc::new(SingleFlight::new(Duration::from_millis(50)));

        let permit = gate.try_acquire().expect("idle gate admits");
        assert!(gate.is_in_flight());
        assert!(gate.try_acquire().is_none(), "second worker must wait");
        drop(permit);
        assert!(!gate.is_in_flight());

        // A worker that finished in time reopens the gate immediately.
        drop(gate.try_acquire().expect("reopened without cooldown"));

        // A timed-out worker keeps the gate until it returns, then cools down.
        let permit = gate.try_acquire().expect("idle gate admits");
        permit.timeout_flag().store(true, Ordering::Release);
        assert!(permit.timed_out());
        assert!(gate.try_acquire().is_none());
        drop(permit);
        assert!(!gate.is_in_flight());
        assert!(gate.try_acquire().is_none(), "late return starts cooldown");

        std::thread::sleep(Duration::from_millis(80));
        assert!(gate.try_acquire().is_some(), "gate recovers after cooldown");
    }

    #[test]
    fn zero_cooldown_reopens_as_soon_as_the_timed_out_worker_returns() {
        let gate = Arc::new(SingleFlight::new(Duration::ZERO));
        let permit = gate.try_acquire().unwrap();
        permit.timeout_flag().store(true, Ordering::Release);
        gate.arm_cooldown();
        assert!(gate.try_acquire().is_none());
        drop(permit);
        assert!(gate.try_acquire().is_some());
    }
}
