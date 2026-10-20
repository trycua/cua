//! The tool call a blocking input worker serves.
//!
//! A tool call stops waiting for its blocking worker at its deadline, but
//! the worker cannot be cancelled: it keeps running on the blocking pool.
//! An input route reads the [`Action`] bound to its thread and hands no
//! further event to the compositor once the call is over. Releasing a held
//! button or key is cleanup and still goes out. The action keeps the call's
//! desktop lease until its last holder drops, so that cleanup ends before
//! the next desktop action starts.

use std::cell::RefCell;
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use cua_driver_core::tool::DesktopLease;

/// One tool call: its deadline, whether its caller stopped waiting, and
/// the desktop lease of the task that started it.
pub struct Action {
    deadline: Instant,
    abandoned: Mutex<bool>,
    fenced: Mutex<(bool, Vec<OwnedFd>)>,
    _lease: Option<DesktopLease>,
}

impl Action {
    pub fn new(budget: Duration) -> Arc<Self> {
        Arc::new(Self {
            deadline: Instant::now() + budget,
            abandoned: Mutex::new(false),
            fenced: Mutex::new((false, Vec::new())),
            _lease: DesktopLease::current(),
        })
    }

    pub fn deadline(&self) -> Instant {
        self.deadline
    }

    /// Run `send`, which hands input events to the compositor, unless the
    /// call is past its deadline or abandoned. [`Action::abandon`] waits for
    /// a `send` under way, so nothing goes out once it has returned. `send`
    /// must not block: queue the events and flush them, then wait for the
    /// compositor outside.
    pub fn admit<T>(&self, send: impl FnOnce() -> T) -> Result<T> {
        let abandoned = self
            .abandoned
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *abandoned || Instant::now() >= self.deadline {
            bail!(
                "{}: the call ran out of time, so no further input was sent",
                super::foreground::CODE_TIMEOUT
            );
        }
        Ok(send())
    }

    /// The caller stopped waiting: refuse every later [`Action::admit`].
    pub fn abandon(&self) {
        let mut fenced = self
            .fenced
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        fenced.0 = true;
        fenced.1.iter().for_each(shut_down);
        drop(fenced);
        *self
            .abandoned
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
    }

    /// Whether the caller stopped waiting, so fenced sockets are shut.
    pub fn is_abandoned(&self) -> bool {
        self.fenced
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .0
    }

    /// Shut `socket` down once this action is abandoned, before waiting for a
    /// send under way, so nothing queued on it is written later.
    pub fn fence(&self, socket: impl AsFd) -> std::io::Result<()> {
        let socket = socket.as_fd().try_clone_to_owned()?;
        let mut fenced = self
            .fenced
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if fenced.0 {
            shut_down(&socket);
        }
        fenced.1.push(socket);
        Ok(())
    }

    /// Abandon this action when the returned guard drops. Hold it where the
    /// caller waits, so a caller that is itself dropped (a cancelled future)
    /// abandons the action as one that timed out does.
    pub fn abandon_on_drop(self: &Arc<Self>) -> AbandonOnDrop {
        AbandonOnDrop(self.clone())
    }
}

fn shut_down(socket: &OwnedFd) {
    unsafe { libc::shutdown(socket.as_raw_fd(), libc::SHUT_RDWR) };
}

/// Abandons its [`Action`] when dropped; see [`Action::abandon_on_drop`].
pub struct AbandonOnDrop(Arc<Action>);

impl Drop for AbandonOnDrop {
    fn drop(&mut self) {
        self.0.abandon();
    }
}

thread_local! {
    static CURRENT: RefCell<Option<Arc<Action>>> = const { RefCell::new(None) };
}

/// The action bound to this thread, if its worker was started for one.
pub fn current() -> Option<Arc<Action>> {
    CURRENT.with(|current| current.borrow().clone())
}

/// Restores the previous binding when dropped.
pub struct Bound(Option<Arc<Action>>);

impl Drop for Bound {
    fn drop(&mut self) {
        let previous = self.0.take();
        CURRENT.with(|current| *current.borrow_mut() = previous);
    }
}

/// Bind `action` to this thread for the input routes it runs.
pub fn bind(action: Arc<Action>) -> Bound {
    Bound(CURRENT.with(|current| current.replace(Some(action))))
}

