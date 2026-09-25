
use super::*;
use cua_driver_core::window_target::{resolve_pid_window_target, PidWindowTargetResolution};

fn window(xid: u64, pid: u32) -> crate::x11::WindowInfo {
    crate::x11::WindowInfo {
        xid,
        pid: Some(pid),
        app_name: "editor".into(),
        title: format!("Document {xid}"),
        is_on_screen: true,
        z_index: Some(1),
        x: 0,
        y: 0,
        width: 640,
        height: 480,
    }
}

#[test]
fn same_pid_sibling_windows_are_ambiguous() {
    let candidates =
        window_target_candidates_for_pid([window(7, 42), window(8, 42), window(9, 99)], 42);
    assert!(matches!(
        resolve_pid_window_target(candidates),
        PidWindowTargetResolution::Ambiguous(windows)
            if windows.iter().map(|window| window.window_id).collect::<Vec<_>>() == [7, 8]
    ));
}
