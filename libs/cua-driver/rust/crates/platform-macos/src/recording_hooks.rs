use crate::ax::element_resolver::FreshAxElements;
pub use cua_driver_core::element_token::recording_target as element_window_local_xy;

pub fn app_state_json_for(window_id: Option<u64>, pid: Option<i64>) -> Option<Vec<u8>> {
    let pid = i32::try_from(pid?).ok()?;
    let resolved_wid = match window_id {
        Some(w) => u32::try_from(w).ok()?,
        None => crate::windows::resolve_main_window_id(pid).ok()?,
    };
    let result = crate::ax::tree::walk_tree(pid, Some(resolved_wid), None);
    let _payload = FreshAxElements::from_nodes(&result.nodes);
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

pub fn capture_desktop_click_point(screen_x: f64, screen_y: f64) {
    if let Some((window, pid)) = cua_driver_core::recording::dispatch_click_target() {
        if let (Ok(window), Ok(pid)) = (u32::try_from(window), i32::try_from(pid)) {
            capture_click_point(pid, window, screen_x, screen_y);
        }
    }
}

pub fn capture_click_point(pid: i32, window_id: u32, screen_x: f64, screen_y: f64) {
    cua_driver_core::recording::capture_dispatch_click_target(window_id.into(), pid.into(), || {
        let frame = crate::tools::px_frame::resolve_window_px_frame(window_id).ok()?;
        Some((
            crate::capture::screenshot_window_bytes(window_id).ok()?,
            (screen_x - frame.bounds.x) * frame.scale,
            (screen_y - frame.bounds.y) * frame.scale,
        ))
    });
}
