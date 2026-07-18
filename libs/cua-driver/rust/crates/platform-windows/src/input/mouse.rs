//! Background mouse injection via PostMessage.
//!
//! All clicks target the **deepest child** HWND at the click point
//! (via ChildWindowFromPointEx), so the message never reaches the top-level
//! chrome that would call SetForegroundWindow in response to WM_LBUTTONDOWN.

use anyhow::{bail, Result};
use std::thread::sleep;
use std::time::Duration;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{ClientToScreen, ScreenToClient};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
    MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK,
    MOUSEEVENTF_WHEEL, MOUSEINPUT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    ChildWindowFromPointEx, GetAncestor, GetClassLongPtrW, GetCursorPos, GetForegroundWindow,
    GetSystemMetrics, GetWindowLongPtrW, PostMessageW, SetCursorPos, SetWindowPos, CS_DBLCLKS,
    CWP_SKIPDISABLED, CWP_SKIPINVISIBLE, CWP_SKIPTRANSPARENT, GA_ROOT, GCL_STYLE, GWL_EXSTYLE,
    HWND_NOTOPMOST, HWND_TOP, HWND_TOPMOST, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, WM_LBUTTONDBLCLK,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDBLCLK, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE,
    WM_RBUTTONDBLCLK, WM_RBUTTONDOWN, WM_RBUTTONUP, WS_EX_TOPMOST,
};

const MK_LBUTTON: u32 = 0x0001;
const MK_MBUTTON: u32 = 0x0010;
const MK_RBUTTON: u32 = 0x0002;

const CLICK_DELAY_MS: u64 = 35;

fn posted_press_message(down: u32, double: u32, click_index: usize, wants_double: bool) -> u32 {
    if wants_double && click_index % 2 == 1 {
        double
    } else {
        down
    }
}

/// Walk from `root` down to the deepest visible child that contains
/// `screen_pt`, mirroring trope-cua's DeepestChildFromScreenPoint.
///
/// Posting to the deepest child avoids the top-level window responding to
/// WM_LBUTTONDOWN by activating itself (focus-steal).
fn deepest_child(root: HWND, screen_pt: POINT) -> (HWND, POINT) {
    let mut current = root;
    for _ in 0..16 {
        let mut client = screen_pt;
        unsafe {
            let _ = ScreenToClient(current, &mut client);
        }
        let child = unsafe {
            ChildWindowFromPointEx(
                current,
                client,
                CWP_SKIPINVISIBLE | CWP_SKIPDISABLED | CWP_SKIPTRANSPARENT,
            )
        };
        // No deeper child, or same window, or outside root's subtree.
        if child.is_invalid() || child == current {
            break;
        }
        // Verify the child is actually a descendant of root.
        let is_child = unsafe { windows::Win32::UI::WindowsAndMessaging::IsChild(root, child) };
        if !is_child.as_bool() && child != root {
            break;
        }
        current = child;
    }
    // Return child-local client coordinates for `current`.
    let mut client = screen_pt;
    unsafe {
        let _ = ScreenToClient(current, &mut client);
    }
    (current, client)
}

/// Post a click at **client-area** coordinates of `root_hwnd`.
///
/// Resolves to the deepest child HWND at the click point before posting,
/// so top-level browser/chrome windows don't activate in response.
pub fn post_click(root: u64, x: i32, y: i32, count: usize, button: &str) -> Result<()> {
    let root_hwnd = HWND(root as *mut _);

    // Convert root-local client → screen.
    let mut screen_pt = POINT { x, y };
    unsafe {
        let _ = ClientToScreen(root_hwnd, &mut screen_pt);
    }

    // Find deepest child and its local client coordinates.
    let (target, client) = deepest_child(root_hwnd, screen_pt);

    post_click_on(target, client.x, client.y, count, button)
}

/// Post a click at **screen** coordinates, resolving the deepest child of
/// `root_hwnd` at that point.  Call this when you already have screen coords.
pub fn post_click_screen(root: u64, sx: i32, sy: i32, count: usize, button: &str) -> Result<()> {
    let root_hwnd = HWND(root as *mut _);
    let screen_pt = POINT { x: sx, y: sy };
    let (target, client) = deepest_child(root_hwnd, screen_pt);
    post_click_on(target, client.x, client.y, count, button)
}

