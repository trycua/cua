//! Keyed scheduling for native action resources.
//!
//! The scheduler owns one bounded lease lifecycle for physical desktop input
//! and fail-fast process-scoped text admission.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum NativeActionResource {
    PhysicalDesktop,
    TextInputProcess(i64),
}

pub(crate) struct NativeActionScheduler {
    gates: Mutex<HashMap<NativeActionResource, Weak<AsyncMutex<()>>>>,
}

impl Default for NativeActionScheduler {
    fn default() -> Self {
        Self {
            gates: Mutex::new(HashMap::new()),
        }
    }
}

impl NativeActionScheduler {
    fn new() -> Self {
        Self::default()
    }

    fn gate(&self, resource: NativeActionResource) -> Arc<AsyncMutex<()>> {
        let mut gates = self
            .gates
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(gate) = gates.get(&resource).and_then(Weak::upgrade) {
            return gate;
        }

        // Weak entries do not keep completed process resources alive. Prune
        // them when admitting a new resource so transient pids stay bounded.
        gates.retain(|_, gate| gate.strong_count() > 0);
        let gate = Arc::new(AsyncMutex::new(()));
        gates.insert(resource, Arc::downgrade(&gate));
        gate
    }

    pub(crate) async fn lock(&self, resource: NativeActionResource) -> OwnedMutexGuard<()> {
        self.gate(resource).lock_owned().await
    }

    pub(crate) fn try_lock(&self, resource: NativeActionResource) -> Option<OwnedMutexGuard<()>> {
        self.gate(resource).try_lock_owned().ok()
    }

    #[cfg(test)]
    fn retained_gate_count(&self) -> usize {
        self.gates
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }
}

pub(crate) fn global() -> &'static NativeActionScheduler {
    static SCHEDULER: OnceLock<NativeActionScheduler> = OnceLock::new();
    SCHEDULER.get_or_init(NativeActionScheduler::new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn text(pid: i64) -> NativeActionResource {
        NativeActionResource::TextInputProcess(pid)
    }

    #[tokio::test]
    async fn same_resource_serializes_while_independent_resources_proceed() {
        let scheduler = NativeActionScheduler::new();
        let first = scheduler.lock(text(7)).await;

        assert!(scheduler.try_lock(text(7)).is_none());
        tokio::time::timeout(Duration::from_millis(20), scheduler.lock(text(8)))
            .await
            .expect("independent resource should not block");

        drop(first);
        tokio::time::timeout(Duration::from_millis(20), scheduler.lock(text(7)))
            .await
            .expect("completed resource should release its lane");
    }

    #[tokio::test]
    async fn cancelled_waiter_does_not_keep_or_poison_a_resource() {
        let scheduler = NativeActionScheduler::new();
        let first = scheduler.lock(text(7)).await;

        assert!(
            tokio::time::timeout(Duration::from_millis(20), scheduler.lock(text(7)))
                .await
                .is_err()
        );
        drop(first);

        tokio::time::timeout(Duration::from_millis(20), scheduler.lock(text(7)))
            .await
            .expect("cancelling a waiter must leave the resource usable");
    }

    #[tokio::test]
    async fn completed_process_gates_are_pruned() {
        let scheduler = NativeActionScheduler::new();
        for pid in 1..65 {
            drop(scheduler.lock(text(pid)).await);
        }

        let _live = scheduler.lock(text(65)).await;
        assert_eq!(scheduler.retained_gate_count(), 1);
    }
}
