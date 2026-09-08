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

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock, Weak},
};

use crate::ax::bindings::{element_screen_center, AXUIElementRef};
use crate::ax::cache::ElementCache;

static ELEMENT_CACHES: OnceLock<Mutex<HashMap<String, Weak<ElementCache>>>> = OnceLock::new();

pub fn set_element_cache(cache: Arc<ElementCache>) {
    let runtime_scope =
        cua_driver_core::tool::current_dispatch_runtime_scope().unwrap_or_else(|| "legacy".into());
    let mut caches = ELEMENT_CACHES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap();
    caches.retain(|_, cache| cache.strong_count() > 0);
    caches.insert(runtime_scope, Arc::downgrade(&cache));
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
    let element_count = result
        .nodes
        .iter()
        .filter(|n| n.element_index.is_some())
        .count();
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
    let runtime_scope =
        cua_driver_core::tool::current_dispatch_runtime_scope().unwrap_or_else(|| "legacy".into());
    let cache = ELEMENT_CACHES
        .get()?
        .lock()
        .unwrap()
        .get(&runtime_scope)?
        .upgrade()?;
    let pid_i32 = i32::try_from(pid).ok()?;
    let window_id_u32 = u32::try_from(window_id).ok()?;
    // Retain so a concurrent get_window_state can't free the element between
    // the lookup and element_screen_center (use-after-free → daemon crash).
    let element = cache.get_element_retained(pid_i32, window_id_u32, element_index as usize)?;
    let (sx, sy) = unsafe { element_screen_center(element.as_ptr() as AXUIElementRef)? };

    // Same frame resolution the pixel action rungs use (origin + Retina scale
    // measured off the capture), so a recording marker can never disagree with
    // the click it is annotating. `None` when the window has no live frame —
    // this hook is best-effort and simply records no coordinates then.
    let frame = crate::tools::px_frame::resolve_window_px_frame(window_id_u32).ok()?;
    let wx = (sx - frame.bounds.x) * frame.scale;
    let wy = (sy - frame.bounds.y) * frame.scale;
    Some((wx, wy))
}
