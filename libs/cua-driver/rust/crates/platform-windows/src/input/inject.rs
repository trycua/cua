//! Universal background mouse actuator: coordinate-routed pointer/touch
//! injection that delivers a click to whatever window sits under a screen
//! point **without** `SetForegroundWindow` and **without** moving the user's
//! mouse cursor.
//!
//! Why this exists: the PostMessage path (`mouse::post_click`) is invisible and
//! never raises, but Chromium/Electron/GTK/WPF content silently ignore posted
//! synthetic `WM_*BUTTON` messages — their input arrives through the *system
//! input queue*, not the per-window message queue. The historical fallback for
//! those was `send_click_synthesized` (SendInput + a `SetForegroundWindow`
//! swap), which is exactly the visible "flash" we want to eliminate.
//!
//! Touch injection routes by coordinate through the system input queue (so
//! Chromium et al. accept it; the OS promotes it to `WM_*BUTTON` for legacy
//! Win32 windows that don't consume `WM_POINTER`), and — per the RE in
//! `docs/windows-background-input-re-plan.md` §4.4 — the kernel injection path
//! (`NtUserInjectMouseInput`/`NtUserInjectTouchInput`) gates only on a
//! per-process injection-enable, NOT on the target being foreground. The one
//! residual is that a tap on an *inactive* top-level window can still trigger
//! click-activation; we contain that with `NoActivateGuard` (WS_EX_NOACTIVATE)
//! plus a `force_foreground_attached(prev_fg)` re-assertion of the USER's
//! foreground afterward. Per the macOS-aligned contract a background actuation
//! never raises/restacks the target: when coordinate-routed input can't reach
//! it (occluded at the point), the caller returns `background_unavailable`
//! rather than raising it (see `target_visible_at_point`).
//!
//! Scope: left-button taps (single/double/triple). Right/middle have no clean
//! touch mapping; callers fall back to their existing routing for those.

use anyhow::{bail, Result};
use core::ffi::c_void;
use std::sync::Mutex;
use std::thread::sleep;
use std::time::Duration;

use windows::Win32::Foundation::{HANDLE, HWND, POINT, RECT};
use windows::Win32::UI::Controls::{
    CreateSyntheticPointerDevice, DestroySyntheticPointerDevice, HSYNTHETICPOINTERDEVICE,
    POINTER_FEEDBACK_DEFAULT, POINTER_TYPE_INFO, POINTER_TYPE_INFO_0,
};
use windows::Win32::UI::Input::Pointer::{
    InjectSyntheticPointerInput, POINTER_FLAG_DOWN, POINTER_FLAG_INCONTACT, POINTER_FLAG_INRANGE,
    POINTER_FLAG_UP, POINTER_FLAG_UPDATE, POINTER_INFO, POINTER_PEN_INFO, POINTER_TOUCH_INFO,
};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetCursorPos, GetForegroundWindow, GetWindowLongPtrW, GetWindowThreadProcessId,
    IsWindow, SetCursorPos, SetForegroundWindow, SetWindowLongPtrW, WindowFromPoint, GA_ROOT,
    GWL_EXSTYLE, PT_PEN, PT_TOUCH, WS_EX_NOACTIVATE,
};

/// Bring `target` to the foreground using the AttachThreadInput trick, which
/// inherits the current foreground thread's FG-lock token so the swap is
/// honored even on a foreground-locked session without UIAccess (mirrors the
/// `bring_to_front` tool). Single attach, no retry loop — bounded. Returns
/// whether `target` actually became foreground.
pub(crate) unsafe fn force_foreground_attached(target: HWND) -> bool {
    let cur = GetForegroundWindow();
    if cur == target {
        return true;
    }
    let my_tid = GetCurrentThreadId();
    let mut pid = 0u32;
    let cur_tid = GetWindowThreadProcessId(cur, Some(&mut pid));
    let attached = cur_tid != 0 && cur_tid != my_tid;
    if attached {
        let _ = AttachThreadInput(my_tid, cur_tid, true);
    }
    let _ = SetForegroundWindow(target);
    if attached {
        let _ = AttachThreadInput(my_tid, cur_tid, false);
    }
    GetForegroundWindow() == target
}

