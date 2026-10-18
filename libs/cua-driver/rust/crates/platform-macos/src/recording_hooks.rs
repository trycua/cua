use crate::ax::bindings::{element_screen_center, AXUIElementRef};
use crate::ax::snapshot::{AxSnapshot, Snapshots};
use cua_driver_core::element_token::ResolvedElement;
use cua_driver_core::snapshot_store::{current_runtime_store, register_runtime_store};
use serde_json::Value;
use std::sync::Arc;

pub fn set_snapshots(cache: Arc<Snapshots>) {
    register_runtime_store(&cache);
}

/// Per-turn application state for trajectory recording. The walk shares the
/// `get_window_state` budget semantics: it stops when `budget.timeout_ms` runs
/// out and reports the shared walk fields so the turn evidence can say the
/// tree is partial.
pub fn app_state_json_for(
    window_id: Option<u64>,
    pid: Option<i64>,
    budget: cua_driver_core::recording::StateCaptureBudget,
) -> Option<Vec<u8>> {
    let pid = i32::try_from(pid?).ok()?;
    let resolved_wid = match window_id {
        Some(w) => u32::try_from(w).ok()?,
        None => crate::windows::resolve_main_window_id(pid).ok()?,
    };
    let result = crate::ax::tree::walk_tree_budgeted(
        pid,
        Some(resolved_wid),
        None,
        crate::ax::tree::DEFAULT_MAX_DEPTH,
        cua_driver_core::walk_budget::WalkBudget::new(
            budget.timeout_ms,
            crate::ax::tree::DEFAULT_MAX_ELEMENTS,
        ),
    );
    let _payload = AxSnapshot::from_nodes(&result.nodes);
    let element_count = result
        .nodes
        .iter()
        .filter(|node| node.element_index.is_some())
        .count();
    let mut payload = serde_json::json!({
        "pid": pid,
        "window_id": resolved_wid,
        "element_count": element_count,
        "tree_markdown": result.tree_markdown,
    });
    result.walk.apply(&mut payload);
    serde_json::to_vec_pretty(&payload).ok()
}

pub fn element_window_local_xy(
    pid: i64,
    args: &Value,
    capture_point: bool,
) -> Option<(u64, Option<(f64, f64)>)> {
    let cache = current_runtime_store::<AxSnapshot>()?;
    let target = cache.resolve(i32::try_from(pid).ok()?, args).ok()?;
    let ResolvedElement::Element {
        window_id, element, ..
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
