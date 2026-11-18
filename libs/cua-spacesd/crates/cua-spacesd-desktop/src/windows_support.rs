// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Windows-native pieces of the CUA adapter that live outside the four
//! provider traits: target enumeration, the desktop presence overlay, and the
//! screen-coordinate mapping shared with the cua-driver pixel tools.

use std::collections::HashMap;

use cua_media_protocol::SurfaceGeometry;
use platform_windows as pw;

use crate::{NativeTarget, NativeWindow};

/// Matches `DWM_CROP_INSET_PX` in the cua-driver Windows capture path: the
/// pixel tools treat window-local (0, 0) as `DWMWA_EXTENDED_FRAME_BOUNDS`
/// top-left plus a one-pixel inset.
const DWM_CROP_INSET_PX: i32 = 1;

pub(crate) fn window_scale_factor(hwnd: u64) -> f64 {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::HiDpi::GetDpiForWindow;

    let dpi = unsafe { GetDpiForWindow(HWND(hwnd as *mut _)) };
    if dpi == 0 {
        1.0
    } else {
        f64::from(dpi) / 96.0
    }
}

fn extended_frame_size(hwnd: u64) -> Option<(u32, u32)> {
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, IsWindow};

    let handle = HWND(hwnd as *mut _);
    unsafe {
        if !IsWindow(handle).as_bool() {
            return None;
        }
        let mut bounds = RECT::default();
        if DwmGetWindowAttribute(
            handle,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut bounds as *mut _ as *mut _,
            std::mem::size_of::<RECT>() as u32,
        )
        .is_err()
            && GetWindowRect(handle, &mut bounds).is_err()
        {
            return None;
        }
        Some((
            (bounds.right - bounds.left).max(1) as u32,
            (bounds.bottom - bounds.top).max(1) as u32,
        ))
    }
}

pub(crate) fn resize_window(
    hwnd: u64,
    width_points: u32,
    height_points: u32,
) -> Result<(u32, u32), String> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOZORDER,
    };

    let scale = window_scale_factor(hwnd);
    let width_px = (f64::from(width_points) * scale)
        .round()
        .clamp(1.0, f64::from(i32::MAX)) as i32;
    let height_px = (f64::from(height_points) * scale)
        .round()
        .clamp(1.0, f64::from(i32::MAX)) as i32;
    unsafe {
        SetWindowPos(
            HWND(hwnd as *mut _),
            HWND(std::ptr::null_mut()),
            0,
            0,
            width_px,
            height_px,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
        )
    }
    .map_err(|error| format!("SetWindowPos failed: {error}"))?;

    let (actual_width_px, actual_height_px) = extended_frame_size(hwnd)
        .ok_or_else(|| "resized window is no longer available".to_owned())?;
    Ok((
        (f64::from(actual_width_px) / scale).round().max(1.0) as u32,
        (f64::from(actual_height_px) / scale).round().max(1.0) as u32,
    ))
}

pub(crate) fn enumerate_windows(on_screen_only: bool) -> Vec<NativeWindow> {
    let process_names: HashMap<u32, String> = pw::win32::list_processes()
        .into_iter()
        .map(|process| (process.pid, process.name))
        .collect();
    pw::win32::windows::list_windows(None)
        .into_iter()
        .filter(|window| !on_screen_only || (window.is_on_screen && !window.minimized))
        .map(|window| {
            let app_name = process_names
                .get(&window.pid)
                .map(|name| name.trim_end_matches(".exe").to_owned())
                .unwrap_or_else(|| "unknown".to_owned());
            NativeWindow {
                native: NativeTarget {
                    pid: i64::from(window.pid),
                    window_id: window.hwnd,
                },
                application_id: None,
                app_name,
                title: window.title,
                geometry: SurfaceGeometry {
                    width_px: window.width.max(1) as u32,
                    height_px: window.height.max(1) as u32,
                    scale_factor: window_scale_factor(window.hwnd),
                },
                visible: window.is_on_screen && !window.minimized,
            }
        })
        .collect()
}

