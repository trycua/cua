//! Recording callbacks exposed to `cua_driver_core::recording`.
//!
//! Two hooks:
//!  - `app_state_json_for` — produces `app_state.json` bytes for a turn folder.
//!  - `element_window_local_xy` — resolves `element_index` to a click point in
//!    window-local screenshot-pixel coordinates so `click.png` is also written
//!    on UIA/MSAA-indexed clicks (not just pixel-addressed ones).

#[cfg(target_os = "windows")]
use std::sync::Arc;

#[cfg(target_os = "windows")]
use cua_driver_core::element_cache::{current_runtime_cache, register_runtime_cache};

#[cfg(target_os = "windows")]
use crate::uia::cache::{CachedSnapshot, SnapshotKind};

#[cfg(target_os = "windows")]
use crate::uia::ElementCache;

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
pub fn set_element_cache(cache: Arc<ElementCache>) {
    register_runtime_cache(&cache);
}

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

/// Called on the dispatch task after scrolling has resolved the final point.
/// Keep capture inside the lazy callback so private/unrecorded turns do no work.
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
        SnapshotKind::Msaa
    } else {
        SnapshotKind::Uia
    };
    let _native_payload = CachedSnapshot::from_nodes(&result.nodes, kind);
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
pub fn element_window_local_xy(
    pid: i64,
    args: &serde_json::Value,
    capture_point: bool,
) -> Option<(u64, Option<(f64, f64)>)> {
    let cache = current_runtime_cache::<CachedSnapshot>()?;
    let pid_u32 = u32::try_from(pid).ok()?;
    let resolved = cache
        .resolve_element_args(
            pid_u32 as i32,
            args.get("element_index")
                .and_then(|value| value.as_u64())
                .map(|value| value as usize),
            args.get("element_token").and_then(|value| value.as_str()),
            args.get("snapshot_id").and_then(|value| value.as_str()),
            args.get("window_id").and_then(|value| value.as_u64()),
            "recording",
        )
        .ok()?;
    let cua_driver_core::element_token::ResolvedElement::Element {
        window_id: Some(window_id),
        element,
        ..
    } = resolved
    else {
        return None;
    };
    if !capture_point {
        return Some((window_id, None));
    }
    let (sx, sy) = element.center;
    // The cached center is in SCREEN coords. Convert to window-local pixel
    // coords by subtracting the window's screen origin (GetWindowRect-equivalent
    // in WindowInfo). Windows captures at logical pixels so no scale factor.
    let wins = crate::win32::list_windows(Some(pid_u32));
    let point = wins
        .iter()
        .find(|w| w.hwnd == window_id)
        .filter(|_| element.rect.is_some())
        .map(|win| ((sx - win.x) as f64, (sy - win.y) as f64));
    Some((window_id, point))
}

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
