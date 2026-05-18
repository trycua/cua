//! UIA-based enumeration of top-level windows.
//!
//! Walks the UI Automation tree from the desktop root and returns one entry
//! per top-level interactable window. UIA surfaces modern containers (WebView2
//! hosts, packaged-UWP frames, browser windows whose chrome lives inside a
//! container HWND) with their real title and bounds — which `EnumWindows`
//! either misses or returns with a misleading parent HWND.
//!
//! The result is shape-compatible with `crate::win32::windows::WindowInfo`
//! (returned as `WindowInfo` directly) so the existing pipeline that consumes
//! `list_windows` output keeps working unchanged. Each record's `hwnd` is the
//! UIA element's `NativeWindowHandle` — i.e. an honest Win32 HWND that downstream
//! code can pass to `GetWindowRect`, `PostMessage`, etc.

use std::cell::RefCell;

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationInvokePattern,
    TreeScope_Children, TreeScope_Subtree, UIA_InvokePatternId,
};
use windows::core::Interface;
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowRect, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
};

use crate::win32::windows::WindowInfo;

/// HRESULT for "COM already initialized in another mode on this thread."
/// Returned by `CoInitializeEx` when something else (a previous call in the
/// same task, or a library on the same OS thread) picked a different
/// apartment. Safe to ignore — COM is up either way.
const RPC_E_CHANGED_MODE: i32 = -2147417850; // 0x80010106

thread_local! {
    /// Per-thread IUIAutomation instance. UIA / COM objects are
    /// apartment-bound, so we deliberately do NOT share one across threads —
    /// each OS thread that calls `enumerate_top_level_windows` initializes
    /// COM as STA exactly once (the first time the cell is `None`) and caches
    /// its own IUIAutomation. On failure the cell stays `None`, so the next
    /// call retries from scratch instead of being stuck with a permanent
    /// "init failed" sentinel.
    static UIA_THREAD_LOCAL: RefCell<Option<IUIAutomation>> = const { RefCell::new(None) };
}

/// Build or fetch the per-thread IUIAutomation instance.
///
/// On the first successful call per OS thread this also initializes COM as
/// STA (UIA's in-process server requires an STA; `RPC_E_CHANGED_MODE` from a
/// pre-existing apartment is treated as "already up"). Any failure leaves
/// the thread-local empty so subsequent calls retry.
fn get_uia() -> Option<IUIAutomation> {
    UIA_THREAD_LOCAL.with(|cell| {
        if let Some(uia) = cell.borrow().as_ref() {
            return Some(uia.clone());
        }
        // First (or post-failure) call on this thread: init COM + build UIA.
        unsafe {
            let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            if hr.is_err() && hr.0 != RPC_E_CHANGED_MODE {
                tracing::debug!(target: "uia_windows_enum", "CoInitializeEx returned {hr:?}");
            }
        }
        let inst: IUIAutomation = match unsafe {
            CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
        } {
            Ok(a) => a,
            Err(e) => {
                tracing::warn!(target: "uia_windows_enum", "CoCreateInstance(CUIAutomation) failed: {e}");
                return None;
            }
        };
        let dup = inst.clone();
        *cell.borrow_mut() = Some(inst);
        Some(dup)
    })
}

/// Enumerate top-level windows visible to UI Automation.
///
/// Returns one `WindowInfo` per non-offscreen child of the UIA desktop root
/// whose `NativeWindowHandle` is non-null and resolves to a window with a
/// non-empty title. Windows whose HWND is zero (pure UIA virtual elements,
/// rare) are skipped because the rest of the driver pipeline keys off HWND.
///
/// Returns an empty vec on any UIA failure — callers should treat UIA as a
/// best-effort source and union with `EnumWindows`.
pub fn enumerate_top_level_windows() -> Vec<WindowInfo> {
    let uia = match get_uia() {
        Some(u) => u,
        None => return Vec::new(),
    };

    unsafe {
        let root = match uia.GetRootElement() {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!(target: "uia_windows_enum", "GetRootElement failed: {e}");
                return Vec::new();
            }
        };
        let condition = match uia.CreateTrueCondition() {
            Ok(c) => c,
            Err(e) => {
                tracing::debug!(target: "uia_windows_enum", "CreateTrueCondition failed: {e}");
                return Vec::new();
            }
        };
        let children = match root.FindAll(TreeScope_Children, &condition) {
            Ok(c) => c,
            Err(e) => {
                tracing::debug!(target: "uia_windows_enum", "FindAll(Children) failed: {e}");
                return Vec::new();
            }
        };

        let count = children.Length().unwrap_or(0);
        let mut out: Vec<WindowInfo> = Vec::with_capacity(count as usize);
        for i in 0..count {
            let elem = match children.GetElement(i) {
                Ok(e) => e,
                Err(_) => continue,
            };
            if let Some(info) = window_info_from_uia_element(&elem) {
                out.push(info);
            }
        }
        out
    }
}

