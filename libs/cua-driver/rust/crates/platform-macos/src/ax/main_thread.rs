// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Thread affinity for AX mutations aimed at this process's own windows.
//!
//! An AX action against another process executes in that process. When an
//! embedded driver targets its own window, AppKit services the action (for
//! `AXRaise`, `NSWindow makeKeyAndOrderFront:`) in this process on the calling
//! thread, and window ordering is main-thread-only. Off the main thread, such
//! a call is queued onto AppKit's main queue and the caller waits for it.

use cua_driver_core::native_operation::NativeOperation;
use std::{
    ffi::c_void,
    sync::{
        mpsc::{self, RecvTimeoutError, SyncSender},
        Arc,
    },
    time::Duration,
};

#[link(name = "System", kind = "framework")]
extern "C" {
    static _dispatch_main_q: u8;
    fn dispatch_async_f(
        queue: *const c_void,
        context: *mut c_void,
        work: unsafe extern "C" fn(*mut c_void),
    );
}

/// Whether an AX mutation against `pid` must run on the main thread before
/// it may run on `on_main_thread`'s thread.
pub(crate) fn needs_main_queue(pid: i32, on_main_thread: bool) -> bool {
    pid == std::process::id() as i32 && !on_main_thread
}

struct Request<F, T> {
    work: F,
    tx: SyncSender<T>,
    start: Arc<NativeOperation>,
}

unsafe extern "C" fn run_request<F, T>(context: *mut c_void)
where
    F: FnOnce() -> T,
{
    let request = unsafe { Box::from_raw(context.cast::<Request<F, T>>()) };
    if !request.start.begin() {
        return;
    }
    // A panic must not unwind into libdispatch; dropping `tx` reports it to
    // the waiting caller as a disconnect.
    let Request { work, tx, .. } = *request;
    if let Ok(result) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)) {
        let _ = tx.send(result);
    }
}

/// Run the AX mutation `work` against `pid` with the thread affinity it
/// needs: inline for another process or on the main thread, else on AppKit's
/// main queue, waiting at most `timeout` for it to start. Work that has not
/// started by then is cancelled so a stale window change cannot fire after the
/// caller gave up; work that has started is awaited, so a timeout is never
/// reported while the mutation can still write.
pub(crate) fn run_ax_mutation<F, T>(
    pid: i32,
    timeout: Duration,
    work: F,
) -> Result<T, RecvTimeoutError>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let on_main_thread = objc2_foundation::MainThreadMarker::new().is_some();
    if !needs_main_queue(pid, on_main_thread) {
        return Ok(work());
    }
    let (tx, rx) = mpsc::sync_channel(1);
    let start = Arc::new(NativeOperation::new());
    let request = Box::new(Request {
        work,
        tx,
        start: Arc::clone(&start),
    });
    unsafe {
        let main_queue = &raw const _dispatch_main_q as *const c_void;
        dispatch_async_f(
            main_queue,
            Box::into_raw(request).cast::<c_void>(),
            run_request::<F, T>,
        );
    }
    wait_for_start(&rx, &start, timeout)
}

fn wait_for_start<T>(
    rx: &mpsc::Receiver<T>,
    start: &NativeOperation,
    timeout: Duration,
) -> Result<T, RecvTimeoutError> {
    match rx.recv_timeout(timeout) {
        Err(RecvTimeoutError::Timeout) if !start.cancel() => {
            rx.recv().map_err(|_| RecvTimeoutError::Disconnected)
        }
        result => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn only_own_process_off_main_thread_is_queued() {
        let own = std::process::id() as i32;
        assert!(needs_main_queue(own, false));
        assert!(!needs_main_queue(own, true));
        assert!(!needs_main_queue(own.wrapping_add(1), false));
        assert!(!needs_main_queue(own.wrapping_add(1), true));
    }

    #[test]
    fn other_process_runs_inline_on_the_calling_thread() {
        let caller = std::thread::current().id();
        let ran_on = run_ax_mutation(
            (std::process::id() as i32).wrapping_add(1),
            Duration::ZERO,
            || std::thread::current().id(),
        );
        assert_eq!(ran_on, Ok(caller));
    }

    #[test]
    fn own_process_waits_for_the_main_queue_and_cancels_on_timeout() {
        // The cargo test harness never drains the main queue, so the
        // own-process mutation must not run on this worker thread.
        let ran = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&ran);
        let result = std::thread::spawn(move || {
            run_ax_mutation(
                std::process::id() as i32,
                Duration::from_millis(50),
                move || flag.store(true, Ordering::Release),
            )
        })
        .join()
        .unwrap();
        assert_eq!(result, Err(RecvTimeoutError::Timeout));
        assert!(!ran.load(Ordering::Acquire));
    }

    #[test]
    fn timeout_after_the_work_started_waits_for_its_result() {
        let (tx, rx) = mpsc::sync_channel(1);
        let start = Arc::new(NativeOperation::new());
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let worker_start = Arc::clone(&start);
        let worker = std::thread::spawn(move || {
            assert!(worker_start.begin());
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            tx.send("written").unwrap();
        });
        started_rx.recv().unwrap();
        let (result_tx, result_rx) = mpsc::channel();
        let waiter = std::thread::spawn(move || {
            result_tx
                .send(wait_for_start(&rx, &start, Duration::ZERO))
                .unwrap();
        });
        let early = result_rx.recv_timeout(Duration::from_millis(50));
        release_tx.send(()).unwrap();
        worker.join().unwrap();
        waiter.join().unwrap();
        assert!(
            early.is_err(),
            "a started mutation was reported before it finished"
        );
        assert_eq!(result_rx.recv().unwrap(), Ok("written"));
    }

    #[test]
    fn started_work_that_never_reports_is_a_disconnect() {
        let (tx, rx) = mpsc::sync_channel::<()>(1);
        let start = NativeOperation::new();
        assert!(start.begin());
        drop(tx);
        assert_eq!(
            wait_for_start(&rx, &start, Duration::ZERO),
            Err(RecvTimeoutError::Disconnected)
        );
    }
}
