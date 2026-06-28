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
use std::sync::{Mutex, MutexGuard, TryLockError};
use std::thread::sleep;
use std::time::{Duration, Instant};

/// Serializes the cloaked-foreground SendInput operations (`inject_key_cloaked`,
/// `inject_text_cloaked`). Concurrent sessions must not interleave foreground
/// swaps + SendInput on the single shared system input queue, or keystrokes get
/// garbled and foreground restores race. Acquired with a hard 1s ceiling so a
/// stuck holder can never deadlock the others — after 1s, callers proceed
/// unserialized (degraded, but never hung).
static FG_SERIAL: Mutex<()> = Mutex::new(());

fn fg_serialize() -> Option<MutexGuard<'static, ()>> {
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        match FG_SERIAL.try_lock() {
            Ok(g) => return Some(g),
            Err(TryLockError::Poisoned(p)) => return Some(p.into_inner()),
            Err(TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    return None; // auto-expire: proceed without the lock
                }
                sleep(Duration::from_millis(20));
            }
        }
    }
}

use windows::Win32::Foundation::{BOOL, FALSE, HANDLE, HWND, POINT, RECT, TRUE};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_CLOAK};
use windows::Win32::UI::Controls::{
    CreateSyntheticPointerDevice, DestroySyntheticPointerDevice, HSYNTHETICPOINTERDEVICE,
    POINTER_FEEDBACK_DEFAULT, POINTER_TYPE_INFO, POINTER_TYPE_INFO_0,
};
use windows::Win32::UI::Input::Pointer::{
    InjectSyntheticPointerInput, POINTER_FLAG_DOWN, POINTER_FLAG_INCONTACT, POINTER_FLAG_INRANGE,
    POINTER_FLAG_UP, POINTER_FLAG_UPDATE, POINTER_INFO, POINTER_PEN_INFO, POINTER_TOUCH_INFO,
};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
    VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetCursorPos, GetForegroundWindow, GetWindowLongPtrW, GetWindowThreadProcessId,
    IsWindow, SetCursorPos, SetForegroundWindow, SetWindowLongPtrW, SetWindowPos,
    SystemParametersInfoW, GA_ROOT, GWL_EXSTYLE, HWND_NOTOPMOST, HWND_TOP, HWND_TOPMOST, PT_PEN,
    PT_TOUCH, SPI_GETFOREGROUNDLOCKTIMEOUT, SPI_SETFOREGROUNDLOCKTIMEOUT, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WS_EX_NOACTIVATE,
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

/// RAII guard that momentarily drops the system foreground-lock timeout so a
/// non-UIAccess process can `SetForegroundWindow`, then restores the user's
/// original value on drop. The change is **in-memory only** — `fWinIni` is 0,
/// so it is NOT written to the user profile (no `SPIF_UPDATEINIFILE`) and never
/// persists past this guard. Required on machines whose foreground-lock is
/// maxed (`SPI_GETFOREGROUNDLOCKTIMEOUT` large), which otherwise denies the
/// raise an occluded WPF window needs.
struct ForegroundLockGuard {
    prev: u32,
    active: bool,
}

impl ForegroundLockGuard {
    unsafe fn disable() -> Self {
        let flags = SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0);
        let mut prev: u32 = 0;
        let got = SystemParametersInfoW(
            SPI_GETFOREGROUNDLOCKTIMEOUT, 0,
            Some(&mut prev as *mut _ as *mut c_void), flags,
        )
        .is_ok();
        if got && prev != 0 {
            // value goes in pvParam for this action; 0 = no lock.
            let _ = SystemParametersInfoW(SPI_SETFOREGROUNDLOCKTIMEOUT, 0, None, flags);
        }
        Self { prev, active: got && prev != 0 }
    }
}

impl Drop for ForegroundLockGuard {
    fn drop(&mut self) {
        if self.active {
            unsafe {
                let _ = SystemParametersInfoW(
                    SPI_SETFOREGROUNDLOCKTIMEOUT, 0,
                    Some(self.prev as usize as *mut c_void),
                    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
                );
            }
        }
    }
}

/// Make this process the "last input event" provider so Windows' foreground
/// lock permits our `SetForegroundWindow`. A non-UIAccess process can normally
/// only set the foreground if it (or the current foreground) sent the last
/// input; injecting a synthetic, side-effect-free keystroke (a lone Ctrl tap —
/// no menu activation like Alt, no cursor movement like a mouse event) makes
/// us that provider for the moment that follows.
unsafe fn foreground_unlock_keypoke() {
    let mk = |up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0x11), // VK_CONTROL
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { Default::default() },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let ev = [mk(false), mk(true)];
    SendInput(&ev, std::mem::size_of::<INPUT>() as i32);
}

