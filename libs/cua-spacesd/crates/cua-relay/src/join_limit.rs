// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Admission control for machine joins. After a relay restart or a network
//! blip every machine reconnects within seconds; a thousand TLS handshakes,
//! token checks and directory writes in the same moment starve the streams
//! that are already working. A token bucket admits a burst, then a steady
//! rate; the rest are told to come back after a jittered `Retry-After`
//! (HTTP 503), which the client honours ([`crate::client`]).

use std::time::{Duration, Instant};

/// Joins per second admitted once the burst is used (`join_rate_per_sec`).
pub const DEFAULT_JOIN_RATE_PER_SEC: f64 = 250.0;
/// Joins admitted at once (`join_burst`).
pub const DEFAULT_JOIN_BURST: u32 = 500;

/// A token bucket for joins. `rate <= 0` admits everything.
#[derive(Debug)]
pub struct JoinLimiter {
    rate: f64,
    burst: f64,
    tokens: f64,
    last: Instant,
}

impl JoinLimiter {
    pub fn new(rate_per_sec: f64, burst: u32, now: Instant) -> Self {
        let burst = f64::from(burst.max(1));
        Self { rate: rate_per_sec, burst, tokens: burst, last: now }
    }

    /// Admits one join, or says how long until one would be.
    pub fn admit(&mut self, now: Instant) -> Result<(), Duration> {
        if self.rate <= 0.0 {
            return Ok(());
        }
        let elapsed = now.saturating_duration_since(self.last).as_secs_f64();
        self.last = now;
        self.tokens = (self.tokens + elapsed * self.rate).min(self.burst);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            return Ok(());
        }
        Err(Duration::from_secs_f64((1.0 - self.tokens) / self.rate))
    }
}

/// The `Retry-After` seconds for a refused join: the wait until a token is
/// there, plus up to `spread` more so the refused do not all return together.
/// At least one second (the header is whole seconds).
pub fn retry_after_secs(wait: Duration, spread: Duration, unit_random: f64) -> u64 {
    let extra = spread.as_secs_f64() * unit_random.clamp(0.0, 1.0);
    (wait.as_secs_f64() + extra).ceil().max(1.0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_burst_is_admitted_then_the_rate() {
        let t0 = Instant::now();
        let mut l = JoinLimiter::new(100.0, 10, t0);
        for _ in 0..10 {
            assert!(l.admit(t0).is_ok());
        }
        let wait = l.admit(t0).unwrap_err();
        assert!(wait <= Duration::from_millis(10) && wait > Duration::ZERO, "{wait:?}");
        // 50 ms later: five more.
        let t1 = t0 + Duration::from_millis(50);
        for _ in 0..5 {
            assert!(l.admit(t1).is_ok());
        }
        assert!(l.admit(t1).is_err());
    }

    #[test]
    fn the_bucket_never_holds_more_than_the_burst() {
        let t0 = Instant::now();
        let mut l = JoinLimiter::new(100.0, 10, t0);
        let later = t0 + Duration::from_secs(3600);
        for _ in 0..10 {
            assert!(l.admit(later).is_ok());
        }
        assert!(l.admit(later).is_err());
    }

    #[test]
    fn no_rate_means_no_limit() {
        let t0 = Instant::now();
        let mut l = JoinLimiter::new(0.0, 1, t0);
        for _ in 0..10_000 {
            assert!(l.admit(t0).is_ok());
        }
    }

    #[test]
    fn a_thousand_at_once_settle_at_the_rate() {
        // 1,000 machines at t0; 250/s with a burst of 500: 500 admitted at once,
        // the rest as the bucket refills, all within about two seconds.
        let t0 = Instant::now();
        let mut l = JoinLimiter::new(250.0, 500, t0);
        let mut pending: Vec<usize> = (0..1000).collect();
        let mut now = t0;
        let mut admitted_at_t0 = 0;
        let mut rounds = 0;
        while !pending.is_empty() {
            let mut next = Vec::new();
            for m in pending {
                match l.admit(now) {
                    Ok(()) => {
                        if now == t0 {
                            admitted_at_t0 += 1;
                        }
                    }
                    Err(_) => next.push(m),
                }
            }
            pending = next;
            now += Duration::from_millis(100);
            rounds += 1;
            assert!(rounds < 100, "never settled");
        }
        assert_eq!(admitted_at_t0, 500);
        assert!(now - t0 <= Duration::from_secs(3), "{:?}", now - t0);
    }

    #[test]
    fn retry_after_is_whole_seconds_and_spread() {
        assert_eq!(retry_after_secs(Duration::from_millis(5), Duration::ZERO, 0.0), 1);
        assert_eq!(retry_after_secs(Duration::from_millis(5), Duration::from_secs(4), 1.0), 5);
        assert_eq!(retry_after_secs(Duration::from_secs(2), Duration::from_secs(4), 0.5), 4);
        let spread: std::collections::BTreeSet<u64> =
            (0..=10).map(|i| retry_after_secs(Duration::from_millis(100), Duration::from_secs(5), f64::from(i) / 10.0)).collect();
        assert!(spread.len() >= 5, "{spread:?}");
    }
}
