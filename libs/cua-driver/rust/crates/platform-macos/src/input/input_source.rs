// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Thread affinity for the Text Input Sources (TIS/TSM) API.
//!
//! HIToolbox enforces one rule for `TISCopyCurrentKeyboardLayoutInputSource`
//! and the rest of TIS/TSM, in its own words: "If you are a UI application,
//! you must call TIS/TSM API on the main thread. If you are a non-UI
//! application (such as a cmd-line tool or a launchagent that does not run an
//! event loop), you must not call TIS/TSM API from multiple threads
//! concurrently." Breaking it kills the process. macOS 26.4 and 26.6 abort
//! when a worker's call overlaps TSM work on another thread. macOS 26.5
//! asserts the main queue (`_dispatch_assert_queue_fail`, SIGTRAP) whenever
//! HIToolbox has to rebuild its input-source list off the main thread, which
//! a daemon started as a plain child process (by a shell or an MCP client,
//! not by LaunchServices) does on its first lookup.
//!
//! Every TIS call in this crate goes through [`run`]:
//! - in a process whose main thread runs an AppKit event loop (the daemon's
//!   cursor overlay or PiP, the permission panel, an embedding app), the work
//!   is queued on the main dispatch queue and the caller waits for it;
//! - otherwise (CLI, tests, a daemon without overlay) it runs on the calling
//!   thread, serialized by one process-wide lock.
//!
//! A main queue that is not serviced (the main thread is blocked) costs one
//! caller [`MAIN_QUEUE_TIMEOUT`]; later callers get `None` at once until the
//! queued work has run, so a stalled main thread never makes every key press
//! wait.
//!
//! [`generation`] changes whenever the selected or enabled keyboard input
//! sources change, so callers can cache what they read (the keyboard layout)
//! and re-read only after a switch. Change notifications only arrive on the
//! main run loop, so [`can_cache`] is false in a process without one.

use std::{
    ffi::c_void,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Mutex, Once,
    },
    time::Duration,
};

/// How long a worker waits for the main queue to run TIS work.
pub const MAIN_QUEUE_TIMEOUT: Duration = Duration::from_secs(1);

/// Serializes TIS calls made off the main queue (non-UI processes), and the
/// switch to main-thread ownership against calls already in flight.
static INLINE_LOCK: Mutex<()> = Mutex::new(());
/// Set once this process's main thread runs (or is about to run) AppKit.
static MAIN_THREAD_OWNS_TIS: AtomicBool = AtomicBool::new(false);
/// Set while a queued main-queue job has outlived a caller's wait.
static MAIN_QUEUE_STALLED: AtomicBool = AtomicBool::new(false);
/// Bumped by input-source change notifications.
static GENERATION: AtomicU64 = AtomicU64::new(0);
/// Set once the change observer is registered on the main run loop.
static OBSERVING: AtomicBool = AtomicBool::new(false);

#[link(name = "System", kind = "framework")]
extern "C" {
    static _dispatch_main_q: u8;
    fn dispatch_async_f(
        queue: *const c_void,
        context: *mut c_void,
        work: unsafe extern "C" fn(*mut c_void),
    );
    fn pthread_main_np() -> i32;
}

#[link(name = "AppKit", kind = "framework")]
extern "C" {
    /// `NSApp`: non-nil once `+[NSApplication sharedApplication]` ran.
    static NSApp: *const c_void;
}

fn on_main_thread() -> bool {
    // SAFETY: no arguments, no preconditions.
    unsafe { pthread_main_np() != 0 }
}

/// Declare that the calling (main) thread is about to run an AppKit event
/// loop, so every later TIS call is sent to the main queue. Call it before
/// `+[NSApplication sharedApplication]`: it waits for TIS calls already
/// running on workers, so AppKit's own TSM setup cannot overlap them.
pub fn main_thread_runs_event_loop() {
    if MAIN_THREAD_OWNS_TIS.load(Ordering::Acquire) {
        return;
    }
    let _guard = INLINE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    MAIN_THREAD_OWNS_TIS.store(true, Ordering::Release);
    if on_main_thread() {
        observe_changes();
    }
}

/// Whether TIS calls belong on the main queue in this process: it said so
/// with [`main_thread_runs_event_loop`], or an embedding app created `NSApp`.
fn main_thread_owns_tis() -> bool {
    // SAFETY: reading AppKit's `NSApp` pointer; a racy read only delays the
    // switch by one call.
    MAIN_THREAD_OWNS_TIS.load(Ordering::Acquire) || unsafe { !NSApp.is_null() }
}

/// Changes whenever the selected or enabled keyboard input sources change.
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}

/// Whether a value read through [`run`] may be cached and keyed by
/// [`generation`]: true once change notifications are observed.
pub fn can_cache() -> bool {
    OBSERVING.load(Ordering::Acquire)
}

/// Run TIS/TSM `work` with the thread affinity HIToolbox requires: on the
/// main queue in a UI process, else on this thread under the process-wide
/// lock. `None` when the main queue did not run it within
/// [`MAIN_QUEUE_TIMEOUT`] or is still stalled from an earlier call; the
/// queued work still runs when the main thread gets to it.
pub fn run<T, F>(work: F) -> Option<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    run_with_timeout(work, MAIN_QUEUE_TIMEOUT)
}

