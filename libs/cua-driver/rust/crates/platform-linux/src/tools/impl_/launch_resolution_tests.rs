
use super::*;

fn window(class: &str) -> crate::x11::WindowInfo {
    crate::x11::WindowInfo {
        xid: 1,
        pid: Some(7),
        app_name: class.into(),
        title: "t".into(),
        is_on_screen: true,
        z_index: None,
        x: 0,
        y: 0,
        width: 1,
        height: 1,
    }
}

#[test]
fn launch_queries_match_window_classes() {
    assert!(window_matches_launch(
        &window("org.gnome.Nautilus"),
        "nautilus"
    ));
    assert!(window_matches_launch(
        &window("org.gnome.Nautilus"),
        "org.gnome.Nautilus"
    ));
    assert!(window_matches_launch(
        &window("Gnome-terminal"),
        "gnome-terminal"
    ));
    assert!(window_matches_launch(
        &window("Gnome-control-center"),
        "gnome-control-center"
    ));
    assert!(window_matches_launch(
        &window("Org.gnome.Nautilus"),
        "org.gnome.Nautilus.desktop"
    ));
    assert!(window_matches_launch(
        &window("Org.gnome.Nautilus"),
        "Files\u{0}org.gnome.Nautilus\u{0}nautilus --new-window"
    ));
    assert!(window_matches_launch(
        &window("Gnome-control-center"),
        "Settings\u{0}Settings\u{0}gnome-control-center"
    ));
    assert!(!window_matches_launch(&window("Gedit"), "nautilus"));
    assert!(!window_matches_launch(&window(""), "nautilus"));
}

#[test]
fn a_missing_process_counts_as_exited() {
    assert!(process_exited(u32::MAX - 1));
    assert!(!process_exited(std::process::id()));
}

#[test]
fn a_handoff_timeout_is_a_refusal_not_a_null_pid_success() {
    let result = launch_handoff_timeout_result("Nautilus", 4242, 8);
    assert_eq!(
        result.is_error,
        Some(true),
        "a hand-off timeout must be reported as an error, not success"
    );
    let s = result.structured_content.unwrap();
    assert_eq!(s["code"], "launch_handoff_timeout");
    assert_eq!(s["effect"], "refused");
    assert_eq!(s["pid"], Value::Null);
    assert_eq!(s["running"], Value::Null);
    assert_eq!(s["launcher_pid"], 4242);
    assert_eq!(s["waited_secs"], 8);
}
