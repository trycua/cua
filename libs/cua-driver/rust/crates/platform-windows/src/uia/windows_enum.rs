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

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use windows::core::{Interface, BSTR};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
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
    GetWindowRect, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
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
const DESKTOP_TIMEOUT_FAST_BAIL_AFTER: u32 = 3;
const DESKTOP_TIMEOUT_COOLDOWN: Duration = Duration::from_secs(30);

static DESKTOP_ENUM_TIMEOUTS: TimeoutCooldown = TimeoutCooldown::new(
    DESKTOP_TIMEOUT_FAST_BAIL_AFTER,
    DESKTOP_TIMEOUT_COOLDOWN.as_millis() as u64,
);

enum UiaDeadlineError {
    Timeout,
    Unavailable,
}

struct TimeoutCooldown {
    consecutive: AtomicU32,
    last_timeout_ms: AtomicU64,
    threshold: u32,
    cooldown_ms: u64,
}

impl TimeoutCooldown {
    const fn new(threshold: u32, cooldown_ms: u64) -> Self {
        Self {
            consecutive: AtomicU32::new(0),
            last_timeout_ms: AtomicU64::new(0),
            threshold,
            cooldown_ms,
        }
    }

    fn should_attempt(&self, now_ms: u64) -> bool {
        if self.consecutive.load(Ordering::Acquire) < self.threshold {
            return true;
        }
        let last = self.last_timeout_ms.load(Ordering::Acquire);
        if now_ms.saturating_sub(last) < self.cooldown_ms {
            return false;
        }
        self.last_timeout_ms
            .compare_exchange(last, now_ms, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    fn record_timeout(&self, now_ms: u64) {
        self.last_timeout_ms.store(now_ms, Ordering::Release);
        let previous = self.consecutive.fetch_add(1, Ordering::AcqRel);
        if previous + 1 == self.threshold {
            tracing::warn!(
                target: "uia_windows_enum",
                "UIA desktop enumeration hit {} consecutive timeouts; skipping UIA union for {}ms",
                self.threshold,
                self.cooldown_ms
            );
        }
    }

    fn record_success(&self) {
        let previous = self.consecutive.swap(0, Ordering::AcqRel);
        if previous >= self.threshold {
            tracing::info!(target: "uia_windows_enum", "UIA desktop enumeration recovered");
        }
    }

    #[cfg(test)]
    fn consecutive(&self) -> u32 {
        self.consecutive.load(Ordering::Acquire)
    }
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
    let (tx, rx) = mpsc::channel();
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = Arc::clone(&cancelled);
    let spawn = thread::Builder::new()
        .name(format!("cua-uia-{stage}"))
        .spawn(move || {
            let result = f(worker_cancelled);
            let _ = tx.send(result);
        });

    if let Err(e) = spawn {
        tracing::warn!(target: "uia_windows_enum", "failed to spawn UIA {stage} thread: {e}");
        return Err(UiaDeadlineError::Unavailable);
    }

    match rx.recv_timeout(timeout) {
        Ok(result) => Ok(result),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            cancelled.store(true, Ordering::Release);
            tracing::warn!(
                target: "uia_windows_enum",
                "UIA {stage} exceeded {}ms; falling back to {fallback}",
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
    let now = now_ms();
    if !DESKTOP_ENUM_TIMEOUTS.should_attempt(now) {
        return Vec::new();
    }

    match run_uia_with_deadline(
        "desktop enumeration",
        DESKTOP_CALL_TIMEOUT,
        "Win32-only window list",
        enumerate_top_level_windows_unbounded,
    ) {
        Ok(windows) => {
            DESKTOP_ENUM_TIMEOUTS.record_success();
            windows
        }
        Err(UiaDeadlineError::Timeout) => {
            DESKTOP_ENUM_TIMEOUTS.record_timeout(now_ms());
            Vec::new()
        }
        Err(UiaDeadlineError::Unavailable) => Vec::new(),
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
        // UWP foreground-steal bypass: gate the entire activation block on
        // `is_xaml_host_hwnd(hwnd)`. For non-XAML hosts the closure is a
        // straight passthrough.
        crate::uia::fg_bypass::run_with_uwp_bypass(hwnd, || {
            if winner_has_expand && winner_has_invoke {
                // Try Expand first, fall back to Invoke if Expand fails.
                if let Ok(pat) = winner.GetCurrentPattern(
                    windows::Win32::UI::Accessibility::UIA_ExpandCollapsePatternId,
                ) {
                    if let Ok(ec) = pat
                        .cast::<windows::Win32::UI::Accessibility::IUIAutomationExpandCollapsePattern>()
                    {
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
            match try_invoke_via_patterns(&elem, hwnd) {
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
) -> anyhow::Result<bool> {
    // Invoke first — that's what most accelerator-targeted controls advertise.
    if let Ok(pattern) = elem.GetCurrentPattern(UIA_InvokePatternId) {
        if let Ok(inv) = pattern.cast::<IUIAutomationInvokePattern>() {
            return crate::uia::fg_bypass::run_with_uwp_bypass(host_hwnd, || {
                inv.Invoke()
                    .map(|()| true)
                    .map_err(|e| anyhow::anyhow!("InvokePattern.Invoke: {e}"))
            });
        }
    }
    // Toggle next — Bold/Italic/Underline-style toolbar buttons sit here.
    if let Ok(pattern) = elem.GetCurrentPattern(UIA_TogglePatternId) {
        if let Ok(tog) = pattern.cast::<IUIAutomationTogglePattern>() {
            return crate::uia::fg_bypass::run_with_uwp_bypass(host_hwnd, || {
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
        (
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn uia_deadline_returns_timeout_when_worker_hangs() {
        let start = Instant::now();
        let result = run_uia_with_deadline(
            "test hang",
            Duration::from_millis(25),
            "test fallback",
            || {
                thread::sleep(Duration::from_secs(60));
                1usize
            },
        );

        assert!(matches!(result, Err(UiaDeadlineError::Timeout)));
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn uia_deadline_returns_worker_result() {
        let result =
            run_uia_with_deadline("test ok", Duration::from_secs(1), "test fallback", || {
                42usize
            });
        assert!(matches!(result, Ok(42)));
    }

    #[test]
    fn desktop_timeout_cooldown_skips_then_reprobes_and_resets() {
        let cooldown = TimeoutCooldown::new(3, 100);
        assert!(cooldown.should_attempt(1_000));

        cooldown.record_timeout(1_000);
        cooldown.record_timeout(1_010);
        assert!(cooldown.should_attempt(1_020));

        cooldown.record_timeout(1_020);
        assert_eq!(cooldown.consecutive(), 3);
        assert!(!cooldown.should_attempt(1_050));

        assert!(cooldown.should_attempt(1_121));
        assert!(!cooldown.should_attempt(1_121));
        assert!(!cooldown.should_attempt(1_150));

        cooldown.record_success();
        assert_eq!(cooldown.consecutive(), 0);
        assert!(cooldown.should_attempt(1_122));
    }
}