/// Internal: post click messages to `hwnd` using its own client coordinates.
fn post_click_on(hwnd: HWND, x: i32, y: i32, count: usize, button: &str) -> Result<()> {
    // UIPI check — Medium-IL daemon → High-IL target silently drops mouse
    // messages just like keyboard ones. Surface an actionable error before
    // PostMessage returns its misleading TRUE. See post_message_blocked_by_uipi.
    if let Some(msg) = crate::input::post_message_blocked_by_uipi(hwnd.0 as u64) {
        anyhow::bail!(msg);
    }

    let (down_msg, double_msg, up_msg, mk_flag) = match button {
        "right" => (WM_RBUTTONDOWN, WM_RBUTTONDBLCLK, WM_RBUTTONUP, MK_RBUTTON),
        "middle" => (WM_MBUTTONDOWN, WM_MBUTTONDBLCLK, WM_MBUTTONUP, MK_MBUTTON),
        _ => (WM_LBUTTONDOWN, WM_LBUTTONDBLCLK, WM_LBUTTONUP, MK_LBUTTON),
    };
    let lparam = make_lparam(x, y);
    let wdown = WPARAM(mk_flag as usize);
    let wup = WPARAM(0);
    let wants_double = unsafe { (GetClassLongPtrW(hwnd, GCL_STYLE) as u32 & CS_DBLCLKS.0) != 0 };
    let prev_fg = unsafe { GetForegroundWindow() };
    let target_root = unsafe {
        let root = GetAncestor(hwnd, GA_ROOT);
        if root.0.is_null() {
            hwnd
        } else {
            root
        }
    };

    // Posted pointer messages are normally non-activating, but WebView hosts can
    // call SetForegroundWindow from their event handlers. Keep the top-level
    // categorically non-activatable until the complete burst has settled.
    let _noact = crate::input::NoActivateGuard::arm(hwnd);
    for i in 0..count {
        let press_msg = posted_press_message(down_msg, double_msg, i, wants_double);
        unsafe {
            // WM_MOUSEMOVE first so hover state is correct before the click.
            PostMessageW(hwnd, WM_MOUSEMOVE, WPARAM(0), lparam)?;
            // Win32 controls do not infer a double-click from two posted DOWN
            // messages. The second press must use WM_*BUTTONDBLCLK.
            PostMessageW(hwnd, press_msg, wdown, lparam)?;
            sleep(Duration::from_millis(CLICK_DELAY_MS));
            PostMessageW(hwnd, up_msg, wup, lparam)?;
        }
        if i + 1 < count {
            sleep(Duration::from_millis(80));
        }
    }
    sleep(Duration::from_millis(50));
    unsafe {
        if !prev_fg.0.is_null() && prev_fg != target_root && GetForegroundWindow() == target_root {
            crate::input::force_foreground_attached(prev_fg);
            sleep(Duration::from_millis(12));
            crate::input::force_foreground_attached(prev_fg);
        }
    }
    Ok(())
}

/// Post a press-drag-release gesture via PostMessage.
///
/// Coordinates are root-hwnd client-area relative.
pub fn post_drag(
    hwnd: u64,
    from_x: i32,
    from_y: i32,
    to_x: i32,
    to_y: i32,
    duration_ms: u64,
    steps: usize,
    button: &str,
) -> Result<()> {
    let root = HWND(hwnd as *mut _);
    let (down_msg, up_msg, mk_flag) = match button {
        "right" => (WM_RBUTTONDOWN, WM_RBUTTONUP, MK_RBUTTON),
        "middle" => (WM_MBUTTONDOWN, WM_MBUTTONUP, MK_MBUTTON),
        _ => (WM_LBUTTONDOWN, WM_LBUTTONUP, MK_LBUTTON),
    };
    let wparam = WPARAM(mk_flag as usize);
    let steps = steps.max(1);
    let step_delay_ms = if steps > 1 {
        duration_ms / steps as u64
    } else {
        duration_ms
    };

    unsafe {
        PostMessageW(root, down_msg, wparam, make_lparam(from_x, from_y))?;
    }
    sleep(Duration::from_millis(CLICK_DELAY_MS));

    for i in 1..=steps {
        let t = i as f64 / steps as f64;
        let ix = from_x + ((to_x - from_x) as f64 * t).round() as i32;
        let iy = from_y + ((to_y - from_y) as f64 * t).round() as i32;
        unsafe {
            PostMessageW(root, WM_MOUSEMOVE, wparam, make_lparam(ix, iy))?;
        }
        if step_delay_ms > 0 {
            sleep(Duration::from_millis(step_delay_ms));
        }
    }

    unsafe {
        PostMessageW(root, up_msg, WPARAM(0), make_lparam(to_x, to_y))?;
    }
    Ok(())
}

