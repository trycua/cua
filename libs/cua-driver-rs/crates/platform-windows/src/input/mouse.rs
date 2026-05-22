//! Background mouse injection via PostMessage.
//!
//! All clicks target the **deepest child** HWND at the click point
//! (via ChildWindowFromPointEx), so the message never reaches the top-level
//! chrome that would call SetForegroundWindow in response to WM_LBUTTONDOWN.

use anyhow::{Result, bail};
use std::thread::sleep;
use std::time::Duration;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{ClientToScreen, ScreenToClient};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN,
    MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEINPUT, SendInput,
};
use windows::Win32::UI::WindowsAndMessaging::{
    ChildWindowFromPointEx, CWP_SKIPDISABLED, CWP_SKIPINVISIBLE, CWP_SKIPTRANSPARENT,
    GetCursorPos, GetForegroundWindow, GetSystemMetrics, PostMessageW, SetCursorPos,
    SetForegroundWindow, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP,
    WM_MOUSEMOVE, WM_RBUTTONDOWN, WM_RBUTTONUP,
};

const MK_LBUTTON: u32 = 0x0001;
const MK_MBUTTON: u32 = 0x0010;
const MK_RBUTTON: u32 = 0x0002;

const CLICK_DELAY_MS: u64 = 35;

/// Walk from `root` down to the deepest visible child that contains
/// `screen_pt`, mirroring trope-cua's DeepestChildFromScreenPoint.
///
/// Posting to the deepest child avoids the top-level window responding to
/// WM_LBUTTONDOWN by activating itself (focus-steal).
fn deepest_child(root: HWND, screen_pt: POINT) -> (HWND, POINT) {
    let mut current = root;
    for _ in 0..16 {
        let mut client = screen_pt;
        unsafe { let _ = ScreenToClient(current, &mut client); }
        let child = unsafe {
            ChildWindowFromPointEx(
                current, client,
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
    unsafe { let _ = ScreenToClient(current, &mut client); }
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
    unsafe { let _ = ClientToScreen(root_hwnd, &mut screen_pt); }

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

    let (down_msg, up_msg, mk_flag) = match button {
        "right"  => (WM_RBUTTONDOWN, WM_RBUTTONUP, MK_RBUTTON),
        "middle" => (WM_MBUTTONDOWN, WM_MBUTTONUP, MK_MBUTTON),
        _        => (WM_LBUTTONDOWN, WM_LBUTTONUP, MK_LBUTTON),
    };
    let lparam  = make_lparam(x, y);
    let wdown   = WPARAM(mk_flag as usize);
    let wup     = WPARAM(0);

    for i in 0..count {
        unsafe {
            // WM_MOUSEMOVE first so hover state is correct before the click.
            PostMessageW(hwnd, WM_MOUSEMOVE, WPARAM(0), lparam)?;
            PostMessageW(hwnd, down_msg, wdown, lparam)?;
            sleep(Duration::from_millis(CLICK_DELAY_MS));
            PostMessageW(hwnd, up_msg, wup, lparam)?;
        }
        if i + 1 < count {
            sleep(Duration::from_millis(80));
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
        "right"  => (WM_RBUTTONDOWN, WM_RBUTTONUP, MK_RBUTTON),
        "middle" => (WM_MBUTTONDOWN, WM_MBUTTONUP, MK_MBUTTON),
        _        => (WM_LBUTTONDOWN, WM_LBUTTONUP, MK_LBUTTON),
    };
    let wparam = WPARAM(mk_flag as usize);
    let steps = steps.max(1);
    let step_delay_ms = if steps > 1 { duration_ms / steps as u64 } else { duration_ms };

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

/// Pack two 16-bit integers into a LPARAM (low word = x, high word = y).
fn make_lparam(x: i32, y: i32) -> LPARAM {
    LPARAM((((y as u16 as u32) << 16) | (x as u16 as u32)) as isize)
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
        "right"  => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
        "middle" => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
        _        => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
    };

    // Convert screen pixel coords to normalized absolute coords spanning the
    // virtual desktop (0..65535 across the union of all monitors). This is
    // what `MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK` expects.
    //
    // Without VIRTUALDESK the coords are relative to the primary monitor only;
    // multi-monitor setups would misroute. Better to always use VIRTUALDESK.
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
    let norm_x = ((sx - vd_x) as i64 * 65535 / vd_w as i64).clamp(0, 65535) as i32;
    let norm_y = ((sy - vd_y) as i64 * 65535 / vd_h as i64).clamp(0, 65535) as i32;

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
                dx: norm_x, dy: norm_y, mouseData: 0,
                dwFlags: down_flag | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                time: 0, dwExtraInfo: 0,
            },
        },
    };
    let up_input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: norm_x, dy: norm_y, mouseData: 0,
                dwFlags: up_flag | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                time: 0, dwExtraInfo: 0,
            },
        },
    };

    unsafe {
        // Save previous foreground + cursor position so we can restore.
        let prev_fg = GetForegroundWindow();
        let mut prev_cursor = POINT::default();
        let _ = GetCursorPos(&mut prev_cursor);

        // Focus the target so the click lands there (mirrors send_key_synthesized).
        let _ = SetForegroundWindow(target);
        sleep(Duration::from_millis(8));

        // Move the cursor first so the OS hover state matches before the click.
        // `SetCursorPos` is the visible cursor move; the MOUSEEVENTF_MOVE input
        // ensures Chromium's input filter sees a coordinated move event.
        let _ = SetCursorPos(sx, sy);

        let count = count.max(1);
        for i in 0..count {
            let events = [move_input, down_input, up_input];
            let sent = SendInput(&events, std::mem::size_of::<INPUT>() as i32);
            if sent as usize != events.len() {
                // Partial insertion — restore foreground+cursor and bail with
                // the standard "needs UIAccess worker" diagnostic.
                let _ = SetCursorPos(prev_cursor.x, prev_cursor.y);
                let _ = SetForegroundWindow(prev_fg);
                bail!(
                    "SendInput inserted only {sent} of {} mouse events. Likely cause: \
                     the daemon is not at UIAccess integrity, so SetForegroundWindow was \
                     rejected and the events landed on the wrong window. Route Chromium \
                     coord clicks through the cua-driver-uia worker.",
                    events.len()
                );
            }
            if i + 1 < count {
                sleep(Duration::from_millis(80));
            }
        }

        // Brief settle so the target processes the click before we restore.
        sleep(Duration::from_millis(40));
        let _ = SetCursorPos(prev_cursor.x, prev_cursor.y);
        let _ = SetForegroundWindow(prev_fg);
    }

    Ok(())
}
