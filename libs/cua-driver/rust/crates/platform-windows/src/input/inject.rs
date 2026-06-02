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
//! click-activation; we contain that with [`ZorderGuard`], which DWM-cloaks the
//! (background) target for the duration of the tap so any transient raise is
//! invisible, then restores the user's foreground window and uncloaks.
//!
//! Scope: left-button taps (single/double/triple). Right/middle have no clean
//! touch mapping; callers fall back to their existing routing for those.

use anyhow::{bail, Result};
use core::ffi::c_void;
use std::thread::sleep;
use std::time::Duration;

use windows::Win32::Foundation::{BOOL, FALSE, HANDLE, HWND, POINT, RECT, TRUE};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_CLOAK};
use windows::Win32::UI::Controls::{
    CreateSyntheticPointerDevice, DestroySyntheticPointerDevice, POINTER_FEEDBACK_DEFAULT,
    POINTER_TYPE_INFO, POINTER_TYPE_INFO_0,
};
use windows::Win32::UI::Input::Pointer::{
    InitializeTouchInjection, InjectSyntheticPointerInput, InjectTouchInput, POINTER_FLAG_DOWN,
    POINTER_FLAG_INCONTACT, POINTER_FLAG_INRANGE, POINTER_FLAG_UP, POINTER_INFO, POINTER_PEN_INFO,
    POINTER_TOUCH_INFO, TOUCH_FEEDBACK_DEFAULT,
};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetForegroundWindow, GetWindowLongPtrW, GetWindowThreadProcessId, SetForegroundWindow,
    SetWindowLongPtrW, SetWindowPos, GA_ROOT, GWL_EXSTYLE, HWND_TOP, PT_PEN, PT_TOUCH, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, WS_EX_NOACTIVATE,
};