/// Press-drag-release via PostMessage, resolving the **deepest child** at the
/// drag-start screen point and posting in that child's client coordinates.
///
/// `post_drag` (above) posts to the top-level frame, so a child-windowed
/// control (a WinForms `Panel`, a Win32 child canvas, …) never sees the drag —
/// the frame gets messages over a region it doesn't own and ignores them. This
/// variant mirrors `post_click`: it hit-tests down to the deepest descendant
/// under the start point and targets that HWND for the whole gesture (a drag
/// stays within one control), with each point converted to the child's own
/// client space. Endpoints are given in **screen** coordinates.
pub fn post_drag_screen(
    root: u64,
    sx_from: i32,
    sy_from: i32,
    sx_to: i32,
    sy_to: i32,
    duration_ms: u64,
    steps: usize,
    button: &str,
) -> Result<()> {
    let root_hwnd = HWND(root as *mut _);
    let (target, c_from) = deepest_child(
        root_hwnd,
        POINT {
            x: sx_from,
            y: sy_from,
        },
    );
    let mut c_to = POINT { x: sx_to, y: sy_to };
    unsafe {
        let _ = ScreenToClient(target, &mut c_to);
    }
    if let Some(msg) = crate::input::post_message_blocked_by_uipi(target.0 as u64) {
        anyhow::bail!(msg);
    }
    let (down_msg, up_msg, mk_flag) = match button {
        "right" => (WM_RBUTTONDOWN, WM_RBUTTONUP, MK_RBUTTON),
        "middle" => (WM_MBUTTONDOWN, WM_MBUTTONUP, MK_MBUTTON),
        _ => (WM_LBUTTONDOWN, WM_LBUTTONUP, MK_LBUTTON),
    };
    let wparam = WPARAM(mk_flag as usize);
    let steps = steps.max(1);
    let step_delay_ms = if steps > 1 {
        duration_ms / steps as u64
    } else {
        duration_ms
    };
    unsafe {
        // Pre-drag MOUSEMOVE (wParam=0, no buttons down yet) then DOWN at from.
        PostMessageW(
            target,
            WM_MOUSEMOVE,
            WPARAM(0),
            make_lparam(c_from.x, c_from.y),
        )?;
        PostMessageW(target, down_msg, wparam, make_lparam(c_from.x, c_from.y))?;
    }
    sleep(Duration::from_millis(CLICK_DELAY_MS));
    for i in 1..=steps {
        let t = i as f64 / steps as f64;
        let ix = c_from.x + ((c_to.x - c_from.x) as f64 * t).round() as i32;
        let iy = c_from.y + ((c_to.y - c_from.y) as f64 * t).round() as i32;
        unsafe {
            PostMessageW(target, WM_MOUSEMOVE, wparam, make_lparam(ix, iy))?;
        }
        if step_delay_ms > 0 {
            sleep(Duration::from_millis(step_delay_ms));
        }
    }
    unsafe {
        PostMessageW(target, up_msg, WPARAM(0), make_lparam(c_to.x, c_to.y))?;
    }
    Ok(())
}

/// Pack two 16-bit integers into a LPARAM (low word = x, high word = y).
///
/// Delegates the bit-math to [`crate::lparam::pack_xy`] so the
/// receiver-side `GET_X_LPARAM` / `GET_Y_LPARAM` sign-extension contract is
/// covered by cross-platform unit tests (see #1979's audit: the PostMessage
/// path packing was a suspect for the multi-monitor wrong-screen symptom,
/// turned out to be correct, and now has regression coverage).
///
/// On the (currently unreachable) out-of-range path we log + clamp rather
/// than panic — every existing call site passes post-`ScreenToClient`
/// window-local coords that fit in `i16` by construction, but if a future
/// caller passes a raw screen coord on a >32k-px virtual desktop, clamping
/// is at least visible in the log instead of silently wrapping.
fn make_lparam(x: i32, y: i32) -> LPARAM {
    match crate::lparam::pack_xy(x, y) {
        Ok(packed) => LPARAM(packed as isize),
        Err(err) => {
            tracing::warn!(
                target: "click",
                "make_lparam: {err}; clamping to i16 range. \
                 If you see this, the caller is passing non-window-local coords."
            );
            let clamp = |v: i32| v.clamp(i16::MIN as i32, i16::MAX as i32);
            let cx = clamp(x);
            let cy = clamp(y);
            let packed =
                crate::lparam::pack_xy(cx, cy).expect("clamped values always fit in i16 range");
            LPARAM(packed as isize)
        }
    }
}

