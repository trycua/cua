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

type Done = futures::future::Shared<futures::channel::oneshot::Receiver<()>>;

static PENDING: Mutex<Vec<Done>> = Mutex::new(Vec::new());

/// Runs `fut` on the current tokio runtime and remembers it for [`settle`].
/// Outside a runtime (a guard dropped at process exit) nothing runs.
pub fn spawn<F>(fut: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    use futures::FutureExt;
    if let Ok(rt) = tokio::runtime::Handle::try_current() {
        let (tx, rx) = futures::channel::oneshot::channel::<()>();
        rt.spawn(async move {
            fut.await;
            let _ = tx.send(());
        });
        let mut pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
        pending.retain(|d| d.peek().is_none());
        pending.push(rx.shared());
    }
}

/// Waits (at most `budget`) for every clean-up [`spawn`] started, whichever
/// caller started it: concurrent cancels each wait for all of them, so one
/// cancel can never return while its own clean-up is still running because
/// another took it from the list. Returns whether they all finished.
pub async fn settle(budget: Duration) -> bool {
    let waits: Vec<Done> = {
        let mut pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
        pending.retain(|d| d.peek().is_none());
        pending.clone()
    };
    tokio::time::timeout(budget, futures::future::join_all(waits))
        .await
        .is_ok()
}

/// Runs `create` (a request that makes something outside this process: an
/// engine's container, a claim) to completion even when the caller drops
/// this future. Dropping a client request does not stop the server: it
/// finishes creating after the caller already looked for what to undo and
/// found nothing, which leaves it behind. If the future is dropped first,
/// the request is awaited in the background (under [`settle`]) and
/// `undo` receives what it made.
pub async fn complete_or_undo<T, E, F, U, UF>(create: F, undo: U) -> Result<T, E>
where
    F: Future<Output = Result<T, E>> + Send + 'static,
    T: Send + 'static,
    E: Send + 'static,
    U: FnOnce(T) -> UF + Send + 'static,
    UF: Future<Output = ()> + Send + 'static,
{
    type Undo<T> = Box<dyn FnOnce(T) -> futures::future::BoxFuture<'static, ()> + Send>;
    struct Guard<T: Send + 'static, E: Send + 'static> {
        task: Option<tokio::task::JoinHandle<Result<T, E>>>,
        undo: Option<Undo<T>>,
    }
    impl<T: Send + 'static, E: Send + 'static> Drop for Guard<T, E> {
        fn drop(&mut self) {
            if let (Some(task), Some(undo)) = (self.task.take(), self.undo.take()) {
                spawn(async move {
                    if let Ok(Ok(made)) = task.await {
                        undo(made).await;
                    }
                });
            }
        }
    }
    let mut guard = Guard {
        task: Some(tokio::spawn(create)),
        undo: Some(Box::new(move |t| Box::pin(undo(t)))),
    };
    let task = guard.task.as_mut().expect("armed");
    let joined = task.await;
    guard.task = None;
    match joined {
        Ok(r) => r,
        Err(e) => std::panic::resume_unwind(e.into_panic()),
    }
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

    #[tokio::test]
    async fn a_dropped_request_that_still_completes_is_undone() {
        let made = Arc::new(AtomicBool::new(false));
        let removed = Arc::new(AtomicBool::new(false));
        let (m, r) = (made.clone(), removed.clone());
        // The "engine" finishes 150 ms after the request; the caller gives
        // up after 10 ms, then looks (as the cancel does) and sees nothing.
        let create = async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            m.store(true, Ordering::SeqCst);
            Ok::<_, ()>(7u8)
        };
        let undo = move |v: u8| async move {
            assert_eq!(v, 7);
            r.store(true, Ordering::SeqCst);
        };
        let out =
            tokio::time::timeout(Duration::from_millis(10), complete_or_undo(create, undo)).await;
        assert!(out.is_err(), "the caller was cancelled");
        assert!(
            !made.load(Ordering::SeqCst),
            "nothing existed when it looked"
        );
        assert!(settle(Duration::from_secs(5)).await);
        assert!(made.load(Ordering::SeqCst), "the request completed anyway");
        assert!(
            removed.load(Ordering::SeqCst),
            "and what it made was removed"
        );
    }

    #[tokio::test]
    async fn a_completed_request_is_kept_and_a_failed_one_undoes_nothing() {
        let removed = Arc::new(AtomicBool::new(false));
        let r = removed.clone();
        let ok = complete_or_undo(async { Ok::<_, ()>(1u8) }, move |_| async move {
            r.store(true, Ordering::SeqCst)
        })
        .await;
        assert_eq!(ok, Ok(1));
        let r = removed.clone();
        let err = complete_or_undo(async { Err::<u8, _>("name in use") }, move |_| async move {
            r.store(true, Ordering::SeqCst)
        })
        .await;
        assert_eq!(err, Err("name in use"));
        assert!(!removed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn concurrent_settles_each_wait_for_every_cleanup() {
        let done = Arc::new(AtomicBool::new(false));
        let d = done.clone();
        spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            d.store(true, Ordering::SeqCst);
        });
        let a = tokio::spawn(settle(Duration::from_secs(5)));
        let b = tokio::spawn(settle(Duration::from_secs(5)));
        // Whichever settle ran second used to find the list already taken.
        assert!(b.await.unwrap() && a.await.unwrap());
        assert!(done.load(Ordering::SeqCst));
    }
}
