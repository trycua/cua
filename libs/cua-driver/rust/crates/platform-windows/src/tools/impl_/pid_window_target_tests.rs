
use super::*;
use cua_driver_core::window_target::{resolve_pid_window_target, PidWindowTargetResolution};

fn window(hwnd: u64, pid: u32) -> crate::win32::WindowInfo {
    crate::win32::WindowInfo {
        hwnd,
        pid,
        title: format!("Document {hwnd}"),
        x: 0,
        y: 0,
        width: 640,
        height: 480,
        is_on_screen: true,
        minimized: false,
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