/// Bring `target` to the foreground using the AttachThreadInput trick, which
/// inherits the current foreground thread's FG-lock token so the swap is
/// honored even on a foreground-locked session without UIAccess (mirrors the
/// `bring_to_front` tool). Single attach, no retry loop — bounded. Returns
/// whether `target` actually became foreground.
unsafe fn force_foreground_attached(target: HWND) -> bool {
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
            let applied = prev != 0 && (prev & want) == 0 && {
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

/// One-time per-process `InitializeTouchInjection`. Subsequent calls would
/// fail with ERROR_ALREADY_INITIALIZED, so gate behind `Once`.
static TOUCH_INIT: std::sync::Once = std::sync::Once::new();

fn ensure_touch_init() {
    TOUCH_INIT.call_once(|| unsafe {
        // maxCount=1: a single contact is all a click needs.
        let _ = InitializeTouchInjection(1, TOUCH_FEEDBACK_DEFAULT);
    });
}

const CLOAK_SIZE: u32 = std::mem::size_of::<BOOL>() as u32;

/// Restore the user's window to the top of the visible z-order WITHOUT
/// activating it. `SWP_NOACTIVATE` sends no `WM_ACTIVATE`/`WM_MOUSEACTIVATE`
/// to either window, so this can never block on a busy target's activation
/// handler (the deadlock that `AttachThreadInput` + `SetForegroundWindow`
/// risks against a webview that's mid-click). It is also not gated by the
/// foreground-lock. Best-effort, instant, hang-free.
unsafe fn restore_z_top(user_win: HWND) {
    let _ = SetWindowPos(
        user_win,
        HWND_TOP,
        0,
        0,
        0,
        0,
        SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
    );
}

unsafe fn set_cloak(h: HWND, on: bool) -> bool {
    let v: BOOL = if on { TRUE } else { FALSE };
    DwmSetWindowAttribute(h, DWMWA_CLOAK, &v as *const _ as *const c_void, CLOAK_SIZE).is_ok()
}

/// RAII guard that hides a background target's transient z-order raise.
///
/// On `arm`: snapshots the user's current foreground window and, if the target
/// isn't already foreground, DWM-cloaks the target (composited to nothing, but
/// still receives input). On `Drop`: re-foregrounds the user's prior window
/// (which pushes the activated target back down to its background z position)
/// and uncloaks the target. Net effect: the user never sees the target rise.
struct ZorderGuard {
    prev_fg: HWND,
    target: HWND,
    cloaked: bool,
}

impl ZorderGuard {
    unsafe fn arm(target: HWND) -> Self {
        let prev_fg = GetForegroundWindow();
        // Only cloak a genuine *background* target. Cloaking the window the
        // user is actively looking at would blink its content.
        let cloaked =
            !target.0.is_null() && target != prev_fg && set_cloak(target, true);
        Self { prev_fg, target, cloaked }
    }
}

impl Drop for ZorderGuard {
    fn drop(&mut self) {
        unsafe {
            // Re-stack the user's window on top (hang-free, no activation
            // messages) BEFORE uncloaking, so the target never flashes above it.
            if !self.prev_fg.0.is_null() && self.prev_fg != self.target {
                restore_z_top(self.prev_fg);
            }
            if self.cloaked {
                let _ = set_cloak(self.target, false);
            }
        }
    }
}

fn touch_contact(x: i32, y: i32, flags: windows::Win32::UI::Input::Pointer::POINTER_FLAGS) -> POINTER_TOUCH_INFO {
    POINTER_TOUCH_INFO {
        pointerInfo: POINTER_INFO {
            pointerType: PT_TOUCH,
            pointerId: 0,
            pointerFlags: flags,
            sourceDevice: HANDLE::default(),
            hwndTarget: HWND::default(), // NULL → system hit-tests by coordinate
            ptPixelLocation: POINT { x, y },
            ..Default::default()
        },
        touchFlags: 0,
        touchMask: 0,
        rcContact: RECT { left: x - 2, top: y - 2, right: x + 2, bottom: y + 2 },
        rcContactRaw: RECT { left: x - 2, top: y - 2, right: x + 2, bottom: y + 2 },
        orientation: 0,
        pressure: 512,
    }
}

/// One down→up tap at screen `(sx, sy)`.
fn tap(sx: i32, sy: i32) -> Result<()> {
    unsafe {
        let down = touch_contact(sx, sy, POINTER_FLAG_DOWN | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT);
        InjectTouchInput(&[down]).map_err(|e| anyhow::anyhow!("InjectTouchInput(down): {e}"))?;
        sleep(Duration::from_millis(25));
        let up = touch_contact(sx, sy, POINTER_FLAG_UP);
        InjectTouchInput(&[up]).map_err(|e| anyhow::anyhow!("InjectTouchInput(up): {e}"))?;
    }
    Ok(())
}

/// One down→up **pen** tap at screen `(sx, sy)`. When `barrel` is set the pen's
/// barrel button is held for the contact, which the system maps to a secondary
/// (right) click — both for `WM_POINTER`-aware apps (Chromium/WPF/UWP) and via
/// pen→mouse promotion for legacy Win32. A fresh synthetic pen device is
/// created and destroyed per tap (right/middle clicks are rare).
fn pen_tap(sx: i32, sy: i32, barrel: bool) -> Result<()> {
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
        let r1 = InjectSyntheticPointerInput(dev, &[down]);
        sleep(Duration::from_millis(25));
        let up = mk(POINTER_FLAG_UP);
        let r2 = InjectSyntheticPointerInput(dev, &[up]);
        let _ = DestroySyntheticPointerDevice(dev);
        r1.and(r2).map_err(|e| anyhow::anyhow!("InjectSyntheticPointerInput(pen): {e}"))?;
    }
    Ok(())
}

/// Inject a click at **screen** coordinates `(sx, sy)`, routed by the system to
/// whatever window is under that point — without a foreground swap and without
/// moving the user's cursor. The target is cloaked for the duration so any
/// click-activation raise stays invisible, then the user's foreground is
/// restored.
///
/// - `left`  → touch injection (promoted to mouse for non-touch apps).
/// - `right` → pen injection with the barrel button held (secondary click).
/// - `middle`→ unsupported (no clean pointer mapping); returns `Err` so the
///   caller can fall back to its existing routing / structured error.
pub fn inject_click_screen(target: u64, sx: i32, sy: i32, count: usize, button: &str) -> Result<()> {
    let target_h = HWND(target as *mut _);
    if let Some(msg) = crate::input::post_message_blocked_by_uipi(target) {
        // Higher-integrity target: injection into its queue is blocked too.
        bail!(msg);
    }

    enum Kind { Touch, PenBarrel }
    let kind = match button {
        "left" => Kind::Touch,
        "right" => Kind::PenBarrel,
        other => bail!("background injection supports left/right buttons only (got {other:?})"),
    };
    if matches!(kind, Kind::Touch) {
        ensure_touch_init();
    }

    // Make the target categorically non-activatable for the click (so neither
    // click-activation nor a self-SetForegroundWindow can steal foreground),
    // and hide/restore any residual z-order via the cloak/SWP guard.
    let _noact = NoActivateGuard::arm(target_h);
    let _guard = unsafe { ZorderGuard::arm(target_h) };
    let count = count.max(1);
    for i in 0..count {
        match kind {
            Kind::Touch => tap(sx, sy)?,
            Kind::PenBarrel => pen_tap(sx, sy, true)?,
        }
        if i + 1 < count {
            sleep(Duration::from_millis(70));
        }
    }
    // _guard drops here: restore the user's foreground + uncloak target.
    Ok(())
}

/// Send `key` (+ optional `modifiers`) to a **background** target via the
/// system input queue, with the target cloaked so the brief focus it needs
/// never shows as a visible raise.
///
/// Keyboard input — unlike mouse — has no coordinate routing: synthesized keys
/// go to the *focused* window of the foreground queue, so the target must hold
/// focus to receive a SendInput accelerator (Ctrl+S, Ctrl+A) that frameworks
/// detect via `GetKeyState`/`TranslateAccelerator`.
///
/// Capability-first contract: the keystroke MUST be delivered. We make a
/// best-effort to preserve the background UX (cloak the target so its brief
/// foreground stint is hidden, restore the user's foreground after), but we do
/// NOT abandon the action to keep the UX:
///   1. Cloak the target (DWM) so any raise is invisible.
///   2. Bring it foreground via the AttachThreadInput trick (beats the
///      foreground-lock even without UIAccess) and SendInput the combo, so
///      `GetKeyState` updates and the accelerator actually fires.
///   3. If focus genuinely can't be obtained, fall back to PostMessage so the
///      key still reaches the window (best-effort; may miss GetKeyState-gated
///      accelerators, but never silently drops the action).
///   4. Restore the user's foreground and uncloak.
pub fn inject_key_cloaked(target: u64, key: &str, modifiers: &[&str]) -> Result<()> {
    let target_h = HWND(target as *mut _);
    if target_h.0.is_null() {
        bail!("invalid target hwnd");
    }
    if let Some(msg) = crate::input::post_message_blocked_by_uipi(target) {
        bail!(msg);
    }

    let prev_fg = unsafe { GetForegroundWindow() };
    let cloaked = unsafe { target_h != prev_fg && set_cloak(target_h, true) };
    let got_fg = unsafe { force_foreground_attached(target_h) };

    let result = if got_fg {
        // Target is foreground (cloaked): send_key_synthesized's own
        // SetForegroundWindow is a no-op success; SendInput updates GetKeyState
        // so the accelerator fires.
        crate::input::send_key_synthesized(target, key, modifiers)
    } else {
        // Couldn't focus the target even with the attach trick — deliver
        // best-effort via PostMessage rather than dropping the action.
        crate::input::post_key(target, key, modifiers)
    };

    unsafe {
        if !prev_fg.0.is_null() && prev_fg != target_h {
            force_foreground_attached(prev_fg);
        }
        if cloaked {
            let _ = set_cloak(target_h, false);
        }
    }
    result
}
