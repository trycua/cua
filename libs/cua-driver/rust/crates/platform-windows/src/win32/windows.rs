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
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, EnumWindows, GetClassNameW, GetWindow, GetWindowRect, GetWindowTextLengthW,
    GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, GW_OWNER,
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
    let win32_windows = list_windows_via_win32(None);
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

/// Resolve one exact Win32 window without entering the global UIA tree.
///
/// Exact `(pid, HWND)` callers already have a native identity anchor. Avoiding
/// the UIA union keeps an unrelated unresponsive provider from blocking that
/// ownership check.
pub(crate) fn find_window_by_pid_and_handle(pid: u32, hwnd: u64) -> Option<WindowInfo> {
    exact_window_from_probe(pid, hwnd, window_info_by_handle)
}

fn exact_window_from_probe(
    pid: u32,
    hwnd: u64,
    probe: impl FnOnce(u64) -> Option<WindowInfo>,
) -> Option<WindowInfo> {
    probe(hwnd).filter(|window| window.pid == pid && window.hwnd == hwnd)
}

/// Return the native owner of one exact HWND without enumerating Win32 or UIA.
pub(crate) fn window_owner_pid(hwnd: u64) -> Option<u32> {
    let hwnd = HWND(hwnd as *mut _);
    if hwnd.0.is_null() || !unsafe { IsWindow(hwnd) }.as_bool() {
        return None;
    }
    let mut pid = 0;
    let thread_id = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    (thread_id != 0 && pid != 0).then_some(pid)
}

#[derive(Clone, Copy, Debug)]
struct PostActionForegroundRelation {
    exact_match: bool,
    target_identity_live: bool,
    target_gone: bool,
    actual_live: bool,
    actual_visible: bool,
    same_pid: bool,
    ownership_reaches_target: bool,
    actual_is_prior_owner: bool,
}

fn post_action_foreground_allowed(relation: PostActionForegroundRelation) -> bool {
    relation.actual_live
        && relation.actual_visible
        && relation.same_pid
        && ((relation.target_identity_live
            && (relation.exact_match || relation.ownership_reaches_target))
            || (relation.target_gone && relation.actual_is_prior_owner))
}

