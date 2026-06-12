//! Recording callbacks exposed to `cua_driver_core::recording`.
//!
//! Two hooks live here:
//!  - `app_state_json_for` — produces `app_state.json` bytes for a turn folder.
//!  - `element_window_local_xy` — resolves `element_index` to a click point in
//!    window-local screenshot-pixel coordinates so `click.png` is also written
//!    on AX-indexed clicks (not just pixel-addressed ones).
//!
//! The element-bounds resolver needs the per-(pid, window_id) element cache,
//! which lives in `ToolState`. `tools::register_all` shares the active cache
//! here via `set_element_cache` at startup.

use std::sync::{Arc, OnceLock};

use crate::ax::cache::ElementCache;
use crate::ax::bindings::{element_screen_center, AXUIElementRef};

static ELEMENT_CACHE: OnceLock<Arc<ElementCache>> = OnceLock::new();

pub fn set_element_cache(cache: Arc<ElementCache>) {
    let _ = ELEMENT_CACHE.set(cache);
}

/// Build `app_state.json` bytes for the turn folder. Walks the AX tree for
/// (pid, window_id) and emits the same shape `get_window_state` returns
/// (minus screenshot fields — those live in `screenshot.png`).
pub fn app_state_json_for(window_id: Option<u64>, pid: Option<i64>) -> Option<Vec<u8>> {
    let pid = i32::try_from(pid?).ok()?;
    let resolved_wid = match window_id {
        Some(w) => u32::try_from(w).ok()?,
        None => crate::windows::resolve_main_window_id(pid).ok()?,
    };
    let result = crate::ax::tree::walk_tree(pid, Some(resolved_wid), None);
    let element_count = result.nodes.iter().filter(|n| n.element_index.is_some()).count();
    let payload = serde_json::json!({
        "pid": pid,
        "window_id": resolved_wid,
        "element_count": element_count,
        "tree_markdown": result.tree_markdown,
    });
    serde_json::to_vec_pretty(&payload).ok()
}

/// Resolve `element_index` to window-local screenshot-pixel coords for
/// (pid, window_id). `element_screen_center` returns SCREEN points; convert
/// by subtracting the window's screen origin and multiplying by the
/// screenshot's pixels-per-point scale.
pub fn element_window_local_xy(window_id: u64, pid: i64, element_index: u32) -> Option<(f64, f64)> {
    let cache = ELEMENT_CACHE.get()?;
    let pid_i32 = i32::try_from(pid).ok()?;
    let window_id_u32 = u32::try_from(window_id).ok()?;
    // Retain so a concurrent get_window_state can't free the element between
    // the lookup and element_screen_center (use-after-free → daemon crash).
    let element = cache.get_element_retained(pid_i32, window_id_u32, element_index as usize)?;
    let (sx, sy) = unsafe { element_screen_center(element.as_ptr() as AXUIElementRef)? };

    let bounds = crate::windows::window_bounds_by_id(window_id_u32)?;
    // Probe the captured PNG's width to derive the Retina scale — the
    // screenshot is in physical pixels, the window bounds are in points.
    let scale = if let Ok(png) = crate::capture::screenshot_window_bytes(window_id_u32) {
        if png.len() >= 24 {
            let pw = u32::from_be_bytes([png[16], png[17], png[18], png[19]]) as f64;
            if bounds.width > 0.0 && pw > bounds.width { pw / bounds.width } else { 1.0 }
        } else { 1.0 }
    } else { 1.0 };

    let wx = (sx - bounds.x) * scale;
    let wy = (sy - bounds.y) * scale;
    Some((wx, wy))
}