/// RAII guard that makes a specific target window **unable to become the
/// foreground/active window** for the duration of an actuation, by adding the
/// `WS_EX_NOACTIVATE` extended style to its top-level window.
///
/// Why this and not a global foreground-lock: our own injected/posted input
/// (or a UIA-Invoke) legitimizes the target's foreground claim, so even a
/// maxed `SPI_*FOREGROUNDLOCKTIMEOUT` won't stop the steal. `WS_EX_NOACTIVATE`
/// is categorical — Windows refuses to activate the window *at all* (clicks,
/// `SetForegroundWindow(self)` from WPF/XAML/Tauri handlers, mouse-activate) —
/// while the window still RECEIVES the click/key. It is per-window (no session
/// side effects) and reversed on drop. Covers the self-activation that the
/// EnableWindow/UWP bypass cannot (WPF `UIElement.Focus()`→SetForegroundWindow).
pub struct NoActivateGuard {
    // Store the handle as an integer so the guard is `Send` and can be held
    // across `.await` in the async tools.
    root_addr: isize,
    prev_exstyle: isize,
    applied: bool,
}

impl NoActivateGuard {
    /// Arm on the top-level (GA_ROOT) ancestor of `hwnd`.
    pub fn arm(hwnd: HWND) -> Self {
        unsafe {
            let root = {
                let r = GetAncestor(hwnd, GA_ROOT);
                if r.0.is_null() { hwnd } else { r }
            };
            let prev = GetWindowLongPtrW(root, GWL_EXSTYLE);
            let want = WS_EX_NOACTIVATE.0 as isize;
            // Apply WS_EX_NOACTIVATE if not already set (prev can be 0, so don't
            // gate on it — we just need to check the bit and set it if absent).
            let applied = (prev & want) == 0 && {
                SetWindowLongPtrW(root, GWL_EXSTYLE, prev | want);
                // Confirm it took (cross-process SetWindowLongPtr can be denied
                // by UIPI on higher-integrity targets).
                (GetWindowLongPtrW(root, GWL_EXSTYLE) & want) != 0
            };
            Self { root_addr: root.0 as isize, prev_exstyle: prev, applied }
        }
    }
}

impl Drop for NoActivateGuard {
    fn drop(&mut self) {
        if self.applied {
            unsafe {
                let _ = SetWindowLongPtrW(HWND(self.root_addr as *mut _), GWL_EXSTYLE, self.prev_exstyle);
            }
        }
    }
}

/// PEN_FLAG_BARREL (winuser.h) — pen barrel button held == secondary (right)
/// button. `penFlags` is a raw u32 in the bindings, so use the literal.
const PEN_FLAG_BARREL: u32 = 0x00000001;

// (Removed ZorderGuard.) The macOS-aligned contract forbids a `background`
// actuation from raising/restacking the target: coordinate-routed pen/touch
// only lands on the topmost VISIBLE window, so an occluded target is reported
// as `background_unavailable` (see `target_visible_at_point`) and the agent
// escalates to `delivery_mode:"foreground"` instead of the driver raising it.

/// One down→up **pen** tap at screen `(sx, sy)`. When `barrel` is set the pen's
/// barrel button is held for the contact, which the system maps to a secondary
/// (right) click — both for `WM_POINTER`-aware apps (Chromium/WPF/UWP) and via
/// pen→mouse promotion for legacy Win32. A fresh synthetic pen device is
/// created and destroyed per tap (right/middle clicks are rare).
fn pen_taps(sx: i32, sy: i32, barrel: bool, count: usize) -> Result<()> {
    unsafe {
        let dev = CreateSyntheticPointerDevice(PT_PEN, 1, POINTER_FEEDBACK_DEFAULT)
            .map_err(|e| anyhow::anyhow!("CreateSyntheticPointerDevice(PEN): {e}"))?;
        let pen_flags = if barrel { PEN_FLAG_BARREL } else { 0 };
        let mk = |flags| POINTER_TYPE_INFO {
            r#type: PT_PEN,
            Anonymous: POINTER_TYPE_INFO_0 {
                penInfo: POINTER_PEN_INFO {
                    pointerInfo: POINTER_INFO {
                        pointerType: PT_PEN,
                        pointerId: 0,
                        pointerFlags: flags,
                        sourceDevice: HANDLE::default(),
                        hwndTarget: HWND::default(),
                        ptPixelLocation: POINT { x: sx, y: sy },
                        ..Default::default()
                    },
                    penFlags: pen_flags,
                    penMask: 0,
                    pressure: 512,
                    rotation: 0,
                    tiltX: 0,
                    tiltY: 0,
                },
            },
        };
        let down = mk(POINTER_FLAG_DOWN | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT);
        let up = mk(POINTER_FLAG_UP);
        // Reuse the SAME synthetic device for every tap. A double/triple click
        // is two/three down-up cycles from one digitizer; creating a fresh
        // device per tap (the old loop) fails the next
        // CreateSyntheticPointerDevice in quick succession — which is exactly
        // why background double_click on Chromium errored. See #1984.
        let mut result: Result<()> = Ok(());
        let n = count.max(1);
        for i in 0..n {
            let r1 = InjectSyntheticPointerInput(dev, &[down]);
            sleep(Duration::from_millis(25));
            let r2 = InjectSyntheticPointerInput(dev, &[up]);
            if let Err(e) = r1.and(r2) {
                result = Err(anyhow::anyhow!("InjectSyntheticPointerInput(pen): {e}"));
                break;
            }
            if i + 1 < n {
                sleep(Duration::from_millis(70));
            }
        }
        let _ = DestroySyntheticPointerDevice(dev);
        result?;
    }
    Ok(())
}

