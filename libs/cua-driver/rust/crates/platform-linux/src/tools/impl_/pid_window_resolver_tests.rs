use super::*;

fn win(xid: u64, z: Option<usize>, on_screen: bool, w: u32, h: u32) -> crate::x11::WindowInfo {
    crate::x11::WindowInfo {
        xid,
        pid: Some(7),
        app_name: "gimp".into(),
        title: format!("w{xid}"),
        is_on_screen: on_screen,
        z_index: z,
        x: 0,
        y: 0,
        width: w,
        height: h,
    }
}

#[test]
fn point_resolver_prefers_the_topmost_window_by_stacking_position() {
    // Main window listed last (as `_NET_CLIENT_LIST_STACKING` bottom-up
    // enumeration does when a dialog sits above it); the dialog wins.
    let dialog = crate::x11::WindowInfo {
        z_index: Some(5),
        ..win(2, None, true, 400, 300)
    };
    let main = crate::x11::WindowInfo {
        z_index: Some(3),
        ..win(1, None, true, 1920, 1080)
    };
    assert_eq!(
        topmost_window_at(&[main.clone(), dialog.clone()], 7, 100.0, 100.0),
        Some(2)
    );
    assert_eq!(topmost_window_at(&[dialog, main], 7, 100.0, 100.0), Some(2));
}

#[test]
fn point_resolver_ignores_windows_without_a_stacking_position_when_one_has_it() {
    let ranked = win(2, Some(0), true, 400, 300);
    let unranked = win(1, None, true, 1920, 1080);
    assert_eq!(
        topmost_window_at(&[ranked, unranked], 7, 10.0, 10.0),
        Some(2)
    );
}

#[test]
fn pid_only_resolver_orders_focus_then_dialog_then_active_then_largest() {
    let main = win(1, Some(1), true, 1920, 1080);
    let dialog = win(2, Some(2), true, 400, 300);
    let dock = win(3, Some(0), true, 200, 900);
    let hidden = win(4, None, false, 3000, 3000);
    let windows = vec![main, dialog, dock, hidden];
    let transient = |xid: u64| (xid == 2).then_some(1u64);
    // 1. core focus inside the pid wins outright (even over a dialog).
    assert_eq!(
        crate::x11::pick_pid_window(&windows, Some(3), transient, Some(1)),
        Some(3)
    );
    // 2. no focus in the pid: its topmost transient dialog.
    assert_eq!(
        crate::x11::pick_pid_window(&windows, None, transient, Some(1)),
        Some(2)
    );
    // 3. no dialog: the WM's active window when it is the pid's.
    assert_eq!(
        crate::x11::pick_pid_window(&windows, None, |_| None, Some(3)),
        Some(3)
    );
    // 4. otherwise the largest mapped toplevel (never the unmapped one).
    assert_eq!(
        crate::x11::pick_pid_window(&windows, None, |_| None, None),
        Some(1)
    );
    assert_eq!(
        crate::x11::pick_pid_window(&windows, None, |_| None, Some(99)),
        Some(1)
    );
    // A focused window that is not the pid's is ignored.
    assert_eq!(
        crate::x11::pick_pid_window(&windows, Some(42), |_| None, None),
        Some(1)
    );
}

#[test]
fn pid_only_resolver_skips_unmapped_dialogs() {
    let main = win(1, Some(1), true, 800, 600);
    let unmapped_dialog = win(2, Some(2), false, 400, 300);
    let windows = vec![main, unmapped_dialog];
    assert_eq!(
        crate::x11::pick_pid_window(&windows, None, |xid| (xid == 2).then_some(1u64), None),
        Some(1)
    );
}
