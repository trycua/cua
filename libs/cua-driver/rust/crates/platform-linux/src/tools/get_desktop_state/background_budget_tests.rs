use super::*;

#[test]
fn synthetic_pointer_drop_detection_covers_gtk_vcl_and_qt() {
    assert!(maps_indicate_synthetic_pointer_dropped(
        "7f /usr/lib/libgtk-3.so.0\n"
    ));
    assert!(maps_indicate_synthetic_pointer_dropped(
        "7f /usr/lib/libgtk-4.so.1\n"
    ));
    assert!(maps_indicate_synthetic_pointer_dropped(
        "7f /opt/libreoffice/program/libmergedlo.so\n"
    ));
    assert!(maps_indicate_synthetic_pointer_dropped(
        "7f /usr/lib/libvcllo.so\n"
    ));
    assert!(maps_indicate_synthetic_pointer_dropped(
        "7f /usr/lib/libQt5Gui.so.5\n"
    ));
    assert!(!maps_indicate_synthetic_pointer_dropped(
        "7f /usr/lib/libgtk-x11-2.0.so.0\n"
    ));
    assert!(!maps_indicate_synthetic_pointer_dropped(
        "7f /usr/lib/libX11.so.6\n"
    ));
}

#[test]
fn desktop_window_lines_expose_pid_and_window_id() {
    let windows = vec![crate::x11::WindowInfo {
        xid: 0x2e00003,
        pid: Some(4321),
        app_name: "libreoffice".into(),
        title: "Untitled 1 - LibreOffice Calc".into(),
        is_on_screen: true,
        z_index: None,
        x: 0,
        y: 27,
        width: 1920,
        height: 1053,
    }];
    let text = desktop_window_lines(&windows, 1.0);
    assert!(text.contains("pid=4321 window_id=48234499"));
    assert!(text.contains("LibreOffice Calc"));
    assert!(text.contains("app=libreoffice"));
    assert!(text.contains("get_window_state(pid, window_id)"));
    assert!(desktop_window_lines(&[], 1.0).contains("none"));
    // A downsized desktop screenshot reports window rects in ITS pixels.
    let scaled = desktop_window_lines(&windows, 1.5);
    assert!(scaled.contains("1280x702 at (0,18)"), "{scaled}");
}
