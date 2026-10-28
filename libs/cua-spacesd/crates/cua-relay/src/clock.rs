// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A clock that reads real time plus a manual advance only tests move.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Real time plus a manual advance that only tests move, so a test makes
/// time pass (a cache entry expiring, a stream staying open long enough) by
/// advancing it rather than by sleeping past, or racing, a real-time bound.
#[derive(Debug, Clone, Default)]
pub struct Clock(Arc<AtomicU64>);

impl Clock {
    /// Moves this clock (and every clone) forward by `by`.
    pub fn advance(&self, by: Duration) {
        self.0.fetch_add(by.as_millis() as u64, Ordering::Relaxed);
    }

    /// Now: `Instant::now()` plus every advance so far.
    pub fn now(&self) -> Instant {
        Instant::now() + Duration::from_millis(self.0.load(Ordering::Relaxed))
    }
}
