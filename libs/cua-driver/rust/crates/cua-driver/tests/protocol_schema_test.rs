//! Pure `tools/list` schema-shape assertions.
//!
//! These never invoke a tool — they only inspect the advertised inputSchemas:
//! that `type_text_chars` is hidden, the `list_windows.on_screen_only` knob, the
//! `set_agent_cursor_motion` Bezier knobs, and the `set_config.capture_mode`
//! enum.

#![cfg(any(target_os = "macos", target_os = "windows"))]

use cua_driver_testkit::RawDriver;

#[test]
fn tools_list_schema_shape() {
    //! `type_text_chars` is an accepted-but-deprecated invoke alias that stays
    //! HIDDEN from tools/list on every platform (the live Windows roster carries
    //! no `type_text_chars` either — the old Windows mirror asserted it was
    //! exposed, but that had never been run). The advertised schemas must still
    //! carry their expected knobs.
    let Some(mut d) = RawDriver::spawn() else { return; };

    d.send(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    d.recv();

    d.send(&serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}));
    let list_resp = d.recv();
    let tools = list_resp["result"]["tools"].as_array().expect("tools array");

    // Deprecated alias is hidden from tools/list (accepted at invoke time only).
    assert!(
        tools.iter().all(|t| t["name"] != "type_text_chars"),
        "type_text_chars should be hidden from tools/list"
    );

    // list_windows schema has on_screen_only.
    let lw = tools.iter().find(|t| t["name"] == "list_windows")
        .expect("list_windows not found in tools/list");
    assert!(lw["inputSchema"]["properties"]["on_screen_only"].is_object(),
        "list_windows schema missing on_screen_only: {:?}", lw["inputSchema"]["properties"]);

    // set_agent_cursor_motion has the Bezier motion knobs.
    let sacm = tools.iter().find(|t| t["name"] == "set_agent_cursor_motion")
        .expect("set_agent_cursor_motion not found in tools/list");
    let sacm_props = &sacm["inputSchema"]["properties"];
    for knob in &["arc_size", "spring", "glide_duration_ms", "dwell_after_click_ms", "idle_hide_ms"] {
        assert!(sacm_props[knob].is_object(),
            "set_agent_cursor_motion schema missing {knob}: {sacm_props:?}");
    }

    // set_config has the capture_mode enum restriction.
    let sc = tools.iter().find(|t| t["name"] == "set_config")
        .expect("set_config not found in tools/list");
    assert!(sc["inputSchema"]["properties"]["capture_mode"]["enum"].is_array(),
        "set_config capture_mode should have enum: {:?}", sc["inputSchema"]["properties"]);
}
