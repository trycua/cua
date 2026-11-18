// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Thread affinity for AX mutations aimed at this process's own windows.
//!
//! An AX action against another process executes in that process. When an
//! embedded driver targets its own window, AppKit services the action (for
//! `AXRaise`, `NSWindow makeKeyAndOrderFront:`) in this process on the calling
//! thread, and window ordering is main-thread-only. Off the main thread, such
//! a call is queued onto AppKit's main queue and the caller waits for it.

use std::{
    ffi::c_void,
    sync::{
        atomic::{AtomicBool, Ordering},
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
    cancelled: Arc<AtomicBool>,
}

unsafe extern "C" fn run_request<F, T>(context: *mut c_void)
where
    F: FnOnce() -> T,
{
    let request = unsafe { Box::from_raw(context.cast::<Request<F, T>>()) };
    if request.cancelled.load(Ordering::Acquire) {
        return;
    }
    let _ = request.tx.send((request.work)());
}

/// Run the AX mutation `work` against `pid` with the thread affinity it
/// needs: inline for another process or on the main thread, else on AppKit's
/// main queue, waiting at most `timeout`. On timeout the queued work is
/// cancelled so a stale window change cannot fire after the caller gave up.
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
    let cancelled = Arc::new(AtomicBool::new(false));
    let request = Box::new(Request {
        work,
        tx,
        cancelled: Arc::clone(&cancelled),
    });
    unsafe {
        let main_queue = &raw const _dispatch_main_q as *const c_void;
        dispatch_async_f(
            main_queue,
            Box::into_raw(request).cast::<c_void>(),
            run_request::<F, T>,
        );
    }
    let result = rx.recv_timeout(timeout);
    if result.is_err() {
        cancelled.store(true, Ordering::Release);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
