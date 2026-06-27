//! Recording callbacks exposed to `cua_driver_core::recording`.
//!
//! Two hooks:
//!  - `app_state_json_for` — produces `app_state.json` bytes for a turn folder.
//!  - `element_window_local_xy` — resolves `element_index` to a click point in
//!    window-local screenshot-pixel coordinates so `click.png` is also written
//!    on UIA/MSAA-indexed clicks (not just pixel-addressed ones).
//!  - `window_bitmap_origin_xy` — resolves the screenshot bitmap's top-left in
//!    screen coordinates so result-summary screen points can be marker-local.

#[cfg(target_os = "windows")]
use std::sync::{Arc, OnceLock};

#[cfg(target_os = "windows")]
use crate::uia::ElementCache;

#[cfg(target_os = "windows")]
static ELEMENT_CACHE: OnceLock<Arc<ElementCache>> = OnceLock::new();

#[cfg(target_os = "windows")]
pub fn set_element_cache(cache: Arc<ElementCache>) {
    let _ = ELEMENT_CACHE.set(cache);
}

#[cfg(target_os = "windows")]
pub fn app_state_json_for(window_id: Option<u64>, pid: Option<i64>) -> Option<Vec<u8>> {
    let pid = u32::try_from(pid?).ok()?;
    let hwnd = match window_id {
        Some(w) => w,
        None => crate::win32::list_windows(Some(pid)).first().map(|w| w.hwnd)?,
    };
    let result = crate::uia::walk_tree(hwnd, None);
    let element_count = result.nodes.iter().filter(|n| n.element_index.is_some()).count();
    let payload = serde_json::json!({
        "pid": pid,
        "window_id": hwnd,
        "element_count": element_count,
        "tree_markdown": result.tree_markdown,
    });
    serde_json::to_vec_pretty(&payload).ok()
}

#[cfg(target_os = "windows")]
pub fn element_window_local_xy(window_id: u64, pid: i64, element_index: u32) -> Option<(f64, f64)> {
    let cache = ELEMENT_CACHE.get()?;
    let pid_u32 = u32::try_from(pid).ok()?;
    let (sx, sy) = cache.get_element_center(pid_u32, window_id, element_index as usize)?;
    // The cached center is in SCREEN coords. Convert to window-local
    // screenshot pixels using the same bitmap origin as pixel clicks.
    let (ox, oy) = window_bitmap_origin_xy(Some(window_id), Some(pid))?;
    Some((sx as f64 - ox, sy as f64 - oy))
}

#[cfg(target_os = "windows")]
pub fn window_bitmap_origin_xy(window_id: Option<u64>, pid: Option<i64>) -> Option<(f64, f64)> {
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
    use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

    let hwnd = match window_id {
        Some(w) => w,
        None => {
            let pid_u32 = u32::try_from(pid?).ok()?;
            crate::win32::list_windows(Some(pid_u32)).first().map(|w| w.hwnd)?
        }
    };

    // Keep this constant in sync with `capture::DWM_CROP_INSET_PX` and
    // `tools::impl_::bitmap_to_screen`.
    const DWM_CROP_INSET_PX: i32 = 1;
    let h = HWND(hwnd as *mut _);
    unsafe {
        let mut dwm = RECT::default();
        let hr = DwmGetWindowAttribute(
            h,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut dwm as *mut _ as *mut _,
            std::mem::size_of::<RECT>() as u32,
        );
        if hr.is_ok() {
            return Some(((dwm.left + DWM_CROP_INSET_PX) as f64,
                         (dwm.top + DWM_CROP_INSET_PX) as f64));
        }

        let mut wr = RECT::default();
        GetWindowRect(h, &mut wr).ok()?;
        Some((wr.left as f64, wr.top as f64))
    }
}

#[cfg(not(target_os = "windows"))]
pub fn app_state_json_for(_window_id: Option<u64>, _pid: Option<i64>) -> Option<Vec<u8>> { None }
#[cfg(not(target_os = "windows"))]
pub fn element_window_local_xy(_window_id: u64, _pid: i64, _element_index: u32) -> Option<(f64, f64)> { None }
#[cfg(not(target_os = "windows"))]
pub fn window_bitmap_origin_xy(_window_id: Option<u64>, _pid: Option<i64>) -> Option<(f64, f64)> { None }
