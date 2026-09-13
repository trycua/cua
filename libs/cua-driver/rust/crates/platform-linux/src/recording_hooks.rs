//! Application-state snapshots used by trajectory recording on Linux.

#[cfg(target_os = "linux")]
pub fn app_state_json_for(window_id: Option<u64>, pid: Option<i64>) -> Option<Vec<u8>> {
    if tokio::runtime::Handle::try_current().is_ok() {
        return std::thread::spawn(move || app_state_json_for_blocking(window_id, pid))
            .join()
            .ok()
            .flatten();
    }
    app_state_json_for_blocking(window_id, pid)
}

#[cfg(target_os = "linux")]
fn app_state_json_for_blocking(window_id: Option<u64>, pid: Option<i64>) -> Option<Vec<u8>> {
    let pid = u32::try_from(pid?).ok()?;
    let window_id = if crate::wayland::is_inject_mode() {
        // Most injected actions already carry the protocol-verified window id.
        // Process-scoped setup calls such as browser_prepare do not, so resolve
        // their single target here instead of classifying required AX evidence
        // as a capture failure.
        match window_id {
            Some(window_id) => window_id,
            None => resolve_window_for_recording(pid, None)?.xid,
        }
    } else {
        resolve_window_for_recording(pid, window_id)?.xid
    };
    let result = if crate::wayland::is_inject_mode() {
        // Evidence capture runs inside the daemon call. Keep it below the
        // transport deadline so an unresponsive renderer cannot block input.
        crate::atspi::walk_tree_for_recording(pid, window_id, std::time::Duration::from_secs(2))
    } else {
        crate::atspi::walk_tree(pid, window_id, None)
    };
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

#[cfg(target_os = "linux")]
pub fn screenshot_for_recording(window_id: Option<u64>, pid: Option<i64>) -> Option<Vec<u8>> {
    if crate::wayland::is_wayland() {
        // Wayland surface ids are connection-scoped, so a later recording hook
        // cannot safely re-attest the action call's per-window crop. Preserve
        // ownership by recording the compositor's complete rendered output.
        return crate::wayland::screenshot_display_dispatch().ok();
    }
    if let Some(window_id) = window_id {
        crate::wayland::screenshot_dispatch(window_id).ok()
    } else if let Some(pid) = pid.and_then(|pid| u32::try_from(pid).ok()) {
        let windows = crate::wayland::list_windows_dispatch(Some(pid));
        windows
            .first()
            .and_then(|window| crate::wayland::screenshot_dispatch(window.xid).ok())
    } else {
        crate::capture::screenshot_display_bytes().ok()
    }
}

#[cfg(target_os = "linux")]
pub fn element_window_local_xy(window_id: u64, pid: i64, element_index: u32) -> Option<(f64, f64)> {
    if tokio::runtime::Handle::try_current().is_ok() {
        return std::thread::spawn(move || {
            element_window_local_xy_blocking(window_id, pid, element_index)
        })
        .join()
        .ok()
        .flatten();
    }
    element_window_local_xy_blocking(window_id, pid, element_index)
}

#[cfg(target_os = "linux")]
fn element_window_local_xy_blocking(
    window_id: u64,
    pid: i64,
    element_index: u32,
) -> Option<(f64, f64)> {
    let pid = u32::try_from(pid).ok()?;
    let (screen_x, screen_y, width, height) =
        crate::atspi::get_element_bounds_for_window(pid, window_id, element_index as usize).ok()?;
    let window = resolve_window_for_recording(pid, Some(window_id))?;
    if crate::wayland::is_wayland() && crate::wayland::hyprland::is_session() {
        let (display_width, display_height, _) = crate::wayland::hyprland::screen_size().ok()?;
        // This hook retains the whole output, not the window-local tool image.
        return hyprland_recording_point(
            (screen_x, screen_y, width, height),
            (window.x, window.y, window.width, window.height),
            (display_width, display_height),
        );
    }
    Some((
        f64::from(screen_x) - f64::from(window.x) + f64::from(width) / 2.0,
        f64::from(screen_y) - f64::from(window.y) + f64::from(height) / 2.0,
    ))
}

#[cfg(any(target_os = "linux", test))]
fn hyprland_recording_point(
    element: (i32, i32, u32, u32),
    window: (i32, i32, u32, u32),
    display: (u32, u32),
) -> Option<(f64, f64)> {
    let (x, y, width, height) = element;
    if width == 0 || height == 0 || window.2 == 0 || window.3 == 0 {
        return None;
    }
    let cx = f64::from(x) + f64::from(width) / 2.0;
    let cy = f64::from(y) + f64::from(height) / 2.0;
    hyprland_output_point((cx, cy), window, display)
}

#[cfg(any(target_os = "linux", test))]
fn hyprland_output_point(
    (cx, cy): (f64, f64),
    window: (i32, i32, u32, u32),
    display: (u32, u32),
) -> Option<(f64, f64)> {
    let in_window = cx >= f64::from(window.0)
        && cy >= f64::from(window.1)
        && cx < f64::from(window.0) + f64::from(window.2)
        && cy < f64::from(window.1) + f64::from(window.3);
    let in_display =
        cx >= 0.0 && cy >= 0.0 && cx < f64::from(display.0) && cy < f64::from(display.1);
    (in_window && in_display).then_some((cx, cy))
}

#[cfg(target_os = "linux")]
pub fn hyprland_pixel_recording_point(
    window_id: Option<u64>,
    pid: Option<i64>,
    x: f64,
    y: f64,
) -> Option<(f64, f64)> {
    let (width, height, _) = crate::wayland::hyprland::screen_size().ok()?;
    match (window_id, pid) {
        (Some(window_id), Some(pid)) => {
            let pid = u32::try_from(pid).ok()?;
            let window = resolve_window_for_recording(pid, Some(window_id))?;
            if window.pid != Some(pid) {
                return None;
            }
            hyprland_output_point(
                (f64::from(window.x) + x, f64::from(window.y) + y),
                (window.x, window.y, window.width, window.height),
                (width, height),
            )
        }
        (None, None) => hyprland_output_point((x, y), (0, 0, width, height), (width, height)),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
fn resolve_window_for_recording(
    pid: u32,
    window_id: Option<u64>,
) -> Option<crate::x11::WindowInfo> {
    let windows = crate::wayland::list_windows_dispatch(Some(pid));
    if crate::wayland::is_wayland() && crate::wayland::hyprland::is_session() {
        return match window_id {
            Some(id) => windows.into_iter().find(|window| window.xid == id),
            None if windows.len() == 1 => windows.into_iter().next(),
            _ => None,
        };
    }
    if crate::wayland::is_wayland() {
        // Foreign-toplevel protocol object ids are scoped to one Wayland
        // connection. Recording hooks open a fresh connection, so re-resolve
        // the target by pid instead of comparing an id from the action call.
        windows.into_iter().next()
    } else if let Some(window_id) = window_id {
        windows.into_iter().find(|window| window.xid == window_id)
    } else {
        windows.into_iter().next()
    }
}

#[cfg(not(target_os = "linux"))]
pub fn app_state_json_for(_window_id: Option<u64>, _pid: Option<i64>) -> Option<Vec<u8>> {
    None
}

#[cfg(not(target_os = "linux"))]
pub fn screenshot_for_recording(_window_id: Option<u64>, _pid: Option<i64>) -> Option<Vec<u8>> {
    None
}

#[cfg(not(target_os = "linux"))]
pub fn element_window_local_xy(
    _window_id: u64,
    _pid: i64,
    _element_index: u32,
) -> Option<(f64, f64)> {
    None
}

#[cfg(test)]
mod tests {
    use super::{hyprland_output_point, hyprland_recording_point};

    #[test]
    fn hyprland_marker_uses_retained_output_coordinates() {
        assert_eq!(
            hyprland_recording_point((130, 250, 20, 30), (100, 200, 940, 780), (1920, 1080)),
            Some((140.0, 265.0))
        );
    }

    #[test]
    fn hyprland_marker_does_not_invent_an_offscreen_point() {
        for element in [(10, 800, 20, 20), (-30, 10, 20, 20), (10, 10, 0, 20)] {
            assert_eq!(
                hyprland_recording_point(element, (0, 0, 940, 780), (1920, 1080)),
                None
            );
        }
        assert_eq!(
            hyprland_recording_point((1900, 10, 80, 20), (1800, 0, 940, 780), (1920, 1080)),
            None
        );
    }

    #[test]
    fn hyprland_pixel_marker_keeps_fractional_output_coordinates() {
        assert_eq!(
            hyprland_output_point((130.5, 250.25), (100, 200, 940, 780), (1920, 1080)),
            Some((130.5, 250.25))
        );
        for point in [(f64::NAN, 200.0), (100.0, f64::INFINITY), (99.0, 200.0)] {
            assert_eq!(
                hyprland_output_point(point, (100, 200, 940, 780), (1920, 1080)),
                None
            );
        }
    }
}
