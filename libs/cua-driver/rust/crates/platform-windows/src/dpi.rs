// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! DPI context for threads owned by an embedded Cua Driver runtime.
//!
//! The standalone driver executable declares Per-Monitor V2 awareness in its
//! manifest. An in-process SDK cannot rely on the importing application to do
//! the same, so Cua-owned threads opt into the physical-pixel context used by
//! Windows capture and input code. A thread's context applies only to that
//! thread: the host process and its own threads keep theirs.

use windows::Win32::UI::HiDpi::{
    AreDpiAwarenessContextsEqual, GetThreadDpiAwarenessContext, SetThreadDpiAwarenessContext,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};

/// Configure the current Cua-owned thread to use the physical-pixel coordinate
/// contract expected by the Windows backend.
///
/// Windows rejects the context only when the OS does not support Per-Monitor
/// V2, and a compatibility override can leave a different context in place.
/// Callers that provide the physical-pixel desktop contract must not continue
/// in either case, because Windows would virtualize capture and input
/// coordinates.
pub fn use_per_monitor_v2_for_current_thread() -> Result<(), String> {
    let previous =
        unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    if previous.0.is_null() {
        return Err(format!(
            "SetThreadDpiAwarenessContext(PER_MONITOR_AWARE_V2) failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    if !current_thread_uses_per_monitor_v2() {
        return Err("the thread did not enter the PER_MONITOR_AWARE_V2 DPI context".into());
    }
    Ok(())
}

/// Wrap the body of an explicitly spawned Windows thread so it runs under
/// Per-Monitor V2. New threads start in the process default context, which in
/// an embedded host is usually DPI-unaware. The body does not run when the
/// context is rejected.
pub fn owned_thread<F, T>(work: F) -> impl FnOnce() -> Result<T, String> + Send + 'static
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    move || {
        use_per_monitor_v2_for_current_thread()?;
        Ok(work())
    }
}

/// Report whether the current thread is running with the Cua Windows DPI
/// contract.
pub fn current_thread_uses_per_monitor_v2() -> bool {
    unsafe {
        AreDpiAwarenessContextsEqual(
            GetThreadDpiAwarenessContext(),
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        )
        .as_bool()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_thread_uses_per_monitor_v2_without_changing_the_spawning_thread() {
        // A fresh OS thread models a host thread; the test harness thread may
        // already have been changed by another test.
        std::thread::spawn(|| {
            let host_dpi = unsafe { GetThreadDpiAwarenessContext() };
            let child = std::thread::spawn(owned_thread(current_thread_uses_per_monitor_v2))
                .join()
                .unwrap();
            assert_eq!(child, Ok(true));
            assert!(unsafe {
                AreDpiAwarenessContextsEqual(host_dpi, GetThreadDpiAwarenessContext()).as_bool()
            });
        })
        .join()
        .unwrap();
    }
}
