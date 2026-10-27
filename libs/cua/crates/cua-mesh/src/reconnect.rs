// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! When the node connects to the coordinator again after a session ends.
//!
//! Exponential backoff with full jitter (a wait drawn uniformly from
//! `0..min(CAP, BASE * 2^attempt)`), so many nodes that lose the relay
//! together do not come back together. The attempt count resets only after
//! a session stayed up for [`HEALTHY`]: a session that is welcomed and then
//! dropped within moments is a failure like a refused connection, so a
//! client the relay keeps dropping slows down instead of retrying at a flat
//! pace. After the relay itself ended a session (a restart, a change), the
//! wait also gets [`spread`], so a fleet told to reconnect at one instant
//! does not do it in the same millisecond.

use std::time::Duration;

/// The window of the first retry.
pub(crate) const BASE: Duration = Duration::from_secs(1);
/// The largest window.
pub(crate) const CAP: Duration = Duration::from_secs(60);
/// A session this long was healthy: the backoff starts over.
pub(crate) const HEALTHY: Duration = Duration::from_secs(60);
/// After a relay-initiated end, the extra random wait is up to this.
pub(crate) const SPREAD: Duration = Duration::from_secs(10);

/// Reconnect pacing for one node.
#[derive(Debug, Default)]
pub(crate) struct Backoff {
    attempt: u32,
}

impl Backoff {
    /// Records that a session ended after `lived` of being welcomed (zero
    /// when it never was) and returns how long to wait before the next
    /// connect. `unit` is a random number in `0.0..=1.0`.
    pub(crate) fn next(&mut self, lived: Duration, unit: f64) -> Duration {
        if lived >= HEALTHY {
            self.attempt = 0;
        }
        let delay = window(self.attempt).mul_f64(unit.clamp(0.0, 1.0));
        self.attempt = self.attempt.saturating_add(1);
        delay
    }

    /// Treats the next retries as at least the `attempt`-th (a refusal is
    /// not worth retrying quickly).
    pub(crate) fn at_least(&mut self, attempt: u32) {
        self.attempt = self.attempt.max(attempt);
    }
}

/// The upper bound of the wait at `attempt` (0 is the first retry).
pub(crate) fn window(attempt: u32) -> Duration {
    BASE.saturating_mul(1u32 << attempt.min(16)).min(CAP)
}

/// The random extra wait after the relay ended a session. `unit` is a
/// random number in `0.0..=1.0`.
pub(crate) fn spread(unit: f64) -> Duration {
    SPREAD.mul_f64(unit.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHORT: Duration = Duration::from_millis(800);

    #[test]
    fn the_window_doubles_from_one_second_to_the_cap() {
        let w: Vec<u64> = (0..9).map(|a| window(a).as_secs()).collect();
        assert_eq!(w, [1, 2, 4, 8, 16, 32, 60, 60, 60]);
        assert_eq!(window(u32::MAX), CAP);
    }

    #[test]
    fn the_wait_is_drawn_from_zero_to_the_window() {
        for attempt in 0..8 {
            let mut lo = Backoff { attempt };
            let mut hi = Backoff { attempt };
            assert_eq!(lo.next(SHORT, 0.0), Duration::ZERO);
            assert_eq!(hi.next(SHORT, 1.0), window(attempt));
            let mut mid = Backoff { attempt };
            assert_eq!(mid.next(SHORT, 0.5), window(attempt) / 2);
        }
        // Out-of-range randomness cannot escape the window.
        assert_eq!(Backoff::default().next(SHORT, 7.0), BASE);
        assert_eq!(Backoff::default().next(SHORT, -1.0), Duration::ZERO);
    }

    #[test]
    fn short_sessions_count_as_failures_and_the_wait_keeps_growing() {
        // The client the relay drops 0.8 s after it connects, every time:
        // the worst case waits must grow to the cap, never stay flat.
        let mut b = Backoff::default();
        let worst: Vec<u64> = (0..8).map(|_| b.next(SHORT, 1.0).as_secs()).collect();
        assert_eq!(worst, [1, 2, 4, 8, 16, 32, 60, 60]);
        // Even a session just under the healthy mark does not reset it.
        let mut b = Backoff::default();
        for _ in 0..4 {
            b.next(Duration::from_secs(59), 1.0);
        }
        assert_eq!(
            b.next(Duration::from_secs(59), 1.0),
            Duration::from_secs(16)
        );
    }

    #[test]
    fn a_failed_connect_counts_the_same() {
        let mut b = Backoff::default();
        let worst: Vec<u64> = (0..4)
            .map(|_| b.next(Duration::ZERO, 1.0).as_secs())
            .collect();
        assert_eq!(worst, [1, 2, 4, 8]);
    }

    #[test]
    fn only_a_session_that_stayed_healthy_resets_it() {
        let mut b = Backoff::default();
        for _ in 0..6 {
            b.next(SHORT, 1.0);
        }
        assert_eq!(b.next(HEALTHY, 1.0), BASE);
        // ... and then it grows again from the start.
        assert_eq!(b.next(SHORT, 1.0), Duration::from_secs(2));
        let mut b = Backoff::default();
        for _ in 0..6 {
            b.next(SHORT, 1.0);
        }
        assert_eq!(b.next(Duration::from_secs(3600), 1.0), BASE);
    }

    #[test]
    fn a_refusal_starts_further_up() {
        let mut b = Backoff::default();
        b.at_least(5);
        assert_eq!(b.next(Duration::ZERO, 1.0), Duration::from_secs(32));
        // It never lowers the attempt.
        b.at_least(1);
        assert_eq!(b.next(Duration::ZERO, 1.0), CAP);
    }

    #[test]
    fn relay_ended_sessions_are_spread_over_ten_seconds() {
        assert_eq!(spread(0.0), Duration::ZERO);
        assert_eq!(spread(0.5), Duration::from_secs(5));
        assert_eq!(spread(1.0), SPREAD);
        assert_eq!(spread(9.0), SPREAD);
    }

    #[test]
    fn a_herd_does_not_reconnect_in_one_instant() {
        // 1000 nodes told to reconnect at once: the draws cover 0..10 s
        // evenly enough that no tenth of a second holds more than a few
        // percent of them.
        let mut buckets = [0u32; 100];
        for _ in 0..1000 {
            let d = spread(rand::random_range(0.0..=1.0));
            assert!(d <= SPREAD);
            buckets[((d.as_millis() / 100) as usize).min(99)] += 1;
        }
        assert!(buckets.iter().all(|&n| n < 50), "{buckets:?}");
        assert!(buckets.iter().filter(|&&n| n > 0).count() > 80);
    }
}
