use super::*;

#[test]
fn record_has_bounds_and_flat_legacy_fields() {
    let w = crate::x11::WindowInfo {
        xid: 42,
        pid: Some(1234),
        app_name: "example-app".to_owned(),
        title: "Example".to_owned(),
        is_on_screen: true,
        z_index: Some(3),
        x: 10,
        y: 20,
        width: 300,
        height: 400,
    };
    let rec = window_record_json(&w);

    // Canonical cross-platform shape: nested `bounds` object.
    let bounds = rec
        .get("bounds")
        .expect("record must carry a `bounds` object");
    assert_eq!(bounds["x"], json!(10));
    assert_eq!(bounds["y"], json!(20));
    assert_eq!(bounds["width"], json!(300));
    assert_eq!(bounds["height"], json!(400));

    // Legacy alias: flat fields must still be present.
    assert_eq!(rec["x"], json!(10));
    assert_eq!(rec["y"], json!(20));
    assert_eq!(rec["width"], json!(300));
    assert_eq!(rec["height"], json!(400));

    // Cross-platform companions.
    assert_eq!(rec["app_name"], json!("example-app"));
    assert_eq!(rec["is_on_screen"], json!(true));
    assert_eq!(rec["z_index"], json!(3));
    assert_eq!(rec["window_id"], json!(42));
    assert_eq!(rec["title"], json!("Example"));
}

#[test]
fn unavailable_wayland_order_serializes_as_null() {
    let w = crate::x11::WindowInfo {
        xid: 43,
        pid: Some(1234),
        app_name: "native-wayland-app".to_owned(),
        title: "Example".to_owned(),
        is_on_screen: true,
        z_index: None,
        x: 0,
        y: 0,
        width: 300,
        height: 400,
    };

    assert_eq!(window_record_json(&w)["z_index"], Value::Null);
}

#[test]
fn desktop_discovery_retains_only_visible_windows_with_live_or_unknown_owners() {
    let window = |xid, pid, is_on_screen| crate::x11::WindowInfo {
        xid,
        pid,
        app_name: "native-wayland-app".to_owned(),
        title: "Example".to_owned(),
        is_on_screen,
        z_index: None,
        x: 10,
        y: 20,
        width: 300,
        height: 400,
    };
    let windows = visible_live_desktop_windows(vec![
        window(1, Some(std::process::id()), true),
        window(2, Some(std::process::id()), false),
        window(3, Some(u32::MAX), true),
        window(4, None, true),
        window(5, None, false),
    ]);

    assert_eq!(
        windows.iter().map(|w| w.xid).collect::<Vec<_>>(),
        vec![1, 4]
    );
    let record = window_record_json(&windows[0]);
    assert_eq!(
        record["bounds"],
        json!({"x": 10, "y": 20, "width": 300, "height": 400})
    );
    assert_eq!(record["app_name"], json!("native-wayland-app"));
    assert_eq!(record["is_on_screen"], json!(true));
    assert_eq!(record["z_index"], Value::Null);
}

#[test]
fn chromium_launch_detection_uses_executable_basename() {
    assert!(chromium_family_program("/usr/bin/google-chrome-stable"));
    assert!(chromium_family_program("CuaTestHarness.Electron"));
    assert!(chromium_family_program("chromium-browser"));
    assert!(!chromium_family_program("/usr/bin/gnome-text-editor"));
}
