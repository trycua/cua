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
//! Both sources apply the same filters (visible, non-iconic, non-empty
//! title). The `filter_pid` argument is applied to the merged list so the
//! union/dedupe pipeline runs unconditionally.

use std::collections::HashSet;
use std::sync::Mutex;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT, TRUE};
use windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowRect, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId, IsIconic, IsWindowVisible,
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

/// Walk `EnumWindows` and collect every visible, non-iconic, non-empty-titled
/// top-level window. No pid filter is applied here — the caller does that on
/// the merged list.
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

    // Skip invisible or minimized windows.
    if IsWindowVisible(hwnd).0 == 0 || IsIconic(hwnd).0 != 0 {
        return TRUE;
    }

    // Get pid.
    let mut pid: u32 = 0;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));

    // Get title (skip empty).
    let title_len = GetWindowTextLengthW(hwnd);
    if title_len == 0 { return TRUE; }
    let mut buf = vec![0u16; (title_len + 1) as usize];
    GetWindowTextW(hwnd, &mut buf);
    let title = {
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..len])
    };
    if title.trim().is_empty() { return TRUE; }

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
