
use super::{is_windowless_desktop_action, GetDesktopStateTool};
use cua_driver_core::tool::Tool;
use serde_json::json;

// ── is_windowless_desktop_action ──────────────────────────────────────────

#[test]
fn windowless_true_for_xy_under_desktop_scope_click_shape() {
    // Click arg shape: {x, y}.
    assert!(is_windowless_desktop_action(
        &json!({"x": 10, "y": 20, "scope": "desktop"})
    ));
}

#[test]
fn windowless_true_for_xy_under_desktop_scope_scroll_shape() {
    // Scroll arg shape: {direction, x, y}.
    assert!(is_windowless_desktop_action(&json!({
        "direction": "down", "x": 10, "y": 20, "scope": "desktop"
    })));
}

#[test]
fn windowless_false_when_pid_present() {
    assert!(!is_windowless_desktop_action(&json!({
        "x": 10, "y": 20, "pid": 5, "scope": "desktop"
    })));
}

#[test]
fn windowless_false_when_window_id_present() {
    assert!(!is_windowless_desktop_action(&json!({
        "x": 10, "y": 20, "window_id": 99, "scope": "desktop"
    })));
}

#[test]
fn windowless_false_under_window_scope() {
    assert!(!is_windowless_desktop_action(&json!({
        "x": 10, "y": 20, "scope": "window"
    })));
}

#[test]
fn windowless_false_when_xy_missing() {
    assert!(!is_windowless_desktop_action(
        &json!({"x": 10, "scope": "desktop"})
    ));
    assert!(!is_windowless_desktop_action(
        &json!({"y": 20, "scope": "desktop"})
    ));
    assert!(!is_windowless_desktop_action(&json!({"scope": "desktop"})));
    // Non-numeric x/y must not qualify.
    assert!(!is_windowless_desktop_action(&json!({
        "x": "10", "y": "20", "scope": "desktop"
    })));
}

// ── get_desktop_state schema ──────────────────────────────────────────────

#[test]
fn get_desktop_state_schema_rejects_window_scoped_fields() {
    let tool = GetDesktopStateTool {
        state: super::ToolState::new(None),
    };
    // The portable get_desktop_state contract pins the accepted fields and
    // annotations; it cannot see a window-scoped field added to live.
    let d = tool.def();
    let props = d.input_schema["properties"].as_object().unwrap();
    assert!(!props.contains_key("pid"), "must not accept pid");
    assert!(
        !props.contains_key("window_id"),
        "must not accept window_id"
    );
    assert_eq!(d.input_schema["additionalProperties"], json!(false));
}
