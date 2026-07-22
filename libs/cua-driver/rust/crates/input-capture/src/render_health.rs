//! Private health signal shared by the indicator and input hook.
//!
//! A recent successful frame submission allows capture; a stalled renderer
//! blocks it. This does not prove that the indicator is unobscured.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const MAX_FRAME_AGE_MS: u64 = 250;

#[derive(Clone)]
pub(crate) struct RenderHealth(Arc<AtomicU64>);

impl RenderHealth {
    pub(crate) fn new() -> Self {
        Self(Arc::new(AtomicU64::new(0)))
    }

    pub(crate) fn submitted(&self, now_ms: u64) {
        self.0.store(now_ms.saturating_add(1), Ordering::Release);
    }

    pub(crate) fn clear(&self) {
        self.0.store(0, Ordering::Release);
    }

    pub(crate) fn is_fresh(&self, now_ms: u64) -> bool {
        let stored = self.0.load(Ordering::Acquire);
        stored != 0 && now_ms.saturating_sub(stored - 1) <= MAX_FRAME_AGE_MS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_a_recent_submission() {
        let health = RenderHealth::new();
        assert!(!health.is_fresh(1000));
        health.submitted(1000);
        assert!(health.is_fresh(1000));
        assert!(!health.is_fresh(1000 + MAX_FRAME_AGE_MS + 1));
        health.clear();
        assert!(!health.is_fresh(1000));
    }
}
