//! Pure `tools/list` schema-shape assertions.
//!
//! These never invoke a tool — they only inspect the advertised inputSchemas:
//! whether `type_text_chars` is hidden (macOS) or exposed with
//! `type_chars_only` (Windows), the `list_windows.on_screen_only` knob, the
//! `set_agent_cursor_motion` Bezier knobs, and the `set_config.capture_mode`
//! enum. macOS and Windows make opposite claims about `type_text_chars`'s
//! visibility, so they stay as two separate platform-gated fns.

#![cfg(any(target_os = "macos", target_os = "windows"))]

use cua_driver_testkit::RawDriver;

#[test]
#[cfg(target_os = "macos")]
fn type_text_chars_type_chars_only_schema() {
    //! Verify the deprecated type_text_chars alias stays hidden from tools/list
    //! while related active tool schemas retain their expected knobs.
    let Some(mut d) = RawDriver::spawn() else { return; };

    d.send(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    d.recv();

    // The deprecated alias is accepted at invoke time but intentionally hidden
    // from tools/list.
    d.send(&serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}));
    let list_resp = d.recv();
    let tools = list_resp["result"]["tools"].as_array().expect("tools array");
    assert!(
        tools.iter().all(|t| t["name"] != "type_text_chars"),
        "deprecated type_text_chars alias should not appear in tools/list"
    );

    // Also verify list_windows schema has on_screen_only.
    let lw = tools.iter().find(|t| t["name"] == "list_windows")
        .expect("list_windows not found in tools/list");
    let lw_props = &lw["inputSchema"]["properties"];
    assert!(lw_props["on_screen_only"].is_object(),
        "list_windows schema missing on_screen_only property: {lw_props:?}");

    // Verify set_agent_cursor_motion has Bezier motion knobs.
    let sacm = tools.iter().find(|t| t["name"] == "set_agent_cursor_motion")
        .expect("set_agent_cursor_motion not found in tools/list");
    let sacm_props = &sacm["inputSchema"]["properties"];
    for knob in &["arc_size", "spring", "glide_duration_ms", "dwell_after_click_ms", "idle_hide_ms"] {
        assert!(sacm_props[knob].is_object(),
            "set_agent_cursor_motion schema missing {knob}: {sacm_props:?}");
    }
    // Also verify set_config schema has the enum restriction.
    let sc = tools.iter().find(|t| t["name"] == "set_config")
        .expect("set_config not found in tools/list");
    let sc_props = &sc["inputSchema"]["properties"];
    assert!(sc_props["capture_mode"]["enum"].is_array(),
        "set_config capture_mode should have enum: {sc_props:?}");
}

#[test]
#[cfg(target_os = "windows")]
fn type_text_chars_schema() {
    let Some(mut d) = RawDriver::spawn() else { return; };

    d.send(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    d.recv();

    d.send(&serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}));
    let list_resp = d.recv();
    let tools = list_resp["result"]["tools"].as_array().expect("tools array");

    let ttc = tools.iter().find(|t| t["name"] == "type_text_chars")
        .expect("type_text_chars not found");
    let props = &ttc["inputSchema"]["properties"];
    assert!(props["type_chars_only"].is_object(),
        "type_text_chars schema missing type_chars_only: {props:?}");

    let lw = tools.iter().find(|t| t["name"] == "list_windows").expect("list_windows not found");
    assert!(lw["inputSchema"]["properties"]["on_screen_only"].is_object());

    let sacm = tools.iter().find(|t| t["name"] == "set_agent_cursor_motion")
        .expect("set_agent_cursor_motion not found");
    let sacm_props = &sacm["inputSchema"]["properties"];
    for knob in &["arc_size", "spring", "glide_duration_ms", "dwell_after_click_ms", "idle_hide_ms"] {
        assert!(sacm_props[knob].is_object(), "missing {knob}: {sacm_props:?}");
    }

    let sc = tools.iter().find(|t| t["name"] == "set_config").expect("set_config not found");
    assert!(sc["inputSchema"]["properties"]["capture_mode"]["enum"].is_array());
}