/// Hit-test screen point `(sx, sy)` against the UIA subtree rooted at
/// `hwnd` and fire `Invoke` on the deepest descendant whose bounding
/// rect contains the point AND which supports `InvokePattern`. Returns
/// `true` iff such an element was found and `Invoke()` succeeded.
///
/// Why a windowed walk and not desktop-wide `ElementFromPoint`:
///
/// 1. Z-order — if the desktop's topmost element at `(sx, sy)` is some
///    other window (a terminal, a chrome window covering the target),
///    `ElementFromPoint` returns *that* element, not anything inside
///    `hwnd`. The (x, y) caller already knows the intended HWND; we
///    should trust it.
///
/// 2. UWP / packaged-app hosting — `ApplicationFrameHost.exe` is the
///    outer host process; the actual UWP content lives in a separate
///    process (e.g. `CalculatorApp.exe`). `ElementFromPoint` has been
///    observed returning the frame's outer Pane (no `InvokePattern`)
///    instead of descending into the cross-process child. Rooting the
///    search at the frame's UIA element and walking with
///    `TreeScope_Subtree` does cross that boundary.
///
/// 3. Vision-mode contract — the agent screenshotted a specific window
///    and is addressing pixels of that window. We respect that
///    intent: the click goes to that window's tree, period.
///
/// Used by the click tool's `(x, y)` path as the **no-focus-steal**
/// route for UWP / WebView2 / packaged-app targets, where
/// `PostMessage(WM_LBUTTONDOWN)` silently no-ops because UWP routes
/// input through `Windows.UI.Input` rather than the HWND message
/// queue. Callers fall back to PostMessage when this returns `false`
/// (e.g. plain Win32 native controls with no UIA InvokePattern at
/// the hit point, or apps with no useful UIA tree at all).
///
/// Implementation: `ElementFromHandle(hwnd)` resolves the root,
/// `FindAll(TreeScope_Subtree, TrueCondition)` enumerates the
/// subtree (including the root, so single-element windows are still
/// hit-testable), and we pick the smallest-area element whose
/// `CurrentBoundingRectangle` contains the point AND which exposes
/// `InvokePattern`. Smallest-area approximates "deepest" without
/// having to track tree depth explicitly.
pub fn try_invoke_in_window_at_point(hwnd: isize, sx: i32, sy: i32) -> bool {
    if hwnd == 0 {
        return false;
    }
    let uia = match get_uia() {
        Some(u) => u,
        None => return false,
    };
    unsafe {
        let root = match uia.ElementFromHandle(HWND(hwnd as *mut _)) {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!(target: "click", "ElementFromHandle(0x{hwnd:x}) failed: {e}");
                return false;
            }
        };
        let cond = match uia.CreateTrueCondition() {
            Ok(c) => c,
            Err(e) => {
                tracing::debug!(target: "click", "CreateTrueCondition failed: {e}");
                return false;
            }
        };
        let arr = match root.FindAll(TreeScope_Subtree, &cond) {
            Ok(a) => a,
            Err(e) => {
                tracing::debug!(target: "click", "FindAll(Subtree) on 0x{hwnd:x} failed: {e}");
                return false;
            }
        };
        let n = arr.Length().unwrap_or(0);
        let mut best: Option<(IUIAutomationElement, i64)> = None;
        for i in 0..n {
            let elem = match arr.GetElement(i) {
                Ok(e) => e,
                Err(_) => continue,
            };
            let rect = match elem.CurrentBoundingRectangle() {
                Ok(r) => r,
                Err(_) => continue,
            };
            if sx < rect.left || sx > rect.right || sy < rect.top || sy > rect.bottom {
                continue;
            }
            if elem.GetCurrentPattern(UIA_InvokePatternId).is_err() {
                continue;
            }
            let w = (rect.right - rect.left).max(0) as i64;
            let h = (rect.bottom - rect.top).max(0) as i64;
            let area = w.saturating_mul(h);
            match &best {
                None => best = Some((elem, area)),
                Some((_, prev)) if area < *prev => best = Some((elem, area)),
                _ => {}
            }
        }
        let (winner, _) = match best {
            Some(b) => b,
            None => {
                tracing::debug!(
                    target: "click",
                    "no InvokePattern descendant of 0x{hwnd:x} contains screen ({sx},{sy}) (scanned {n} elems)"
                );
                return false;
            }
        };
        let pattern = match winner.GetCurrentPattern(UIA_InvokePatternId) {
            Ok(p) => p,
            Err(_) => return false,
        };
        let inv: IUIAutomationInvokePattern = match pattern.cast() {
            Ok(i) => i,
            Err(_) => return false,
        };
        match inv.Invoke() {
            Ok(()) => true,
            Err(e) => {
                tracing::debug!(target: "click", "UIA Invoke (windowed) at ({sx},{sy}) failed: {e}");
                false
            }
        }
    }
}

