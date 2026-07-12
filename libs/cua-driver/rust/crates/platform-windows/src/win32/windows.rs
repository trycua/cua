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
//! Both sources apply the same filters (visible, non-empty title). Minimized
//! windows remain addressable and are reported as off-screen so callers can
//! restore them explicitly. The `filter_pid` argument is applied to the merged
//! list so the union/dedupe pipeline runs unconditionally.

use std::collections::HashSet;
use std::sync::Mutex;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT, TRUE};
use windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, EnumWindows, GetClassNameW, GetWindowRect, GetWindowTextLengthW,
    GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
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
    pub is_on_screen: bool,
    pub minimized: bool,
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

/// Walk `EnumWindows` and collect every visible, non-empty-titled
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

    // Invisible helper windows are not user-addressable. Iconic windows are:
    // retain them with explicit state so callers can restore them.
    if IsWindowVisible(hwnd).0 == 0 {
        return TRUE;
    }
    let minimized = IsIconic(hwnd).0 != 0;

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
        is_on_screen: !minimized,
        minimized,
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

fn window_class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    if n <= 0 {
        String::new()
    } else {
        String::from_utf16_lossy(&buf[..n as usize])
    }
}

/// Resolve a packaged-app (UWP) process id to the `ApplicationFrameWindow`
/// that actually hosts it.
///
/// ## Why this exists
///
/// `IApplicationActivationManager::ActivateApplication` (see `launch_uwp`)
/// returns the **real** packaged-app pid — e.g. `CalculatorApp.exe`,
/// `SystemSettings.exe`. But a modern UWP app's *top-level* window is not
/// owned by that process. `ApplicationFrameHost.exe` owns the top-level
/// `ApplicationFrameWindow` (title bar, caption, the HWND a user drags); the
/// app's own process only owns a `Windows.UI.Core.CoreWindow` reparented
/// *inside* that frame as a child. Consequences:
///
/// - `list_windows(Some(app_pid))` is **empty** — the app process owns no
///   top-level window, and the frame's `GetWindowThreadProcessId` reports the
///   AFH pid, not `app_pid`.
/// - One `ApplicationFrameHost.exe` pid hosts **many** unrelated UWP apps, so
///   the AFH pid alone is not an app identity — only the specific frame HWND
///   disambiguates.
///
/// So after a UWP launch, the handles a caller must actually drive are
/// `(frame_hwnd, afh_pid)` — NOT the `(app_pid, …)` pair `launch_app` would
/// otherwise report, which resolves to no window at all.
///
/// ## How the mapping is made
///
/// The stable identity link is process ownership of the hosted child: AFH
/// reparents the app's `CoreWindow` under the frame, and that child window's
/// `GetWindowThreadProcessId` reports the **app** pid (the child stays owned
/// by the app process even though it lives under the AFH frame). We therefore
/// walk every visible top-level `ApplicationFrameWindow`, scan its child
/// windows, and return the first frame that owns a child whose process id
/// equals `app_pid`. This keys off OS-level window parentage rather than a raw
/// HWND/pid the caller cached, so it stays correct across the HWND churn UWP
/// activations exhibit in the first moments after launch.
///
/// Returns `None` if no hosting frame is found yet (the frame can lag the
/// process by a few hundred ms — callers should retry) or if `app_pid` is 0
/// (brokered activations that report no pid; not resolvable by this path).
pub fn resolve_uwp_host_window(app_pid: u32) -> Option<WindowInfo> {
    if app_pid == 0 {
        return None;
    }

    struct FrameScan {
        /// pid we're hunting for among each frame's child windows.
        target_app_pid: u32,
        /// Set to the hosting frame's HWND once a child match is found.
        matched_frame: Option<HWND>,
    }

    // Outer pass: every top-level ApplicationFrameWindow. For each, an inner
    // EnumChildWindows pass looks for a child owned by `target_app_pid`.
    unsafe extern "system" fn frame_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let scan = &mut *(lparam.0 as *mut FrameScan);

        if IsWindowVisible(hwnd).0 == 0 || IsIconic(hwnd).0 != 0 {
            return TRUE;
        }
        // Only ApplicationFrameHost frames host UWP CoreWindows; skip the rest
        // cheaply before paying for a child enumeration.
        if window_class_name(hwnd) != "ApplicationFrameWindow" {
            return TRUE;
        }

        // Inner pass: does this frame own a child window belonging to the
        // launched app process?
        struct ChildScan {
            target_app_pid: u32,
            found: bool,
        }
        unsafe extern "system" fn child_cb(child: HWND, lparam: LPARAM) -> BOOL {
            let cs = &mut *(lparam.0 as *mut ChildScan);
            let mut child_pid: u32 = 0;
            GetWindowThreadProcessId(child, Some(&mut child_pid));
            if child_pid == cs.target_app_pid {
                cs.found = true;
                return windows::Win32::Foundation::FALSE; // stop enumerating children
            }
            TRUE
        }

        let mut child_scan = ChildScan {
            target_app_pid: scan.target_app_pid,
            found: false,
        };
        let _ = EnumChildWindows(
            hwnd,
            Some(child_cb),
            LPARAM(&mut child_scan as *mut ChildScan as isize),
        );

        if child_scan.found {
            scan.matched_frame = Some(hwnd);
            return windows::Win32::Foundation::FALSE; // stop enumerating frames
        }
        TRUE
    }

    let mut scan = FrameScan {
        target_app_pid: app_pid,
        matched_frame: None,
    };
    unsafe {
        let _ = EnumWindows(
            Some(frame_cb),
            LPARAM(&mut scan as *mut FrameScan as isize),
        );
    }

    let frame = scan.matched_frame?;

    // Build the WindowInfo for the frame itself: its HWND is what the caller
    // drives, and its owning pid is the AFH pid (what get_window_state will
    // validate `window_id` against and what list_windows reports for it).
    let mut afh_pid: u32 = 0;
    unsafe { GetWindowThreadProcessId(frame, Some(&mut afh_pid)) };

    let title = unsafe {
        let len = GetWindowTextLengthW(frame);
        if len == 0 {
            String::new()
        } else {
            let mut buf = vec![0u16; (len + 1) as usize];
            GetWindowTextW(frame, &mut buf);
            let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            String::from_utf16_lossy(&buf[..n])
        }
    };

    let (x, y, w, h) = get_window_bounds(frame);

    Some(WindowInfo {
        hwnd: frame.0 as u64,
        pid: afh_pid,
        title,
        x,
        y,
        width: w,
        height: h,
        is_on_screen: true,
        minimized: false,
    })
}
