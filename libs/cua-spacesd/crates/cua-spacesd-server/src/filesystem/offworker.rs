// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Values that must be dropped on a blocking thread.
//!
//! Closing a file is a syscall, and on the Cua Volume (`/volume`, a FUSE
//! mount served by the volume helper, which dials the volume listener this
//! very runtime runs) it waits for an answer that only this runtime can
//! produce. Dropping a `tokio::fs::File` on an async worker closes it right
//! there: with one worker (a 1 vCPU Space) the worker waits for a task it
//! alone could run, and cua-spacesd stops answering. [`OffWorker`] closes
//! on the blocking pool instead.

use std::ops::{Deref, DerefMut};

/// A value dropped on the blocking pool (when a runtime is running).
pub struct OffWorker<T: Send + 'static>(Option<T>);

impl<T: Send + 'static> OffWorker<T> {
    /// Wraps `value`.
    pub fn new(value: T) -> Self {
        Self(Some(value))
    }

    /// [`OffWorker::close`] without consuming: the value is gone afterwards
    /// (a later deref panics), so call it last.
    pub async fn close_in_place(&mut self) {
        if let Some(value) = self.0.take() {
            let _ = tokio::task::spawn_blocking(move || drop(value)).await;
        }
    }

    /// Drops the value on the blocking pool and waits for it (use before
    /// something that needs the file closed, such as a rename).
    pub async fn close(mut self) {
        if let Some(value) = self.0.take() {
            let _ = tokio::task::spawn_blocking(move || drop(value)).await;
        }
    }
}

impl<T: Send + 'static> Deref for OffWorker<T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.0.as_ref().expect("value present until closed")
    }
}

impl<T: Send + 'static> DerefMut for OffWorker<T> {
    fn deref_mut(&mut self) -> &mut T {
        self.0.as_mut().expect("value present until closed")
    }
}

impl<T: Send + 'static> Drop for OffWorker<T> {
    fn drop(&mut self) {
        if let Some(value) = self.0.take() {
            match tokio::runtime::Handle::try_current() {
                Ok(handle) => {
                    // Detached: nothing waits for the close.
                    drop(handle.spawn_blocking(move || drop(value)));
                }
                Err(_) => drop(value),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A value whose drop waits for a task of the runtime it is dropped
    /// from: dropped on the only worker, it would wait forever.
    struct WaitsForRuntime(std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<bool>);

    impl Drop for WaitsForRuntime {
        fn drop(&mut self) {
            let answered = self.0.recv_timeout(Duration::from_secs(10)).is_ok();
            let _ = self.1.send(answered);
        }
    }

    fn rig() -> (
        WaitsForRuntime,
        std::sync::mpsc::Sender<()>,
        std::sync::mpsc::Receiver<bool>,
    ) {
        let (answer_tx, answer_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        (WaitsForRuntime(answer_rx, done_tx), answer_tx, done_rx)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn a_dropped_value_is_closed_off_the_only_worker() {
        let (value, answer, done) = rig();
        let guard = OffWorker::new(value);
        // The task that answers the close runs on the same, only worker.
        let server = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            answer.send(()).unwrap();
        });
        drop(guard);
        server.await.unwrap();
        assert!(done.recv_timeout(Duration::from_secs(10)).unwrap());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn close_waits_for_the_drop_off_the_only_worker() {
        let (value, answer, done) = rig();
        let guard = OffWorker::new(value);
        let server = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            answer.send(()).unwrap();
        });
        guard.close().await;
        server.await.unwrap();
        assert!(done.try_recv().unwrap());
    }
}
