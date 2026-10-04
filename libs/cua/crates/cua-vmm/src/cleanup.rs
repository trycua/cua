//! Clean-up a dropped operation leaves behind to finish.
//!
//! A create can be cancelled at any await point: its future is dropped.
//! Work that outlives the future (a pull `lume serve` runs on its own, a
//! claim a server already made) is undone by a drop guard, which cannot
//! await; it hands the clean-up to [`spawn`]. Whoever cancelled awaits
//! [`settle`] before saying the create is gone, so "cancelled" means the
//! clean-up finished (or ran out of time), not that it was started.

use std::future::Future;
use std::sync::Mutex;
use std::time::Duration;

static PENDING: Mutex<Vec<tokio::task::JoinHandle<()>>> = Mutex::new(Vec::new());

/// Runs `fut` on the current tokio runtime and remembers it for [`settle`].
/// Outside a runtime (a guard dropped at process exit) nothing runs.
pub fn spawn<F>(fut: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    if let Ok(rt) = tokio::runtime::Handle::try_current() {
        let handle = rt.spawn(fut);
        let mut pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
        pending.retain(|h| !h.is_finished());
        pending.push(handle);
    }
}

/// Waits (at most `budget`) for every clean-up [`spawn`] started. Returns
/// whether they all finished.
pub async fn settle(budget: Duration) -> bool {
    let handles: Vec<_> = std::mem::take(&mut *PENDING.lock().unwrap_or_else(|e| e.into_inner()));
    tokio::time::timeout(budget, futures::future::join_all(handles))
        .await
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[tokio::test]
    async fn settle_waits_for_what_a_guard_spawned() {
        let done = Arc::new(AtomicBool::new(false));
        let d = done.clone();
        spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            d.store(true, Ordering::SeqCst);
        });
        assert!(settle(Duration::from_secs(5)).await);
        assert!(
            done.load(Ordering::SeqCst),
            "settle returned before the clean-up ran"
        );
    }
}
