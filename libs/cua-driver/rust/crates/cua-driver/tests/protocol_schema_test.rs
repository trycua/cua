//! Pure `tools/list` schema-shape assertions.
//!
//! These never invoke a tool — they only inspect the advertised inputSchemas:
//! that `type_text_chars` is hidden, the `list_windows.on_screen_only` knob, the
//! `set_agent_cursor_motion` Bezier knobs, delivery and scope enums, and the
//! `set_config.capture_mode` enum and the per-session capture-scope contract.

#![cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]

use cua_driver_testkit::RawDriver;

#[test]
fn tools_list_schema_shape() {
    //! `type_text_chars` is an accepted-but-deprecated invoke alias that stays
    //! HIDDEN from tools/list on every platform (the live Windows roster carries
    //! no `type_text_chars` either — the old Windows mirror asserted it was
    //! exposed, but that had never been run). The advertised schemas must still
    //! carry their expected knobs.
    let mut d = RawDriver::spawn().expect("spawn source-built driver for schema test");

    d.send(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    d.recv();

    d.send(&serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}));
    let list_resp = d.recv();
    let tools = list_resp["result"]["tools"]
        .as_array()
        .expect("tools array");

    let properties = |name: &str| {
        &tools
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap_or_else(|| panic!("{name} not found in tools/list"))["inputSchema"]
            ["properties"]
    };
    let enum_contains = |schema: &serde_json::Value, expected: &str| {
        schema["enum"]
            .as_array()
            .map(|values| values.iter().any(|value| value.as_str() == Some(expected)))
            .unwrap_or(false)
    };

    // Deprecated alias is hidden from tools/list (accepted at invoke time only).
    assert!(
        tools.iter().all(|t| t["name"] != "type_text_chars"),
        "type_text_chars should be hidden from tools/list"
    );

    // Browser v1 is a cross-platform source-owned surface. Keep its full
    // roster, capability inputs, and read-only boundary pinned at the spawned
    // driver layer so platform registration drift cannot pass core-only tests.
    for name in [
        "get_browser_state",
        "browser_prepare",
        "browser_navigate",
        "browser_click",
        "browser_type",
    ] {
        assert!(
            tools.iter().any(|tool| tool["name"] == name),
            "browser tool {name} missing from tools/list"
        );
    }
    let browser_state = tools
        .iter()
        .find(|tool| tool["name"] == "get_browser_state")
        .expect("get_browser_state not found in tools/list");
    assert_eq!(
        browser_state["annotations"]["readOnlyHint"], true,
        "get_browser_state must remain side-effect free"
    );
    for field in ["pid", "window_id", "target_id", "tab_id", "session"] {
        assert!(
            browser_state["inputSchema"]["properties"][field].is_object(),
            "get_browser_state schema missing {field}"
        );
    }
    for (name, required) in [
        ("browser_prepare", &["pid"][..]),
        ("browser_navigate", &["target_id", "tab_id", "url"][..]),
        ("browser_click", &["target_id", "tab_id"][..]),
        ("browser_type", &["target_id", "tab_id", "ref", "text"][..]),
    ] {
        let tool = tools
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap_or_else(|| panic!("{name} not found in tools/list"));
        assert_eq!(
            tool["annotations"]["readOnlyHint"], false,
            "{name} must remain a mutation"
        );
        assert!(
            tool["inputSchema"]["properties"]["session"].is_object(),
            "{name} schema must expose the public session field"
        );
        let advertised = tool["inputSchema"]["required"]
            .as_array()
            .unwrap_or_else(|| panic!("{name} required fields missing"));
        for field in required {
            assert!(
                advertised.iter().any(|value| value.as_str() == Some(field)),
                "{name} schema missing required field {field}: {advertised:?}"
            );
        }
    }

    for tool in [
        "click",
        "double_click",
        "right_click",
        "type_text",
        "press_key",
        "hotkey",
        "scroll",
    ] {
        let delivery = &properties(tool)["delivery_mode"];
        assert!(
            enum_contains(delivery, "background") && enum_contains(delivery, "foreground"),
            "{tool}.delivery_mode should advertise background and foreground: {delivery:?}"
        );
    }
    // Capture scope belongs to the session lifecycle on every platform. The
    // action-level `scope` selects a coordinate/transport form but cannot
    // override the session policy enforced by the registry.
    let capture_scope = &properties("start_session")["capture_scope"];
    for expected in ["auto", "window", "desktop"] {
        assert!(
            enum_contains(capture_scope, expected),
            "start_session.capture_scope should advertise {expected}: {capture_scope:?}"
        );
    }
    assert!(
        tools.iter().any(|tool| tool["name"] == "escalate_session"),
        "escalate_session missing from tools/list"
    );
    assert!(
        tools.iter().any(|tool| tool["name"] == "get_session_state"),
        "get_session_state missing from tools/list"
    );
    assert!(
        properties("set_config")["capture_scope"].is_null(),
        "persistent set_config.capture_scope must remain retired"
    );
    for action in [
        "click", "type_text", "press_key", "hotkey", "scroll", "drag", "move_cursor",
    ] {
        let scope = &properties(action)["scope"];
        assert!(
            enum_contains(scope, "window") && enum_contains(scope, "desktop"),
            "{action}.scope should advertise window and desktop: {scope:?}"
        );
    }

    // list_windows schema has on_screen_only.
    let lw = tools
        .iter()
        .find(|t| t["name"] == "list_windows")
        .expect("list_windows not found in tools/list");
    assert!(
        lw["inputSchema"]["properties"]["on_screen_only"].is_object(),
        "list_windows schema missing on_screen_only: {:?}",
        lw["inputSchema"]["properties"]
    );

    // set_agent_cursor_motion has the Bezier motion knobs.
    let sacm = tools
        .iter()
        .find(|t| t["name"] == "set_agent_cursor_motion")
        .expect("set_agent_cursor_motion not found in tools/list");
    let sacm_props = &sacm["inputSchema"]["properties"];
    for knob in &[
        "arc_size",
        "spring",
        "glide_duration_ms",
        "dwell_after_click_ms",
        "idle_hide_ms",
    ] {
        assert!(
            sacm_props[knob].is_object(),
            "set_agent_cursor_motion schema missing {knob}: {sacm_props:?}"
        );
    }

    // capture_mode enum restriction. On macOS, capture_mode is a per-call param
    // on get_window_state (removed from set_config — behavior knobs are per-call
    // now, not settings). On Windows it is still also a set_config field.
    #[cfg(target_os = "macos")]
    {
        let gws = tools
            .iter()
            .find(|t| t["name"] == "get_window_state")
            .expect("get_window_state not found in tools/list");
        assert!(
            gws["inputSchema"]["properties"]["capture_mode"]["enum"].is_array(),
            "get_window_state capture_mode should have enum: {:?}",
            gws["inputSchema"]["properties"]
        );
    }
    #[cfg(target_os = "windows")]
    {
        let sc = tools
            .iter()
            .find(|t| t["name"] == "set_config")
            .expect("set_config not found in tools/list");
        assert!(
            sc["inputSchema"]["properties"]["capture_mode"]["enum"].is_array(),
            "set_config capture_mode should have enum: {:?}",
            sc["inputSchema"]["properties"]
        );
    }
}

#[test]
#[cfg(target_os = "linux")]
fn linux_cursor_motion_knobs_are_applied() {
    let mut driver = RawDriver::spawn().expect("spawn source-built Linux driver");
    driver.send(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {}
    }));
    driver.recv();

    driver.send(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "set_agent_cursor_motion",
            "arguments": {
                "session": "schema-linux",
                "arc_size": 0.4,
                "spring": 0.85,
                "glide_duration_ms": 500,
                "dwell_after_click_ms": 200,
                "idle_hide_ms": 5000
            }
        }
    }));
    let response = driver.recv();
    assert!(
        !response["result"]["isError"].as_bool().unwrap_or(false),
        "Linux cursor motion update failed: {response:?}"
    );
    let structured = &response["result"]["structuredContent"];
    assert_eq!(structured["arc_size"].as_f64(), Some(0.4));
    assert_eq!(structured["glide_duration_ms"].as_f64(), Some(500.0));
    assert_eq!(structured["idle_hide_ms"].as_f64(), Some(5000.0));
}
