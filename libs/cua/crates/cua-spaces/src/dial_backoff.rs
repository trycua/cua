// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Backoff for your computers that do not answer.
//!
//! The apps poll the Space list every few seconds and connect to every
//! Space on it, so a computer that is offline was dialed again on every
//! poll: a mesh probe, a relay round trip and a TLS connection each time,
//! for each such computer (F53). After a failed connection to a computer
//! the relay's directory lists as offline, it is not dialed again for
//! [`FIRST`], doubling up to [`LONGEST`]; meanwhile its connection fails at
//! once with the same reason. The directory says when it comes back
//! (online, or connected again since): that ends the wait at once, so a
//! computer that returns is reachable on the next poll. A computer listed
//! online is always dialed (its cua-spacesd may be a moment from
//! connecting).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The first wait after a failed connection.
pub const FIRST: Duration = Duration::from_secs(20);
/// The longest wait.
pub const LONGEST: Duration = Duration::from_secs(5 * 60);

/// What the relay's directory said about a computer: online, and when it
/// last connected. A change ends the wait.
pub type Presence = Option<(bool, Option<u64>)>;

struct Failed {
    until: Instant,
    wait: Duration,
    presence: Presence,
    reason: String,
}

/// Failed connections by Space id.
#[derive(Default)]
pub struct DialBackoff {
    failed: Mutex<HashMap<String, Failed>>,
}

impl DialBackoff {
    /// Why `space` is not dialed now (its last failure), or `None` to dial.
    pub fn check(&self, space: &str, presence: Presence, now: Instant) -> Option<String> {
        let mut failed = self.failed.lock().unwrap_or_else(|e| e.into_inner());
        let f = failed.get(space)?;
        if f.presence != presence {
            // It came back (or went away and returned): dial now.
            failed.remove(space);
            return None;
        }
        if now >= f.until {
            return None;
        }
        let left = f.until.saturating_duration_since(now).as_secs().max(1);
        Some(format!("{} (not retried for {left} s)", f.reason))
    }

    /// Records a failed connection to `space`.
    pub fn fail(&self, space: &str, presence: Presence, reason: String, now: Instant) {
        let mut failed = self.failed.lock().unwrap_or_else(|e| e.into_inner());
        let wait = match failed.get(space) {
            Some(f) if f.presence == presence => (f.wait * 2).min(LONGEST),
            _ => FIRST,
        };
        failed.insert(
            space.to_string(),
            Failed {
                until: now + wait,
                wait,
                presence,
                reason,
            },
        );
    }

    /// A connection to `space` worked.
    pub fn clear(&self, space: &str) {
        self.failed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(space);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OFFLINE: Presence = Some((false, Some(100)));

    #[test]
    fn a_computer_that_does_not_answer_waits_longer_each_time() {
        let b = DialBackoff::default();
        let t0 = Instant::now();
        assert_eq!(b.check("relay:m", OFFLINE, t0), None);
        b.fail("relay:m", OFFLINE, "offline".into(), t0);
        // Polled every 10 s: not dialed until the first wait passes.
        let why = b
            .check("relay:m", OFFLINE, t0 + Duration::from_secs(10))
            .unwrap();
        assert!(why.starts_with("offline"), "{why}");
        let t1 = t0 + FIRST;
        assert_eq!(b.check("relay:m", OFFLINE, t1), None);
        b.fail("relay:m", OFFLINE, "offline".into(), t1);
        assert!(b.check("relay:m", OFFLINE, t1 + FIRST).is_some());
        assert_eq!(b.check("relay:m", OFFLINE, t1 + FIRST * 2), None);
        // Capped.
        let mut t = t1 + FIRST * 2;
        for _ in 0..10 {
            b.fail("relay:m", OFFLINE, "offline".into(), t);
            t += LONGEST;
            assert_eq!(b.check("relay:m", OFFLINE, t), None);
        }
    }

    #[test]
    fn a_computer_that_comes_back_is_dialed_at_once() {
        let b = DialBackoff::default();
        let t0 = Instant::now();
        b.fail("relay:m", OFFLINE, "offline".into(), t0);
        let back = Some((true, Some(200)));
        assert_eq!(b.check("relay:m", back, t0 + Duration::from_secs(1)), None);
        // And its next failure starts over at the first wait.
        b.fail("relay:m", back, "refused".into(), t0);
        assert!(
            b.check("relay:m", back, t0 + FIRST - Duration::from_secs(1))
                .is_some()
        );
        assert_eq!(b.check("relay:m", back, t0 + FIRST), None);
    }

    #[test]
    fn a_working_connection_clears_the_wait() {
        let b = DialBackoff::default();
        let t0 = Instant::now();
        b.fail("relay:m", OFFLINE, "offline".into(), t0);
        b.clear("relay:m");
        assert_eq!(b.check("relay:m", OFFLINE, t0), None);
    }
}