fn owner_chain_reaches_target(
    target: u64,
    actual: u64,
    mut owner_of: impl FnMut(u64) -> Option<u64>,
) -> bool {
    let mut current = actual;
    for _ in 0..64 {
        let Some(owner) = owner_of(current) else {
            return false;
        };
        if owner == target {
            return true;
        }
        if owner == current || owner == actual {
            return false;
        }
        current = owner;
    }
    false
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ForegroundTarget {
    hwnd: u64,
    pid: u32,
    owner: Option<u64>,
}

/// Snapshot the exact target identity and owner before global input is sent.
pub(crate) fn capture_foreground_target(target: u64) -> Option<ForegroundTarget> {
    let pid = window_owner_pid(target)?;
    let owner = unsafe { GetWindow(HWND(target as *mut _), GW_OWNER) }
        .ok()
        .filter(|owner| !owner.0.is_null())
        .map(|owner| owner.0 as usize as u64);
    Some(ForegroundTarget {
        hwnd: target,
        pid,
        owner,
    })
}

/// Verify the foreground before or after an exact-target global input action.
///
/// The requested HWND must become foreground before input is sent. The action
/// may then legitimately open a same-process owned popup or modal, so the
/// post-action check accepts that transient only when it is still a live,
/// visible HWND and its complete `GW_OWNER` chain reaches the exact target.
/// A dismiss action may instead destroy the requested owned modal; that is
/// accepted only when foreground returns to its snapshotted same-process owner.
/// Unrelated same-process siblings and foreign foreground windows fail closed.
pub(crate) fn foreground_matches_target_or_owned_window(
    target: ForegroundTarget,
    actual: u64,
) -> bool {
    let current_target_pid = window_owner_pid(target.hwnd);
    let actual_pid = window_owner_pid(actual);
    let actual_hwnd = HWND(actual as *mut _);
    let actual_visible = actual_pid.is_some() && unsafe { IsWindowVisible(actual_hwnd) }.as_bool();
    let ownership_reaches_target = target.hwnd != actual
        && owner_chain_reaches_target(target.hwnd, actual, |hwnd| {
            let owner = unsafe { GetWindow(HWND(hwnd as *mut _), GW_OWNER) }
                .ok()
                .unwrap_or_default();
            (!owner.0.is_null()).then_some(owner.0 as usize as u64)
        });

    post_action_foreground_allowed(PostActionForegroundRelation {
        exact_match: target.hwnd == actual,
        target_identity_live: current_target_pid == Some(target.pid),
        target_gone: current_target_pid.is_none(),
        actual_live: actual_pid.is_some(),
        actual_visible,
        same_pid: actual_pid == Some(target.pid),
        ownership_reaches_target,
        actual_is_prior_owner: target.owner == Some(actual),
    })
}

fn window_info_by_handle(hwnd: u64) -> Option<WindowInfo> {
    let native = HWND(hwnd as *mut _);
    let pid = window_owner_pid(hwnd)?;
    if !unsafe { IsWindowVisible(native) }.as_bool() {
        return None;
    }
    let title_len = unsafe { GetWindowTextLengthW(native) };
    if title_len == 0 {
        return None;
    }
    let mut buf = vec![0u16; (title_len + 1) as usize];
    unsafe { GetWindowTextW(native, &mut buf) };
    let title_len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let title = String::from_utf16_lossy(&buf[..title_len]);
    if title.trim().is_empty() {
        return None;
    }
    let minimized = unsafe { IsIconic(native) }.as_bool();
    let (x, y, width, height) = get_window_bounds(native);
    Some(WindowInfo {
        hwnd,
        pid,
        title,
        x,
        y,
        width,
        height,
        is_on_screen: !minimized,
        minimized,
    })
}

pub(crate) fn list_windows_via_win32(filter_pid: Option<u32>) -> Vec<WindowInfo> {
    let mut windows = enumerate_via_enum_windows();
    if let Some(pid) = filter_pid {
        windows.retain(|window| window.pid == pid);
    }
    windows
}

/// Walk `EnumWindows` and collect every visible, non-empty-titled
/// top-level window. No pid filter is applied here — the caller does that on
/// the merged list.
fn enumerate_via_enum_windows() -> Vec<WindowInfo> {
    let state = Mutex::new(EnumState {
        windows: Vec::new(),
    });
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
    if title_len == 0 {
        return TRUE;
    }
    let mut buf = vec![0u16; (title_len + 1) as usize];
    GetWindowTextW(hwnd, &mut buf);
    let title = {
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..len])
    };
    if title.trim().is_empty() {
        return TRUE;
    }

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
        (
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top,
        )
    }
}

