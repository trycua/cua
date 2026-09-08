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

// We pattern-match against `UIA_*ControlTypeId` constants from the `windows`
// crate, which use mixed case we can't rename. The lint's suggested rewrite
// (UIA_BUTTON_CONTROL_TYPE_ID) would silently shadow the external constant
// with a fresh local binding and break the match. Mirrors overlay.rs:12.
#![allow(non_upper_case_globals)]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use windows::core::{Interface, BSTR};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationInvokePattern,
    IUIAutomationTogglePattern, TreeScope_Children, TreeScope_Subtree,
    UIA_AcceleratorKeyPropertyId, UIA_ButtonControlTypeId, UIA_CheckBoxControlTypeId,
    UIA_HyperlinkControlTypeId, UIA_InvokePatternId, UIA_ListItemControlTypeId,
    UIA_MenuItemControlTypeId, UIA_RadioButtonControlTypeId, UIA_SplitButtonControlTypeId,
    UIA_TabItemControlTypeId, UIA_TogglePatternId, UIA_TreeItemControlTypeId, UIA_CONTROLTYPE_ID,
    UIA_PROPERTY_ID,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetWindowRect, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
    IsIconic, IsWindow, IsWindowVisible, GA_ROOT,
};

use crate::win32::windows::WindowInfo;

/// HRESULT for "COM already initialized in another mode on this thread."
/// Returned by `CoInitializeEx` when something else (a previous call in the
/// same task, or a library on the same OS thread) picked a different
/// apartment. Safe to ignore — COM is up either way.
const RPC_E_CHANGED_MODE: i32 = -2147417850; // 0x80010106
/// Desktop child enumeration: one wedged provider must not stall list_windows.
const DESKTOP_CALL_TIMEOUT: Duration = Duration::from_secs(2);
/// Subtree scans: keep the pre-existing interactive operation budget.
const SUBTREE_OP_TIMEOUT: Duration = Duration::from_secs(4);
const UIA_RECOVERY_COOLDOWN: Duration = Duration::from_secs(30);

enum UiaDeadlineError {
    Timeout,
    Busy,
    Unavailable,
}

/// Process-wide gate for blocking UIA calls.
///
/// Windows offers no safe way to cancel a COM provider call in another thread.
/// The gate therefore remains owned by a timed-out worker until that worker
/// actually returns. Retries fail fast instead of stranding more threads. Once
/// the provider recovers, the worker releases the gate after a short cooldown.
struct UiaSingleFlight {
    in_flight: AtomicBool,
    cooldown_until_ms: AtomicU64,
    cooldown_ms: u64,
}

impl UiaSingleFlight {
    const fn new(cooldown_ms: u64) -> Self {
        Self {
            in_flight: AtomicBool::new(false),
            cooldown_until_ms: AtomicU64::new(0),
            cooldown_ms,
        }
    }

    fn run<T, F>(
        self: &Arc<Self>,
        stage: &'static str,
        timeout: Duration,
        fallback: &'static str,
        f: F,
    ) -> Result<T, UiaDeadlineError>
    where
        T: Send + 'static,
        F: FnOnce(Arc<AtomicBool>) -> T + Send + 'static,
    {
        let now = now_ms();
        if now < self.cooldown_until_ms.load(Ordering::Acquire)
            || self
                .in_flight
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            tracing::debug!(
                target: "uia_windows_enum",
                "UIA {stage} skipped while another provider call is in flight or cooling down"
            );
            return Err(UiaDeadlineError::Busy);
        }

        let (tx, rx) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let worker_gate = Arc::clone(self);
        let spawn = thread::Builder::new()
            .name(format!("cua-uia-{stage}"))
            .spawn(move || {
                let _guard = InFlightGuard {
                    gate: worker_gate,
                    cancelled: Arc::clone(&worker_cancelled),
                };
                let result = f(worker_cancelled);
                let _ = tx.send(result);
            });

        if let Err(e) = spawn {
            self.in_flight.store(false, Ordering::Release);
            tracing::warn!(target: "uia_windows_enum", "failed to spawn UIA {stage} thread: {e}");
            return Err(UiaDeadlineError::Unavailable);
        }

        match rx.recv_timeout(timeout) {
            Ok(result) => Ok(result),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                cancelled.store(true, Ordering::Release);
                tracing::warn!(
                    target: "uia_windows_enum",
                    "UIA {stage} exceeded {}ms; falling back to {fallback}; no other UIA worker will start until this provider call returns",
                    timeout.as_millis()
                );
                Err(UiaDeadlineError::Timeout)
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                tracing::warn!(target: "uia_windows_enum", "UIA {stage} thread exited without a result");
                Err(UiaDeadlineError::Unavailable)
            }
        }
    }
}