/// Returns `true` when `hwnd` is a top-level frame of a Chromium-based browser
/// — Edge, Chrome, Brave, Vivaldi, Opera, Chromium, Arc, Thorium, Iridium,
/// Yandex, or any other Chromium-derivative. Matches by window class name,
/// which is stable across versions and consistent across Chromium forks.
///
/// Chromium uses the window class `Chrome_WidgetWin_1` (or `Chrome_WidgetWin_0`
/// for in-process child frames; both should be treated the same way). Electron
/// apps that embed Chromium also use this class, so Electron app coord clicks
/// will route through the SendInput path too — that's intentional, same root
/// cause (#1623).
///
/// Cheap call: one `GetClassNameW` to a 32-char buffer + a `matches!` against
/// the known prefixes. Suitable to call inline in the click dispatch hot path.
pub fn is_chromium_target_window(hwnd: u64) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::GetClassNameW;
    if hwnd == 0 {
        tracing::debug!(target: "click", "is_chromium_target_window: hwnd=0 short-circuit");
        return false;
    }
    let mut buf = [0u16; 64];
    let n = unsafe { GetClassNameW(HWND(hwnd as *mut _), &mut buf) };
    if n <= 0 {
        tracing::debug!(
            target: "click",
            "is_chromium_target_window: GetClassNameW returned {n} for hwnd=0x{hwnd:x}"
        );
        return false;
    }
    let class_name = String::from_utf16_lossy(&buf[..n as usize]);
    // Chromium-family classes. Match by prefix because Chromium suffixes a
    // 0/1 digit; future Chromium forks may use other suffixes.
    let is_chromium = class_name.starts_with("Chrome_WidgetWin_")
        // Electron sometimes uses CefBrowserWindow or similar — be permissive.
        || class_name.starts_with("CefBrowser");
    tracing::debug!(
        target: "click",
        "is_chromium_target_window: hwnd=0x{hwnd:x} class={class_name:?} → {is_chromium}"
    );
    is_chromium
}

/// Return true when `hwnd` hosts a Chromium/WebView2 renderer child even if
/// its own top-level class is framework-specific (for example a Tauri host).
/// Keep this separate from [`is_chromium_target_window`]: embedded WebView2
/// surfaces support some UIA/top-level background routes that direct Chromium
/// frames do not, so delivery policy needs to distinguish the two shapes.
pub fn has_chromium_descendant(hwnd: u64) -> bool {
    use windows::Win32::Foundation::{BOOL, FALSE, LPARAM, TRUE};
    use windows::Win32::UI::WindowsAndMessaging::{EnumChildWindows, GetClassNameW};

    if hwnd == 0 {
        return false;
    }
    struct Scan {
        found: bool,
    }
    unsafe extern "system" fn child_cb(child: HWND, lparam: LPARAM) -> BOOL {
        let scan = &mut *(lparam.0 as *mut Scan);
        let mut buf = [0u16; 64];
        let n = GetClassNameW(child, &mut buf);
        if n > 0 {
            let class = String::from_utf16_lossy(&buf[..n as usize]);
            if class.starts_with("Chrome_WidgetWin_") || class.starts_with("CefBrowser") {
                scan.found = true;
                return FALSE;
            }
        }
        TRUE
    }

    let mut scan = Scan { found: false };
    unsafe {
        let _ = EnumChildWindows(
            HWND(hwnd as *mut _),
            Some(child_cb),
            LPARAM(&mut scan as *mut Scan as isize),
        );
    }
    scan.found
}

/// Click at **screen** coordinates `(sx, sy)` via `SendInput` against the
/// system input queue, briefly focusing `target` so the click lands there.
///
/// Why this exists alongside `post_click_screen`: PostMessage(WM_LBUTTONDOWN)
/// to Chromium-based browsers' top-level frame HWND (or Chrome_RenderWidgetHostHWND
/// descendant) doesn't fire DOM `onclick` / `mousedown` handlers. Chromium's
/// input thread architecture requires events with `SendInput`-queue origin —
/// the same constraint that broke modifier-state hotkey delivery (#1614/#1618)
/// applies to coord clicks on Chromium content (#1623).
///
/// `SendInput` puts the synthetic mouse events on the **system input queue**,
/// where Chromium's input filter accepts them. The trade-off is a brief
/// foreground swap + visible cursor jump (mitigated by saving/restoring the
/// previous foreground HWND and previous cursor position after the click).
///
/// UIAccess constraint: `SetForegroundWindow` is restricted from non-UIAccess
/// processes when not driven by user input. The `cua-driver-uia` worker runs
/// at UIAccess integrity precisely so this restriction is lifted; outside the
/// worker, the foreground swap may silently fail and SendInput land on the
/// wrong window. Callers should funnel Chromium coord clicks through the
/// uia worker (the MCP proxy already prefers the uia pipe over the regular
/// pipe when both are running).
pub fn send_click_synthesized(
    target: u64,
    sx: i32,
    sy: i32,
    count: usize,
    button: &str,
) -> Result<()> {
    send_click_synthesized_mods(target, sx, sy, count, button, &[])
}

