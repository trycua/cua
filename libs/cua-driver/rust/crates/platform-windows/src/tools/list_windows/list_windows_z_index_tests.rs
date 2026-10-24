use super::{drop_own_windows, exact_window_ownership_result, z_index_from_front_to_back};
use crate::win32::WindowInfo;

#[test]
fn explicit_pid_hwnd_guard_refuses_wrong_or_stale_owners() {
    assert!(exact_window_ownership_result(42, 7, Some(42)).is_ok());

    let wrong = exact_window_ownership_result(42, 7, Some(99)).unwrap_err();
    assert_eq!(wrong.is_error, Some(true));
    assert_eq!(
        wrong.structured_content.as_ref().unwrap()["code"],
        "window_target_mismatch"
    );
    assert_eq!(wrong.structured_content.as_ref().unwrap()["owner_pid"], 99);

    let stale = exact_window_ownership_result(42, 7, None).unwrap_err();
    assert_eq!(
        stale.structured_content.as_ref().unwrap()["code"],
        "window_target_not_found"
    );
}

fn window(hwnd: u64, pid: u32) -> WindowInfo {
    WindowInfo {
        hwnd,
        pid,
        title: format!("window {hwnd}"),
        x: 0,
        y: 0,
        width: 1280,
        height: 800,
        is_on_screen: true,
        minimized: false,
    }
}

#[test]
fn the_drivers_own_windows_are_dropped_and_the_rest_keep_contiguous_z() {
    let mut windows = vec![window(1, 7), window(2, 42), window(3, 7), window(4, 9)];
    drop_own_windows(&mut windows, 7);
    assert_eq!(windows.iter().map(|w| w.hwnd).collect::<Vec<_>>(), [2, 4]);
    let z: Vec<_> = (0..windows.len())
        .map(|i| z_index_from_front_to_back(windows.len(), i))
        .collect();
    assert_eq!(z, [1, 0]);
}
