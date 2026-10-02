//! Cancellation ownership shared by the ABI and native operations.
use std::sync::{
    atomic::{AtomicU8, Ordering},
    Arc,
};
use tokio::sync::Notify;

tokio::task_local! {
    /// Scoped by the ABI; captured before entering a native worker.
    pub static NATIVE_OPERATION: Arc<NativeOperation>;
}

#[derive(Default)]
pub struct NativeOperation {
    // 0: queued, 1: native work started, 2: cancelled before native work.
    state: AtomicU8,
    changed: Notify,
}

impl NativeOperation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn current() -> Arc<Self> {
        NATIVE_OPERATION.try_with(Arc::clone).unwrap_or_default()
    }

    /// Once native work starts, its owner must await the actual outcome.
    pub fn begin(&self) -> bool {
        self.state
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .map_or_else(|state| state == 1, |_| true)
    }

    pub fn cancel(&self) -> bool {
        let cancelled = self
            .state
            .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire)
            .is_ok();
        if cancelled {
            self.changed.notify_one();
        }
        cancelled
    }

    pub async fn cancelled(&self) {
        loop {
            let changed = self.changed.notified();
            if self.state.load(Ordering::Acquire) == 2 {
                return;
            }
            changed.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_prevents_every_later_native_start() {
        let operation = NativeOperation::new();
        assert!(operation.cancel());
        assert!(!operation.begin());
        assert!(!operation.begin());
    }

    #[test]
    fn started_operation_keeps_ownership_through_subsequent_native_steps() {
        let operation = NativeOperation::new();
        assert!(operation.begin());
        assert!(!operation.cancel());
        assert!(operation.begin());
    }

    #[test]
    fn cancellation_and_native_start_have_one_winner() {
        for _ in 0..100 {
            let operation = Arc::new(NativeOperation::new());
            let barrier = Arc::new(std::sync::Barrier::new(2));
            let other = operation.clone();
            let rendezvous = barrier.clone();
            let start = std::thread::spawn(move || {
                rendezvous.wait();
                other.begin()
            });
            barrier.wait();
            let cancelled = operation.cancel();
            assert_ne!(cancelled, start.join().unwrap());
            assert_eq!(operation.begin(), !cancelled);
        }
    }
}