/// Like [`send_click_synthesized`] but HOLDS the named modifier keys
/// (cmd/shift/option/ctrl) for the duration of the click: modifier-down via
/// SendInput before the click sequence, modifier-up after. Mirrors the macOS
/// click `modifier` surface. Only this SendInput (foreground / desktop-scope)
/// path can carry modifiers — the background UIA-Invoke and PostMessage paths
/// have no keyboard state to hold them, so a `modifier` passed to a background
/// pixel/element click is necessarily ignored on those rungs.
pub fn send_click_synthesized_mods(
    target: u64,
    sx: i32,
    sy: i32,
    count: usize,
    button: &str,
    modifiers: &[&str],
) -> Result<()> {
    send_click_synthesized_mods_impl(target, sx, sy, count, button, modifiers, false)
}

/// SendInput click for an explicit foreground request. Unlike the historical
/// z-order-assisted path, this activates the target and does not add
/// `WS_EX_NOACTIVATE`, so retained-mode frameworks such as WPF process the
/// system-queue pointer event. The caller owns any later foreground restore.
pub fn send_click_synthesized_active_mods(
    target: u64,
    sx: i32,
    sy: i32,
    count: usize,
    button: &str,
    modifiers: &[&str],
) -> Result<()> {
    send_click_synthesized_mods_impl(target, sx, sy, count, button, modifiers, true)
}

fn send_click_synthesized_mods_impl(
    target: u64,
    sx: i32,
    sy: i32,
    count: usize,
    button: &str,
    modifiers: &[&str],
    activate: bool,
) -> Result<()> {
    let target = HWND(target as *mut _);
    if target.0.is_null() {
        bail!("invalid target hwnd");
    }
    if let Some(msg) = crate::input::post_message_blocked_by_uipi(target.0 as u64) {
        // Same UIPI defense as PostMessage path — SendInput from non-UIAccess
        // would fail just as silently as PostMessage when target is at higher
        // integrity. Surface the diagnostic early.
        bail!(msg);
    }

    let (down_flag, up_flag) = match button {
        "right" => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
        "middle" => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
        _ => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
    };

    // Convert screen pixel coords to normalized absolute coords spanning the
    // virtual desktop (0..65535 across the union of all monitors). This is
    // what `MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK` expects.
    //
    // Without VIRTUALDESK the coords are relative to the primary monitor only;
    // multi-monitor setups would misroute. Better to always use VIRTUALDESK.
    //
    // Math lives in `crate::virtualdesk` so it can be unit-tested cross-platform
    // (no Win32 runtime required) — see issue #1979 for the negative-offset
    // multi-monitor case the tests there pin down.
    let (vd_x, vd_y) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
        )
    };
    let (vd_w, vd_h) = unsafe {
        (
            GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1),
            GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1),
        )
    };
    let (norm_x, norm_y) =
        crate::virtualdesk::to_virtualdesk_absolute(sx, sy, vd_x, vd_y, vd_w, vd_h);

    let move_input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: norm_x,
                dy: norm_y,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let down_input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: down_flag,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let up_input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: up_flag,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };

    unsafe {
        // Save previous foreground + cursor position so we can restore.
        let prev_fg = GetForegroundWindow();
        let mut prev_cursor = POINT::default();
        let _ = GetCursorPos(&mut prev_cursor);

        // Bring the target to the top of the VISIBLE z-order so the
        // coordinate-routed SendInput mouse click lands on it — WITHOUT stealing
        // focus. `SetWindowPos(HWND_TOPMOST, SWP_NOACTIVATE)` is **lock-free**:
        // it works from a non-UIAccess process even on a maxed foreground-lock
        // (unlike `SetForegroundWindow`, which the lock denies), and sends no
        // WM_ACTIVATE. `NoActivateGuard` then keeps the click itself from
        // activating the target. This is the macOS-aligned "front → act →
        // restore" for pointer input, done the one Windows way that doesn't
        // need UIAccess — the technique the OG GTK path used. (Keyboard
        // foreground still needs *real* focus; only pointer can be z-routed.)
        // Capture whether the target was ALREADY always-on-top so we don't strip
        // that state on restore — only demote below if WE promoted it.
        let was_topmost = (GetWindowLongPtrW(target, GWL_EXSTYLE) as u32) & WS_EX_TOPMOST.0 != 0;
        let foreground_attach_failed = activate && !crate::input::force_foreground_attached(target);
        let noactivate = (!activate).then(|| crate::input::NoActivateGuard::arm(target));
        if !activate || foreground_attach_failed {
            let flags = if activate {
                SWP_NOMOVE | SWP_NOSIZE
            } else {
                SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE
            };
            let _ = SetWindowPos(target, HWND_TOPMOST, 0, 0, 0, 0, flags);
        }

        // Move the cursor so the OS hover state matches before the click; the
        // MOUSEEVENTF_MOVE input ensures Chromium's input filter sees a
        // coordinated move event.
        let _ = SetCursorPos(sx, sy);

        // Hold modifier keys (ctrl/shift/alt/win) across the click — the macOS
        // `modifier` surface. Pressed via the system input queue so apps that
        // poll GetKeyState (WPF, Chromium) observe the held state, then released
        // after the click loop below.
        let (mod_downs, mod_ups) = crate::input::keyboard::modifier_hold_inputs(modifiers);
        if !mod_downs.is_empty() {
            SendInput(&mod_downs, std::mem::size_of::<INPUT>() as i32);
            sleep(Duration::from_millis(5));
        }

        let count = count.max(1);
        let mut sent_ok = true;
        for i in 0..count {
            // Only the move record carries absolute coordinates. Button-only
            // records act at the current pointer position; adding ABSOLUTE to
            // them can prevent retained-mode controls from seeing the press.
            let events = [move_input, down_input, up_input];
            let sent = SendInput(&events, std::mem::size_of::<INPUT>() as i32);
            if sent as usize != events.len() {
                sent_ok = false;
                break;
            }
            if i + 1 < count {
                sleep(Duration::from_millis(80));
            }
        }

        // Release any held modifiers (reverse order) before restoring z-order.
        if !mod_ups.is_empty() {
            SendInput(&mod_ups, std::mem::size_of::<INPUT>() as i32);
        }

        // Let the target process mouse-up before any background-route restore.
        // Retained-mode frameworks establish capture/focus on mouse-down and can
        // lose the click if the real cursor is warped away while those queued
        // messages are still being dispatched.
        sleep(Duration::from_millis(if activate { 120 } else { 40 }));
        if !was_topmost && (!activate || foreground_attach_failed) {
            let _ = SetWindowPos(
                target,
                HWND_NOTOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
            );
        }
        if !activate {
            if !prev_fg.0.is_null() && prev_fg != target {
                let _ = SetWindowPos(
                    prev_fg,
                    HWND_TOP,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
                );
            }
            let _ = SetCursorPos(prev_cursor.x, prev_cursor.y);
        }
        drop(noactivate);
        if !sent_ok {
            bail!("SendInput inserted fewer mouse events than expected for the foreground click.");
        }
        if activate {
            let foreground_root = GetAncestor(GetForegroundWindow(), GA_ROOT);
            let target_root = GetAncestor(target, GA_ROOT);
            if foreground_root != target_root {
                bail!("The foreground click did not activate its target window.");
            }
        }
    }

    Ok(())
}

