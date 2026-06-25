//! Enumerate top-level windows on Windows.
//!
//! Two enumeration sources, then union + dedupe by HWND:
//!
//! 1. **`EnumWindows`** (canonical z-order) — the Win32 enumerator walks the
//!    window manager's z-order list top-to-bottom, so iteration order IS the
//!    actual z-order. We list these first so the merged array's index reflects
//!    the true Win32 stacking for any HWND the Win32 path can see.
//!
//! 2. **UI Automation** (extra coverage) — appended after `EnumWindows`,
//!    contributing only HWNDs Win32 didn't surface. UIA exposes modern
//!    containers (WebView2 hosts, packaged-UWP frames, Electron container
//!    HWNDs) that `EnumWindows` may miss or return as wrapper HWNDs. UIA's
//!    `FindAll(TreeScope::Children, ...)` makes no z-order guarantee, so we
//!    deliberately do NOT let it reorder anything Win32 already reported.
//!
//! Both sources apply the same listability filter (`is_listable_top_level`:
//! visible, non-iconic, owner-less, non-cloaked). The title is read for
//! display only and is **not** a filter — empty-caption windows (WPF
//! `HwndWrapper[...]`, borderless / custom-chrome apps) are listed. The
//! `filter_pid` argument is applied to the merged list so the union/dedupe
//! pipeline runs unconditionally.

use std::collections::HashSet;
use std::sync::Mutex;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT, TRUE};
use windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindow, GetWindowRect, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId, IsIconic, IsWindowVisible, GW_OWNER,
};

#[derive(Debug, Clone)]
pub struct WindowInfo {
    /// HWND cast to u64 for serialization.
    pub hwnd: u64,
    /// pid owning the window.
    pub pid: u32,
    pub title: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

struct EnumState {
    windows: Vec<WindowInfo>,
}

/// List top-level visible windows. If `filter_pid` is Some, only that process.
///
/// `EnumWindows` first — its iteration order is the Win32 window manager's
/// z-order (top-to-bottom), so the merged array's index doubles as a z_index
/// for any HWND the Win32 path saw. Then UIA-only entries are appended (UIA
/// makes no z-order guarantee, so it must not be allowed to reorder anything
/// EnumWindows already reported). The pid filter is applied to the merged
/// list.
pub fn list_windows(filter_pid: Option<u32>) -> Vec<WindowInfo> {
    let win32_windows = enumerate_via_enum_windows();
    let uia_windows = crate::uia::enumerate_top_level_windows();

    let mut seen: HashSet<u64> = HashSet::with_capacity(uia_windows.len() + win32_windows.len());
    let mut merged: Vec<WindowInfo> = Vec::with_capacity(uia_windows.len() + win32_windows.len());

    // EnumWindows first — canonical Win32 z-order.
    for w in win32_windows {
        if seen.insert(w.hwnd) {
            merged.push(w);
        }
    }
    // Then any UIA-only HWND that EnumWindows didn't surface (modern
    // containers, WebView2 hosts, etc.). These get appended (no claim on
    // z-order priority relative to the Win32 list).
    for w in uia_windows {
        if seen.insert(w.hwnd) {
            merged.push(w);
        }
    }

    if let Some(fp) = filter_pid {
        merged.retain(|w| w.pid == fp);
    }
    merged
}

/// Walk `EnumWindows` and collect every listable top-level window (see
/// `is_listable_top_level`). The title is read for display but is not a
/// filter, so empty-caption windows are included. No pid filter is applied
/// here — the caller does that on the merged list.
fn enumerate_via_enum_windows() -> Vec<WindowInfo> {
    let state = Mutex::new(EnumState { windows: Vec::new() });
    let state_ptr = &state as *const Mutex<EnumState> as isize;
    unsafe {
        let _ = EnumWindows(Some(enum_windows_cb), LPARAM(state_ptr));
    }
    state.into_inner().unwrap().windows
}

unsafe extern "system" fn enum_windows_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let state = &*(lparam.0 as *const Mutex<EnumState>);

