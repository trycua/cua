//! Recording callbacks exposed to `cua_driver_core::recording`.
//!
#[cfg(target_os = "windows")]
use crate::uia::element_resolver::{ElementBackend, FreshUiaElements};

use cua_driver_core::recording::ScreenshotCapture;

#[cfg(target_os = "windows")]
use windows::Win32::Foundation::HWND;

#[cfg(target_os = "windows")]
use windows::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;

#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::{
    GetLastActivePopup, GetWindowThreadProcessId, IsWindow, IsWindowVisible,
};

#[cfg(target_os = "windows")]
/// Resolve the window whose application evidence should be captured. Keep a
/// live explicit HWND so occluded/background turns capture the exact target.
/// When an action closes a modal HWND, fall back to another top-level window
/// owned by the same pid for the post-action application state.
#[cfg(target_os = "windows")]
pub fn resolve_window_for_recording(window_id: Option<u64>, pid: Option<i64>) -> Option<u64> {
    if let Some(window_id) = window_id {
        let hwnd = HWND(window_id as *mut _);
        if unsafe { IsWindow(hwnd) }.as_bool() {
            let popup = unsafe { GetLastActivePopup(hwnd) };
            if !unsafe { IsWindowEnabled(hwnd) }.as_bool()
                && popup != hwnd
                && unsafe { IsWindow(popup) }.as_bool()
                && unsafe { IsWindowVisible(popup) }.as_bool()
            {
                let mut popup_pid = 0;
                unsafe { GetWindowThreadProcessId(popup, Some(&mut popup_pid)) };
                if pid.and_then(|value| u32::try_from(value).ok()) == Some(popup_pid) {
                    return Some(popup.0 as u64);
                }
            }
            return Some(window_id);
        }
    }
    let pid = u32::try_from(pid?).ok()?;
    crate::win32::list_windows(Some(pid))
        .first()
        .map(|window| window.hwnd)
}

#[cfg(target_os = "windows")]
pub fn screenshot_for_recording(window_id: Option<u64>, pid: Option<i64>) -> ScreenshotCapture {
    if window_id.is_none() && pid.is_none() {
        return crate::capture::screenshot_display_bytes()
            .map(ScreenshotCapture::captured)
            .unwrap_or_else(|_| ScreenshotCapture::unavailable("capture_failed"));
    }
    let Some(hwnd) = resolve_window_for_recording(window_id, pid) else {
        return ScreenshotCapture::unavailable("target_unavailable");
    };
    match crate::capture::screenshot_window_bytes_with_occlusion(hwnd) {
        Ok((_, true)) => ScreenshotCapture::unavailable("background_occluded"),
        Ok((png, false)) => ScreenshotCapture::captured(png),
        Err(error) if error.to_string().contains("minimized window") => {
            ScreenshotCapture::unavailable("target_minimized")
        }
        Err(_) => ScreenshotCapture::unavailable("capture_failed"),
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn capture_dispatch_click_target(window_id: u64, pid: u32, x: i32, y: i32) {
    cua_driver_core::recording::capture_dispatch_click_target(window_id, i64::from(pid), || {
        crate::capture::screenshot_window_click_target(window_id, x, y)
    });
}

#[cfg(target_os = "windows")]
pub fn app_state_json_for(window_id: Option<u64>, pid: Option<i64>) -> Option<Vec<u8>> {
    let pid = u32::try_from(pid?).ok()?;
    let hwnd = resolve_window_for_recording(window_id, Some(pid.into()))?;
    let result = crate::uia::walk_tree(hwnd, None);
    let kind = if result.nodes.iter().any(|node| node.msaa_role.is_some()) {
        ElementBackend::Msaa
    } else {
        ElementBackend::Uia
    };
    let _native_payload = FreshUiaElements::from_nodes(&result.nodes, kind);
    let element_count = result
        .nodes
        .iter()
        .filter(|n| n.element_index.is_some())
        .count();
    let payload = serde_json::json!({
        "pid": pid,
        "window_id": hwnd,
        "element_count": element_count,
        "tree_markdown": result.tree_markdown,
    });
    serde_json::to_vec_pretty(&payload).ok()
}

#[cfg(target_os = "windows")]
pub use cua_driver_core::element_token::recording_target as element_window_local_xy;

#[cfg(not(target_os = "windows"))]
pub fn app_state_json_for(_window_id: Option<u64>, _pid: Option<i64>) -> Option<Vec<u8>> {
    None
}
#[cfg(not(target_os = "windows"))]
pub fn resolve_window_for_recording(_window_id: Option<u64>, _pid: Option<i64>) -> Option<u64> {
    None
}
#[cfg(not(target_os = "windows"))]
pub fn screenshot_for_recording(_window_id: Option<u64>, _pid: Option<i64>) -> ScreenshotCapture {
    ScreenshotCapture::unavailable("unsupported_platform")
}
#[cfg(not(target_os = "windows"))]
pub fn element_window_local_xy(
    _pid: i64,
    _args: &serde_json::Value,
    _capture_point: bool,
) -> Option<(u64, Option<(f64, f64)>)> {
    None
}