/// Inject a click at **screen** coordinates `(sx, sy)`, routed by the system to
/// whatever window is under that point — without a foreground swap and without
/// moving the user's cursor. The target is cloaked for the duration so any
/// click-activation raise stays invisible, then the user's foreground is
/// restored.
///
/// - `left`  → synthetic-pen primary tap (proven path; promoted to a left click
///   for non-pointer-aware apps, and accepted directly by Chromium/WPF/UWP).
/// - `right` → synthetic-pen tap with the barrel button held (secondary click).
/// - `middle`→ unsupported (no clean pointer mapping); returns `Err` so the
///   caller can fall back to its existing routing / structured error.
///
/// We use the same `CreateSyntheticPointerDevice`/`InjectSyntheticPointerInput`
/// path for both buttons — `InjectTouchInput` proved unreliable for left-clicks
/// on Chromium content (returned errors), whereas synthetic-pen injection lands
/// reliably and routes by coordinate with no foreground dependency.
pub fn inject_click_screen(target: u64, sx: i32, sy: i32, count: usize, button: &str) -> Result<()> {
    if target == 0 {
        bail!("inject_click_screen: null target window");
    }
    let target_h = HWND(target as *mut _);
    unsafe {
        if !IsWindow(target_h).as_bool() {
            bail!("inject_click_screen: invalid or stale target HWND");
        }
    }
    if let Some(msg) = crate::input::post_message_blocked_by_uipi(target) {
        // Higher-integrity target: injection into its queue is blocked too.
        bail!(msg);
    }

    let barrel = match button {
        "left" => false,
        "right" => true,
        other => bail!("background injection supports left/right buttons only (got {other:?})"),
    };

    // macOS-aligned contract: a `background` actuation NEVER fronts or raises
    // (mirrors macOS CGEvent-to-pid, which never touches z-order/focus). Synthetic
    // pen/touch is coordinate-routed to the TOP-MOST VISIBLE window at the point,
    // so if the target is occluded there we'd silently click the occluder. We do
    // NOT raise to win the hit-test (that's the foreground rung's job, which the
    // agent opts into). Instead bail — the caller surfaces `background_unavailable`
    // and the agent escalates to `delivery_mode:"foreground"`.
    if unsafe { !target_visible_at_point(target_h, sx, sy) } {
        bail!(
            "background coordinate injection cannot reach this target at ({sx},{sy}) \
             — it is occluded by another window (raising it would break the \
             no-foreground contract). Escalate to delivery_mode:\"foreground\"."
        );
    }
    // Remember who the user had in front so we can re-assert it below.
    let prev_fg = unsafe { GetForegroundWindow() };
    // Make the target non-activatable for the click so click-activation can't
    // steal foreground. No z-order raise.
    let result = {
        let _noact = NoActivateGuard::arm(target_h);
        // One synthetic device does all `count` taps (single/double/triple click).
        pen_taps(sx, sy, barrel, count)
    };
    // `WS_EX_NOACTIVATE` is NOT categorical against a Chromium/Electron content
    // window that calls `SetForegroundWindow(self)` from its (async) click
    // handler. Re-assert the USER's foreground (this restores focus where the
    // user left it — it never raises the target). Short settle + repeat to win
    // the race against the async self-activation.
    unsafe {
        if result.is_ok() && !prev_fg.0.is_null() && prev_fg != target_h {
            force_foreground_attached(prev_fg);
            sleep(Duration::from_millis(12));
            force_foreground_attached(prev_fg);
        }
    }
    result
}