#[cfg(test)]
mod exact_window_tests {
    use super::{
        exact_window_from_probe, owner_chain_reaches_target, post_action_foreground_allowed,
        PostActionForegroundRelation, WindowInfo,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn window(pid: u32, hwnd: u64) -> WindowInfo {
        WindowInfo {
            hwnd,
            pid,
            title: "Exact target".into(),
            x: 0,
            y: 0,
            width: 100,
            height: 100,
            is_on_screen: true,
            minimized: false,
        }
    }

    #[test]
    fn exact_lookup_probes_only_the_requested_native_handle() {
        let calls = AtomicUsize::new(0);
        let found = exact_window_from_probe(42, 0x1234, |hwnd| {
            calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(hwnd, 0x1234);
            Some(window(42, hwnd))
        });

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(found.map(|window| window.hwnd), Some(0x1234));
    }

    #[test]
    fn exact_lookup_rejects_wrong_pid_or_handle() {
        assert!(exact_window_from_probe(42, 7, |_| Some(window(43, 7))).is_none());
        assert!(exact_window_from_probe(42, 7, |_| Some(window(42, 8))).is_none());
    }

    fn relation(
        exact_match: bool,
        same_pid: bool,
        ownership_reaches_target: bool,
    ) -> PostActionForegroundRelation {
        PostActionForegroundRelation {
            exact_match,
            target_identity_live: true,
            target_gone: false,
            actual_live: true,
            actual_visible: true,
            same_pid,
            ownership_reaches_target,
            actual_is_prior_owner: false,
        }
    }

    #[test]
    fn exact_or_owned_modal_foreground_is_allowed() {
        assert!(post_action_foreground_allowed(relation(true, true, false)));
        assert!(post_action_foreground_allowed(relation(false, true, true)));
    }

    #[test]
    fn nested_owned_popup_chain_reaches_exact_target() {
        let owners = [(30, 20), (20, 10)];
        assert!(owner_chain_reaches_target(10, 30, |hwnd| {
            owners
                .iter()
                .find_map(|(child, owner)| (*child == hwnd).then_some(*owner))
        }));
    }

    #[test]
    fn unrelated_same_pid_sibling_and_foreign_foreground_are_denied() {
        assert!(!post_action_foreground_allowed(relation(
            false, true, false
        )));
        assert!(!post_action_foreground_allowed(relation(
            false, false, true
        )));
        assert!(!owner_chain_reaches_target(10, 30, |hwnd| {
            (hwnd == 30).then_some(40)
        }));
    }

    #[test]
    fn dismissed_owned_modal_may_return_to_its_snapshotted_owner_only() {
        let mut dismissed = relation(false, true, false);
        dismissed.target_identity_live = false;
        dismissed.target_gone = true;
        dismissed.actual_is_prior_owner = true;
        assert!(post_action_foreground_allowed(dismissed));

        dismissed.actual_is_prior_owner = false;
        assert!(!post_action_foreground_allowed(dismissed));

        dismissed.actual_is_prior_owner = true;
        dismissed.same_pid = false;
        assert!(!post_action_foreground_allowed(dismissed));
    }

    #[test]
    fn stale_reused_invisible_or_cyclic_foreground_is_denied() {
        let mut stale = relation(false, true, true);
        stale.target_identity_live = false;
        assert!(!post_action_foreground_allowed(stale));
        stale.target_identity_live = true;
        stale.actual_live = false;
        assert!(!post_action_foreground_allowed(stale));
        stale.actual_live = true;
        stale.actual_visible = false;
        assert!(!post_action_foreground_allowed(stale));
        assert!(!owner_chain_reaches_target(10, 30, |_| Some(30)));
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
        let _ = EnumWindows(Some(frame_cb), LPARAM(&mut scan as *mut FrameScan as isize));
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

/// Resolve the real packaged-app process hosted by a specific
/// `ApplicationFrameWindow`.
///
/// An ApplicationFrameHost pid is shared by unrelated apps, so callers must
/// provide the frame HWND as well as the host pid. The hosted CoreWindow child
/// retains ownership by the packaged app process and supplies the identity we
/// need for attribution.
pub fn resolve_uwp_app_pid(host_pid: u32, frame_hwnd: u64) -> Option<u32> {
    let frame = HWND(frame_hwnd as *mut _);
    if unsafe { IsWindow(frame) }.0 == 0 || window_class_name(frame) != "ApplicationFrameWindow" {
        return None;
    }

    let mut owner_pid = 0;
    unsafe { GetWindowThreadProcessId(frame, Some(&mut owner_pid)) };
    if owner_pid != host_pid {
        return None;
    }

    struct ChildScan {
        host_pid: u32,
        app_pid: Option<u32>,
    }

    unsafe extern "system" fn child_cb(child: HWND, lparam: LPARAM) -> BOOL {
        let scan = &mut *(lparam.0 as *mut ChildScan);
        let mut child_pid = 0;
        GetWindowThreadProcessId(child, Some(&mut child_pid));
        if child_pid != 0
            && child_pid != scan.host_pid
            && window_class_name(child) == "Windows.UI.Core.CoreWindow"
        {
            scan.app_pid = Some(child_pid);
            return windows::Win32::Foundation::FALSE;
        }
        TRUE
    }

    let mut scan = ChildScan {
        host_pid,
        app_pid: None,
    };
    unsafe {
        let _ = EnumChildWindows(
            frame,
            Some(child_cb),
            LPARAM(&mut scan as *mut ChildScan as isize),
        );
    }
    scan.app_pid
}
