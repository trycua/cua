//! Foreground-steal bypass for UWP / XAML / WinUI UIA activation calls.
//!
//! UWP/XAML/WinUI apps self-foreground during UIA `InvokePattern.Invoke`
//! (and `ExpandCollapse.Expand`, `Toggle.Toggle`, `SelectionItem.Select` —
//! anything the XAML message-loop processes as an input-like event). The
//! XAML host unconditionally calls `SetForegroundWindow(self)` when handling
//! those, stealing focus from whatever the user had on top.
//!
//! Wrapping the call in `EnableWindow(host, FALSE) / call / EnableWindow(host, TRUE)`
//! silently suppresses the self-activation while letting the UIA pattern call
//! still execute — UIA pattern delivery uses the kernel accessibility channel,
//! not the input queue gated by `EnableWindow`.
//!
//! Empirical evidence (`flash-repro/14-multi-uwp-v3.ps1`, 2026-05-24):
//!   - Baseline UIA Invoke against UWP Calculator num5 button: user's
//!     foreground window dropped below the target in 91% of poller samples.
//!   - With this bypass: 0/507 z-drops across Calculator, Clock, Settings.
//!
//! Non-UWP / classic Win32 apps don't exhibit the bug
//! (`flash-repro/15-non-uwp.ps1`, Notepad: 0/45 baseline z-drops). The bypass
//! is therefore gated on `crate::input::is_xaml_host_hwnd` and is a no-op
//! for non-XAML hosts.

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::WindowsAndMessaging::{GA_ROOT, GetAncestor};

/// RAII guard that disables a window on construction and restores its
/// previous enabled-state on Drop. Always re-arms on Drop even if the
/// wrapped action panics.
pub struct DisabledHwndGuard {
    hwnd: HWND,
    was_enabled: bool,
    armed: bool,
}

impl DisabledHwndGuard {
    /// Disable `hwnd` for the lifetime of the guard. No-op for null HWND.
    pub fn disable(hwnd: HWND) -> Self {
        if hwnd.0.is_null() {
            return Self { hwnd, was_enabled: false, armed: false };
        }
        // `EnableWindow` returns nonzero iff the window was *previously
        // disabled* — invert to get the "was enabled" state we want to
        // restore at Drop time.
        let was_disabled = unsafe { EnableWindow(hwnd, false).as_bool() };
        Self { hwnd, was_enabled: !was_disabled, armed: true }
    }
}

impl Drop for DisabledHwndGuard {
    fn drop(&mut self) {
        if self.armed {
            unsafe {
                let _ = EnableWindow(self.hwnd, self.was_enabled);
            }
        }
    }
}

/// Wrap an activation closure (Invoke / Expand / Toggle / SelectionItem.Select)
/// in a UWP foreground-steal bypass.
///
/// `host_hwnd` is the top-level HWND of the window that contains the target
/// UIA element. When it identifies as an XAML host (`is_xaml_host_hwnd`), the
/// HWND is disabled for the duration of `action`. For non-XAML / classic
/// Win32 hosts the closure runs unmodified — empirically those don't
/// self-foreground.
///
/// The guard restores the previous enabled-state at Drop, so a panic in
/// `action` still leaves the window in a usable state.
pub fn run_with_uwp_bypass<T>(host_hwnd: isize, action: impl FnOnce() -> T) -> T {
    let _guard = make_guard(host_hwnd);
    action()
}

fn make_guard(host_hwnd: isize) -> Option<DisabledHwndGuard> {
    if host_hwnd == 0 {
        return None;
    }
    if !crate::input::is_xaml_host_hwnd(host_hwnd as u64) {
        return None;
    }
    let h = HWND(host_hwnd as *mut _);
    // Defensive: walk up to the root in case the caller handed us a child
    // HWND inside the XAML host's HWND tree. UWP elements always disable
    // cleanly via the AppFrame root.
    let root = unsafe { GetAncestor(h, GA_ROOT) };
    let target = if root.0.is_null() { h } else { root };
    Some(DisabledHwndGuard::disable(target))
}