struct InFlightGuard {
    gate: Arc<UiaSingleFlight>,
    cancelled: Arc<AtomicBool>,
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        if self.cancelled.load(Ordering::Acquire) {
            self.gate.cooldown_until_ms.store(
                now_ms().saturating_add(self.gate.cooldown_ms),
                Ordering::Release,
            );
            tracing::warn!(
                target: "uia_windows_enum",
                "timed-out UIA provider call returned; cooling down for {}ms before recovery probe",
                self.gate.cooldown_ms
            );
        }
        self.gate.in_flight.store(false, Ordering::Release);
    }
}

fn uia_single_flight() -> &'static Arc<UiaSingleFlight> {
    static GATE: OnceLock<Arc<UiaSingleFlight>> = OnceLock::new();
    GATE.get_or_init(|| {
        Arc::new(UiaSingleFlight::new(
            UIA_RECOVERY_COOLDOWN.as_millis() as u64
        ))
    })
}

fn now_ms() -> u64 {
    // Monotonic: cooldown must not depend on wall-clock jumps.
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

fn run_uia_with_deadline<T, F>(
    stage: &'static str,
    timeout: Duration,
    fallback: &'static str,
    f: F,
) -> Result<T, UiaDeadlineError>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    run_uia_with_deadline_cancelable(stage, timeout, fallback, |_| f())
}

fn run_uia_with_deadline_cancelable<T, F>(
    stage: &'static str,
    timeout: Duration,
    fallback: &'static str,
    f: F,
) -> Result<T, UiaDeadlineError>
where
    T: Send + 'static,
    F: FnOnce(Arc<AtomicBool>) -> T + Send + 'static,
{
    uia_single_flight().run(stage, timeout, fallback, f)
}

struct ComInit {
    needs_uninit: bool,
}

impl ComInit {
    fn new() -> Self {
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr.is_err() && hr.0 != RPC_E_CHANGED_MODE {
            tracing::debug!(target: "uia_windows_enum", "CoInitializeEx returned {hr:?}");
        }
        Self {
            needs_uninit: hr.is_ok(),
        }
    }
}

impl Drop for ComInit {
    fn drop(&mut self) {
        if self.needs_uninit {
            unsafe { CoUninitialize() };
        }
    }
}

/// Build an IUIAutomation instance after COM is initialized.
fn get_uia() -> Option<IUIAutomation> {
    match unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) } {
        Ok(a) => Some(a),
        Err(e) => {
            tracing::warn!(target: "uia_windows_enum", "CoCreateInstance(CUIAutomation) failed: {e}");
            None
        }
    }
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
    match run_uia_with_deadline(
        "desktop enumeration",
        DESKTOP_CALL_TIMEOUT,
        "Win32-only window list",
        enumerate_top_level_windows_unbounded,
    ) {
        Ok(windows) => windows,
        Err(UiaDeadlineError::Timeout | UiaDeadlineError::Busy | UiaDeadlineError::Unavailable) => {
            Vec::new()
        }
    }
}

