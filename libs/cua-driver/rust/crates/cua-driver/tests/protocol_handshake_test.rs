//! MCP JSON-RPC handshake + tools/list contract tests.
//!
//! initialize handshake, the registered-tools roster, per-tool capability
//! claims, the capability/schema version envelope, and the unknown-tool /
//! unknown-action error surfaces. Split out of the old monolithic
//! `mcp_protocol_test.rs`; mac/windows pairs are merged into one fn that
//! branches only where the platforms genuinely differ.

#![cfg(any(target_os = "macos", target_os = "windows"))]

use cua_driver_testkit::RawDriver;

#[test]
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn initialize_handshake() {
    let Some(mut d) = RawDriver::spawn() else { return; };

    // 1. Send initialize request.
    d.send(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "clientInfo": { "name": "test-client", "version": "0.0.1" }
        }
    }));
    let resp = d.recv();
    assert_eq!(resp["jsonrpc"], "2.0");
    assert_eq!(resp["id"], 1);
    assert!(resp["result"]["protocolVersion"].is_string());
    // serverInfo.name differs across platforms.
    if cfg!(target_os = "windows") {
        assert_eq!(resp["result"]["serverInfo"]["name"], "cua-driver-rs");
    } else {
        assert_eq!(resp["result"]["serverInfo"]["name"], "cua-driver");
    }

    // 2. Send notifications/initialized (no response expected).
    d.send(&serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    }));

    // 3. Request tool list.
    d.send(&serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}));
    let resp = d.recv();
    assert_eq!(resp["id"], 2);
    let tools = resp["result"]["tools"].as_array().expect("tools array");
    assert!(!tools.is_empty(), "Should have at least one tool");

    // Verify tool names match reference implementation.
    let tool_names: Vec<&str> = tools.iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    for expected in &["list_apps", "list_windows", "get_window_state", "click", "type_text", "press_key"] {
        assert!(
            tool_names.contains(expected),
            "Missing tool: {} (have: {:?})", expected, tool_names
        );
    }

    // 4. Verify unknown method returns -32601.
    d.send(&serde_json::json!({"jsonrpc": "2.0", "id": 3, "method": "unknown/method"}));
    let resp = d.recv();
    assert_eq!(resp["id"], 3);
    assert_eq!(resp["error"]["code"], -32601);
}