/// Screen position of a window-local frame pixel, using the same
/// extended-frame-bounds origin as the cua-driver pixel tools.
pub(crate) fn frame_point_to_screen(hwnd: u64, x: f64, y: f64) -> Option<(f64, f64)> {
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, IsWindow};

    let handle = HWND(hwnd as *mut _);
    unsafe {
        if !IsWindow(handle).as_bool() {
            return None;
        }
        let mut bounds = RECT::default();
        let dwm = DwmGetWindowAttribute(
            handle,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut bounds as *mut _ as *mut _,
            std::mem::size_of::<RECT>() as u32,
        );
        if dwm.is_ok() {
            return Some((
                f64::from(bounds.left + DWM_CROP_INSET_PX) + x,
                f64::from(bounds.top + DWM_CROP_INSET_PX) + y,
            ));
        }
        let mut rect = RECT::default();
        if GetWindowRect(handle, &mut rect).is_err() {
            return None;
        }
        Some((f64::from(rect.left) + x, f64::from(rect.top) + y))
    }
}

/// Start the cursor-overlay render thread once per process. Every remote
/// user's cursor is a keyed overlay instance with its own palette.
pub(crate) fn activate_overlay() {
    static ONCE: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        pw::overlay::init(cursor_overlay::CursorConfig::default());
        pw::overlay::run_on_thread();
    });
}

pub(crate) fn overlay_move(key: &str, x: f64, y: f64) {
    pw::overlay::send_command(
        key.to_owned(),
        cursor_overlay::OverlayCommand::SnapTo {
            x,
            y,
            heading_radians: None,
        },
    );
}

/// Keep the overlay window stacked just above the window the cursor is over.
/// Without a pin the overlay never re-raises itself, so windows created after
/// the daemon bury it and cursors paint invisibly underneath.
pub(crate) fn overlay_pin_above(key: &str, hwnd: u64) {
    pw::overlay::send_command(
        key.to_owned(),
        cursor_overlay::OverlayCommand::PinAbove(hwnd),
    );
}

/// The host's physical cursor resolved to a window-local position: the root
/// window under the pointer, frame-relative coordinates in the same space as
/// captured frames, and the primary-button state.
pub(crate) fn host_cursor() -> Option<(u32, u64, f64, f64, bool)> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetAncestor, GetCursorPos, GetWindowThreadProcessId, WindowFromPoint, GA_ROOT,
    };

    unsafe {
        let mut point = POINT::default();
        if GetCursorPos(&mut point).is_err() {
            return None;
        }
        let hit = WindowFromPoint(point);
        if hit.0.is_null() {
            return None;
        }
        let root = GetAncestor(hit, GA_ROOT);
        if root.0.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(root, Some(&mut pid));
        if pid == 0 {
            return None;
        }
        let hwnd = root.0 as u64;
        let (origin_x, origin_y) = frame_point_to_screen(hwnd, 0.0, 0.0)?;
        let pressed = (GetAsyncKeyState(i32::from(VK_LBUTTON.0)) as u16 & 0x8000) != 0;
        Some((
            pid,
            hwnd,
            f64::from(point.x) - origin_x,
            f64::from(point.y) - origin_y,
            pressed,
        ))
    }
}

pub(crate) fn overlay_set_visible(key: &str, visible: bool) {
    pw::overlay::send_command(
        key.to_owned(),
        cursor_overlay::OverlayCommand::SetEnabled(visible),
    );
}

pub(crate) fn overlay_set_pressed(key: &str, pressed: bool, x: f64, y: f64) {
    if pressed {
        pw::overlay::send_command(
            key.to_owned(),
            cursor_overlay::OverlayCommand::ClickPulse { x, y },
        );
    }
    pw::overlay::send_command(
        key.to_owned(),
        cursor_overlay::OverlayCommand::SetPressed(pressed),
    );
}

pub(crate) fn overlay_remove(key: &str) {
    pw::overlay::remove_cursor(key.to_owned());
}