fn enumerate_top_level_windows_unbounded() -> Vec<WindowInfo> {
    // Keep this first so COM interfaces drop before CoUninitialize.
    let _com = ComInit::new();
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

/// Exercise the real desktop enumeration call shape for `cua-driver doctor`
/// without allowing health checks to bypass the process-wide single-flight
/// bound used by window and interaction tools.
pub(crate) fn probe_desktop_availability() -> Result<(), String> {
    match run_uia_with_deadline(
        "health probe",
        DESKTOP_CALL_TIMEOUT,
        "Win32-only window tools",
        probe_desktop_availability_unbounded,
    ) {
        Ok(result) => result,
        Err(UiaDeadlineError::Timeout) => Err(format!(
            "UI Automation desktop enumeration exceeded {}ms; a UIA provider may be hung. Window tools will fall back to Win32-only enumeration until it recovers.",
            DESKTOP_CALL_TIMEOUT.as_millis()
        )),
        Err(UiaDeadlineError::Busy) => Err(
            "UI Automation is busy with an earlier timed-out provider call; window tools are temporarily using Win32-only enumeration."
                .to_owned(),
        ),
        Err(UiaDeadlineError::Unavailable) => {
            Err("UI Automation health worker is unavailable".to_owned())
        }
    }
}

fn probe_desktop_availability_unbounded() -> Result<(), String> {
    // Declared first so UIA interfaces are released before COM is uninitialized.
    let _com = ComInit::new();
    let automation =
        get_uia().ok_or_else(|| "CoCreateInstance(CUIAutomation) failed".to_owned())?;
    unsafe {
        let root = automation
            .GetRootElement()
            .map_err(|e| format!("GetRootElement: {e}"))?;
        let condition = automation
            .CreateTrueCondition()
            .map_err(|e| format!("CreateTrueCondition: {e}"))?;
        let _children = root
            .FindAll(TreeScope_Children, &condition)
            .map_err(|e| format!("FindAll(TreeScope_Children): {e}"))?;
    }
    Ok(())
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
/// Returns `true` when the element's control type has a *coord-independent*
/// primary action — i.e. a UIA `Invoke()` on it does something semantically
/// equivalent to "click the element" regardless of where inside its bounding
/// rectangle the click was requested.
///
/// Used by the `x, y` click path to decide whether to take the UIA Invoke
/// route or fall through to PostMessage with the literal coords. The split
/// matters for canvases, panes, and custom-drawn surfaces where Invoke would
/// fire `mousedown` at the element centre — losing the caller's pixel
/// precision (see #1621).
fn is_coord_independent_action(elem: &IUIAutomationElement) -> bool {
    let ct: UIA_CONTROLTYPE_ID = match unsafe { elem.CurrentControlType() } {
        Ok(t) => t,
        Err(_) => return false,
    };
    matches!(
        ct,
        UIA_ButtonControlTypeId
            | UIA_MenuItemControlTypeId
            | UIA_HyperlinkControlTypeId
            | UIA_TabItemControlTypeId
            | UIA_ListItemControlTypeId
            | UIA_CheckBoxControlTypeId
            | UIA_RadioButtonControlTypeId
            | UIA_SplitButtonControlTypeId
            | UIA_TreeItemControlTypeId
    )
}

pub fn try_invoke_in_window_at_point(hwnd: isize, sx: i32, sy: i32) -> bool {
    run_uia_with_deadline_cancelable(
        "window hit-test invoke",
        SUBTREE_OP_TIMEOUT,
        "PostMessage click delivery",
        move |cancelled| try_invoke_in_window_at_point_unbounded(hwnd, sx, sy, &cancelled),
    )
    .unwrap_or(false)
}

fn try_invoke_in_window_at_point_unbounded(
    hwnd: isize,
    sx: i32,
    sy: i32,
    cancelled: &AtomicBool,
) -> bool {
    // Keep this first so COM interfaces drop before CoUninitialize.
    let _com = ComInit::new();
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
            // Accept elements that support EITHER InvokePattern OR
            // ExpandCollapsePattern. Qt menu-bar items advertise both —
            // Invoke does nothing on them, only Expand opens the submenu.
            // (See FreeCAD finding 2026-05-21: clicking File menu via Invoke
            // returned ✅ but the menu never opened.)
            let has_invoke = elem.GetCurrentPattern(UIA_InvokePatternId).is_ok();
            let has_expand = elem
                .GetCurrentPattern(windows::Win32::UI::Accessibility::UIA_ExpandCollapsePatternId)
                .is_ok();
            if !has_invoke && !has_expand {
                continue;
            }
            // For coordinate-addressed clicks, only accept elements whose
            // control type has a *coord-independent* primary action. UIA
            // `Invoke()` fires the element's default action at its centre,
            // ignoring the requested (sx, sy). For container surfaces
            // (Pane, Image, Custom, Document, Group, etc.) that means the
            // caller's pixel precision is silently lost — see #1621, where
            // `click(canvas, x=110, y=677)` reported success but actually
            // fired the canvas's `mousedown` at its centre (152, 77).
            // Buttons / MenuItems / Hyperlinks / TabItems / ListItems /
            // CheckBoxes / RadioButtons / SplitButtons / TreeItems all
            // have a single primary action whose location is the element
            // itself — Invoke is the right path for those. Everything
            // else falls through to PostMessage with the literal coords.
            if !is_coord_independent_action(&elem) {
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
                    "no Invoke/ExpandCollapse descendant of 0x{hwnd:x} contains screen ({sx},{sy}) (scanned {n} elems)"
                );
                return false;
            }
        };
        if cancelled.load(Ordering::Acquire) {
            tracing::debug!(target: "click", "UIA hit-test invoke cancelled before activation");
            return false;
        }
        // Pattern preference for menu items: when both Invoke AND
        // ExpandCollapse are advertised, the element is almost always a
        // top-level MenuItem whose intended click behaviour is "open the
        // submenu" — Invoke would be a no-op. Prefer ExpandCollapse.Expand
        // in that case. Pure-Invoke leaves (buttons, links, etc.) go
        // through Invoke as before.
        let winner_has_expand = winner
            .GetCurrentPattern(windows::Win32::UI::Accessibility::UIA_ExpandCollapsePatternId)
            .is_ok();
        let winner_has_invoke = winner.GetCurrentPattern(UIA_InvokePatternId).is_ok();
        if cancelled.load(Ordering::Acquire) {
            return false;
        }
        // UWP foreground-steal bypass: gate the entire activation block on
        // `is_xaml_host_hwnd(hwnd)`. For non-XAML hosts the closure is a
        // straight passthrough.
        crate::uia::fg_bypass::run_with_uwp_bypass(hwnd, || {
            if cancelled.load(Ordering::Acquire) {
                return false;
            }
            if winner_has_expand && winner_has_invoke {
                // Try Expand first, fall back to Invoke if Expand fails.
                if let Ok(pat) = winner.GetCurrentPattern(
                    windows::Win32::UI::Accessibility::UIA_ExpandCollapsePatternId,
                ) {
                    if let Ok(ec) = pat
                        .cast::<windows::Win32::UI::Accessibility::IUIAutomationExpandCollapsePattern>()
                    {
                        if cancelled.load(Ordering::Acquire) {
                            return false;
                        }
                        if ec.Expand().is_ok() {
                            return true;
                        }
                    }
                }
                // Expand failed — fall through to Invoke as best-effort.
            } else if winner_has_expand && !winner_has_invoke {
                if let Ok(pat) = winner.GetCurrentPattern(
                    windows::Win32::UI::Accessibility::UIA_ExpandCollapsePatternId,
                ) {
                    if let Ok(ec) = pat
                        .cast::<windows::Win32::UI::Accessibility::IUIAutomationExpandCollapsePattern>()
                    {
                        if cancelled.load(Ordering::Acquire) {
                            return false;
                        }
                        return ec.Expand().is_ok();
                    }
                }
                return false;
            }
            let pattern = match winner.GetCurrentPattern(UIA_InvokePatternId) {
                Ok(p) => p,
                Err(_) => return false,
            };
            let inv: IUIAutomationInvokePattern = match pattern.cast() {
                Ok(i) => i,
                Err(_) => return false,
            };
            if cancelled.load(Ordering::Acquire) {
                return false;
            }
            match inv.Invoke() {
                Ok(()) => true,
                Err(e) => {
                    tracing::debug!(target: "click", "UIA Invoke (windowed) at ({sx},{sy}) failed: {e}");
                    false
                }
            }
        })
    }
}

