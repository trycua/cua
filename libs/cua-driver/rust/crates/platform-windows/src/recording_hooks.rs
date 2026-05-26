//! Recording callbacks exposed to `cua_driver_core::recording`.
//!
//! Two hooks:
//!  - `app_state_json_for` — produces `app_state.json` bytes for a turn folder.
//!  - `element_window_local_xy` — resolves `element_index` to a click point in
//!    window-local screenshot-pixel coordinates so `click.png` is also written
//!    on UIA/MSAA-indexed clicks (not just pixel-addressed ones).

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
    let pid = pid? as u32;
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
    let (sx, sy) = cache.get_element_center(pid as u32, window_id, element_index as usize)?;
    // The cached center is in SCREEN coords. Convert to window-local pixel
    // coords by subtracting the window's screen origin (GetWindowRect-equivalent
    // in WindowInfo). Windows captures at logical pixels so no scale factor.
    let wins = crate::win32::list_windows(Some(pid as u32));
    let win = wins.iter().find(|w| w.hwnd == window_id)?;
    Some(((sx - win.x) as f64, (sy - win.y) as f64))
}

#[cfg(not(target_os = "windows"))]
pub fn app_state_json_for(_window_id: Option<u64>, _pid: Option<i64>) -> Option<Vec<u8>> { None }
#[cfg(not(target_os = "windows"))]
pub fn element_window_local_xy(_window_id: u64, _pid: i64, _element_index: u32) -> Option<(f64, f64)> { None }
