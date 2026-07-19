//! Transport-level contract tests for per-session capture scope.

#![cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]

use cua_driver_testkit::RawDriver;
use serde_json::{json, Value};

fn call(driver: &mut RawDriver, id: u64, name: &str, arguments: Value) -> Value {
    driver.send(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": {"name": name, "arguments": arguments}
    }));
    driver.recv()
}

fn code(response: &Value) -> Option<&str> {
    response["result"]["structuredContent"]["code"].as_str()
}

#[test]
fn policies_are_isolated_immutable_and_enforced_over_mcp() {
    let mut driver = RawDriver::spawn().expect("spawn source-built driver");
    driver.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    driver.recv();

    for (id, session, scope) in [
        (2, "scope-auto", "auto"),
        (3, "scope-window", "window"),
        (4, "scope-desktop", "desktop"),
    ] {
        let response = call(
            &mut driver,
            id,
            "start_session",
            json!({"session": session, "capture_scope": scope}),
        );
        assert_eq!(
            response["result"]["structuredContent"]["capture_scope"], scope,
            "start_session failed: {response}"
        );
    }

    let auto_desktop = call(
        &mut driver,
        5,
        "get_desktop_state",
        json!({"session": "scope-auto"}),
    );
    assert_eq!(code(&auto_desktop), Some("desktop_escalation_required"));

    let window_desktop = call(
        &mut driver,
        6,
        "get_desktop_state",
        json!({"session": "scope-window"}),
    );
    assert_eq!(code(&window_desktop), Some("desktop_scope_disabled"));

    let window_screen_size = call(
        &mut driver,
        60,
        "get_screen_size",
        json!({"session": "scope-window"}),
    );
    assert_eq!(code(&window_screen_size), Some("desktop_scope_disabled"));
    let desktop_screen_size = call(
        &mut driver,
        61,
        "get_screen_size",
        json!({"session": "scope-desktop"}),
    );
    assert!(
        code(&desktop_screen_size).is_none(),
        "desktop scope should permit global screen geometry: {desktop_screen_size}"
    );

    let desktop_window = call(
        &mut driver,
        7,
        "get_window_state",
        json!({"session": "scope-desktop"}),
    );
    assert_eq!(code(&desktop_window), Some("window_scope_disabled"));

    let escalated = call(
        &mut driver,
        8,
        "escalate_session",
        json!({
            "session": "scope-auto",
            "reason": "foreground_ineffective",
            "detail": "window ladder exhausted"
        }),
    );
    assert_eq!(
        escalated["result"]["structuredContent"]["effective_scope"],
        "desktop"
    );
    let auto_window = call(
        &mut driver,
        9,
        "get_window_state",
        json!({"session": "scope-auto"}),
    );
    assert_eq!(code(&auto_window), Some("window_scope_disabled"));

    let conflict = call(
        &mut driver,
        10,
        "start_session",
        json!({"session": "scope-window", "capture_scope": "desktop"}),
    );
    assert_eq!(code(&conflict), Some("session_policy_conflict"));

    let ended = call(
        &mut driver,
        11,
        "end_session",
        json!({"session": "scope-window"}),
    );
    assert_eq!(ended["result"]["structuredContent"]["active"], false);
    let revived = call(
        &mut driver,
        12,
        "start_session",
        json!({"session": "scope-window", "capture_scope": "desktop"}),
    );
    assert_eq!(revived["result"]["structuredContent"]["revived"], true);
    assert_eq!(
        revived["result"]["structuredContent"]["capture_scope"],
        "desktop"
    );
}

#[test]
fn persistent_capture_scope_key_is_retired() {
    let mut driver = RawDriver::spawn().expect("spawn source-built driver");
    driver.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    driver.recv();
    let response = call(
        &mut driver,
        2,
        "set_config",
        json!({"key": "capture_scope", "value": "desktop"}),
    );
    assert_eq!(code(&response), Some("config_key_retired"));
    assert_eq!(
        response["result"]["structuredContent"]["replacement"],
        "start_session.capture_scope"
    );
}