/// Find a descendant of `hwnd` whose UIA `AcceleratorKey` property matches
/// `combo` (e.g. `ctrl+s`) and fire its `InvokePattern`.
///
/// Modern XAML / WinUI / UWP apps ignore posted WM_KEYDOWN/WM_KEYUP messages;
/// their keyboard accelerators are surfaced through UI Automation instead.
/// This helper keeps that routing narrow by requiring an advertised
/// AcceleratorKey match before invoking anything.
pub fn try_invoke_accelerator_in_window(hwnd: isize, combo: &str) -> anyhow::Result<(bool, usize)> {
    let combo = combo.to_owned();
    run_uia_with_deadline_cancelable(
        "accelerator invoke",
        SUBTREE_OP_TIMEOUT,
        "keyboard message delivery",
        move |cancelled| try_invoke_accelerator_in_window_unbounded(hwnd, &combo, &cancelled),
    )
    .unwrap_or_else(|_| {
        Err(anyhow::anyhow!(
            "UIA accelerator scan exceeded {}ms; a UIA provider in the target app is likely unresponsive",
            SUBTREE_OP_TIMEOUT.as_millis()
        ))
    })
}

fn try_invoke_accelerator_in_window_unbounded(
    hwnd: isize,
    combo: &str,
    cancelled: &AtomicBool,
) -> anyhow::Result<(bool, usize)> {
    // Keep this first so COM interfaces drop before CoUninitialize.
    let _com = ComInit::new();
    if hwnd == 0 {
        bail!("invalid target hwnd 0");
    }
    let target = canonical_accelerator(combo)
        .ok_or_else(|| anyhow::anyhow!("invalid accelerator combo `{combo}`"))?;
    let uia = get_uia().ok_or_else(|| anyhow::anyhow!("UI Automation is unavailable"))?;

    unsafe {
        let root = uia
            .ElementFromHandle(HWND(hwnd as *mut _))
            .with_context(|| format!("ElementFromHandle(0x{hwnd:x}) failed"))?;
        let cond = uia
            .CreateTrueCondition()
            .context("CreateTrueCondition failed")?;
        let arr = root
            .FindAll(TreeScope_Subtree, &cond)
            .with_context(|| format!("FindAll(TreeScope_Subtree) on 0x{hwnd:x} failed"))?;
        let count = arr.Length().unwrap_or(0);
        let mut matched_failure: Option<String> = None;

        for i in 0..count {
            let elem = match arr.GetElement(i) {
                Ok(e) => e,
                Err(e) => {
                    tracing::debug!(
                        target: "uia_windows_enum",
                        "GetElement({i}) failed while scanning accelerator {combo}: {e}"
                    );
                    continue;
                }
            };
            // Primary match: the UIA AcceleratorKey property — the conventional
            // place a WinUI / XAML control advertises its shortcut.
            let mut accelerator: Option<String> =
                read_current_bstr(&elem, UIA_AcceleratorKeyPropertyId);
            // Fallback match: many shipping XAML apps (e.g. modern Notepad)
            // don't set AcceleratorKey at all and instead encode the shortcut
            // in the visible element name as a parenthetical hint like
            // "Bold (Ctrl+B)". Scan the Name property for that pattern so
            // toolbar buttons remain reachable via hotkey.
            if accelerator.is_none() {
                if let Some(name) =
                    read_current_bstr(&elem, windows::Win32::UI::Accessibility::UIA_NamePropertyId)
                {
                    if let Some(extracted) = extract_shortcut_from_name(&name) {
                        accelerator = Some(extracted);
                    }
                }
            }
            let accelerator = match accelerator {
                Some(a) => a,
                None => continue,
            };
            let Some(candidate) = canonical_accelerator(&accelerator) else {
                continue;
            };
            if candidate != target {
                continue;
            }

            if cancelled.load(Ordering::Acquire) {
                tracing::debug!(
                    target: "uia_windows_enum",
                    "UIA accelerator invoke cancelled before activation"
                );
                return Ok((false, count as usize));
            }
            // Pattern fallback chain. Different XAML controls expose
            // different "activation" patterns: a Save button uses Invoke, a
            // Bold toggle uses Toggle, a list item uses SelectionItem. Try
            // Invoke first (the conventional shortcut handler), then Toggle
            // (Bold/Italic/etc.). The Notepad toolbar in particular has Bold
            // as a TogglePattern button — calling .Invoke on it returns the
            // misleading "operation completed successfully (0x00000000)"
            // error because Invoke isn't supported on the element.
            match try_invoke_via_patterns(&elem, hwnd, cancelled) {
                Ok(true) => return Ok((true, count as usize)),
                Ok(false) => {
                    matched_failure = Some(format!(
                        "matched AcceleratorKey `{accelerator}`, but the element exposes \
                         neither InvokePattern nor TogglePattern"
                    ));
                }
                Err(e) => {
                    matched_failure = Some(format!(
                        "matched AcceleratorKey `{accelerator}`, but Invoke / Toggle failed: {e}"
                    ));
                }
            }
        }

        if let Some(reason) = matched_failure {
            bail!("{reason}");
        }
        Ok((false, count as usize))
    }
}

