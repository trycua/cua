//! Token buckets: the rate limits on public links.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

struct Bucket {
    tokens: f64,
    at: Instant,
}

/// A token bucket per key: `rate` requests a second, bursts up to `burst`.
pub struct RateLimiter {
    rate: f64,
    burst: f64,
    buckets: Mutex<HashMap<String, Bucket>>,
}

impl RateLimiter {
    /// A limiter allowing `rate` per second with bursts of `burst`.
    pub fn new(rate: f64, burst: f64) -> Self {
        Self {
            rate,
            burst,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Takes a token for `key`: `Ok`, or how long until one is free.
    pub fn check(&self, key: &str) -> Result<(), Duration> {
        self.check_at(key, Instant::now())
    }

    fn check_at(&self, key: &str, now: Instant) -> Result<(), Duration> {
        let mut g = self.buckets.lock().expect("limiter");
        if g.len() > 4096 {
            // Forget keys idle long enough to be full again.
            let idle = Duration::from_secs_f64((self.burst / self.rate).max(1.0));
            g.retain(|_, b| now.duration_since(b.at) < idle);
        }
        let b = g.entry(key.to_string()).or_insert(Bucket {
            tokens: self.burst,
            at: now,
        });
        let elapsed = now.duration_since(b.at).as_secs_f64();
        b.tokens = (b.tokens + elapsed * self.rate).min(self.burst);
        b.at = now;
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            Ok(())
        } else {
            Err(Duration::from_secs_f64(
                ((1.0 - b.tokens) / self.rate).max(0.001),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bursts_then_refuses_then_refills() {
        let l = RateLimiter::new(2.0, 3.0);
        let t0 = Instant::now();
        for _ in 0..3 {
            assert!(l.check_at("k", t0).is_ok());
        }
        let wait = l.check_at("k", t0).unwrap_err();
        assert!(wait <= Duration::from_millis(500) && wait > Duration::ZERO);
        // Another key has its own bucket.
        assert!(l.check_at("other", t0).is_ok());
        assert!(l.check_at("k", t0 + Duration::from_millis(600)).is_ok());
    }
}