fn run_with_timeout<T, F>(work: F, timeout: Duration) -> Option<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    if on_main_thread() {
        if main_thread_owns_tis() {
            observe_changes();
            return Some(work());
        }
        let _guard = INLINE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        return Some(work());
    }
    if !main_thread_owns_tis() {
        let _guard = INLINE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Re-check under the lock: the main thread may have claimed TIS
        // while this caller waited for it.
        if !MAIN_THREAD_OWNS_TIS.load(Ordering::Acquire) {
            return Some(work());
        }
    }
    if MAIN_QUEUE_STALLED.load(Ordering::Acquire) {
        return None;
    }
    let (tx, rx) = mpsc::sync_channel(1);
    let job: Box<dyn FnOnce() + Send> = Box::new(move || {
        observe_changes();
        let _ = tx.send(work());
        MAIN_QUEUE_STALLED.store(false, Ordering::Release);
    });
    let context = Box::into_raw(Box::new(job));
    // SAFETY: `_dispatch_main_q` is libdispatch's main queue; `run_job`
    // takes ownership of `context` and runs exactly once.
    unsafe {
        dispatch_async_f(
            &raw const _dispatch_main_q as *const c_void,
            context.cast::<c_void>(),
            run_job,
        );
    }
    match rx.recv_timeout(timeout) {
        Ok(value) => Some(value),
        Err(_) => {
            MAIN_QUEUE_STALLED.store(true, Ordering::Release);
            None
        }
    }
}

unsafe extern "C" fn run_job(context: *mut c_void) {
    // SAFETY: `context` is the `Box<Box<dyn FnOnce()>>` leaked in `run`.
    let job = unsafe { Box::from_raw(context.cast::<Box<dyn FnOnce() + Send>>()) };
    job();
}

/// Register for input-source change notifications. Main thread only: the
/// distributed center delivers on the run loop of the registering thread.
fn observe_changes() {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| {
        use core_foundation::base::CFTypeRef;
        use core_foundation::string::CFStringRef;

        type CFNotificationCenterRef = *const c_void;
        type Callback = extern "C" fn(
            CFNotificationCenterRef,
            *const c_void,
            CFStringRef,
            *const c_void,
            CFTypeRef,
        );
        #[link(name = "CoreFoundation", kind = "framework")]
        extern "C" {
            fn CFNotificationCenterGetDistributedCenter() -> CFNotificationCenterRef;
            fn CFNotificationCenterAddObserver(
                center: CFNotificationCenterRef,
                observer: *const c_void,
                callback: Callback,
                name: CFStringRef,
                object: *const c_void,
                suspension_behavior: isize,
            );
        }
        #[link(name = "Carbon", kind = "framework")]
        extern "C" {
            static kTISNotifySelectedKeyboardInputSourceChanged: CFStringRef;
            static kTISNotifyEnabledKeyboardInputSourcesChanged: CFStringRef;
        }
        extern "C" fn changed(
            _: CFNotificationCenterRef,
            _: *const c_void,
            _: CFStringRef,
            _: *const c_void,
            _: CFTypeRef,
        ) {
            GENERATION.fetch_add(1, Ordering::AcqRel);
        }
        const DELIVER_IMMEDIATELY: isize = 4;
        // SAFETY: registered on the main thread with a static callback and
        // no observer context; the observer lives for the process.
        unsafe {
            let center = CFNotificationCenterGetDistributedCenter();
            for name in [
                kTISNotifySelectedKeyboardInputSourceChanged,
                kTISNotifyEnabledKeyboardInputSourcesChanged,
            ] {
                CFNotificationCenterAddObserver(
                    center,
                    std::ptr::null(),
                    changed,
                    name,
                    std::ptr::null(),
                    DELIVER_IMMEDIATELY,
                );
            }
        }
        OBSERVING.store(true, Ordering::Release);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{atomic::AtomicUsize, Arc};

    // The cargo test harness runs tests on worker threads, never creates
    // NSApp and never services the main queue: a non-UI process.

    #[test]
    fn non_ui_workers_run_inline_and_never_overlap() {
        let running = Arc::new(AtomicUsize::new(0));
        let overlaps = Arc::new(AtomicUsize::new(0));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let running = Arc::clone(&running);
                let overlaps = Arc::clone(&overlaps);
                std::thread::spawn(move || {
                    for _ in 0..50 {
                        let running = Arc::clone(&running);
                        let overlaps = Arc::clone(&overlaps);
                        let caller = std::thread::current().id();
                        let ran_on = run(move || {
                            if running.fetch_add(1, Ordering::AcqRel) != 0 {
                                overlaps.fetch_add(1, Ordering::AcqRel);
                            }
                            std::thread::yield_now();
                            running.fetch_sub(1, Ordering::AcqRel);
                            std::thread::current().id()
                        });
                        assert_eq!(ran_on, Some(caller));
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(overlaps.load(Ordering::Acquire), 0);
    }

    #[test]
    fn nothing_is_cached_without_change_notifications() {
        assert!(!can_cache());
    }
}