fn read_current_bstr(
    element: &IUIAutomationElement,
    property_id: UIA_PROPERTY_ID,
) -> Option<String> {
    unsafe {
        let variant = element.GetCurrentPropertyValue(property_id).ok()?;
        if variant.as_raw().Anonymous.Anonymous.vt == 8 {
            let bstr = BSTR::from_raw(variant.as_raw().Anonymous.Anonymous.Anonymous.bstrVal);
            let s = bstr.to_string();
            std::mem::forget(bstr);
            let trimmed = s.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_owned())
            }
        } else {
            None
        }
    }
}

fn canonical_accelerator(value: &str) -> Option<String> {
    let mut modifiers: Vec<String> = Vec::new();
    let mut keys: Vec<String> = Vec::new();

    for raw in value.split('+') {
        let token = canonical_accelerator_token(raw);
        if token.is_empty() {
            continue;
        }
        if accelerator_modifier_rank(&token).is_some() {
            modifiers.push(token);
        } else {
            keys.push(token);
        }
    }

    if keys.is_empty() {
        return None;
    }

    modifiers.sort_by_key(|m| accelerator_modifier_rank(m).unwrap_or(usize::MAX));
    modifiers.dedup();
    modifiers.extend(keys);
    Some(modifiers.join("+"))
}

