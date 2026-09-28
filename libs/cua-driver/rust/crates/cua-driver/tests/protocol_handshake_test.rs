//! MCP JSON-RPC tools/list roster and tool-error surface tests.
//!
//! The registered-tools roster and the unknown-tool / unknown-action error
//! surfaces. The initialize envelope, version fields, and unknown-method error
//! are owned by `compatibility_contract_test`; per-tool capability and
//! annotation fields by `schema_consistency_test`. Split out of the old
//! monolithic `mcp_protocol_test.rs`; mac/windows pairs are merged into one fn
//! that branches only where the platforms genuinely differ.

#![cfg(any(target_os = "macos", target_os = "windows"))]

use cua_driver_testkit::RawDriver;

#[test]
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn all_expected_tools_registered() {
    //! Verify that all tools from the reference implementation are registered.
    //! Adding a new tool to the reference requires adding it to this list.
    let Some(mut d) = RawDriver::spawn() else {
        return;
    };

    d.send(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    d.recv();

    d.send(&serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
    let resp = d.recv();
    let tools = resp["result"]["tools"].as_array().expect("tools array");
    let names: std::collections::HashSet<&str> =
        tools.iter().filter_map(|t| t["name"].as_str()).collect();

    // Baseline roster that must be registered on every platform. (The old
    // Windows mirror additionally listed type_text_chars + screenshot, which
    // the Windows build does NOT register — a stale, never-run assertion.)
    let expected: &[&str] = &[
        "list_apps",
        "list_windows",
        "get_window_state",
        "launch_app",
        "click",
        "double_click",
        "right_click",
        "type_text",
        "press_key",
        "hotkey",
        "set_value",
        "scroll",
        "zoom",
        "get_screen_size",
        "get_cursor_position",
        "move_cursor",
        "set_agent_cursor_enabled",
        "set_agent_cursor_motion",
        "get_agent_cursor_state",
        "check_permissions",
        "get_config",
        "set_config",
        "get_accessibility_tree",
        "start_recording",
        "stop_recording",
        "get_recording_state",
        "replay_trajectory",
        "page",
    ];
    for name in expected {
        assert!(
            names.contains(name),
            "Missing tool: {name}  (registered: {names:?})"
        );
    }
}

#[test]
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn tools_call_unknown_tool() {
    let Some(mut d) = RawDriver::spawn() else {
        return;
    };

    d.send(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    d.recv();

    d.send(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": { "name": "nonexistent_tool", "arguments": {} }
    }));
    let resp = d.recv();
    // Error should be in the content with isError=true, not a protocol error.
    let is_error = resp["result"]["isError"].as_bool().unwrap_or(false);
    assert!(is_error, "Expected isError=true for unknown tool");
}

#[test]
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn page_unknown_action_error() {
    //! Verify the cross-platform `page` tool is registered and rejects an
    //! unknown action with a meaningful error.
    let Some(mut d) = RawDriver::spawn() else {
        return;
    };

    d.send(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    d.recv();

    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"page","arguments":{
            "pid": 1,
            "window_id": 0,
            "action": "definitely_not_a_real_action"
        }}
    }));
    let resp = d.recv();

    assert!(
        resp["result"]["isError"].as_bool().unwrap_or(false),
        "expected isError=true for unknown action: {resp:?}"
    );
    let text = resp["result"]["content"][0]["text"].as_str().unwrap_or("");
    assert!(
        text.to_ascii_lowercase().contains("unknown action"),
        "error text should mention 'Unknown action': {text}"
    );
}