/// Press-hold-move-release drag via `SendInput`. Companion to
/// [`send_click_synthesized`] for the `drag` tool's `delivery_mode:"foreground"`
/// path.
///
/// Why a SendInput drag is needed at all: the PostMessage drag path posts
/// `WM_LBUTTONDOWN` + `WM_MOUSEMOVE`s + `WM_LBUTTONUP` to the target HWND.
/// PostMessage does NOT update the per-thread keyboard state that
/// `GetKeyState(VK_LBUTTON)` reads, so frameworks that poll the button-held
/// state during their drag handler (WPF's Thumb.IsDragging logic does this
/// via Mouse.LeftButton, which polls GetKeyState) never observe the button
/// as down and the drag is a no-op. SendInput goes through the system
/// input queue and DOES update GetKeyState, so a WPF Slider thumb actually
/// tracks the drag.
///
/// Same UIAccess constraints as [`send_click_synthesized`] — the
/// `SetForegroundWindow` swap is rejected from non-UIAccess processes
/// when foreground-lock is active; route through `cua-driver-uia.exe`
/// for reliable operation.
pub fn send_drag_synthesized(
    target: u64,
    sx_from: i32,
    sy_from: i32,
    sx_to: i32,
    sy_to: i32,
    duration_ms: u64,
    steps: usize,
    button: &str,
) -> Result<()> {
    let target = HWND(target as *mut _);
    if target.0.is_null() {
        bail!("invalid target hwnd");
    }
    if let Some(msg) = crate::input::post_message_blocked_by_uipi(target.0 as u64) {
        bail!(msg);
    }

    let (down_flag, up_flag) = match button {
        "right" => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
        "middle" => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
        _ => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
    };

    let (vd_x, vd_y, vd_w, vd_h) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1),
            GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1),
        )
    };
    // Same VIRTUALDESK normalization as `send_click_synthesized`; see
    // `crate::virtualdesk` for the math + the cross-platform unit tests.
    let norm = |sx: i32, sy: i32| -> (i32, i32) {
        crate::virtualdesk::to_virtualdesk_absolute(sx, sy, vd_x, vd_y, vd_w, vd_h)
    };
    let make_input = |dx: i32, dy: i32, flags| INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: 0,
                dwFlags: flags | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };

    let steps = steps.max(1);
    let step_delay_ms = if steps > 1 {
        duration_ms / steps as u64
    } else {
        0
    };

    unsafe {
        let prev_fg = GetForegroundWindow();
        let mut prev_cursor = POINT::default();
        let _ = GetCursorPos(&mut prev_cursor);

        // Lock-free z-order raise (no focus steal) so the coordinate-routed drag
        // lands on the target — same technique as send_click_synthesized.
        // SetForegroundWindow is lock-denied without UIAccess and isn't needed
        // for pointer input; NoActivateGuard keeps the press from activating it.
        let _noact = crate::input::NoActivateGuard::arm(target);
        // Capture whether the target was ALREADY always-on-top so we only demote
        // below if WE promoted it (else we'd strip a legitimate topmost window).
        let was_topmost = (GetWindowLongPtrW(target, GWL_EXSTYLE) as u32) & WS_EX_TOPMOST.0 != 0;
        let _ = SetWindowPos(
            target,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
        );

        // 1. Move + press at the start of the drag.
        let (nfx, nfy) = norm(sx_from, sy_from);
        let _ = SetCursorPos(sx_from, sy_from);
        let prelude = [
            make_input(nfx, nfy, MOUSEEVENTF_MOVE),
            make_input(nfx, nfy, down_flag),
        ];
        let sent = SendInput(&prelude, std::mem::size_of::<INPUT>() as i32);
        if sent as usize != prelude.len() {
            if !was_topmost {
                let _ = SetWindowPos(
                    target,
                    HWND_NOTOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
                );
            }
            if !prev_fg.0.is_null() && prev_fg != target {
                let _ = SetWindowPos(
                    prev_fg,
                    HWND_TOP,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
                );
            }
            let _ = SetCursorPos(prev_cursor.x, prev_cursor.y);
            bail!(
                "SendInput drag-prelude inserted {sent}/{} events",
                prelude.len()
            );
        }

        // 2. Interpolate the path. SetCursorPos + MOUSEEVENTF_MOVE in lockstep
        //    so both the visible cursor and the system input queue track the
        //    same path — WPF's drag-handler watches GetKeyState during each
        //    move event.
        for i in 1..=steps {
            let t = i as f64 / steps as f64;
            let x = sx_from + ((sx_to - sx_from) as f64 * t).round() as i32;
            let y = sy_from + ((sy_to - sy_from) as f64 * t).round() as i32;
            let (nx, ny) = norm(x, y);
            let _ = SetCursorPos(x, y);
            let mv = [make_input(nx, ny, MOUSEEVENTF_MOVE)];
            let _ = SendInput(&mv, std::mem::size_of::<INPUT>() as i32);
            if step_delay_ms > 0 {
                sleep(Duration::from_millis(step_delay_ms));
            }
        }

        // 3. Release at the end.
        let (ntx, nty) = norm(sx_to, sy_to);
        let release = [make_input(ntx, nty, up_flag)];
        let _ = SendInput(&release, std::mem::size_of::<INPUT>() as i32);

        // Brief settle, then restore z-order (demote target, restack user's
        // window — no activation) and the cursor.
        sleep(Duration::from_millis(40));
        if !was_topmost {
            let _ = SetWindowPos(
                target,
                HWND_NOTOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
            );
        }
        if !prev_fg.0.is_null() && prev_fg != target {
            let _ = SetWindowPos(
                prev_fg,
                HWND_TOP,
                0,
                0,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
            );
        }
        let _ = SetCursorPos(prev_cursor.x, prev_cursor.y);
        drop(_noact);
    }

    Ok(())
}