    // Listable == a real, targetable top-level window. We deliberately do NOT
    // gate on the title: a visible, non-iconic, owner-less, non-cloaked window
    // is a legitimate target even with an empty caption (WPF, borderless /
    // custom-chrome apps). Filtering on a non-empty title used to hide these
    // from the agent even though `debug_window_info` could see them — see
    // trycua/cua#2020.
    if !is_listable_top_level(hwnd) {
        return TRUE;
    }

    // Get pid.
    let mut pid: u32 = 0;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));

    // Read the caption for display only — empty is fine. The tool layer
    // already renders "(no title)" for these records.
    let title = window_title(hwnd);

    // Get bounds — prefer DWM extended frame bounds (includes shadow), fallback to GetWindowRect.
    let (x, y, w, h) = get_window_bounds(hwnd);

    state.lock().unwrap().windows.push(WindowInfo {
        hwnd: hwnd.0 as u64,
        pid,
        title,
        x,
        y,
        width: w,
        height: h,
    });

    TRUE
}

/// Is `hwnd` a real, agent-targetable top-level window?
///
/// Single source of truth for what `list_windows` exposes, shared by the
/// `EnumWindows` and UI Automation enumeration paths so the two can't drift
/// (that drift was the root cause of trycua/cua#2020). A window qualifies when
/// it is:
///
///   - visible (`IsWindowVisible`) and not minimized (`!IsIconic`),
///   - a true top-level window — no owner (`GW_OWNER` is null), which excludes
///     tool-tips, owned pop-ups and transient child surfaces, and
///   - not DWM-cloaked (`DWMWA_CLOAKED == 0`), which excludes the hidden
///     background frames of suspended UWP / `ApplicationFrameHost` apps that
///     still report `IsWindowVisible == true`.
///
/// What is deliberately NOT checked: the window title. An empty caption is not
/// a signal that a window is unreal — WPF apps (`HwndWrapper[App.exe;;<guid>]`),
/// borderless / custom-chrome apps, and various splash/tool windows ship
/// visible, owner-less top-level windows with no caption. The owner + cloaked
/// gates here express "is this a real window?" directly, which is what the
/// non-empty-title check was a poor proxy for.
pub(crate) fn is_listable_top_level(hwnd: HWND) -> bool {
    unsafe {
        if IsWindowVisible(hwnd).0 == 0 || IsIconic(hwnd).0 != 0 {
            return false;
        }
        // Owner-less == genuine top-level. `GetWindow(GW_OWNER)` yields the
        // owner HWND, or null/err when there is none. Mirrors the top-level
        // test `debug_window_info` uses, so the two tools agree on a given HWND.
        if !GetWindow(hwnd, GW_OWNER).unwrap_or_default().is_invalid() {
            return false;
        }
        // Suspended UWP / ApplicationFrameHost shells keep `WS_VISIBLE` but are
        // cloaked by DWM (not actually on screen). Drop them.
        if is_cloaked(hwnd) {
            return false;
        }
        true
    }
}

/// True iff DWM reports `hwnd` as cloaked — hidden by the compositor even
/// though `WS_VISIBLE` is set (suspended UWP app, window on another virtual
/// desktop, etc.). Returns false if the attribute can't be read.
unsafe fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked: u32 = 0;
    let ok = DwmGetWindowAttribute(
        hwnd,
        DWMWA_CLOAKED,
        &mut cloaked as *mut u32 as *mut _,
        std::mem::size_of::<u32>() as u32,
    );
    ok.is_ok() && cloaked != 0
}

/// Read a window's caption via `GetWindowTextW`. Returns an empty string for
/// untitled windows — which are still listed (see `is_listable_top_level`).
/// Shared by the `EnumWindows` and UIA enumeration paths so both report the
/// OS-level caption identically.
pub(crate) fn window_title(hwnd: HWND) -> String {
    unsafe {
        let title_len = GetWindowTextLengthW(hwnd);
        if title_len == 0 {
            return String::new();
        }
        let mut buf = vec![0u16; (title_len + 1) as usize];
        GetWindowTextW(hwnd, &mut buf);
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..len])
    }
}

