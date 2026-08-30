//! DPI context for threads owned by an embedded Cua Driver runtime.
//!
//! The standalone driver executable declares Per-Monitor V2 awareness in its
//! manifest. An in-process SDK cannot rely on the importing application to do
//! the same, so Cua-owned worker threads opt into the physical-pixel context
//! used by Windows capture and input code.

use windows::Win32::UI::HiDpi::{
    AreDpiAwarenessContextsEqual, GetThreadDpiAwarenessContext, SetThreadDpiAwarenessContext,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};

/// Configure the current Cua-owned thread to use the physical-pixel coordinate
/// contract expected by the Windows backend.
///
/// Returns an error when Windows rejects the requested context. Callers that
/// provide the physical-pixel desktop contract must not continue in that
/// case, because Windows would virtualize capture and input coordinates.
pub fn use_per_monitor_v2_for_current_thread() -> Result<(), String> {
    let previous =
        unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    if previous.0.is_null() {
        Err(format!(
            "SetThreadDpiAwarenessContext(PER_MONITOR_AWARE_V2) failed: {}",
            std::io::Error::last_os_error()
        ))
    } else {
        Ok(())
    }
}

/// Report whether the current thread is running with the Cua Windows DPI
/// contract. This is used by the SDK executor regression test.
pub fn current_thread_uses_per_monitor_v2() -> bool {
    unsafe {
        AreDpiAwarenessContextsEqual(
            GetThreadDpiAwarenessContext(),
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        )
        .as_bool()
    }
}
