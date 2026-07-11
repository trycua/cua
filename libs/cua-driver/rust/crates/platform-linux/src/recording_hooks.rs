//! Application-state snapshots used by trajectory recording on Linux.

#[cfg(target_os = "linux")]
pub fn app_state_json_for(window_id: Option<u64>, pid: Option<i64>) -> Option<Vec<u8>> {
    let pid = u32::try_from(pid?).ok()?;
    let window_id = window_id.or_else(|| {
        crate::wayland::list_windows_dispatch(Some(pid))
            .first()
            .map(|window| window.xid)
    })?;
    if !crate::wayland::list_windows_dispatch(Some(pid))
        .iter()
        .any(|window| window.xid == window_id)
    {
        return None;
    }
    let result = crate::atspi::walk_tree(pid, window_id, None);
    if result.nodes.is_empty() || result.tree_markdown.trim().is_empty() {
        return None;
    }
    let element_count = result
        .nodes
        .iter()
        .filter(|node| node.element_index.is_some())
        .count();
    let payload = serde_json::json!({
        "pid": pid,
        "window_id": window_id,
        "element_count": element_count,
        "tree_markdown": result.tree_markdown,
    });
    serde_json::to_vec_pretty(&payload).ok()
}

#[cfg(not(target_os = "linux"))]
pub fn app_state_json_for(_window_id: Option<u64>, _pid: Option<i64>) -> Option<Vec<u8>> {
    None
}