/// Standard wheel notch delta. A `mouseData` value of `±WHEEL_DELTA` is one
/// detent of the physical mouse wheel.
const WHEEL_DELTA: i32 = 120;

/// Compute the `MOUSEINPUT::mouseData` value for a wheel event of `ticks`
/// detents. Positive ticks = wheel forward/up (vertical) or right (horizontal);
/// negative = down / left. `mouseData` is a `u32` field carrying a signed
/// 32-bit delta, so we compute as `i32` then bit-cast to `u32` (this is what
/// the Win32 docs mean by "the value is a multiple of WHEEL_DELTA").
///
/// Factored out of [`send_wheel_synthesized`] so the sign/magnitude encoding is
/// unit-testable without a live display / `SendInput`.
fn wheel_mouse_data(ticks: i32) -> u32 {
    (WHEEL_DELTA * ticks) as u32
}

/// Synthesize a single mouse-wheel event at screen coordinates `(sx, sy)` via
/// `SendInput`.
///
/// The OS routes wheel input to the window **under the cursor**, not the
/// foreground window, so we `SetCursorPos(sx, sy)` first to place the wheel
/// over the intended target. `ticks` encodes both magnitude and direction:
/// positive scrolls up (vertical) / right (horizontal), negative scrolls down /
/// left — matching the `MOUSEEVENTF_WHEEL` / `MOUSEEVENTF_HWHEEL` convention
/// where `mouseData = WHEEL_DELTA * ticks`.
///
/// Unlike [`send_click_synthesized`] this does NOT do a foreground swap: wheel
/// delivery follows the cursor, so positioning the cursor is sufficient. The
/// cursor is restored to its previous position afterward.
pub fn send_wheel_synthesized(sx: i32, sy: i32, ticks: i32, horizontal: bool) -> Result<()> {
    let flag = if horizontal {
        MOUSEEVENTF_HWHEEL
    } else {
        MOUSEEVENTF_WHEEL
    };
    let mouse_data = wheel_mouse_data(ticks);

    let wheel_input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: mouse_data,
                dwFlags: flag,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };

    unsafe {
        let mut prev_cursor = POINT::default();
        let _ = GetCursorPos(&mut prev_cursor);

        // Wheel routes to the window under the cursor — place it on the target.
        let _ = SetCursorPos(sx, sy);

        let events = [wheel_input];
        let sent = SendInput(&events, std::mem::size_of::<INPUT>() as i32);
        if sent as usize != events.len() {
            let _ = SetCursorPos(prev_cursor.x, prev_cursor.y);
            bail!("SendInput inserted {sent}/{} wheel events", events.len());
        }

        // Brief settle, then restore the cursor.
        sleep(Duration::from_millis(20));
        let _ = SetCursorPos(prev_cursor.x, prev_cursor.y);
    }

    Ok(())
}