/// Admit `send` under the action bound to this thread; with none, run it.
pub fn admit<T>(send: impl FnOnce() -> T) -> Result<T> {
    match current() {
        Some(action) => action.admit(send),
        None => Ok(send()),
    }
}

/// Fence `socket` under the action bound to this thread; with none, do nothing.
pub fn fence(socket: impl AsFd) -> std::io::Result<()> {
    current().map_or(Ok(()), |action| action.fence(socket))
}

/// `spawn_blocking` with a hard deadline. A wedged X server (or a client
/// holding a server grab) must surface as a structured `foreground_timeout`
/// error instead of a tool call that never returns. The blocking task is not
/// cancellable; it is left to finish (or leak) on the blocking pool, with its
/// [`Action`] abandoned so its input routes send nothing more. The action
/// keeps the call's desktop lease, so the next desktop action waits until
/// the worker has stopped. Started from a thread already bound to an action, the worker
/// runs under that action, within what is left of its deadline, and its
/// timeout abandons that action too.
pub async fn spawn_blocking_bounded<T: Send + 'static>(
    label: &'static str,
    budget: Duration,
    f: impl FnOnce() -> Result<T> + Send + 'static,
) -> std::result::Result<Result<T>, tokio::task::JoinError> {
    let outer = current();
    let action = outer.clone().unwrap_or_else(|| Action::new(budget));
    let budget = budget.min(action.deadline.saturating_duration_since(Instant::now()));
    let _abandon = outer.is_none().then(|| action.abandon_on_drop());
    let worker = {
        let action = action.clone();
        move || {
            let _bound = bind(action);
            f()
        }
    };
    match tokio::time::timeout(budget, tokio::task::spawn_blocking(worker)).await {
        Ok(joined) => joined,
        Err(_elapsed) => {
            action.abandon();
            Ok(Err(anyhow::anyhow!(
                "{}: {label} did not complete within {budget:?}; the X server or the target \
                 application may be wedged. Take a screenshot before retrying",
                super::foreground::CODE_TIMEOUT
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_abandoned_or_expired_action_admits_nothing() {
        let action = Action::new(Duration::from_secs(60));
        assert_eq!(action.admit(|| 1).unwrap(), 1);
        action.abandon();
        let refused = action.admit(|| panic!("sent after abandon")).unwrap_err();
        assert_eq!(
            crate::input::foreground::error_code(&refused),
            Some(crate::input::foreground::CODE_TIMEOUT)
        );

        let expired = Action::new(Duration::ZERO);
        expired
            .admit(|| panic!("sent after the deadline"))
            .unwrap_err();
    }

    #[test]
    fn abandon_waits_for_a_send_under_way() {
        let action = Action::new(Duration::from_secs(60));
        let (started, sending) = std::sync::mpsc::channel();
        let worker = {
            let action = action.clone();
            std::thread::spawn(move || {
                action
                    .admit(|| {
                        started.send(()).unwrap();
                        std::thread::sleep(Duration::from_millis(200));
                        Instant::now()
                    })
                    .unwrap()
            })
        };
        sending.recv().unwrap();
        action.abandon();
        let abandoned_at = Instant::now();
        assert!(worker.join().unwrap() <= abandoned_at);
    }

    #[test]
    fn abandon_shuts_a_fenced_socket_without_waiting_for_a_wedged_send() {
        use std::io::Write;
        let (mut socket, peer) = std::os::unix::net::UnixStream::pair().unwrap();
        let action = Action::new(Duration::from_secs(60));
        action.fence(&socket).unwrap();
        let (started, sending) = std::sync::mpsc::channel();
        let worker = {
            let action = action.clone();
            std::thread::spawn(move || {
                action.admit(|| {
                    started.send(()).unwrap();
                    loop {
                        if let Err(error) = socket.write_all(&[0; 65536]) {
                            return error;
                        }
                    }
                })
            })
        };
        sending.recv().unwrap();
        let abandoning = Instant::now();
        action.abandon();
        assert!(abandoning.elapsed() < Duration::from_secs(5));
        assert!(worker.join().unwrap().is_ok());
        drop(peer);
    }

    #[test]
    fn a_socket_fenced_after_abandon_is_shut_at_once() {
        use std::io::{Read, Write};
        let (mut socket, mut peer) = std::os::unix::net::UnixStream::pair().unwrap();
        let action = Action::new(Duration::from_secs(60));
        action.abandon();
        action.fence(&socket).unwrap();
        assert!(socket.write_all(b"late").is_err());
        assert_eq!(peer.read(&mut [0; 4]).unwrap(), 0);
    }

    fn sending_worker(sent: Arc<std::sync::atomic::AtomicUsize>) -> impl FnOnce() -> Result<()> {
        move || {
            for _ in 0..50 {
                admit(|| sent.fetch_add(1, std::sync::atomic::Ordering::SeqCst))?;
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_timed_out_input_worker_sends_nothing_more() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let sent = Arc::new(AtomicUsize::new(0));
        let result = spawn_blocking_bounded(
            "test input",
            Duration::from_millis(100),
            sending_worker(sent.clone()),
        )
        .await
        .unwrap();
        assert!(result.is_err());
        let at_timeout = sent.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(sent.load(Ordering::SeqCst), at_timeout);
    }

    #[tokio::test]
    async fn an_input_worker_whose_caller_was_dropped_sends_nothing_more() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let sent = Arc::new(AtomicUsize::new(0));
        let call = spawn_blocking_bounded(
            "test input",
            Duration::from_secs(60),
            sending_worker(sent.clone()),
        );
        assert!(tokio::time::timeout(Duration::from_millis(100), call)
            .await
            .is_err());
        let at_drop = sent.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(sent.load(Ordering::SeqCst) <= at_drop + 1);
    }

    #[test]
    fn a_nested_worker_keeps_the_outer_deadline_and_its_timeout_ends_the_outer_call() {
        let outer = Action::new(Duration::from_millis(100));
        let _bound = bind(outer.clone());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let started = Instant::now();
        let inner = runtime
            .block_on(spawn_blocking_bounded(
                "inner",
                Duration::from_secs(60),
                || {
                    std::thread::sleep(Duration::from_millis(400));
                    Ok(())
                },
            ))
            .unwrap();
        assert!(inner.is_err());
        assert!(started.elapsed() < Duration::from_secs(5));
        admit(|| panic!("the outer call sent after its inner worker timed out")).unwrap_err();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cleanup_that_outlives_a_timed_out_worker_ends_before_the_next_call() {
        use cua_driver_core::tool::DesktopCall;
        let events = Arc::new(Mutex::new(Vec::new()));
        let cleanup = Arc::new(Mutex::new(None));
        let call_1 = {
            let events = events.clone();
            let cleanup = cleanup.clone();
            DesktopCall::acquire()
                .await
                .unwrap()
                .scope(spawn_blocking_bounded(
                    "call 1",
                    Duration::from_millis(50),
                    move || {
                        std::thread::sleep(Duration::from_millis(100));
                        let action = current().unwrap();
                        *cleanup.lock().unwrap() = Some(std::thread::spawn(move || {
                            let _desktop = action;
                            std::thread::sleep(Duration::from_millis(300));
                            events.lock().unwrap().push("call 1 released its press");
                        }));
                        Ok(())
                    },
                ))
                .await
                .unwrap()
        };
        assert!(call_1.is_err());
        DesktopCall::acquire()
            .await
            .unwrap()
            .scope(async { events.lock().unwrap().push("call 2 pressed") })
            .await;
        if let Some(cleanup) = cleanup.lock().unwrap().take() {
            cleanup.join().unwrap();
        }
        assert_eq!(
            *events.lock().unwrap(),
            ["call 1 released its press", "call 2 pressed"]
        );
    }

    #[test]
    fn a_nested_worker_shares_the_outer_action() {
        let outer = Action::new(Duration::from_secs(60));
        let _bound = bind(outer.clone());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let inner = runtime
            .block_on(spawn_blocking_bounded(
                "inner",
                Duration::from_secs(60),
                || Ok(current().unwrap()),
            ))
            .unwrap()
            .unwrap();
        assert!(Arc::ptr_eq(&inner, &outer));
        assert_eq!(admit(|| 1).unwrap(), 1);
    }
}