fn canonical_accelerator_token(value: &str) -> String {
    if value == " " {
        return "space".to_owned();
    }
    let compact = value.trim().to_ascii_lowercase().replace([' ', '-'], "");
    match compact.as_str() {
        "control" => "ctrl".to_owned(),
        "windows" | "window" | "meta" | "cmd" | "command" => "win".to_owned(),
        "return" => "enter".to_owned(),
        "esc" => "escape".to_owned(),
        "del" => "delete".to_owned(),
        "ins" => "insert".to_owned(),
        "pgup" => "pageup".to_owned(),
        "pgdn" => "pagedown".to_owned(),
        "spacebar" => "space".to_owned(),
        _ => compact,
    }
}

fn accelerator_modifier_rank(value: &str) -> Option<usize> {
    match value {
        "ctrl" => Some(0),
        "shift" => Some(1),
        "alt" => Some(2),
        "win" => Some(3),
        _ => None,
    }
}

/// Try to "activate" a UIA element via the conventional patterns in priority
/// order: InvokePattern (for buttons / menu items that fire an action), then
/// TogglePattern (for Bold / Italic / etc. that flip a binary state).
///
/// `host_hwnd` is the top-level HWND containing the element; it gates the
/// UWP foreground-steal bypass (see `crate::uia::fg_bypass`). Pass `0` if
/// unknown — the bypass becomes a no-op and Invoke/Toggle run unwrapped.
///
/// Returns `Ok(true)` if a pattern was found AND its Invoke/Toggle call
/// succeeded. `Ok(false)` means the element exposes neither pattern (caller
/// should surface an actionable error). `Err` means a pattern was found but
/// its call failed (caller should surface the underlying error).
unsafe fn try_invoke_via_patterns(
    elem: &IUIAutomationElement,
    host_hwnd: isize,
    cancelled: &AtomicBool,
) -> anyhow::Result<bool> {
    // Invoke first — that's what most accelerator-targeted controls advertise.
    if let Ok(pattern) = elem.GetCurrentPattern(UIA_InvokePatternId) {
        if let Ok(inv) = pattern.cast::<IUIAutomationInvokePattern>() {
            if cancelled.load(Ordering::Acquire) {
                return Ok(false);
            }
            return crate::uia::fg_bypass::run_with_uwp_bypass(host_hwnd, || {
                if cancelled.load(Ordering::Acquire) {
                    return Ok(false);
                }
                inv.Invoke()
                    .map(|()| true)
                    .map_err(|e| anyhow::anyhow!("InvokePattern.Invoke: {e}"))
            });
        }
    }
    // Toggle next — Bold/Italic/Underline-style toolbar buttons sit here.
    if let Ok(pattern) = elem.GetCurrentPattern(UIA_TogglePatternId) {
        if let Ok(tog) = pattern.cast::<IUIAutomationTogglePattern>() {
            if cancelled.load(Ordering::Acquire) {
                return Ok(false);
            }
            return crate::uia::fg_bypass::run_with_uwp_bypass(host_hwnd, || {
                if cancelled.load(Ordering::Acquire) {
                    return Ok(false);
                }
                tog.Toggle()
                    .map(|()| true)
                    .map_err(|e| anyhow::anyhow!("TogglePattern.Toggle: {e}"))
            });
        }
    }
    Ok(false)
}

