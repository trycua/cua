use platform_linux::wayland::kwin_helper::{
    available, correlate_atspi_window, parse_snapshot, require_active_target, with_focused_window,
    CorrelationError,
};
use platform_linux::x11::WindowInfo;

fn atspi_window(pid: u32, x: i32, y: i32, width: u32, height: u32) -> WindowInfo {
    WindowInfo {
        xid: 99,
        pid: Some(pid),
        app_name: "org.example.Editor".into(),
        title: "Editor".into(),
        is_on_screen: true,
        z_index: None,
        x,
        y,
        width,
        height,
    }
}

#[test]
fn parses_versioned_kwin_snapshot() {
    let snapshot = parse_snapshot(
        r#"[{"token":41,"pid":1200,"x":100,"y":80,"w":800,"h":600,"active":false,"minimized":false,"stacking":3}]"#,
    )
    .expect("valid KWin helper snapshot");

    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot[0].token, 41);
    assert_eq!(snapshot[0].pid, 1200);
    assert_eq!((snapshot[0].x, snapshot[0].y), (100, 80));
}

#[test]
fn preserves_title_with_apostrophe_from_dbus_string() {
    let snapshot = parse_snapshot(
        r#"[{"token":41,"pid":1200,"x":100,"y":80,"w":800,"h":600,"active":false,"minimized":false,"stacking":3,"title":"owner's editor","app_id":"org.example.Editor"}]"#,
    )
    .expect("apostrophe in the decoded D-Bus string must remain valid JSON");

    assert_eq!(snapshot[0].title, "owner's editor");
}

#[test]
fn parses_snapshot_when_title_contains_closing_bracket() {
    let snapshot = parse_snapshot(
        r#"[{"token":41,"pid":1200,"x":100,"y":80,"w":800,"h":600,"active":false,"minimized":false,"stacking":3,"title":"A ] title","app_id":"org.example.Editor"}]"#,
    )
    .expect("title brackets must not break snapshot parsing");

    assert_eq!(snapshot[0].title, "A ] title");
}

#[test]
fn parses_fractional_plasma_geometry() {
    let snapshot = parse_snapshot(
        r#"[{"token":41,"pid":1200,"x":100,"y":80,"w":332,"h":99.33333333333336,"active":false,"minimized":false,"stacking":3}]"#,
    )
    .expect("finite fractional geometry is valid metadata");

    assert_eq!(snapshot[0].height, 99);
}

#[test]
fn correlates_one_same_pid_window_with_bounded_frame_delta() {
    let snapshot = parse_snapshot(
        r#"[{"token":41,"pid":1200,"x":100,"y":80,"w":800,"h":600,"active":false,"minimized":false,"stacking":3}]"#,
    )
    .unwrap();

    let target = correlate_atspi_window(&atspi_window(1200, 104, 112, 792, 560), &snapshot)
        .expect("one bounded geometry match");
    assert_eq!(target.token, 41);
}

#[test]
fn rejects_duplicate_same_pid_geometry_candidates() {
    let snapshot = parse_snapshot(
        r#"[{"token":41,"pid":1200,"x":100,"y":80,"w":800,"h":600,"active":false,"minimized":false,"stacking":3},{"token":42,"pid":1200,"x":102,"y":82,"w":798,"h":598,"active":false,"minimized":false,"stacking":4}]"#,
    )
    .unwrap();

    assert_eq!(
        correlate_atspi_window(&atspi_window(1200, 104, 112, 792, 560), &snapshot),
        Err(CorrelationError::Ambiguous)
    );
}

#[test]
fn refuses_minimized_same_pid_window() {
    let snapshot = parse_snapshot(
        r#"[{"token":41,"pid":1200,"x":100,"y":80,"w":800,"h":600,"active":false,"minimized":true,"stacking":3}]"#,
    )
    .unwrap();

    assert_eq!(
        correlate_atspi_window(&atspi_window(1200, 104, 112, 792, 560), &snapshot),
        Err(CorrelationError::NoMatch)
    );
}

#[test]
fn rejects_zero_token() {
    assert!(parse_snapshot(
        r#"[{"token":0,"pid":1200,"x":0,"y":0,"w":10,"h":10,"active":false,"minimized":false,"stacking":0}]"#,
    )
    .is_none());
}

#[test]
fn rejects_duplicate_tokens() {
    assert!(parse_snapshot(
        r#"[{"token":41,"pid":1200,"x":0,"y":0,"w":10,"h":10,"active":false,"minimized":false,"stacking":0},{"token":41,"pid":1300,"x":20,"y":20,"w":10,"h":10,"active":false,"minimized":false,"stacking":1}]"#,
    )
    .is_none());
}

#[test]
fn rejects_multiple_active_records() {
    assert!(parse_snapshot(
        r#"[{"token":41,"pid":1200,"x":0,"y":0,"w":10,"h":10,"active":true,"minimized":false,"stacking":0},{"token":42,"pid":1300,"x":20,"y":20,"w":10,"h":10,"active":true,"minimized":false,"stacking":1}]"#,
    )
    .is_none());
}

#[test]
fn refuses_when_a_different_token_is_active() {
    let snapshot = parse_snapshot(
        r#"[{"token":41,"pid":1200,"x":100,"y":80,"w":800,"h":600,"active":false,"minimized":false,"stacking":3},{"token":42,"pid":1300,"x":0,"y":0,"w":800,"h":600,"active":true,"minimized":false,"stacking":4}]"#,
    )
    .unwrap();

    assert_eq!(
        require_active_target(&snapshot, 41),
        Err(CorrelationError::WrongActiveTarget)
    );
}

#[test]
fn raw_kwin_input_is_refused_before_dispatch() {
    assert!(
        !available(),
        "focus-bound KWin input must not be advertised as safe"
    );

    let called = std::cell::Cell::new(false);
    let error = with_focused_window(1200, 41, || {
        called.set(true);
        Ok(())
    })
    .expect_err("focus-bound KWin input must refuse");

    assert!(
        !called.get(),
        "input body must not run on the focus-only KWin path"
    );
    assert!(error.to_string().contains("target-bound KWin input path"));
}