/// Return the current foreground window for desktop-scope keyboard and drag
/// operations, which intentionally target whatever the user can currently see.
pub fn foreground_window() -> Result<u64> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        bail!("no foreground window is available");
    }
    Ok(hwnd.0 as u64)
}

/// Move the real OS pointer to a desktop coordinate.
pub fn move_cursor_desktop(x: i32, y: i32) -> Result<()> {
    unsafe { SetCursorPos(x, y) }
        .map_err(|error| anyhow::anyhow!("SetCursorPos({x}, {y}) failed: {error}"))
}

#[cfg(test)]
mod wheel_tests {
    use super::{posted_press_message, wheel_mouse_data, WHEEL_DELTA};
    use windows::Win32::UI::WindowsAndMessaging::{WM_LBUTTONDBLCLK, WM_LBUTTONDOWN};

    #[test]
    fn posted_double_click_uses_the_win32_double_click_message() {
        assert_eq!(
            posted_press_message(WM_LBUTTONDOWN, WM_LBUTTONDBLCLK, 0, true),
            WM_LBUTTONDOWN
        );
        assert_eq!(
            posted_press_message(WM_LBUTTONDOWN, WM_LBUTTONDBLCLK, 1, true),
            WM_LBUTTONDBLCLK
        );
        assert_eq!(
            posted_press_message(WM_LBUTTONDOWN, WM_LBUTTONDBLCLK, 2, true),
            WM_LBUTTONDOWN
        );
        assert_eq!(
            posted_press_message(WM_LBUTTONDOWN, WM_LBUTTONDBLCLK, 1, false),
            WM_LBUTTONDOWN
        );
    }

    #[test]
    fn wheel_data_up_is_positive_one_notch() {
        // +1 tick (up / right) → +WHEEL_DELTA, bit-cast to u32.
        assert_eq!(wheel_mouse_data(1), WHEEL_DELTA as u32);
        assert_eq!(wheel_mouse_data(1), 120u32);
    }

    #[test]
    fn wheel_data_down_is_negative_one_notch() {
        // -1 tick (down / left) → -WHEEL_DELTA, bit-cast: 0xFFFFFF88.
        assert_eq!(wheel_mouse_data(-1), (-WHEEL_DELTA) as u32);
        assert_eq!(wheel_mouse_data(-1), 0xFFFF_FF88);
    }

    #[test]
    fn wheel_data_scales_with_ticks() {
        assert_eq!(wheel_mouse_data(3), (3 * WHEEL_DELTA) as u32);
        assert_eq!(wheel_mouse_data(3), 360u32);
        assert_eq!(wheel_mouse_data(-3) as i32, -360);
    }

    #[test]
    fn wheel_data_zero_is_zero() {
        assert_eq!(wheel_mouse_data(0), 0);
    }
}