/// True when `target`'s top-level root is the window actually visible at screen
/// point `(sx, sy)` — i.e. coordinate-routed pen/touch there would land on it
/// rather than an occluder. Used to enforce the macOS-aligned rule that a
/// `background` actuation never raises: when this is false the injection bails
/// and the tool returns `background_unavailable` (escalate to foreground).
unsafe fn target_visible_at_point(target: HWND, sx: i32, sy: i32) -> bool {
    let top = WindowFromPoint(POINT { x: sx, y: sy });
    if top.0.is_null() {
        return false;
    }
    let top_root = GetAncestor(top, GA_ROOT);
    let target_root = GetAncestor(target, GA_ROOT);
    !top_root.0.is_null() && top_root == target_root
}

/// True when screen point `(x, y)` lies within `hwnd`'s window rectangle.
///
/// Guards **element_index** clicks: an element's cached center can fall outside
/// its own window when the element is scrolled out of a ScrollViewer or pushed
/// off-screen (e.g. a tall form on a small display). Tapping the raw coordinate
/// then lands on whatever is actually there — the taskbar, the desktop, another
/// window — instead of the intended element. The click tool turns a `false`
/// here into a clear error rather than clicking the wrong target. Returns `true`
/// (fail-open) when the rect can't be read, so legitimate clicks are never
/// blocked by a transient query failure.
pub fn point_in_window_bounds(hwnd: u64, x: i32, y: i32) -> bool {
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;
    if hwnd == 0 {
        return true;
    }
    let mut r = RECT::default();
    let ok = unsafe { GetWindowRect(HWND(hwnd as *mut _), &mut r).is_ok() };
    if !ok {
        return true;
    }
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

/// One pen press-drag-release from screen `(sx0,sy0)` to `(sx1,sy1)`, with
/// `steps` interpolated in-contact UPDATE points between the down and the up.
/// A single synthetic pen device is created for the whole stroke. The barrel
/// button is held when `barrel` is set (secondary-button drag).
fn pen_drag(sx0: i32, sy0: i32, sx1: i32, sy1: i32, steps: usize, barrel: bool) -> Result<()> {
    unsafe {
        let dev = CreateSyntheticPointerDevice(PT_PEN, 1, POINTER_FEEDBACK_DEFAULT)
            .map_err(|e| anyhow::anyhow!("CreateSyntheticPointerDevice(PEN): {e}"))?;
        let pen_flags = if barrel { PEN_FLAG_BARREL } else { 0 };
        let mk = |flags, x: i32, y: i32| POINTER_TYPE_INFO {
            r#type: PT_PEN,
            Anonymous: POINTER_TYPE_INFO_0 {
                penInfo: POINTER_PEN_INFO {
                    pointerInfo: POINTER_INFO {
                        pointerType: PT_PEN,
                        pointerId: 0,
                        pointerFlags: flags,
                        sourceDevice: HANDLE::default(),
                        hwndTarget: HWND::default(),
                        ptPixelLocation: POINT { x, y },
                        ..Default::default()
                    },
                    penFlags: pen_flags,
                    penMask: 0,
                    pressure: 512,
                    rotation: 0,
                    tiltX: 0,
                    tiltY: 0,
                },
            },
        };
        // Press at the start.
        let down = mk(POINTER_FLAG_DOWN | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT, sx0, sy0);
        let mut res = InjectSyntheticPointerInput(dev, &[down]);
        // Interpolated in-contact moves so frameworks that gate drag-tracking on
        // motion (rather than a single down→up) see a continuous stroke.
        let steps = steps.max(1);
        for i in 1..=steps {
            sleep(Duration::from_millis(8));
            let t = i as f64 / steps as f64;
            let x = sx0 + ((sx1 - sx0) as f64 * t).round() as i32;
            let y = sy0 + ((sy1 - sy0) as f64 * t).round() as i32;
            let mv = mk(POINTER_FLAG_UPDATE | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT, x, y);
            res = res.and(InjectSyntheticPointerInput(dev, &[mv]));
        }
        // Release at the end.
        sleep(Duration::from_millis(8));
        let up = mk(POINTER_FLAG_UP, sx1, sy1);
        res = res.and(InjectSyntheticPointerInput(dev, &[up]));
        let _ = DestroySyntheticPointerDevice(dev);
        res.map_err(|e| anyhow::anyhow!("InjectSyntheticPointerInput(pen drag): {e}"))?;
    }
    Ok(())
}

/// A **persistent** synthetic touch digitizer, created once and never
/// destroyed. This is load-bearing: a transient (per-stroke) device is gone
/// before WPF's WISP stylus stack can bind to it, so the OS falls back to
/// legacy touch→mouse promotion — which drags the user's cursor to the
/// contact. A *standing* digitizer is enumerated as a real tablet, so WPF (and
/// other stylus/pointer-aware frameworks) consume the contact as touch/stylus
/// and promote it to mouse INTERNALLY, without the OS moving the system cursor.
static TOUCH_DEV: Mutex<isize> = Mutex::new(0);

/// One **touch** press-drag-release from screen `(sx0,sy0)` to `(sx1,sy1)` with
/// `steps` interpolated in-contact moves, via the persistent [`TOUCH_DEV`].
/// Unlike a pen (an absolute *cursor* device — injecting one drags the user's
/// mouse pointer along), a touch contact from a standing digitizer is consumed
/// as touch/stylus and does NOT move the user's cursor. Serialized on the
/// single shared device (one stroke at a time across all sessions).
fn touch_drag(sx0: i32, sy0: i32, sx1: i32, sy1: i32, steps: usize) -> Result<()> {
    let mut dev_guard = TOUCH_DEV.lock().unwrap_or_else(|e| e.into_inner());
    unsafe {
        // A non-pointer-aware window (WPF) makes the OS promote the PRIMARY touch
        // contact to a mouse event, which drags the system cursor to the contact
        // — and the OS gates delivery on the cursor actually reaching it, so the
        // move can't be prevented from a background process (pinning/clipping the
        // cursor just drops the input). What we CAN do is snap the cursor back to
        // exactly where the user left it the instant the stroke ends, so the net
        // displacement is zero and (with a fast, few-step stroke) the excursion
        // is a brief flick rather than a sustained drag. Pointer-aware targets
        // (Chromium) never promote, so the cursor never moves and this restore is
        // a harmless no-op.
        let mut cpos = POINT::default();
        let have_cpos = GetCursorPos(&mut cpos).is_ok();
        let dev = if *dev_guard != 0 {
            HSYNTHETICPOINTERDEVICE(*dev_guard as *mut c_void)
        } else {
            let d = CreateSyntheticPointerDevice(PT_TOUCH, 1, POINTER_FEEDBACK_DEFAULT)
                .map_err(|e| anyhow::anyhow!("CreateSyntheticPointerDevice(TOUCH): {e}"))?;
            *dev_guard = d.0 as isize;
            d
        };
        let mk = |flags, x: i32, y: i32| POINTER_TYPE_INFO {
            r#type: PT_TOUCH,
            Anonymous: POINTER_TYPE_INFO_0 {
                touchInfo: POINTER_TOUCH_INFO {
                    pointerInfo: POINTER_INFO {
                        pointerType: PT_TOUCH,
                        pointerId: 0,
                        pointerFlags: flags,
                        sourceDevice: HANDLE::default(),
                        hwndTarget: HWND::default(),
                        ptPixelLocation: POINT { x, y },
                        ..Default::default()
                    },
                    touchFlags: 0,
                    touchMask: 0,
                    rcContact: RECT { left: x - 2, top: y - 2, right: x + 2, bottom: y + 2 },
                    rcContactRaw: RECT { left: x - 2, top: y - 2, right: x + 2, bottom: y + 2 },
                    orientation: 0,
                    pressure: 512,
                },
            },
        };
        // Fast stroke: line-tool canvases only need down→up (the segment is the
        // straight line between them), so a few in-contact frames with a tiny
        // dwell is plenty — and the shorter the stroke, the briefer the cursor
        // excursion before we snap it back.
        let down = mk(POINTER_FLAG_DOWN | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT, sx0, sy0);
        let mut res = InjectSyntheticPointerInput(dev, &[down]);
        let steps = steps.clamp(1, 3);
        for i in 1..=steps {
            sleep(Duration::from_millis(2));
            let t = i as f64 / steps as f64;
            let x = sx0 + ((sx1 - sx0) as f64 * t).round() as i32;
            let y = sy0 + ((sy1 - sy0) as f64 * t).round() as i32;
            let mv = mk(POINTER_FLAG_UPDATE | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT, x, y);
            res = res.and(InjectSyntheticPointerInput(dev, &[mv]));
        }
        sleep(Duration::from_millis(2));
        let up = mk(POINTER_FLAG_UP, sx1, sy1);
        res = res.and(InjectSyntheticPointerInput(dev, &[up]));
        // Snap the cursor back to where the user left it. The OS processes the
        // promoted mouse messages slightly after injection, so a single restore
        // right after the `up` can be overrun by that late move — settle briefly,
        // then restore, and restore once more to win the race. No-op for
        // pointer-aware targets (Chromium) that never moved the cursor.
        if have_cpos {
            let _ = SetCursorPos(cpos.x, cpos.y);
            sleep(Duration::from_millis(12));
            let _ = SetCursorPos(cpos.x, cpos.y);
        }
        // device intentionally NOT destroyed — see TOUCH_DEV.
        res.map_err(|e| anyhow::anyhow!("InjectSyntheticPointerInput(touch drag): {e}"))?;
    }
    Ok(())
}

/// Inject a press-drag-release at **screen** coordinates, routed by the system
/// to whatever window is under the path — without a foreground swap and without
/// moving the user's cursor. This is the background fallback for canvases whose
/// content (Chromium/WPF/GTK) silently drops a PostMessage drag: synthetic-pen
/// input arrives through the system input queue and is accepted directly
/// (Chromium/WPF) or promoted to mouse for legacy Win32. `left` → primary
/// stroke, `right` → barrel-held secondary stroke; the target is held
/// non-activatable + cloaked so any transient raise stays invisible.
pub fn inject_drag_screen(
    target: u64,
    sx0: i32,
    sy0: i32,
    sx1: i32,
    sy1: i32,
    steps: usize,
    button: &str,
) -> Result<()> {
    if target == 0 {
        bail!("inject_drag_screen: null target window");
    }
    let target_h = HWND(target as *mut _);
    unsafe {
        if !IsWindow(target_h).as_bool() {
            bail!("inject_drag_screen: invalid or stale target HWND");
        }
    }
    if let Some(msg) = crate::input::post_message_blocked_by_uipi(target) {
        bail!(msg);
    }
    let barrel = match button {
        "left" => false,
        "right" => true,
        other => bail!("background injection supports left/right buttons only (got {other:?})"),
    };
    // macOS-aligned contract: a `background` drag NEVER fronts/raises. WPF's
    // Wisp stylus stack only PROCESSES injected input while the window is the
    // ACTIVE foreground (verified by RE) — and background is not allowed to
    // force that — so a WPF drag is simply undeliverable in background. Bail so
    // the tool returns background_unavailable (escalate to foreground).
    if crate::input::delivery::is_wpf_target_window(target) {
        bail!(
            "background drag is not deliverable to a WPF target — its Wisp stylus \
             stack only processes injected input while the window is foreground, \
             which background must not force. Escalate to delivery_mode:\"foreground\"."
        );
    }
    // Coordinate-routed touch/pen lands on the topmost VISIBLE window at the
    // start point. If the target is occluded there, bail rather than raise it.
    if unsafe { !target_visible_at_point(target_h, sx0, sy0) } {
        bail!(
            "background drag cannot reach this target at the start point ({sx0},{sy0}) \
             — it is occluded by another window. Escalate to delivery_mode:\"foreground\"."
        );
    }
    let prev_fg = unsafe { GetForegroundWindow() };
    // Left drag → touch contact (coordinate-routed). Right/barrel drag has no
    // touch equivalent, so fall back to a pen (rare).
    let stroke = |()| if barrel { pen_drag(sx0, sy0, sx1, sy1, steps, true) } else { touch_drag(sx0, sy0, sx1, sy1, steps) };
    // Chromium/GTK: pointer-aware, process injection in the background — hold
    // non-activatable (no raise), inject, then re-assert the user's foreground.
    let r = {
        let _noact = NoActivateGuard::arm(target_h);
        stroke(())
    };
    unsafe {
        if !prev_fg.0.is_null() && prev_fg != target_h {
            force_foreground_attached(prev_fg);
        }
    }
    r
}

// (Removed inject_key_cloaked / inject_text_cloaked.) The macOS-aligned
// contract forbids a `background` actuation from grabbing focus — even a
// *cloaked* (hidden) one. Keyboard/text that the target's input stack would
// drop in the background (VCL/classic-Win32 accelerators, Chromium key-combos,
// WPF/terminal text) is now reported as `background_unavailable`; the agent
// escalates to `delivery_mode:"foreground"`, which uses the explicit
// SetForegroundWindow path (send_key_synthesized / send_text_synthesized).

