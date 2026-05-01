//! Background mouse injection via PostMessage.
//!
//! PostMessage is asynchronous and does NOT bring the window to the foreground.
//! Coordinates are window-client-relative.

use anyhow::Result;
use std::thread::sleep;
use std::time::Duration;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    PostMessageW, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP,
    WM_MOUSEMOVE, WM_RBUTTONDOWN, WM_RBUTTONUP, MK_LBUTTON, MK_MBUTTON, MK_RBUTTON,
};

const CLICK_DELAY_MS: u64 = 35;

/// Post a left-click (down + up) at window-local coordinates.
///
/// `hwnd` — the target window handle (cast to isize).
/// `x`, `y` — client-area coordinates (window-local).
/// `count` — number of clicks (multi-click = repeated down/up pairs).
/// `button` — "left" | "right" | "middle" (default "left").
pub fn post_click(hwnd: u64, x: i32, y: i32, count: usize, button: &str) -> Result<()> {
    let hwnd = HWND(hwnd as isize);
    let (down_msg, up_msg, mk_flag) = match button {
        "right" => (WM_RBUTTONDOWN, WM_RBUTTONUP, MK_RBUTTON),
        "middle" => (WM_MBUTTONDOWN, WM_MBUTTONUP, MK_MBUTTON),
        _ => (WM_LBUTTONDOWN, WM_LBUTTONUP, MK_LBUTTON),
    };

    let lparam = make_lparam(x, y);
    let wparam_down = mk_flag.0 as usize;

    for _ in 0..count {
        unsafe {
            PostMessageW(hwnd, down_msg, windows::Win32::Foundation::WPARAM(wparam_down), lparam)?;
            sleep(Duration::from_millis(CLICK_DELAY_MS));
            PostMessageW(hwnd, up_msg, windows::Win32::Foundation::WPARAM(0), lparam)?;
        }
        if count > 1 {
            sleep(Duration::from_millis(80));
        }
    }
    Ok(())
}

/// Post a press-drag-release gesture via PostMessage.
///
/// `hwnd` — target window handle. `from_x/y`, `to_x/y` — client-area coords.
/// `duration_ms` — total budget for the drag path. `steps` — number of
/// intermediate `WM_MOUSEMOVE` events linearly interpolated along the path.
/// `button` — "left" | "right" | "middle".
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
    let hwnd = HWND(hwnd as isize);
    let (down_msg, up_msg, mk_flag) = match button {
        "right"  => (WM_RBUTTONDOWN, WM_RBUTTONUP, MK_RBUTTON),
        "middle" => (WM_MBUTTONDOWN, WM_MBUTTONUP, MK_MBUTTON),
        _        => (WM_LBUTTONDOWN, WM_LBUTTONUP, MK_LBUTTON),
    };
    let wparam = mk_flag.0 as usize;
    let steps = steps.max(1);
    let step_delay_ms = if steps > 1 { duration_ms / steps as u64 } else { duration_ms };

    // MouseDown at start.
    unsafe {
        PostMessageW(
            hwnd, down_msg,
            windows::Win32::Foundation::WPARAM(wparam),
            make_lparam(from_x, from_y),
        )?;
    }
    sleep(Duration::from_millis(CLICK_DELAY_MS));

    // Interpolated WM_MOUSEMOVE steps.
    for i in 1..=steps {
        let t = i as f64 / steps as f64;
        let ix = from_x + ((to_x - from_x) as f64 * t).round() as i32;
        let iy = from_y + ((to_y - from_y) as f64 * t).round() as i32;
        unsafe {
            PostMessageW(
                hwnd, WM_MOUSEMOVE,
                windows::Win32::Foundation::WPARAM(wparam),
                make_lparam(ix, iy),
            )?;
        }
        if step_delay_ms > 0 {
            sleep(Duration::from_millis(step_delay_ms));
        }
    }

    // MouseUp at end.
    unsafe {
        PostMessageW(
            hwnd, up_msg,
            windows::Win32::Foundation::WPARAM(0),
            make_lparam(to_x, to_y),
        )?;
    }
    Ok(())
}

/// Pack two 16-bit integers into a LPARAM (low word = x, high word = y).
fn make_lparam(x: i32, y: i32) -> windows::Win32::Foundation::LPARAM {
    windows::Win32::Foundation::LPARAM((((y as u16 as u32) << 16) | (x as u16 as u32)) as isize)
}