/// Forcefully bring `target` to the foreground — beating the foreground lock
/// even from a non-UIAccess process — by combining the AttachThreadInput trick
/// with the synthetic-input unlock above. Used for WPF, which only processes
/// injected stylus while it is the active foreground window (so an occluded WPF
/// must be genuinely raised). Returns whether `target` became foreground.
unsafe fn force_foreground_hard(target: HWND) -> bool {
    if GetForegroundWindow() == target {
        return true;
    }
    let my_tid = GetCurrentThreadId();
    let cur = GetForegroundWindow();
    let mut pid = 0u32;
    let cur_tid = GetWindowThreadProcessId(cur, Some(&mut pid));
    let attached = cur_tid != 0 && cur_tid != my_tid;
    if attached {
        let _ = AttachThreadInput(my_tid, cur_tid, true);
    }
    foreground_unlock_keypoke();
    let _ = SetForegroundWindow(target);
    let _ = SetWindowPos(target, HWND_TOP, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
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

/// Put `win` into / out of the always-on-top (topmost) band WITHOUT activating
/// it. `SWP_NOACTIVATE` means no focus/foreground change. The topmost band sits
/// above ALL normal windows — including an *active* occluder — which `HWND_TOP`
/// alone does not guarantee for a non-activated (esp. `WS_EX_NOACTIVATE`)
/// window. Used to make a blocked injection target win the coordinate hit-test.
unsafe fn set_topmost(win: HWND, on: bool) {
    let after = if on { HWND_TOPMOST } else { HWND_NOTOPMOST };
    let _ = SetWindowPos(win, after, 0, 0, 0, 0, SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE);
}

unsafe fn set_cloak(h: HWND, on: bool) -> bool {
    let v: BOOL = if on { TRUE } else { FALSE };
    DwmSetWindowAttribute(h, DWMWA_CLOAK, &v as *const _ as *const c_void, CLOAK_SIZE).is_ok()
}

/// RAII guard that lands coordinate-routed injection on an occluded background
/// target without stealing focus.
///
/// Coordinate injection (pen/touch) is delivered to the TOP-MOST **visible**
/// window at the screen point — and a DWM-cloaked window is *excluded* from
/// hit-testing (verified: injection over a cloaked target lands on the occluder
/// instead). So we cannot hide the target; to drive it when it's blocked we
/// briefly raise it to the top of the z-order on `arm` — with `SWP_NOACTIVATE`,
/// so the user's window keeps focus/foreground (no activation, no input-queue
/// attach) — and on `Drop` restore the user's window to the top. The target is
/// visible on top only for the few milliseconds of the actuation.
struct ZorderGuard {
    prev_fg: HWND,
    target: HWND,
    raised: bool,
}

impl ZorderGuard {
    unsafe fn arm(target: HWND) -> Self {
        let prev_fg = GetForegroundWindow();
        // TODO: Check if target is actually occluded (WindowFromPoint over its
        // client rect) before raising, and preserve its original topmost state
        // (via GetWindowLongPtr/WS_EX_TOPMOST) so drop() can restore it instead
        // of unconditionally demoting. Current behavior: raise any non-foreground
        // target into topmost band (works for common case, but loses original z
        // and raises even when not occluded).
        let raised = !target.0.is_null() && target != prev_fg;
        if raised {
            set_topmost(target, true);
        }
        Self { prev_fg, target, raised }
    }
}

impl Drop for ZorderGuard {
    fn drop(&mut self) {
        unsafe {
            if self.raised {
                // Drop the target back out of the topmost band, then re-stack the
                // user's window on top (hang-free, no activation messages).
                set_topmost(self.target, false);
                if !self.prev_fg.0.is_null() && self.prev_fg != self.target {
                    restore_z_top(self.prev_fg);
                }
            }
        }
    }
}

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

    // Remember who the user actually had in front, so we can reclaim it below.
    let prev_fg = unsafe { GetForegroundWindow() };
    // Make the target non-activatable for the click (so click-activation can't
    // steal foreground) and raise/restore z-order via the SWP guard.
    let result = {
        let _noact = NoActivateGuard::arm(target_h);
        let _guard = unsafe { ZorderGuard::arm(target_h) };
        // One synthetic device does all `count` taps (single/double/triple click).
        pen_taps(sx, sy, barrel, count)
    };
    // Guards dropped: target demoted out of the topmost band, user's window
    // restacked. But `WS_EX_NOACTIVATE` is NOT categorical against a
    // Chromium/Electron content window that calls `SetForegroundWindow(self)`
    // from its (async) click handler — measured: it still raised. The promoted
    // click is processed slightly AFTER `pen_taps` returns, so a single
    // z-restore loses the race. Reclaim the user's foreground explicitly with a
    // short settle + repeat — the exact race-win pattern already used by
    // `inject_drag_screen` (force_foreground_attached) and `touch_drag` (double
    // SetCursorPos). The action already LANDED (`pen_taps` succeeded); this only
    // puts focus back where the user left it. UIA-Invoke single clicks take the
    // categorical `EnableWindow` shield instead (see `uia::fg_bypass`); this is
    // the fallback for the injection-only actions (double/right/vision clicks).
    unsafe {
        if result.is_ok() && !prev_fg.0.is_null() && prev_fg != target_h {
            force_foreground_attached(prev_fg);
            sleep(Duration::from_millis(12));
            force_foreground_attached(prev_fg);
        }
    }
    result
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
    let prev_fg = unsafe { GetForegroundWindow() };
    // Left drag → touch contact (coordinate-routed). Right/barrel drag has no
    // touch equivalent, so fall back to a pen (rare).
    let stroke = |()| if barrel { pen_drag(sx0, sy0, sx1, sy1, steps, true) } else { touch_drag(sx0, sy0, sx1, sy1, steps) };

    // WPF (Wisp input) only PROCESSES injected stylus while it is the ACTIVE
    // foreground window — raising it in z while it stays inactive is not enough
    // (verified by RE). So for WPF we must briefly activate it (a visible raise
    // + focus, which active⇒foreground⇒topmost also un-occludes), inject, then
    // restore the user's foreground. Other coordinate-injection targets
    // (Chromium/GTK) are pointer-aware and process injection in the background,
    // so we hold them non-activatable and only raise them into the topmost band
    // to win the hit-test when occluded — no focus steal.
    let needs_active = crate::input::dispatch::is_wpf_target_window(target);
    if needs_active {
        // Break the no-raise contract for WPF: fully raise+activate it for the
        // brief moment of the stroke so its Wisp input stack processes the
        // injected stylus, then restore the user's window. The machine's
        // foreground-lock is dropped (in-memory only) for this window so the
        // raise is permitted, and restored immediately afterwards.
        let _lock = unsafe { ForegroundLockGuard::disable() };
        unsafe { force_foreground_hard(target_h); }
        let r = stroke(());
        unsafe {
            if !prev_fg.0.is_null() && prev_fg != target_h {
                force_foreground_hard(prev_fg);
            }
        }
        return r;
    }
    // Chromium/GTK: pointer-aware, process injection in the background — hold
    // non-activatable + raise into the topmost band to win the hit-test when
    // occluded, no focus steal.
    let r = {
        let _noact = NoActivateGuard::arm(target_h);
        let _guard = unsafe { ZorderGuard::arm(target_h) };
        stroke(())
    };
    unsafe {
        if !prev_fg.0.is_null() && prev_fg != target_h {
            force_foreground_attached(prev_fg);
        }
    }
    r
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

    let _serial = fg_serialize(); // one cloaked-foreground op at a time (1s ceiling)
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

/// Type `text` into a **background** target via real SendInput Unicode
/// keystrokes, cloaked so the brief focus is hidden, then restore foreground.
///
/// For targets that ignore a posted `WM_CHAR` (WPF, whose TextBox only consumes
/// real keyboard input routed through its own input manager), `post_type_text`
/// silently does nothing. This delivers genuine `KEYEVENTF_UNICODE` keystrokes
/// to the focused control while the target briefly (and invisibly) holds focus.
/// Capability-first: the text is delivered; the focus flicker is hidden and the
/// user's foreground restored. Caller should focus the field first (a prior
/// background click on it) so the keystrokes land in the right control.
pub fn inject_text_cloaked(target: u64, text: &str) -> Result<()> {
    let target_h = HWND(target as *mut _);
    if target_h.0.is_null() {
        bail!("invalid target hwnd");
    }
    if let Some(msg) = crate::input::post_message_blocked_by_uipi(target) {
        bail!(msg);
    }
    let _serial = fg_serialize(); // one cloaked-foreground op at a time (1s ceiling)
    let prev_fg = unsafe { GetForegroundWindow() };
    let cloaked = unsafe { target_h != prev_fg && set_cloak(target_h, true) };
    let got_fg = unsafe { force_foreground_attached(target_h) };

    let result = if got_fg {
        unsafe { send_unicode(text) }
    } else {
        crate::input::post_type_text(target, text)
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

fn key_unicode(unit: u16, up: bool) -> INPUT {
    let mut flags = KEYEVENTF_UNICODE;
    if up {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: unit,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

unsafe fn send_unicode(text: &str) -> Result<()> {
    let mut ev: Vec<INPUT> = Vec::with_capacity(text.len() * 2);
    for u in text.encode_utf16() {
        ev.push(key_unicode(u, false));
        ev.push(key_unicode(u, true));
    }
    if ev.is_empty() {
        return Ok(());
    }
    let sent = SendInput(&ev, std::mem::size_of::<INPUT>() as i32);
    if sent as usize != ev.len() {
        bail!("SendInput typed only {sent} of {} key events", ev.len());
    }
    Ok(())
}