/// Build a `WindowInfo` from a single UIA child element of the desktop root.
/// Returns `None` if the element doesn't correspond to a real, on-screen,
/// non-empty-titled HWND.
unsafe fn window_info_from_uia_element(elem: &IUIAutomationElement) -> Option<WindowInfo> {
    // NativeWindowHandle is an i32-sized handle in UIA; cast to HWND.
    let raw = elem.CurrentNativeWindowHandle().ok()?;
    if raw.0.is_null() {
        return None;
    }
    let hwnd = HWND(raw.0);

    // Drop minimized / off-screen windows. UIA's IsOffscreen flag covers
    // both "iconic" and "behind another window such that no part is visible"
    // — for top-level windows it matches the EnumWindows path's intent of
    // showing only currently-visible candidates.
    if let Ok(flag) = elem.CurrentIsOffscreen() {
        if flag.as_bool() {
            return None;
        }
    }

    // Resolve pid via the standard Win32 path. We deliberately do not trust
    // the UIA ProcessId property here because the rest of the driver indexes
    // windows by (hwnd, pid) tuples obtained from `GetWindowThreadProcessId`,
    // and we want bit-identical agreement.
    let mut pid: u32 = 0;
    let _ = GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid == 0 {
        return None;
    }

    // Title — prefer Win32 GetWindowTextW for parity with the EnumWindows path.
    // UIA's `CurrentName` sometimes returns the AX-friendly label (e.g. the
    // tab title) instead of the OS-level window caption, which would diverge
    // from any caller already keyed on the GetWindowText value.
    let title_len = GetWindowTextLengthW(hwnd);
    if title_len == 0 {
        return None;
    }
    let mut buf = vec![0u16; (title_len + 1) as usize];
    GetWindowTextW(hwnd, &mut buf);
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let title = String::from_utf16_lossy(&buf[..len]);
    if title.trim().is_empty() {
        return None;
    }

    let (x, y, w, h) = window_bounds(hwnd);

    Some(WindowInfo {
        hwnd: hwnd.0 as u64,
        pid,
        title,
        x,
        y,
        width: w,
        height: h,
    })
}

/// Bounds via DWM extended frame (excludes drop-shadow on W11) with
/// `GetWindowRect` fallback — same logic as the EnumWindows path.
fn window_bounds(hwnd: HWND) -> (i32, i32, i32, i32) {
    unsafe {
        let mut rect = RECT::default();
        let ok = DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut rect as *mut RECT as *mut _,
            std::mem::size_of::<RECT>() as u32,
        );
        if ok.is_err() {
            let _ = GetWindowRect(hwnd, &mut rect);
        }
        (rect.left, rect.top, rect.right - rect.left, rect.bottom - rect.top)
    }
}