/// Extract a shortcut hint from a UIA element name like `"Bold (Ctrl+B)"`,
/// `"Italic (Ctrl+I)"`, `"Save (Ctrl+S)"` — modern XAML apps (notably modern
/// Notepad) don't set `AcceleratorKey` but encode the shortcut in the visible
/// name. Returns the parenthesized accelerator string if one is present and
/// contains a modifier-like token; otherwise `None`.
fn extract_shortcut_from_name(name: &str) -> Option<String> {
    let open = name.rfind('(')?;
    let close = name[open..].find(')')?;
    let inner = name[open + 1..open + close].trim();
    if inner.is_empty() {
        return None;
    }
    // Require at least one modifier-like token to avoid matching arbitrary
    // parentheticals (e.g. "(2)" or "(beta)").
    let has_modifier = inner.split('+').any(|tok| {
        let t = tok.trim().to_ascii_lowercase();
        matches!(
            t.as_str(),
            "ctrl" | "control" | "shift" | "alt" | "win" | "windows" | "meta" | "cmd" | "command"
        )
    });
    if has_modifier {
        Some(inner.to_owned())
    } else {
        None
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

    // UIA is authoritative about whether the provider considers the element
    // off-screen. An unknown value is tolerated for normal UIA geometry, but
    // the Win32 fallback below requires an explicit `false`.
    let is_offscreen = elem.CurrentIsOffscreen().ok().map(|flag| flag.as_bool());
    if let Some(true) = is_offscreen {
        return None;
    }

    let prefer_win32_bounds = !elem
        .CurrentBoundingRectangle()
        .ok()
        .is_some_and(rect_is_valid);

    if !IsWindow(hwnd).as_bool() {
        return None;
    }

    // A UIA desktop child must still resolve to the same top-level HWND.
    // Reject child/inherited handles before reading any identity or geometry.
    if GetAncestor(hwnd, GA_ROOT) != hwnd {
        return None;
    }

    // Resolve pid via the standard Win32 path. We deliberately do not trust
    // the UIA ProcessId property here because the rest of the driver indexes
    // windows by (hwnd, pid) tuples obtained from `GetWindowThreadProcessId`,
    // and we want bit-identical agreement.
    let mut pid: u32 = 0;
    let thread_id = GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if thread_id == 0 || pid == 0 {
        return None;
    }

    if prefer_win32_bounds {
        // The element came from this process's UIA desktop root, which already
        // constrains it to the caller's desktop and session. For the exceptional
        // Win32 path, also require an explicitly on-screen, capturable window.
        if is_offscreen != Some(false)
            || !IsWindowVisible(hwnd).as_bool()
            || IsIconic(hwnd).as_bool()
            || window_is_cloaked(hwnd)
        {
            return None;
        }
        let uia_pid = elem.CurrentProcessId().ok()?;
        if uia_pid <= 0 || uia_pid as u32 != pid {
            return None;
        }
    }

    let (x, y, width, height) = window_bounds(hwnd, prefer_win32_bounds)?;

    if prefer_win32_bounds {
        // Re-read both sides of the identity after geometry lookup so a stale
        // or reused HWND cannot be accepted as the original UIA window.
        let mut pid_after = 0u32;
        let thread_after = GetWindowThreadProcessId(hwnd, Some(&mut pid_after));
        if !IsWindow(hwnd).as_bool()
            || elem.CurrentNativeWindowHandle().ok() != Some(hwnd)
            || elem
                .CurrentProcessId()
                .ok()
                .is_none_or(|candidate| candidate <= 0 || candidate as u32 != pid)
            || thread_after != thread_id
            || pid_after != pid
        {
            return None;
        }
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

    Some(WindowInfo {
        hwnd: hwnd.0 as u64,
        pid,
        title,
        x,
        y,
        width,
        height,
        is_on_screen: true,
        minimized: false,
    })
}

fn rect_is_valid(rect: RECT) -> bool {
    rect.right > rect.left && rect.bottom > rect.top
}

fn window_is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked = 0u32;
    unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked as *mut u32 as *mut _,
            std::mem::size_of::<u32>() as u32,
        )
        .is_err()
            || cloaked != 0
    }
}

fn select_window_rect(
    prefer_win32: bool,
    dwm_rect: Option<RECT>,
    win32_rect: Option<RECT>,
) -> Option<RECT> {
    if prefer_win32 {
        win32_rect.filter(|rect| rect_is_valid(*rect))
    } else {
        dwm_rect
            .filter(|rect| rect_is_valid(*rect))
            .or_else(|| win32_rect.filter(|rect| rect_is_valid(*rect)))
    }
}