fn get_window_bounds(hwnd: HWND) -> (i32, i32, i32, i32) {
    unsafe {
        let mut rect = RECT::default();
        // Try DwmGetWindowAttribute for accurate bounds (excludes drop shadow on W11).
        let ok = DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut rect as *mut RECT as *mut _,
            std::mem::size_of::<RECT>() as u32,
        );
        if ok.is_err() {
            // Fallback to GetWindowRect.
            let _ = GetWindowRect(hwnd, &mut rect);
        }
        (rect.left, rect.top, rect.right - rect.left, rect.bottom - rect.top)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, PeekMessageW,
        RegisterClassExW, ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW, MSG, PM_REMOVE,
        SW_SHOWNOACTIVATE, WINDOW_EX_STYLE, WNDCLASSEXW, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
    };

    unsafe extern "system" fn test_wnd_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        DefWindowProcW(h, m, w, l)
    }

    /// Drain the calling thread's message queue a few times so the freshly
    /// created window finishes coming up (and DWM settles its cloaked state)
    /// before we enumerate.
    fn pump_messages(rounds: usize) {
        unsafe {
            for _ in 0..rounds {
                let mut msg = MSG::default();
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }

    /// Regression test for trycua/cua#2020: a visible, owner-less,
    /// **empty-title** top-level window must be enumerated by `list_windows`.
    ///
    /// Before the fix, both enumeration sources dropped any window with
    /// `GetWindowTextLengthW == 0`, so WPF (`HwndWrapper[App.exe;;<guid>]`),
    /// borderless and custom-chrome apps were invisible to the agent even
    /// though `debug_window_info` could list them.
    ///
    /// `#[ignore]` because it needs an interactive window station to create a
    /// visible top-level window; run it via the Windows sandbox harness runner
    /// or locally with
    /// `cargo test -p platform-windows -- --ignored empty_title`.
    #[test]
    #[ignore]
    fn empty_title_top_level_window_is_listed() {
        unsafe {
            let hinstance = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
            let class_name: Vec<u16> = "Cua.Test.EmptyTitleWindow\0".encode_utf16().collect();

            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(test_wnd_proc),
                hInstance: hinstance.into(),
                lpszClassName: PCWSTR(class_name.as_ptr()),
                ..Default::default()
            };
            // Ignore the return: a re-run in the same process sees the class
            // already registered, which is harmless.
            RegisterClassExW(&wc);

            // Empty window name == empty caption — the whole point of the test.
            let empty_title: Vec<u16> = "\0".encode_utf16().collect();
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                PCWSTR(class_name.as_ptr()),
                PCWSTR(empty_title.as_ptr()),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                100,
                100,
                320,
                80,
                None,
                None,
                hinstance,
                None,
            )
            .expect("CreateWindowExW failed");

            // RAII guard so the visible test window is always destroyed, even
            // if a precondition assertion below panics before the explicit
            // teardown runs.
            struct WindowGuard(HWND);
            impl Drop for WindowGuard {
                fn drop(&mut self) {
                    unsafe {
                        let _ = DestroyWindow(self.0);
                    }
                }
            }
            let window_guard = WindowGuard(hwnd);

            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            pump_messages(5);

            // Preconditions: the OS really gave us an empty-caption window, and
            // the shared predicate accepts it despite the empty title.
            assert_eq!(
                window_title(hwnd),
                "",
                "test precondition: the created window must be untitled"
            );
            assert!(
                is_listable_top_level(hwnd),
                "an empty-title visible owner-less top-level window must be listable"
            );

            let pid = GetCurrentProcessId();
            let windows = list_windows(Some(pid));
            let found = windows.iter().find(|w| w.hwnd == hwnd.0 as u64).cloned();

            // Tear the window down before the final asserts. Dropping the guard
            // runs DestroyWindow; if an assertion above already panicked, the
            // guard's Drop ran during unwind, so the window is gone either way.
            drop(window_guard);
            pump_messages(2);

            let found = found.expect(
                "empty-title top-level window was dropped by list_windows — #2020 regression",
            );
            assert_eq!(
                found.title, "",
                "listed record should carry the (empty) OS caption verbatim"
            );
            assert_eq!(
                found.pid, pid,
                "listed record pid should match the creating process"
            );
        }
    }
}
