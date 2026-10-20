//! The tool call a blocking input worker serves.
//!
//! A tool call stops waiting for its blocking worker at its deadline, but
//! the worker cannot be cancelled: it keeps running on the blocking pool,
//! and the desktop may meanwhile admit another action. An input route reads
//! the [`Action`] bound to its thread and hands no further event to the
//! compositor once the call is over. Releasing a held button or key is
//! cleanup and still goes out.

use std::cell::RefCell;
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Result};

/// One tool call: its deadline, and whether its caller stopped waiting.
pub struct Action {
    deadline: Instant,
    abandoned: Mutex<bool>,
    fenced: Mutex<(bool, Vec<OwnedFd>)>,
}

impl Action {
    pub fn new(budget: Duration) -> Arc<Self> {
        Arc::new(Self {
            deadline: Instant::now() + budget,
            abandoned: Mutex::new(false),
            fenced: Mutex::new((false, Vec::new())),
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
}