#[test]
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn all_expected_tools_registered() {
    //! Verify that all tools from the reference implementation are registered.
    //! Adding a new tool to the reference requires adding it to this list.
    let Some(mut d) = RawDriver::spawn() else { return; };

    d.send(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    d.recv();

    d.send(&serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
    let resp = d.recv();
    let tools = resp["result"]["tools"].as_array().expect("tools array");
    let names: std::collections::HashSet<&str> = tools.iter()
        .filter_map(|t| t["name"].as_str())
        .collect();

    // Windows additionally registers `type_text_chars` and `screenshot`.
    let expected: &[&str] = if cfg!(target_os = "windows") {
        &[
            "list_apps", "list_windows", "get_window_state", "launch_app",
            "click", "double_click", "right_click", "type_text", "type_text_chars",
            "press_key", "hotkey", "set_value", "scroll", "screenshot", "zoom",
            "get_screen_size", "get_cursor_position",
            "move_cursor", "set_agent_cursor_enabled", "set_agent_cursor_motion",
            "get_agent_cursor_state", "check_permissions", "get_config", "set_config",
            "get_accessibility_tree",
            "start_recording", "stop_recording", "get_recording_state", "replay_trajectory",
            "page",
        ]
    } else {
        &[
            "list_apps", "list_windows", "get_window_state", "launch_app",
            "click", "double_click", "right_click", "type_text",
            "press_key", "hotkey", "set_value", "scroll", "zoom",
            "get_screen_size", "get_cursor_position",
            "move_cursor", "set_agent_cursor_enabled", "set_agent_cursor_motion",
            "get_agent_cursor_state", "check_permissions", "get_config", "set_config",
            "get_accessibility_tree",
            "start_recording", "stop_recording", "get_recording_state", "replay_trajectory",
            "page",
        ]
    };
    for name in expected {
        assert!(names.contains(name), "Missing tool: {name}  (registered: {names:?})");
    }
}

#[test]
#[cfg(target_os = "macos")]
fn tools_list_includes_per_tool_capabilities() {
    //! Surface 4 contract (part 1): every entry in `tools/list` must
    //! include a `capabilities` array of dotted-namespace tokens.
    //! Downstream consumers (Hermes' cua_backend.py, Codex) dispatch
    //! by capability instead of hardcoding tool names, so this is the
    //! wire contract. Additive-only — existing keys must still be
    //! present.
    let Some(mut d) = RawDriver::spawn() else { return; };

    d.send(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    d.recv();

    d.send(&serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
    let resp = d.recv();
    let result = &resp["result"];

    let tools = result["tools"].as_array().expect("tools array");
    assert!(!tools.is_empty(), "tools array must be non-empty");

    // Every entry has a `capabilities` array (may be empty for tools
    // without a mapping in `default_capabilities_for`).
    for tool in tools {
        let name = tool["name"].as_str().unwrap_or("<no name>");
        assert!(tool["capabilities"].is_array(),
            "tool {name:?} missing capabilities array: {tool:?}");
    }

    // Specifically: `click` claims input.pointer.click + .left, and
    // `type_text` claims input.keyboard.type — these are the contracts
    // Hermes' cua_backend.py is expected to route by.
    let click = tools.iter().find(|t| t["name"] == "click")
        .expect("click tool must be registered");
    let click_caps: Vec<&str> = click["capabilities"].as_array().unwrap()
        .iter().filter_map(|v| v.as_str()).collect();
    assert!(click_caps.contains(&"input.pointer.click"),
        "click missing input.pointer.click: {click_caps:?}");
    assert!(click_caps.contains(&"input.pointer.click.left"),
        "click missing input.pointer.click.left: {click_caps:?}");

    let type_text = tools.iter().find(|t| t["name"] == "type_text")
        .expect("type_text tool must be registered");
    let tt_caps: Vec<&str> = type_text["capabilities"].as_array().unwrap()
        .iter().filter_map(|v| v.as_str()).collect();
    assert!(tt_caps.contains(&"input.keyboard.type"),
        "type_text missing input.keyboard.type: {tt_caps:?}");

    // Regression guard for additive-only contract: every pre-existing
    // top-level field on a tool entry must still be present.
    for tool in tools {
        let name = tool["name"].as_str().unwrap_or("<no name>");
        assert!(tool["name"].is_string(), "{name}: name missing");
        assert!(tool["description"].is_string(), "{name}: description missing");
        assert!(tool["inputSchema"].is_object(), "{name}: inputSchema missing");
        assert!(tool["annotations"].is_object(), "{name}: annotations missing");
    }
}

#[test]
#[cfg(target_os = "macos")]
fn tools_list_includes_capability_and_schema_versions() {
    //! Surface 4 contract (part 2): top-level `tools/list` response
    //! must include `capability_version` and `schema_version` so
    //! downstream consumers (Hermes' cua_backend.py, Codex) can gate
    //! strict-vs-tolerant capability matching by version. Both pinned
    //! at "1" on first ship.
    let Some(mut d) = RawDriver::spawn() else { return; };

    d.send(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    d.recv();

    d.send(&serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
    let resp = d.recv();
    let result = &resp["result"];

    // Both version fields default to "1" on first ship — the surface
    // is brand new, so there's no pre-existing contract to bump.
    assert_eq!(result["capability_version"], "1",
        "capability_version must default to \"1\" on first ship");
    assert_eq!(result["schema_version"], "1",
        "schema_version must default to \"1\" on first ship");

    // Additive guard: tools array must still be there alongside the
    // new envelope keys — old consumers that only read `tools` keep
    // working.
    assert!(result["tools"].is_array(),
        "tools array must still be present alongside version envelope");
    assert!(!result["tools"].as_array().unwrap().is_empty(),
        "tools array must be non-empty");
}

#[test]
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn tools_call_unknown_tool() {
    let Some(mut d) = RawDriver::spawn() else { return; };

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
    let Some(mut d) = RawDriver::spawn() else { return; };

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
