//! Pure `tools/list` schema-shape assertions.
//!
//! These never invoke a tool — they only inspect the advertised inputSchemas:
//! that `type_text_chars` is hidden, the `list_windows.on_screen_only` knob, the
//! `set_agent_cursor_motion` Bezier knobs, delivery and scope enums, and the
//! `set_config.capture_mode` enum.

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
    // macOS selects desktop coordinates per click. Linux and Windows retain
    // the process-level capture_scope gate used by their desktop-state tools.
    #[cfg(target_os = "macos")]
    {
        let scope = &properties("click")["scope"];
        assert!(
            enum_contains(scope, "window") && enum_contains(scope, "desktop"),
            "click.scope should advertise window and desktop: {scope:?}"
        );
    }
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    {
        let capture_scope = &properties("set_config")["capture_scope"];
        assert!(
            enum_contains(capture_scope, "window") && enum_contains(capture_scope, "desktop"),
            "set_config.capture_scope should advertise window and desktop: {capture_scope:?}"
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
