use crate::ax::bindings::{element_screen_center, AXUIElementRef};
use crate::ax::cache::{CachedSnapshot, ElementCache};
use cua_driver_core::element_cache::{current_runtime_cache, register_runtime_cache};
use cua_driver_core::element_token::ResolvedElement;
use cua_driver_core::tool_args::ArgsExt;
use serde_json::Value;
use std::sync::Arc;

pub fn set_element_cache(cache: Arc<ElementCache>) {
    register_runtime_cache(&cache);
}

pub fn app_state_json_for(window_id: Option<u64>, pid: Option<i64>) -> Option<Vec<u8>> {
    let pid = i32::try_from(pid?).ok()?;
    let resolved_wid = match window_id {
        Some(w) => u32::try_from(w).ok()?,
        None => crate::windows::resolve_main_window_id(pid).ok()?,
    };
    let result = crate::ax::tree::walk_tree(pid, Some(resolved_wid), None);
    let _payload = CachedSnapshot::from_nodes(&result.nodes);
    let element_count = result
        .nodes
        .iter()
        .filter(|node| node.element_index.is_some())
        .count();
    let payload = serde_json::json!({
        "pid": pid,
        "window_id": resolved_wid,
        "element_count": element_count,
        "tree_markdown": result.tree_markdown,
    });
    serde_json::to_vec_pretty(&payload).ok()
}

pub fn element_window_local_xy(
    pid: i64,
    args: &Value,
    capture_point: bool,
) -> Option<(u64, Option<(f64, f64)>)> {
    let cache = current_runtime_cache::<CachedSnapshot>()?;
    let target = cache
        .resolve_element_args(
            i32::try_from(pid).ok()?,
            args.opt_u64("element_index").map(|index| index as usize),
            args.get("element_token").and_then(Value::as_str),
            args.get("snapshot_id").and_then(Value::as_str),
            args.opt_u64("window_id"),
            "recording",
        )
        .ok()?;
    let ResolvedElement::Element {
        window_id: Some(window_id),
        element,
        ..
    } = target
    else {
        return None;
    };
    let point = capture_point
        .then(|| unsafe { element_screen_center(element.as_ptr() as AXUIElementRef) })
        .flatten()
        .and_then(|(sx, sy)| {
            let frame =
                crate::tools::px_frame::resolve_window_px_frame(u32::try_from(window_id).ok()?)
                    .ok()?;
            Some((
                (sx - frame.bounds.x) * frame.scale,
                (sy - frame.bounds.y) * frame.scale,
            ))
        });
    Some((window_id, point))
}