/// Bounds via DWM extended frame (excludes drop-shadow on W11) with
/// `GetWindowRect` fallback. Invalid UIA geometry prefers the latter directly.
/// The driver is Per-Monitor V2 aware, so both sources use physical pixels.
fn window_bounds(hwnd: HWND, prefer_win32: bool) -> Option<(i32, i32, i32, i32)> {
    unsafe {
        let mut dwm_rect = RECT::default();
        let dwm_rect = DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut dwm_rect as *mut RECT as *mut _,
            std::mem::size_of::<RECT>() as u32,
        )
        .ok()
        .map(|()| dwm_rect);

        // API success does not guarantee usable geometry. This is the actual
        // source-selection rule: invalid DWM data falls through to Win32 too.
        let need_win32 = prefer_win32 || dwm_rect.is_none_or(|rect| !rect_is_valid(rect));
        let mut win32_rect = RECT::default();
        let win32_rect = need_win32
            .then(|| {
                GetWindowRect(hwnd, &mut win32_rect)
                    .ok()
                    .map(|()| win32_rect)
            })
            .flatten();

        let rect = select_window_rect(prefer_win32, dwm_rect, win32_rect)?;
        Some((
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use windows::Win32::Foundation::RECT;

    fn rect() -> RECT {
        RECT {
            left: 200,
            top: 52,
            right: 1400,
            bottom: 852,
        }
    }

    #[test]
    fn invalid_uia_bounds_prefer_the_reported_win32_rect() {
        let win32 = rect();
        assert_eq!(select_window_rect(true, None, Some(win32)), Some(win32));
    }

    #[test]
    fn normal_windows_keep_valid_dwm_geometry() {
        let dwm = rect();
        let mut win32 = dwm;
        win32.left -= 8;
        assert_eq!(select_window_rect(false, Some(dwm), Some(win32)), Some(dwm));
    }

    #[test]
    fn invalid_dwm_geometry_falls_through_and_invalid_win32_fails_closed() {
        let empty = RECT::default();
        let win32 = rect();
        assert_eq!(
            select_window_rect(false, Some(empty), Some(win32)),
            Some(win32)
        );
        assert!(select_window_rect(true, Some(win32), Some(empty)).is_none());
    }

    #[test]
    fn wedged_provider_has_bounded_worker_growth_and_recovers() {
        let gate = Arc::new(UiaSingleFlight::new(40));
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let starts = Arc::new(AtomicUsize::new(0));
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let side_effects = Arc::new(AtomicUsize::new(0));

        let first_start = Instant::now();
        let result = gate.run("test wedge", Duration::from_millis(20), "test", {
            let starts = Arc::clone(&starts);
            let active = Arc::clone(&active);
            let max_active = Arc::clone(&max_active);
            let side_effects = Arc::clone(&side_effects);
            move |cancelled| {
                starts.fetch_add(1, Ordering::AcqRel);
                let current = active.fetch_add(1, Ordering::AcqRel) + 1;
                max_active.fetch_max(current, Ordering::AcqRel);
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                if !cancelled.load(Ordering::Acquire) {
                    side_effects.fetch_add(1, Ordering::AcqRel);
                }
                active.fetch_sub(1, Ordering::AcqRel);
            }
        });
        assert!(matches!(result, Err(UiaDeadlineError::Timeout)));
        assert!(first_start.elapsed() < Duration::from_millis(500));
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        let retries_start = Instant::now();
        for _ in 0..100 {
            let starts = Arc::clone(&starts);
            let retry = gate.run("test retry", Duration::from_secs(1), "test", move |_| {
                starts.fetch_add(1, Ordering::AcqRel);
            });
            assert!(matches!(retry, Err(UiaDeadlineError::Busy)));
        }
        assert!(retries_start.elapsed() < Duration::from_millis(500));
        assert_eq!(starts.load(Ordering::Acquire), 1);
        assert_eq!(max_active.load(Ordering::Acquire), 1);

        release_tx.send(()).unwrap();
        let recovery_deadline = Instant::now() + Duration::from_secs(1);
        while gate.in_flight.load(Ordering::Acquire) && Instant::now() < recovery_deadline {
            thread::yield_now();
        }
        assert!(!gate.in_flight.load(Ordering::Acquire));
        assert_eq!(side_effects.load(Ordering::Acquire), 0);

        let during_cooldown = gate.run("test cooldown", Duration::from_secs(1), "test", |_| 1);
        assert!(matches!(during_cooldown, Err(UiaDeadlineError::Busy)));
        let cooldown_deadline = Instant::now() + Duration::from_secs(1);
        while now_ms() < gate.cooldown_until_ms.load(Ordering::Acquire)
            && Instant::now() < cooldown_deadline
        {
            thread::yield_now();
        }

        let recovered = gate.run("test recovery", Duration::from_secs(1), "test", {
            let starts = Arc::clone(&starts);
            let active = Arc::clone(&active);
            let max_active = Arc::clone(&max_active);
            move |_| {
                starts.fetch_add(1, Ordering::AcqRel);
                let current = active.fetch_add(1, Ordering::AcqRel) + 1;
                max_active.fetch_max(current, Ordering::AcqRel);
                active.fetch_sub(1, Ordering::AcqRel);
                42
            }
        });
        assert!(matches!(recovered, Ok(42)));
        assert_eq!(starts.load(Ordering::Acquire), 2);
        assert_eq!(max_active.load(Ordering::Acquire), 1);
    }
}
