//! Integration tests for the MCP JSON-RPC 2.0 protocol.
//!
//! Tests the server protocol without any platform-specific automation —
//! verifies request/response format, initialize handshake, tools/list,
//! and error handling.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Path to the built binary.
fn binary_path() -> std::path::PathBuf {
    // CARGO_MANIFEST_DIR is crates/cua-driver; workspace root is two levels up.
    let manifest = std::env::var("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR not set");
    std::path::PathBuf::from(manifest)
        .parent().unwrap()  // crates/
        .parent().unwrap()  // workspace root (cua-driver-rs/)
        .join("target/debug/cua-driver")
}

fn send_request(stdin: &mut impl Write, request: &serde_json::Value) {
    let line = serde_json::to_string(request).unwrap();
    writeln!(stdin, "{}", line).unwrap();
}

fn read_response(reader: &mut impl BufRead) -> serde_json::Value {
    let mut line = String::new();
    reader.read_line(&mut line).expect("read line");
    serde_json::from_str(line.trim()).expect("parse JSON")
}

#[test]
#[cfg(target_os = "macos")]
fn test_initialize_handshake() {
    let binary = binary_path();
    if !binary.exists() {
        eprintln!("Binary not found at {:?} — run `cargo build` first", binary);
        return;
    }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn cua-driver");

    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    // 1. Send initialize request.
    send_request(stdin, &serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "clientInfo": { "name": "test-client", "version": "0.0.1" }
        }
    }));

    let resp = read_response(&mut stdout);
    assert_eq!(resp["jsonrpc"], "2.0");
    assert_eq!(resp["id"], 1);
    assert!(resp["result"]["protocolVersion"].is_string());
    assert_eq!(resp["result"]["serverInfo"]["name"], "cua-driver-rs");

    // 2. Send notifications/initialized (no response expected).
    send_request(stdin, &serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    }));

    // 3. Request tool list.
    send_request(stdin, &serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list"
    }));

    let resp = read_response(&mut stdout);
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
    send_request(stdin, &serde_json::json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "unknown/method"
    }));
    let resp = read_response(&mut stdout);
    assert_eq!(resp["id"], 3);
    assert_eq!(resp["error"]["code"], -32601);

    child.kill().ok();
}

#[test]
#[cfg(target_os = "macos")]
fn test_tools_call_list_apps() {
    let binary = binary_path();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");

    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    // Initialize.
    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    // Call list_apps.
    send_request(stdin, &serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": { "name": "list_apps", "arguments": {} }
    }));
    let resp = read_response(&mut stdout);
    assert_eq!(resp["id"], 2);
    assert!(resp["result"]["content"].is_array());
    let content = &resp["result"]["content"][0];
    assert_eq!(content["type"], "text");
    let text = content["text"].as_str().unwrap();
    // Should contain some running apps.
    assert!(text.contains("Found"), "Expected app list text, got: {}", text);

    child.kill().ok();
}

#[test]
#[cfg(target_os = "macos")]
fn test_tools_call_unknown_tool() {
    let binary = binary_path();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");

    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    send_request(stdin, &serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": { "name": "nonexistent_tool", "arguments": {} }
    }));
    let resp = read_response(&mut stdout);
    // Error should be in the content with isError=true, not a protocol error.
    let result = &resp["result"];
    let is_error = result["isError"].as_bool().unwrap_or(false);
    assert!(is_error, "Expected isError=true for unknown tool");

    child.kill().ok();
}

#[test]
#[cfg(target_os = "macos")]
fn test_concurrent_clients() {
    //! Verify two concurrent cua-driver-rs processes both respond correctly.
    //! This tests the "multiple cua-driver processes" scenario relevant to
    //! the multi-cursor use case.

    let binary = binary_path();
    if !binary.exists() { return; }

    let mut processes: Vec<std::process::Child> = (0..2).map(|_| {
        Command::new(&binary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn")
    }).collect();

    for (i, child) in processes.iter_mut().enumerate() {
        let stdin = child.stdin.as_mut().unwrap();
        let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

        send_request(stdin, &serde_json::json!({
            "jsonrpc": "2.0",
            "id": i + 1,
            "method": "initialize",
            "params": {}
        }));

        let resp = read_response(&mut stdout);
        assert_eq!(resp["id"], (i + 1) as i64);
        assert!(resp["result"]["protocolVersion"].is_string(),
            "Process {i} failed to initialize");
    }

    for mut child in processes {
        child.kill().ok();
    }
}

#[test]
#[cfg(target_os = "macos")]
fn test_overlay_move_cursor_stays_alive() {
    //! Verify that calling move_cursor with the overlay enabled does not crash the process.
    //! The overlay renders to a transparent NSWindow on the main thread; the MCP server
    //! sends commands via the global channel.  Any crash in the render thread (e.g. wrong
    //! GCD queue pointer, bad CGImage argument types) would cause this test to fail with
    //! an early EOF or a non-zero exit code.

    let binary = binary_path();
    if !binary.exists() { return; }

    // Start WITHOUT --no-overlay so the AppKit render loop runs.
    let mut child = Command::new(&binary)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");

    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    // Initialize.
    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    // Call move_cursor several times to drive the overlay render loop.
    for (id, (x, y)) in [(2, (100.0_f64, 200.0_f64)), (3, (400.0, 300.0)), (4, (800.0, 600.0))] {
        send_request(stdin, &serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": { "name": "move_cursor", "arguments": { "x": x, "y": y } }
        }));
        let resp = read_response(&mut stdout);
        assert_eq!(resp["id"], id);
        // Should succeed (isError absent or false).
        let is_error = resp["result"]["isError"].as_bool().unwrap_or(false);
        assert!(!is_error, "move_cursor failed: {:?}", resp);
    }

    // Also call get_agent_cursor_state — should report the last position.
    send_request(stdin, &serde_json::json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "tools/call",
        "params": { "name": "get_agent_cursor_state", "arguments": {} }
    }));
    let resp = read_response(&mut stdout);
    assert_eq!(resp["id"], 5);
    assert!(!resp["result"]["isError"].as_bool().unwrap_or(false));

    // Give the render loop a chance to tick (two frames ≈ 33 ms).
    std::thread::sleep(Duration::from_millis(100));

    // Process must still be alive.
    assert!(
        child.try_wait().expect("try_wait").is_none(),
        "cua-driver crashed during overlay move_cursor test"
    );

    child.kill().ok();
}

#[test]
#[cfg(target_os = "macos")]
fn test_all_expected_tools_registered() {
    //! Verify that all tools from the reference implementation are registered.
    //! Adding a new tool to the macOS reference requires adding it to this list.
    let binary = binary_path();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");

    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
    let resp = read_response(&mut stdout);
    let tools = resp["result"]["tools"].as_array().expect("tools array");
    let names: std::collections::HashSet<&str> = tools.iter()
        .filter_map(|t| t["name"].as_str())
        .collect();

    let expected = [
        "list_apps", "list_windows", "get_window_state", "launch_app",
        "click", "double_click", "right_click", "type_text", "type_text_chars",
        "press_key", "hotkey", "set_value", "scroll", "screenshot", "zoom",
        "get_screen_size", "get_cursor_position",
        "move_cursor", "set_agent_cursor_enabled", "set_agent_cursor_motion",
        "get_agent_cursor_state", "check_permissions", "get_config", "set_config",
        "get_accessibility_tree",
        "set_recording", "get_recording_state", "replay_trajectory",
        "browser_eval",
    ];
    for name in &expected {
        assert!(names.contains(name), "Missing tool: {name}  (registered: {names:?})");
    }

    child.kill().ok();
}

#[test]
#[cfg(target_os = "macos")]
fn test_get_screen_size_and_cursor_position() {
    let binary = binary_path();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");

    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    // get_screen_size
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"get_screen_size","arguments":{}}
    }));
    let resp = read_response(&mut stdout);
    assert!(!resp["result"]["isError"].as_bool().unwrap_or(false), "get_screen_size failed: {resp:?}");
    let sc = &resp["result"]["structuredContent"];
    assert!(sc["width"].as_f64().unwrap_or(0.0) > 0.0, "width should be positive");
    assert!(sc["height"].as_f64().unwrap_or(0.0) > 0.0, "height should be positive");

    // get_cursor_position
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"get_cursor_position","arguments":{}}
    }));
    let resp = read_response(&mut stdout);
    assert!(!resp["result"]["isError"].as_bool().unwrap_or(false), "get_cursor_position failed: {resp:?}");
    // x and y may be 0,0 or any value — just verify the keys exist and are numbers.
    assert!(resp["result"]["structuredContent"]["x"].is_number(), "x should be a number");
    assert!(resp["result"]["structuredContent"]["y"].is_number(), "y should be a number");

    child.kill().ok();
}

#[test]
#[cfg(target_os = "macos")]
fn test_get_config_and_check_permissions() {
    let binary = binary_path();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");

    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"get_config","arguments":{}}
    }));
    let resp = read_response(&mut stdout);
    assert!(!resp["result"]["isError"].as_bool().unwrap_or(false));
    assert_eq!(resp["result"]["structuredContent"]["platform"], "macos");

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"check_permissions","arguments":{}}
    }));
    let resp = read_response(&mut stdout);
    assert!(!resp["result"]["isError"].as_bool().unwrap_or(false));
    // Returns structured content with accessibility and screen_recording booleans.
    assert!(resp["result"]["structuredContent"]["accessibility"].is_boolean());

    // set_config — change capture_mode to "ax" and verify get_config reflects the new value.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":4,"method":"tools/call",
        "params":{"name":"set_config","arguments":{"capture_mode": "ax", "max_image_dimension": 1920}}
    }));
    let resp = read_response(&mut stdout);
    assert!(resp["error"].is_null(), "Protocol error from set_config: {resp:?}");
    let content = resp["result"]["content"].as_array().expect("content array");
    assert!(!content.is_empty(), "set_config returned empty content");

    // get_config should now reflect the updated values.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":5,"method":"tools/call",
        "params":{"name":"get_config","arguments":{}}
    }));
    let resp = read_response(&mut stdout);
    assert_eq!(resp["result"]["structuredContent"]["capture_mode"], "ax",
        "get_config should reflect set_config change");
    assert_eq!(resp["result"]["structuredContent"]["max_image_dimension"], 1920,
        "get_config should reflect max_image_dimension change");

    child.kill().ok();
}

#[test]
#[cfg(target_os = "macos")]
fn test_multi_cursor_instance_state() {
    //! Two cursor instances can be created with different IDs; each has independent state.
    let binary = binary_path();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");

    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    // Move cursor "agent1" to (100, 200).
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"move_cursor","arguments":{"x":100.0,"y":200.0,"cursor_id":"agent1"}}
    }));
    let r2 = read_response(&mut stdout);
    assert!(!r2["result"]["isError"].as_bool().unwrap_or(false));

    // Move cursor "agent2" to (300, 400).
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"move_cursor","arguments":{"x":300.0,"y":400.0,"cursor_id":"agent2"}}
    }));
    let r3 = read_response(&mut stdout);
    assert!(!r3["result"]["isError"].as_bool().unwrap_or(false));

    // Configure cursor "agent1" with a custom color.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":4,"method":"tools/call",
        "params":{"name":"set_agent_cursor_motion","arguments":{
            "cursor_id":"agent1","cursor_color":"#FF0000","cursor_label":"AI-1"
        }}
    }));
    let r4 = read_response(&mut stdout);
    assert!(!r4["result"]["isError"].as_bool().unwrap_or(false));

    // Hide cursor "agent2".
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":5,"method":"tools/call",
        "params":{"name":"set_agent_cursor_enabled","arguments":{"enabled":false,"cursor_id":"agent2"}}
    }));
    let r5 = read_response(&mut stdout);
    assert!(!r5["result"]["isError"].as_bool().unwrap_or(false));

    // get_agent_cursor_state — should have multiple instances.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":6,"method":"tools/call",
        "params":{"name":"get_agent_cursor_state","arguments":{}}
    }));
    let resp = read_response(&mut stdout);
    assert!(!resp["result"]["isError"].as_bool().unwrap_or(false));
    let cursors = &resp["result"]["structuredContent"]["cursors"];
    assert!(cursors.as_array().map(|a| a.len() >= 2).unwrap_or(false),
        "Expected at least 2 cursor instances, got: {cursors:?}");

    std::thread::sleep(Duration::from_millis(50));
    assert!(child.try_wait().expect("try_wait").is_none(), "cua-driver crashed during multi-cursor test");

    child.kill().ok();
}

#[test]
#[cfg(target_os = "macos")]
fn test_set_agent_cursor_motion_bezier_knobs() {
    //! set_agent_cursor_motion with Bezier/timing knobs — verifies schema accepts them
    //! and returns a non-error response with the updated values in the response text.
    let binary = binary_path();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    // Set Bezier motion knobs including integer-encoded numbers (common from MCP clients).
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"set_agent_cursor_motion","arguments":{
            "cursor_id": "agent1",
            "arc_size": 0.4,
            "spring": 0.85,
            "glide_duration_ms": 500,
            "dwell_after_click_ms": 200,
            "idle_hide_ms": 5000
        }}
    }));
    let resp = read_response(&mut stdout);
    assert!(resp["error"].is_null(), "Protocol error from set_agent_cursor_motion: {resp:?}");
    assert!(!resp["result"]["isError"].as_bool().unwrap_or(false),
        "set_agent_cursor_motion returned isError: {resp:?}");
    // Response text should mention the new glide duration.
    let text = resp["result"]["content"][0]["text"].as_str().unwrap_or("");
    assert!(text.contains("500") || text.contains("motion"),
        "Expected motion summary in response, got: {text}");

    child.kill().ok();
}

#[test]
#[cfg(target_os = "macos")]
fn test_concurrent_clients_with_cursor_moves() {
    //! Two concurrent cua-driver processes, each driving their own cursor, should not crash.
    //! This covers the multi-cursor use case where two Codex agents run simultaneously.
    let binary = binary_path();
    if !binary.exists() { return; }

    let mut processes: Vec<(std::process::Child, usize)> = (0..2).map(|i| {
        let child = Command::new(&binary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn");
        (child, i)
    }).collect();

    // Initialize all processes.
    for (child, i) in &mut processes {
        let stdin = child.stdin.as_mut().unwrap();
        let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());
        send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
        let resp = read_response(&mut stdout);
        assert_eq!(resp["id"], 1, "Process {i} initialize failed");
    }

    // Each process moves its cursor to different positions.
    let positions = vec![(100.0_f64, 200.0_f64), (500.0, 600.0)];
    for ((child, i), (px, py)) in processes.iter_mut().zip(positions.iter()) {
        let stdin = child.stdin.as_mut().unwrap();
        let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());
        send_request(stdin, &serde_json::json!({
            "jsonrpc":"2.0","id":2,"method":"tools/call",
            "params":{"name":"move_cursor","arguments":{"x":px,"y":py}}
        }));
        let resp = read_response(&mut stdout);
        assert!(!resp["result"]["isError"].as_bool().unwrap_or(false),
            "Process {i} move_cursor failed: {resp:?}");
    }

    std::thread::sleep(Duration::from_millis(100));

    for (child, i) in &mut processes {
        assert!(child.try_wait().expect("try_wait").is_none(),
            "cua-driver process {i} crashed during concurrent cursor test");
        child.kill().ok();
    }
}

#[test]
#[cfg(target_os = "macos")]
fn test_zoom_tool_returns_jpeg() {
    //! Call zoom on a visible window and verify the result contains a JPEG image.
    //! Skips gracefully if no windows are visible (headless environment).
    let binary = binary_path();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");

    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    // List on-screen windows only — off-screen/minimized windows may not be capturable.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"list_windows","arguments":{"on_screen_only": true}}
    }));
    let resp = read_response(&mut stdout);
    let windows = resp["result"]["structuredContent"]["windows"]
        .as_array()
        .cloned()
        .unwrap_or_default();

    // Try windows until one captures successfully.
    let mut found_jpeg = false;
    let mut tried = 0usize;
    for win in windows.iter().take(5) {
        let Some(wid) = win["window_id"].as_u64() else { continue; };
        tried += 1;

        send_request(stdin, &serde_json::json!({
            "jsonrpc":"2.0","id": 2 + tried as u64,"method":"tools/call",
            "params":{"name":"zoom","arguments":{
                "window_id": wid,
                "x1": 0, "y1": 0, "x2": 100, "y2": 100
            }}
        }));
        let r = read_response(&mut stdout);
        if r["result"]["isError"].as_bool().unwrap_or(false) {
            // This window might be off-screen or not capturable — try the next.
            continue;
        }
        let content = r["result"]["content"].as_array().expect("content array");
        found_jpeg = content.iter().any(|c| {
            c["type"] == "image" && c["mimeType"].as_str().unwrap_or("") == "image/jpeg"
                && c["data"].as_str().map(|s| s.len() > 10).unwrap_or(false)
        });
        if found_jpeg {
            // Also verify structuredContent.
            let sc = &r["result"]["structuredContent"];
            assert!(sc["width"].as_f64().unwrap_or(0.0) > 0.0, "Expected positive width: {sc:?}");
            assert!(sc["height"].as_f64().unwrap_or(0.0) > 0.0, "Expected positive height: {sc:?}");
            assert_eq!(sc["format"].as_str().unwrap_or(""), "jpeg", "Expected format=jpeg: {sc:?}");
            break;
        }
    }

    if tried == 0 {
        eprintln!("No on-screen windows found — skipping zoom test");
        child.kill().ok();
        return;
    }
    assert!(found_jpeg, "Expected at least one window to return a valid JPEG from zoom");

    child.kill().ok();
}

#[test]
#[cfg(target_os = "macos")]
fn test_type_text_chars_tool() {
    //! Verify type_text_chars with delay_ms is accepted without error (dry-run via TextEdit or
    //! a pid that accepts WM_CHAR). We just verify the tool responds with a non-error.
    //! Skips gracefully if no visible TextEdit window.
    let binary = binary_path();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");

    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    // list_apps to find a target (e.g. TextEdit).
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"list_apps","arguments":{}}
    }));
    let resp = read_response(&mut stdout);
    let apps = resp["result"]["structuredContent"]["apps"].as_array();
    let textedit_pid = apps.and_then(|arr| {
        arr.iter().find(|a| a["name"].as_str().unwrap_or("").contains("TextEdit"))
            .and_then(|a| a["pid"].as_i64())
    });

    let Some(pid) = textedit_pid else {
        eprintln!("TextEdit not running — skipping type_text_chars test");
        child.kill().ok();
        return;
    };

    // type_text_chars with delay_ms=5 — just verify the tool invocation is accepted.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"type_text_chars","arguments":{
            "pid": pid, "text": "hi", "delay_ms": 5
        }}
    }));
    let resp = read_response(&mut stdout);
    assert!(!resp["result"]["isError"].as_bool().unwrap_or(false),
        "type_text_chars returned error: {resp:?}");
    let msg = resp["result"]["content"][0]["text"].as_str().unwrap_or("");
    assert!(msg.contains("Typed"), "Unexpected message: {msg}");

    child.kill().ok();
}

#[test]
#[cfg(target_os = "macos")]
fn test_recording_session() {
    //! Enable recording, invoke move_cursor (non-read-only), disable, verify action.json written.
    let binary = binary_path();
    if !binary.exists() { return; }

    let tmp_dir = std::env::temp_dir().join(format!("cua-driver-rs-rec-test-{}", std::process::id()));
    let tmp_str = tmp_dir.to_string_lossy().to_string();

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    // Enable recording.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"set_recording","arguments":{"enabled":true,"output_dir":tmp_str}}
    }));
    let resp = read_response(&mut stdout);
    assert!(!resp["result"]["isError"].as_bool().unwrap_or(false), "set_recording failed: {resp:?}");
    assert!(resp["result"]["structuredContent"]["enabled"].as_bool().unwrap_or(false));

    // Invoke move_cursor — should be recorded.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"move_cursor","arguments":{"x":100.0,"y":200.0}}
    }));
    read_response(&mut stdout);

    // get_recording_state — next_turn should be 2.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":4,"method":"tools/call",
        "params":{"name":"get_recording_state","arguments":{}}
    }));
    let resp = read_response(&mut stdout);
    let next_turn = resp["result"]["structuredContent"]["next_turn"].as_u64().unwrap_or(0);
    assert!(next_turn >= 2, "expected next_turn >= 2, got {next_turn}");

    // Disable recording.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":5,"method":"tools/call",
        "params":{"name":"set_recording","arguments":{"enabled":false}}
    }));
    let resp = read_response(&mut stdout);
    assert!(!resp["result"]["structuredContent"]["enabled"].as_bool().unwrap_or(true));
    child.kill().ok();

    // Small sync wait for the write to flush.
    std::thread::sleep(Duration::from_millis(50));

    // Verify turn-00001/action.json was written.
    let action_path = tmp_dir.join("turn-00001").join("action.json");
    assert!(action_path.exists(), "Expected {action_path:?} to exist");
    let content: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&action_path).unwrap()
    ).unwrap();
    assert_eq!(content["tool"].as_str().unwrap_or(""), "move_cursor");
    assert_eq!(content["arguments"]["x"].as_f64().unwrap_or(0.0), 100.0);

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
#[cfg(target_os = "macos")]
fn test_recording_screenshot_capture() {
    //! When recording is active and a tool call includes a window_id, a screenshot.png
    //! should appear alongside action.json in the turn folder.
    let binary = binary_path();
    if !binary.exists() { return; }

    let tmp_dir = std::env::temp_dir().join(format!("cua-driver-rs-rec-ss-{}", std::process::id()));
    let tmp_str = tmp_dir.to_string_lossy().to_string();

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    // Find a capturable on-screen window.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"list_windows","arguments":{"on_screen_only":true}}
    }));
    let resp = read_response(&mut stdout);
    let wins = resp["result"]["structuredContent"]["windows"].as_array().cloned().unwrap_or_default();
    // Prefer windows with non-empty titles (more likely capturable).
    let win = wins.iter()
        .find(|w| w["pid"].as_i64().is_some() && !w["title"].as_str().unwrap_or("").is_empty())
        .or_else(|| wins.iter().find(|w| w["pid"].as_i64().is_some()));
    let Some(win) = win else {
        eprintln!("No on-screen windows — skipping recording screenshot test");
        child.kill().ok();
        return;
    };
    let window_id = win["window_id"].as_u64().unwrap();
    let pid = win["pid"].as_i64().unwrap();

    // Enable recording.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"set_recording","arguments":{"enabled":true,"output_dir":tmp_str}}
    }));
    let resp = read_response(&mut stdout);
    assert!(!resp["result"]["isError"].as_bool().unwrap_or(false), "set_recording failed: {resp:?}");

    // Invoke click (non-read-only) with window_id + pid — recording should capture screenshot.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":4,"method":"tools/call",
        "params":{"name":"click","arguments":{"pid": pid, "window_id": window_id, "x":0.0,"y":0.0}}
    }));
    read_response(&mut stdout); // result may be error if coords are off-window — that's ok

    // Disable recording.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":5,"method":"tools/call",
        "params":{"name":"set_recording","arguments":{"enabled":false}}
    }));
    read_response(&mut stdout);
    child.kill().ok();

    std::thread::sleep(Duration::from_millis(100));

    // action.json must exist.
    let turn_dir = tmp_dir.join("turn-00001");
    let action_path = turn_dir.join("action.json");
    assert!(action_path.exists(), "Expected action.json at {action_path:?}");
    let content: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&action_path).unwrap()
    ).unwrap();
    assert_eq!(content["tool"].as_str().unwrap_or(""), "click");

    // screenshot.png should exist if screencapture succeeded for this window.
    let ss_path = turn_dir.join("screenshot.png");
    if !ss_path.exists() {
        eprintln!("screenshot.png not written for window {window_id} (may be uncapturable) — skipping PNG check");
    } else {
        let ss_bytes = std::fs::read(&ss_path).unwrap();
        assert!(!ss_bytes.is_empty(), "screenshot.png should not be empty");
        assert_eq!(&ss_bytes[..4], b"\x89PNG", "screenshot.png should be a valid PNG");

        // click.png should also exist (click at 0,0 with crosshair marker).
        let click_path = turn_dir.join("click.png");
        if !click_path.exists() {
            eprintln!("click.png not written — skipping click PNG check");
        } else {
            let click_bytes = std::fs::read(&click_path).unwrap();
            assert!(!click_bytes.is_empty(), "click.png should not be empty");
            assert_eq!(&click_bytes[..4], b"\x89PNG", "click.png should be a valid PNG");
        }
    }

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
#[cfg(target_os = "macos")]
fn test_replay_trajectory() {
    //! Write a minimal trajectory (one move_cursor turn), replay it, verify succeeded=1.
    let binary = binary_path();
    if !binary.exists() { return; }

    let traj_dir = std::env::temp_dir().join(format!("cua-driver-rs-replay-{}", std::process::id()));
    let turn_dir = traj_dir.join("turn-00001");
    std::fs::create_dir_all(&turn_dir).unwrap();
    std::fs::write(
        turn_dir.join("action.json"),
        r#"{"tool":"move_cursor","arguments":{"x":50.0,"y":60.0}}"#,
    ).unwrap();

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"replay_trajectory","arguments":{
            "dir": traj_dir.to_string_lossy(),
            "delay_ms": 0
        }}
    }));
    let resp = read_response(&mut stdout);
    assert!(!resp["result"]["isError"].as_bool().unwrap_or(false), "replay error: {resp:?}");
    let sc = &resp["result"]["structuredContent"];
    assert_eq!(sc["attempted"].as_u64().unwrap_or(0), 1, "expected attempted=1");
    assert_eq!(sc["succeeded"].as_u64().unwrap_or(0), 1, "expected succeeded=1");
    assert_eq!(sc["failed"].as_u64().unwrap_or(99), 0, "expected failed=0");

    child.kill().ok();
    let _ = std::fs::remove_dir_all(&traj_dir);
}

#[test]
#[cfg(target_os = "macos")]
fn test_browser_eval_no_cdp_error() {
    //! Verify browser_eval is registered and returns a meaningful error when
    //! no Chromium process is listening on the given cdp_port.
    let binary = binary_path();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    // Use a port that is almost certainly not listening.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"browser_eval","arguments":{
            "expression": "1+1",
            "cdp_port": 19722
        }}
    }));
    let resp = read_response(&mut stdout);

    // The tool should return isError=true with a connection-refused message.
    assert!(
        resp["result"]["isError"].as_bool().unwrap_or(false),
        "expected isError=true when CDP port is not listening: {resp:?}"
    );
    let text = resp["result"]["content"][0]["text"].as_str().unwrap_or("");
    assert!(
        text.contains("browser_eval"),
        "error text should mention browser_eval: {text}"
    );

    child.kill().ok();
}

#[test]
#[cfg(target_os = "macos")]
fn test_screenshot_no_window_id() {
    //! Call screenshot without window_id — should capture the full display and return a PNG.
    let binary = binary_path();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"screenshot","arguments":{}}
    }));
    let resp = read_response(&mut stdout);
    assert!(resp["error"].is_null(), "Protocol error from screenshot: {resp:?}");
    let content = resp["result"]["content"].as_array().expect("content array");
    assert!(!content.is_empty(), "screenshot should return at least one content item");

    child.kill().ok();
}

#[test]
#[cfg(target_os = "macos")]
fn test_screenshot_jpeg_format() {
    //! Call screenshot with format=jpeg — should return a JPEG image.
    let binary = binary_path();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"screenshot","arguments":{"format":"jpeg","quality":70}}
    }));
    let resp = read_response(&mut stdout);
    assert!(resp["error"].is_null(), "Protocol error from screenshot: {resp:?}");
    let content = resp["result"]["content"].as_array().expect("content array");
    let is_error = resp["result"]["isError"].as_bool().unwrap_or(false);
    if !is_error {
        let has_jpeg = content.iter().any(|c| {
            c["type"] == "image" && c["mimeType"].as_str().unwrap_or("") == "image/jpeg"
                && c["data"].as_str().map(|s| s.len() > 10).unwrap_or(false)
        });
        assert!(has_jpeg, "Expected image/jpeg in screenshot response, got: {content:?}");
        // Verify structured content has width and height.
        let sc = &resp["result"]["structuredContent"];
        assert!(sc["width"].as_f64().unwrap_or(0.0) > 0.0, "Expected positive width in structuredContent");
        assert!(sc["height"].as_f64().unwrap_or(0.0) > 0.0, "Expected positive height in structuredContent");
        assert_eq!(sc["format"].as_str().unwrap_or(""), "jpeg", "Expected format=jpeg");
    }

    child.kill().ok();
}

#[test]
#[cfg(target_os = "macos")]
fn test_hotkey_keys_array() {
    //! Verify hotkey accepts a keys array (["cmd","a"]) without a protocol error.
    let binary = binary_path();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"list_windows","arguments":{}}
    }));
    let resp = read_response(&mut stdout);
    let windows = resp["result"]["structuredContent"]["windows"].as_array();
    let first_win = windows.and_then(|a| a.iter().find(|w| w["pid"].as_i64().is_some()));

    let Some(win) = first_win else {
        eprintln!("No windows found — skipping hotkey keys array test");
        child.kill().ok();
        return;
    };
    let pid = win["pid"].as_i64().unwrap();
    let wid = win["window_id"].as_u64().unwrap();

    // keys array should be parsed and the tool invoked; result may succeed or fail depending
    // on whether the target app accepts the hotkey, but there must be no protocol error.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"hotkey","arguments":{
            "pid": pid, "window_id": wid, "keys": ["cmd","a"]
        }}
    }));
    let resp = read_response(&mut stdout);
    assert!(resp["error"].is_null(), "Protocol error from hotkey: {resp:?}");
    let content = resp["result"]["content"].as_array().expect("content array");
    assert!(!content.is_empty(), "hotkey should return content");

    child.kill().ok();
}

#[test]
#[cfg(target_os = "macos")]
fn test_get_accessibility_tree() {
    //! get_accessibility_tree returns a lightweight process+window snapshot.
    let binary = binary_path();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"get_accessibility_tree","arguments":{}}
    }));
    let resp = read_response(&mut stdout);
    assert!(resp["error"].is_null(), "Protocol error: {resp:?}");
    assert!(!resp["result"]["isError"].as_bool().unwrap_or(false),
        "get_accessibility_tree returned error: {resp:?}");

    let sc = &resp["result"]["structuredContent"];
    // macOS backend returns "apps"; Linux returns "processes". Either key is valid.
    let has_apps = sc["apps"].is_array() || sc["processes"].is_array();
    assert!(has_apps, "Expected apps or processes array in structured content, got: {sc:?}");

    let content = resp["result"]["content"].as_array().expect("content array");
    assert!(!content.is_empty(), "Expected non-empty content");

    child.kill().ok();
}

#[test]
#[cfg(target_os = "macos")]
fn test_get_window_state_ax_mode() {
    //! get_window_state with capture_mode="ax" should return a tree but no screenshot image.
    let binary = binary_path();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"list_windows","arguments":{}}
    }));
    let resp = read_response(&mut stdout);
    let windows = resp["result"]["structuredContent"]["windows"].as_array();
    let first_win = windows.and_then(|a| a.iter().find(|w| {
        w["pid"].as_i64().is_some() && w["window_id"].as_u64().is_some()
    }));

    let Some(win) = first_win else {
        eprintln!("No windows found — skipping get_window_state ax test");
        child.kill().ok();
        return;
    };
    let pid = win["pid"].as_i64().unwrap();
    let wid = win["window_id"].as_u64().unwrap();

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"get_window_state","arguments":{
            "pid": pid, "window_id": wid, "capture_mode": "ax"
        }}
    }));
    let resp = read_response(&mut stdout);
    assert!(resp["error"].is_null(), "Protocol error from get_window_state: {resp:?}");

    let content = resp["result"]["content"].as_array().expect("content array");
    // ax mode must NOT return an image content item.
    let has_image = content.iter().any(|c| c["type"] == "image");
    assert!(!has_image, "capture_mode=ax should not return an image, got: {content:?}");
    assert!(!content.is_empty(), "Expected at least one content item");

    // structuredContent must include window_id, pid, element_count.
    let sc = &resp["result"]["structuredContent"];
    assert_eq!(sc["window_id"].as_u64().unwrap_or(0), wid, "structuredContent.window_id mismatch");
    assert_eq!(sc["pid"].as_i64().unwrap_or(0), pid, "structuredContent.pid mismatch");
    assert!(sc["element_count"].is_number(), "expected element_count in structuredContent: {sc:?}");
    // ax mode — screenshot_width should NOT be present.
    assert!(sc["screenshot_width"].is_null(), "ax mode should not have screenshot_width: {sc:?}");

    // Also test vision mode: screenshot_width/height should be present.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":4,"method":"tools/call",
        "params":{"name":"get_window_state","arguments":{
            "pid": pid, "window_id": wid, "capture_mode": "vision"
        }}
    }));
    let vision_resp = read_response(&mut stdout);
    assert!(vision_resp["error"].is_null(), "Protocol error from vision get_window_state: {vision_resp:?}");
    if !vision_resp["result"]["isError"].as_bool().unwrap_or(false) {
        let vsc = &vision_resp["result"]["structuredContent"];
        assert!(vsc["screenshot_width"].as_f64().unwrap_or(0.0) > 0.0,
            "vision mode should have screenshot_width: {vsc:?}");
        assert!(vsc["screenshot_height"].as_f64().unwrap_or(0.0) > 0.0,
            "vision mode should have screenshot_height: {vsc:?}");
    }

    child.kill().ok();
}

#[test]
#[cfg(target_os = "macos")]
fn test_type_text_chars_type_chars_only_schema() {
    //! Verify type_text_chars schema includes type_chars_only param (cross-platform parity),
    //! and that passing type_chars_only=true doesn't cause an additionalProperties error.
    let binary = binary_path();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    // Verify schema contains type_chars_only via tools/list.
    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}));
    let list_resp = read_response(&mut stdout);
    let tools = list_resp["result"]["tools"].as_array().expect("tools array");
    let ttc = tools.iter().find(|t| t["name"] == "type_text_chars")
        .expect("type_text_chars not found in tools/list");
    let props = &ttc["inputSchema"]["properties"];
    assert!(props["type_chars_only"].is_object(),
        "type_text_chars schema missing type_chars_only property: {props:?}");

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

    child.kill().ok();
}

#[test]
#[cfg(target_os = "macos")]
fn test_list_windows_structured_content() {
    //! Verify list_windows returns structuredContent.windows array with expected fields.
    let binary = binary_path();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"list_windows","arguments":{}}
    }));
    let resp = read_response(&mut stdout);
    assert!(resp["error"].is_null(), "Protocol error: {resp:?}");
    assert!(!resp["result"]["isError"].as_bool().unwrap_or(false),
        "list_windows returned error: {resp:?}");

    let sc = &resp["result"]["structuredContent"];
    assert!(sc["windows"].is_array(), "Expected windows array in structuredContent: {sc:?}");

    // If there are any windows, verify the expected fields are present.
    if let Some(wins) = sc["windows"].as_array() {
        if let Some(w) = wins.first() {
            assert!(w["window_id"].is_number(), "window_id missing from window: {w:?}");
            assert!(w["pid"].is_number(), "pid missing from window: {w:?}");
        }
    }

    child.kill().ok();
}

#[test]
#[cfg(target_os = "macos")]
fn test_press_key_harmless() {
    //! press_key with a harmless key (F24 — virtually no app responds to it) sent to the
    //! first available window. Just verifies the tool doesn't return a protocol error.
    let binary = binary_path();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    // Find a window to target.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"list_windows","arguments":{}}
    }));
    let resp = read_response(&mut stdout);
    let wins = resp["result"]["structuredContent"]["windows"].as_array();
    let Some(win) = wins.and_then(|a| a.iter().find(|w| w["pid"].as_i64().is_some())) else {
        eprintln!("No windows found — skipping press_key test");
        child.kill().ok();
        return;
    };
    let pid = win["pid"].as_i64().unwrap();
    let wid = win["window_id"].as_u64().unwrap();

    // F24 is a function key that virtually no app binds — safe to send.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"press_key","arguments":{"pid": pid, "window_id": wid, "key": "F24"}}
    }));
    let resp = read_response(&mut stdout);
    assert!(resp["error"].is_null(), "Protocol error from press_key: {resp:?}");
    // Tool may succeed or return an app error, but must NOT be a protocol-level error.
    let content = resp["result"]["content"].as_array().expect("content array");
    assert!(!content.is_empty(), "press_key returned empty content");

    child.kill().ok();
}

#[test]
#[cfg(target_os = "macos")]
fn test_scroll_tool() {
    //! scroll with direction=down, by=line, amount=1 against the first available window.
    //! Verifies the tool is accepted and returns content, not a protocol error.
    let binary = binary_path();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    // Find a pid with a window.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"list_apps","arguments":{}}
    }));
    let resp = read_response(&mut stdout);
    let apps = resp["result"]["structuredContent"]["apps"].as_array();
    let Some(app) = apps.and_then(|a| a.iter().find(|a| a["pid"].as_i64().is_some())) else {
        eprintln!("No apps found — skipping scroll test");
        child.kill().ok();
        return;
    };
    let pid = app["pid"].as_i64().unwrap();

    // scroll with minimal parameters — window_id is auto-resolved from pid.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"scroll","arguments":{"pid": pid, "direction": "down", "amount": 1}}
    }));
    let resp = read_response(&mut stdout);
    assert!(resp["error"].is_null(), "Protocol error from scroll: {resp:?}");
    let content = resp["result"]["content"].as_array().expect("content array");
    assert!(!content.is_empty(), "scroll returned empty content");

    child.kill().ok();
}

#[test]
#[cfg(target_os = "macos")]
fn test_click_pixel_path() {
    //! click at window-local (5, 5) — title bar area, safe to click without disrupting UI.
    //! Verifies the pixel-coordinate path for click works without a protocol error.
    let binary = binary_path();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    // Find an on-screen window.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"list_windows","arguments":{"on_screen_only": true}}
    }));
    let resp = read_response(&mut stdout);
    let wins = resp["result"]["structuredContent"]["windows"].as_array();
    let Some(win) = wins.and_then(|a| a.iter().find(|w| w["pid"].as_i64().is_some())) else {
        eprintln!("No on-screen windows — skipping click pixel path test");
        child.kill().ok();
        return;
    };
    let pid = win["pid"].as_i64().unwrap();
    let wid = win["window_id"].as_u64().unwrap();

    // Click at (5, 5) in window-local coords — title bar, harmless.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"click","arguments":{"pid": pid, "window_id": wid, "x": 5.0, "y": 5.0}}
    }));
    let resp = read_response(&mut stdout);
    assert!(resp["error"].is_null(), "Protocol error from click: {resp:?}");
    let content = resp["result"]["content"].as_array().expect("content array");
    assert!(!content.is_empty(), "click returned empty content");

    child.kill().ok();
}

#[test]
#[cfg(target_os = "macos")]
fn test_type_text_tool() {
    //! Opens TextEdit (or reuses it if already running) and types a short string via type_text.
    //! Skips gracefully if TextEdit is not available or has no window.
    let binary = binary_path();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    // Launch TextEdit (no-op if already running).
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"launch_app","arguments":{"name":"TextEdit"}}
    }));
    read_response(&mut stdout);
    std::thread::sleep(std::time::Duration::from_millis(800));

    // Find TextEdit's pid.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"list_apps","arguments":{}}
    }));
    let resp = read_response(&mut stdout);
    let apps = resp["result"]["structuredContent"]["apps"].as_array();
    let Some(app) = apps.and_then(|a| {
        a.iter().find(|a| {
            a["name"].as_str().map(|n| n.eq_ignore_ascii_case("textedit")).unwrap_or(false)
        })
    }) else {
        eprintln!("TextEdit not found — skipping type_text test");
        child.kill().ok();
        return;
    };
    let pid = app["pid"].as_i64().unwrap();

    // Find TextEdit's first on-screen window.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":4,"method":"tools/call",
        "params":{"name":"list_windows","arguments":{"pid": pid, "on_screen_only": true}}
    }));
    let resp = read_response(&mut stdout);
    let wins = resp["result"]["structuredContent"]["windows"].as_array();
    let Some(win) = wins.and_then(|a| a.first()) else {
        eprintln!("TextEdit has no on-screen windows — skipping type_text test");
        child.kill().ok();
        return;
    };
    let wid = win["window_id"].as_u64().unwrap();

    // type_text into TextEdit's window.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":5,"method":"tools/call",
        "params":{"name":"type_text","arguments":{"pid": pid, "window_id": wid, "text": "cua-test"}}
    }));
    let resp = read_response(&mut stdout);
    assert!(resp["error"].is_null(), "Protocol error from type_text: {resp:?}");
    let content = resp["result"]["content"].as_array().expect("content array");
    assert!(!content.is_empty(), "type_text returned empty content");

    child.kill().ok();
}

#[test]
#[cfg(target_os = "macos")]
fn test_double_click_and_right_click_pixel_path() {
    //! double_click and right_click at title-bar coords — verifies both tools accept pixel path.
    let binary = binary_path();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"list_windows","arguments":{"on_screen_only": true}}
    }));
    let resp = read_response(&mut stdout);
    let wins = resp["result"]["structuredContent"]["windows"].as_array();
    let Some(win) = wins.and_then(|a| a.iter().find(|w| w["pid"].as_i64().is_some())) else {
        eprintln!("No on-screen windows — skipping double/right click test");
        child.kill().ok();
        return;
    };
    let pid = win["pid"].as_i64().unwrap();
    let wid = win["window_id"].as_u64().unwrap();

    // double_click at safe coords.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"double_click","arguments":{"pid": pid, "window_id": wid, "x": 5.0, "y": 5.0}}
    }));
    let resp = read_response(&mut stdout);
    assert!(resp["error"].is_null(), "Protocol error from double_click: {resp:?}");
    assert!(!resp["result"]["content"].as_array().map(|a| a.is_empty()).unwrap_or(true),
        "double_click returned empty content");

    // right_click at same coords.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":4,"method":"tools/call",
        "params":{"name":"right_click","arguments":{"pid": pid, "window_id": wid, "x": 5.0, "y": 5.0}}
    }));
    let resp = read_response(&mut stdout);
    assert!(resp["error"].is_null(), "Protocol error from right_click: {resp:?}");
    assert!(!resp["result"]["content"].as_array().map(|a| a.is_empty()).unwrap_or(true),
        "right_click returned empty content");

    child.kill().ok();
}

#[test]
#[cfg(target_os = "macos")]
fn test_set_value_via_element_index() {
    //! get_window_state on TextEdit → find a text-area element → set_value on it.
    //! Skips gracefully if TextEdit is not available.
    let binary = binary_path();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    // Ensure TextEdit is running.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"launch_app","arguments":{"name":"TextEdit"}}
    }));
    read_response(&mut stdout);
    std::thread::sleep(std::time::Duration::from_millis(800));

    // Find TextEdit.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"list_apps","arguments":{}}
    }));
    let resp = read_response(&mut stdout);
    let apps = resp["result"]["structuredContent"]["apps"].as_array();
    let Some(app) = apps.and_then(|a| {
        a.iter().find(|a| a["name"].as_str()
            .map(|n| n.eq_ignore_ascii_case("textedit")).unwrap_or(false))
    }) else {
        eprintln!("TextEdit not found — skipping set_value test");
        child.kill().ok();
        return;
    };
    let pid = app["pid"].as_i64().unwrap();

    // Get first window.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":4,"method":"tools/call",
        "params":{"name":"list_windows","arguments":{"pid": pid, "on_screen_only": true}}
    }));
    let resp = read_response(&mut stdout);
    let wins = resp["result"]["structuredContent"]["windows"].as_array();
    let Some(win) = wins.and_then(|a| a.first()) else {
        eprintln!("TextEdit has no on-screen windows — skipping set_value test");
        child.kill().ok();
        return;
    };
    let wid = win["window_id"].as_u64().unwrap();

    // Walk AX tree to get element_count — we'll target element_index 0.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":5,"method":"tools/call",
        "params":{"name":"get_window_state","arguments":{"pid": pid, "window_id": wid, "capture_mode": "ax"}}
    }));
    let resp = read_response(&mut stdout);
    assert!(resp["error"].is_null(), "Protocol error from get_window_state: {resp:?}");
    let element_count = resp["result"]["structuredContent"]["element_count"].as_u64().unwrap_or(0);
    if element_count == 0 {
        eprintln!("No AX elements — skipping set_value test");
        child.kill().ok();
        return;
    }

    // set_value on element 0 — this is the document / text area in a new TextEdit document.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":6,"method":"tools/call",
        "params":{"name":"set_value","arguments":{"pid": pid, "window_id": wid, "element_index": 0, "value": "cua-test-value"}}
    }));
    let resp = read_response(&mut stdout);
    // set_value may fail if element 0 is not settable; accept tool-level errors but not protocol errors.
    assert!(resp["error"].is_null(), "Protocol error from set_value: {resp:?}");
    let content = resp["result"]["content"].as_array().expect("content array");
    assert!(!content.is_empty(), "set_value returned empty content");

    child.kill().ok();
}

#[test]
#[cfg(target_os = "macos")]
fn test_click_debug_image_out() {
    //! click with debug_image_out writes a PNG crosshair file and then proceeds.
    //! Verifies the debug capture path works end-to-end.
    let binary = binary_path();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"list_windows","arguments":{"on_screen_only": true}}
    }));
    let resp = read_response(&mut stdout);
    let wins = resp["result"]["structuredContent"]["windows"].as_array();
    // Prefer a window with a non-empty title to avoid uncapturable overlay/system windows.
    let Some(win) = wins.and_then(|a| a.iter().find(|w| {
        w["pid"].as_i64().is_some() && !w["title"].as_str().unwrap_or("").is_empty()
    }).or_else(|| a.iter().find(|w| w["pid"].as_i64().is_some()))) else {
        eprintln!("No on-screen windows — skipping debug_image_out test");
        child.kill().ok();
        return;
    };
    let pid = win["pid"].as_i64().unwrap();
    let wid = win["window_id"].as_u64().unwrap();

    let dbg_path = std::env::temp_dir().join(format!("cua-driver-rs-dbg-{}.png", std::process::id()));
    let dbg_path_str = dbg_path.to_string_lossy().to_string();

    // Click with debug_image_out — should write the PNG and then proceed.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"click","arguments":{
            "pid": pid, "window_id": wid,
            "x": 10.0, "y": 10.0,
            "debug_image_out": dbg_path_str
        }}
    }));
    let resp = read_response(&mut stdout);
    assert!(resp["error"].is_null(), "Protocol error from click+debug_image_out: {resp:?}");

    // The tool may return isError if screencapture can't capture this particular window
    // (e.g., overlay windows, system windows). If that happens, skip the PNG check.
    let is_error = resp["result"]["isError"].as_bool().unwrap_or(false);
    if is_error {
        let msg = resp["result"]["content"][0]["text"].as_str().unwrap_or("?");
        eprintln!("debug_image_out screenshot failed for this window ({msg}) — skipping PNG check");
        child.kill().ok();
        return;
    }

    // Verify the PNG was actually written.
    assert!(dbg_path.exists(), "debug_image_out PNG was not written to {dbg_path:?}");
    let meta = std::fs::metadata(&dbg_path).expect("metadata");
    assert!(meta.len() > 100, "debug_image_out PNG is suspiciously small");

    let _ = std::fs::remove_file(&dbg_path);
    child.kill().ok();
}

#[test]
#[cfg(target_os = "macos")]
fn test_concurrent_multi_driver_isolation() {
    //! Two cua-driver processes running simultaneously with different cursor IDs.
    //! Verifies that concurrent sessions don't interfere — each tracks its own
    //! cursor state and the overlay stays alive under concurrent load.
    let binary = binary_path();
    if !binary.exists() { return; }

    // Spawn two drivers concurrently.
    let mut child_a = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn driver A");
    let mut child_b = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn driver B");

    let stdin_a = child_a.stdin.as_mut().unwrap();
    let mut stdout_a = BufReader::new(child_a.stdout.as_mut().unwrap());
    let stdin_b = child_b.stdin.as_mut().unwrap();
    let mut stdout_b = BufReader::new(child_b.stdout.as_mut().unwrap());

    // Initialize both.
    send_request(stdin_a, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    send_request(stdin_b, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout_a);
    read_response(&mut stdout_b);

    // Driver A: set_agent_cursor_enabled for cursor "alpha", Driver B for cursor "beta".
    send_request(stdin_a, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"set_agent_cursor_enabled","arguments":{"cursor_id":"alpha","enabled":true}}
    }));
    send_request(stdin_b, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"set_agent_cursor_enabled","arguments":{"cursor_id":"beta","enabled":false}}
    }));
    let resp_a = read_response(&mut stdout_a);
    let resp_b = read_response(&mut stdout_b);
    assert!(resp_a["error"].is_null(), "Driver A set_agent_cursor_enabled failed: {resp_a:?}");
    assert!(resp_b["error"].is_null(), "Driver B set_agent_cursor_enabled failed: {resp_b:?}");

    // Each driver queries its own cursor state — should reflect what it set.
    // get_agent_cursor_state takes no arguments; returns { "cursors": [...] }.
    send_request(stdin_a, &serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"get_agent_cursor_state","arguments":{}}
    }));
    send_request(stdin_b, &serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"get_agent_cursor_state","arguments":{}}
    }));
    let state_a = read_response(&mut stdout_a);
    let state_b = read_response(&mut stdout_b);
    assert!(state_a["error"].is_null(), "Driver A get_agent_cursor_state failed: {state_a:?}");
    assert!(state_b["error"].is_null(), "Driver B get_agent_cursor_state failed: {state_b:?}");

    // cursors is an array of { config: { cursor_id, enabled, ... }, x, y }.
    // Cursor alpha should be enabled (we set true), beta disabled (we set false).
    // The state lives per process — processes don't share CursorRegistry.
    let cursors_a = state_a["result"]["structuredContent"]["cursors"].as_array();
    let cursors_b = state_b["result"]["structuredContent"]["cursors"].as_array();

    let alpha_enabled = cursors_a.and_then(|arr| arr.iter().find(|c| {
        c["config"]["cursor_id"].as_str() == Some("alpha")
    })).and_then(|c| c["config"]["enabled"].as_bool());
    let beta_enabled = cursors_b.and_then(|arr| arr.iter().find(|c| {
        c["config"]["cursor_id"].as_str() == Some("beta")
    })).and_then(|c| c["config"]["enabled"].as_bool());

    assert_eq!(alpha_enabled, Some(true),  "Cursor alpha should be enabled in driver A");
    assert_eq!(beta_enabled,  Some(false), "Cursor beta should be disabled in driver B");

    // Move cursor on driver A without crashing driver B.
    send_request(stdin_a, &serde_json::json!({
        "jsonrpc":"2.0","id":4,"method":"tools/call",
        "params":{"name":"move_cursor","arguments":{"cursor_id":"alpha","x":100,"y":100}}
    }));
    send_request(stdin_b, &serde_json::json!({
        "jsonrpc":"2.0","id":4,"method":"tools/call",
        "params":{"name":"move_cursor","arguments":{"cursor_id":"beta","x":200,"y":200}}
    }));
    let mv_a = read_response(&mut stdout_a);
    let mv_b = read_response(&mut stdout_b);
    assert!(mv_a["error"].is_null(), "Driver A move_cursor failed: {mv_a:?}");
    assert!(mv_b["error"].is_null(), "Driver B move_cursor failed: {mv_b:?}");

    child_a.kill().ok();
    child_b.kill().ok();
}

#[test]
#[cfg(target_os = "macos")]
fn test_set_config_screenshot_resize() {
    //! set_config(max_image_dimension=200) then screenshot — returned image must
    //! have both dimensions ≤ 200. Verifies the ResizeRegistry pipeline end-to-end.
    let binary = binary_path();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    // Set a tiny max_image_dimension so the full display screenshot will be resized.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"set_config","arguments":{"max_image_dimension":200}}
    }));
    let resp = read_response(&mut stdout);
    assert!(resp["error"].is_null(), "set_config failed: {resp:?}");
    assert!(!resp["result"]["isError"].as_bool().unwrap_or(false), "set_config returned error: {resp:?}");

    // Capture full display; must be ≤ 200 px in both dimensions.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"screenshot","arguments":{}}
    }));
    let resp = read_response(&mut stdout);
    assert!(resp["error"].is_null(), "screenshot failed: {resp:?}");

    let w = resp["result"]["structuredContent"]["width"].as_u64().unwrap_or(9999);
    let h = resp["result"]["structuredContent"]["height"].as_u64().unwrap_or(9999);
    assert!(w <= 200, "screenshot width {w} should be ≤ 200 after max_image_dimension=200");
    assert!(h <= 200, "screenshot height {h} should be ≤ 200 after max_image_dimension=200");

    // get_config should reflect the updated value.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":4,"method":"tools/call",
        "params":{"name":"get_config","arguments":{}}
    }));
    let resp = read_response(&mut stdout);
    let dim = resp["result"]["structuredContent"]["max_image_dimension"].as_u64();
    assert_eq!(dim, Some(200), "get_config should reflect max_image_dimension=200");

    child.kill().ok();
}

#[test]
#[cfg(target_os = "macos")]
fn test_zoom_from_zoom_click_round_trip() {
    //! Verify the zoom → from_zoom click pipeline:
    //! 1. click(from_zoom=true) with no zoom context returns the expected error.
    //! 2. zoom() stores context; subsequent click(from_zoom=true) does NOT return
    //!    that error (translation succeeded, even if the click itself may fail).
    let binary = binary_path();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    // List on-screen windows to find a target window and pid.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"list_windows","arguments":{"on_screen_only":true}}
    }));
    let resp = read_response(&mut stdout);
    let wins = resp["result"]["structuredContent"]["windows"].as_array().cloned().unwrap_or_default();
    let Some(win) = wins.iter().find(|w| w["pid"].as_i64().is_some()) else {
        eprintln!("No on-screen windows — skipping from_zoom test");
        child.kill().ok();
        return;
    };
    let window_id = win["window_id"].as_u64().unwrap();
    let pid = win["pid"].as_i64().unwrap();

    // Step 1: click with from_zoom=true and NO prior zoom context — expect error.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"click","arguments":{"pid": pid, "window_id": window_id, "x":10.0,"y":10.0,"from_zoom":true}}
    }));
    let resp = read_response(&mut stdout);
    let is_err = resp["result"]["isError"].as_bool().unwrap_or(false);
    let err_text = resp["result"]["content"][0]["text"].as_str().unwrap_or("");
    assert!(is_err, "Expected error when from_zoom=true with no context, got: {resp:?}");
    assert!(err_text.contains("no zoom context"), "Expected 'no zoom context' error, got: {err_text}");

    // Step 2: call zoom on that window to store context.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":4,"method":"tools/call",
        "params":{"name":"zoom","arguments":{"window_id": window_id, "pid": pid, "x1":0,"y1":0,"x2":50,"y2":50}}
    }));
    let resp = read_response(&mut stdout);
    if resp["result"]["isError"].as_bool().unwrap_or(false) {
        // Window may not be capturable (e.g. system UI) — skip.
        eprintln!("zoom failed — skipping from_zoom click test");
        child.kill().ok();
        return;
    }
    // Verify zoom returned structured content with width/height.
    let sc = &resp["result"]["structuredContent"];
    assert!(sc["width"].as_u64().unwrap_or(0) > 0, "zoom should return positive width: {sc:?}");

    // Step 3: click with from_zoom=true — should NOT return the "no zoom context" error.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":5,"method":"tools/call",
        "params":{"name":"click","arguments":{"pid": pid, "window_id": window_id, "x":10.0,"y":10.0,"from_zoom":true}}
    }));
    let resp = read_response(&mut stdout);
    let is_err = resp["result"]["isError"].as_bool().unwrap_or(false);
    let err_text = resp["result"]["content"][0]["text"].as_str().unwrap_or("");
    // Translation should succeed — if click fails it's for another reason (target app state), not missing context.
    assert!(!err_text.contains("no zoom context"),
        "After zoom(), from_zoom click should not say 'no zoom context', got: {err_text}");
    // (is_err may be true if the click target was invalid; that's acceptable.)
    let _ = is_err;

    child.kill().ok();
}

#[test]
#[cfg(target_os = "macos")]
fn test_set_recording_video_experimental_accepted() {
    //! video_experimental is a valid schema parameter — must be accepted without error.
    let binary = binary_path();
    if !binary.exists() { return; }

    let tmp_dir = std::env::temp_dir().join(format!("cua-driver-rs-videxp-{}", std::process::id()));
    let tmp_str = tmp_dir.to_string_lossy().to_string();
    std::fs::create_dir_all(&tmp_dir).unwrap();

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    // Enable recording with video_experimental=true — must not return a schema error.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"set_recording","arguments":{
            "enabled": true, "output_dir": tmp_str, "video_experimental": true
        }}
    }));
    let resp = read_response(&mut stdout);
    // Must not be a JSON-RPC protocol error.
    assert!(resp["error"].is_null(), "Expected no JSON-RPC error, got: {resp:?}");
    // Tool result may note the feature is not implemented, but must not be a schema validation error.
    let is_err = resp["result"]["isError"].as_bool().unwrap_or(false);
    assert!(!is_err, "set_recording with video_experimental should succeed (note-only): {resp:?}");

    // Disable recording.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"set_recording","arguments":{"enabled":false}}
    }));
    read_response(&mut stdout);
    child.kill().ok();
    let _ = std::fs::remove_dir_all(&tmp_dir);
}

// ─────────────────────────────────────────────────────────────────────────────
// Windows integration tests
//
// Mirrors the macOS tests above with Windows-specific adaptations:
//   - binary path includes .exe
//   - list_apps returns structuredContent.processes (not .apps)
//   - get_config returns platform="windows"
//   - check_permissions returns { elevated, uia, post_message } (not accessibility)
//   - get_accessibility_tree returns processes + windows keys
//   - type_text tests target Notepad instead of TextEdit
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(target_os = "windows")]
fn binary_path_windows() -> std::path::PathBuf {
    binary_path().with_extension("exe")
}

#[test]
#[cfg(target_os = "windows")]
fn test_initialize_handshake_windows() {
    let binary = binary_path_windows();
    if !binary.exists() {
        eprintln!("Binary not found at {:?} — run `cargo build` first", binary);
        return;
    }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn cua-driver");

    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "clientInfo": { "name": "test-client", "version": "0.0.1" }
        }
    }));
    let resp = read_response(&mut stdout);
    assert_eq!(resp["jsonrpc"], "2.0");
    assert_eq!(resp["id"], 1);
    assert!(resp["result"]["protocolVersion"].is_string());
    assert_eq!(resp["result"]["serverInfo"]["name"], "cua-driver-rs");

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}));

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
    let resp = read_response(&mut stdout);
    assert_eq!(resp["id"], 2);
    let tools = resp["result"]["tools"].as_array().expect("tools array");
    assert!(!tools.is_empty());

    let tool_names: Vec<&str> = tools.iter()
        .filter_map(|t| t["name"].as_str()).collect();
    for expected in &["list_apps", "list_windows", "get_window_state", "click", "type_text", "press_key"] {
        assert!(tool_names.contains(expected), "Missing tool: {}", expected);
    }

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":3,"method":"unknown/method"}));
    let resp = read_response(&mut stdout);
    assert_eq!(resp["id"], 3);
    assert_eq!(resp["error"]["code"], -32601);

    child.kill().ok();
}

#[test]
#[cfg(target_os = "windows")]
fn test_tools_call_list_apps_windows() {
    let binary = binary_path_windows();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"list_apps","arguments":{}}
    }));
    let resp = read_response(&mut stdout);
    assert_eq!(resp["id"], 2);
    assert!(resp["result"]["content"].is_array());
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Found"), "Expected process list text, got: {}", text);
    // Windows backend returns "processes" key.
    assert!(resp["result"]["structuredContent"]["processes"].is_array(),
        "Expected processes array: {:?}", resp["result"]["structuredContent"]);

    child.kill().ok();
}

#[test]
#[cfg(target_os = "windows")]
fn test_tools_call_unknown_tool_windows() {
    let binary = binary_path_windows();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"nonexistent_tool","arguments":{}}
    }));
    let resp = read_response(&mut stdout);
    let is_error = resp["result"]["isError"].as_bool().unwrap_or(false);
    assert!(is_error, "Expected isError=true for unknown tool");

    child.kill().ok();
}

#[test]
#[cfg(target_os = "windows")]
fn test_all_expected_tools_registered_windows() {
    let binary = binary_path_windows();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
    let resp = read_response(&mut stdout);
    let tools = resp["result"]["tools"].as_array().expect("tools array");
    let names: std::collections::HashSet<&str> = tools.iter()
        .filter_map(|t| t["name"].as_str()).collect();

    let expected = [
        "list_apps", "list_windows", "get_window_state", "launch_app",
        "click", "double_click", "right_click", "type_text", "type_text_chars",
        "press_key", "hotkey", "set_value", "scroll", "screenshot", "zoom",
        "get_screen_size", "get_cursor_position",
        "move_cursor", "set_agent_cursor_enabled", "set_agent_cursor_motion",
        "get_agent_cursor_state", "check_permissions", "get_config", "set_config",
        "get_accessibility_tree",
        "set_recording", "get_recording_state", "replay_trajectory",
        "browser_eval",
    ];
    for name in &expected {
        assert!(names.contains(name), "Missing tool: {name}  (registered: {names:?})");
    }

    child.kill().ok();
}

#[test]
#[cfg(target_os = "windows")]
fn test_get_screen_size_and_cursor_position_windows() {
    let binary = binary_path_windows();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"get_screen_size","arguments":{}}
    }));
    let resp = read_response(&mut stdout);
    assert!(!resp["result"]["isError"].as_bool().unwrap_or(false), "get_screen_size failed: {resp:?}");
    let sc = &resp["result"]["structuredContent"];
    assert!(sc["width"].as_f64().unwrap_or(0.0) > 0.0);
    assert!(sc["height"].as_f64().unwrap_or(0.0) > 0.0);

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"get_cursor_position","arguments":{}}
    }));
    let resp = read_response(&mut stdout);
    assert!(!resp["result"]["isError"].as_bool().unwrap_or(false), "get_cursor_position failed: {resp:?}");
    assert!(resp["result"]["structuredContent"]["x"].is_number());
    assert!(resp["result"]["structuredContent"]["y"].is_number());

    child.kill().ok();
}

#[test]
#[cfg(target_os = "windows")]
fn test_get_config_and_check_permissions_windows() {
    let binary = binary_path_windows();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"get_config","arguments":{}}
    }));
    let resp = read_response(&mut stdout);
    assert!(!resp["result"]["isError"].as_bool().unwrap_or(false));
    assert_eq!(resp["result"]["structuredContent"]["platform"], "windows");

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"check_permissions","arguments":{}}
    }));
    let resp = read_response(&mut stdout);
    assert!(!resp["result"]["isError"].as_bool().unwrap_or(false));
    let sc = &resp["result"]["structuredContent"];
    // Windows returns elevated, uia, post_message booleans.
    assert!(sc["uia"].as_bool().unwrap_or(false), "uia should be true: {sc:?}");
    assert!(sc["post_message"].as_bool().unwrap_or(false), "post_message should be true: {sc:?}");
    assert!(sc["elevated"].is_boolean(), "elevated should be a boolean: {sc:?}");

    // set_config — change capture_mode and verify get_config reflects it.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":4,"method":"tools/call",
        "params":{"name":"set_config","arguments":{"capture_mode":"ax","max_image_dimension":1920}}
    }));
    let resp = read_response(&mut stdout);
    assert!(resp["error"].is_null(), "Protocol error from set_config: {resp:?}");

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":5,"method":"tools/call",
        "params":{"name":"get_config","arguments":{}}
    }));
    let resp = read_response(&mut stdout);
    assert_eq!(resp["result"]["structuredContent"]["capture_mode"], "ax");
    assert_eq!(resp["result"]["structuredContent"]["max_image_dimension"], 1920);

    child.kill().ok();
}

#[test]
#[cfg(target_os = "windows")]
fn test_concurrent_clients_windows() {
    let binary = binary_path_windows();
    if !binary.exists() { return; }

    let mut processes: Vec<std::process::Child> = (0..2).map(|_| {
        Command::new(&binary)
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
            .spawn().expect("spawn")
    }).collect();

    for (i, child) in processes.iter_mut().enumerate() {
        let stdin = child.stdin.as_mut().unwrap();
        let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());
        send_request(stdin, &serde_json::json!({
            "jsonrpc":"2.0","id": i + 1,"method":"initialize","params":{}
        }));
        let resp = read_response(&mut stdout);
        assert_eq!(resp["id"], (i + 1) as i64);
        assert!(resp["result"]["protocolVersion"].is_string(), "Process {i} failed to initialize");
    }

    for mut child in processes { child.kill().ok(); }
}

#[test]
#[cfg(target_os = "windows")]
fn test_list_windows_structured_content_windows() {
    let binary = binary_path_windows();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"list_windows","arguments":{}}
    }));
    let resp = read_response(&mut stdout);
    assert!(resp["error"].is_null(), "Protocol error: {resp:?}");
    assert!(!resp["result"]["isError"].as_bool().unwrap_or(false));
    let sc = &resp["result"]["structuredContent"];
    assert!(sc["windows"].is_array(), "Expected windows array: {sc:?}");
    if let Some(wins) = sc["windows"].as_array() {
        if let Some(w) = wins.first() {
            assert!(w["window_id"].is_number(), "window_id missing: {w:?}");
            assert!(w["pid"].is_number(), "pid missing: {w:?}");
        }
    }

    child.kill().ok();
}

#[test]
#[cfg(target_os = "windows")]
fn test_get_accessibility_tree_windows() {
    let binary = binary_path_windows();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"get_accessibility_tree","arguments":{}}
    }));
    let resp = read_response(&mut stdout);
    assert!(resp["error"].is_null(), "Protocol error: {resp:?}");
    assert!(!resp["result"]["isError"].as_bool().unwrap_or(false));
    let sc = &resp["result"]["structuredContent"];
    // Windows backend returns both "processes" and "windows" keys.
    assert!(sc["processes"].is_array(), "Expected processes array: {sc:?}");
    assert!(sc["windows"].is_array(), "Expected windows array: {sc:?}");
    let content = resp["result"]["content"].as_array().expect("content array");
    assert!(!content.is_empty());

    child.kill().ok();
}

#[test]
#[cfg(target_os = "windows")]
fn test_screenshot_windows() {
    let binary = binary_path_windows();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    // Full-display screenshot (no window_id).
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"screenshot","arguments":{}}
    }));
    let resp = read_response(&mut stdout);
    assert!(resp["error"].is_null(), "Protocol error: {resp:?}");
    let content = resp["result"]["content"].as_array().expect("content array");
    assert!(!content.is_empty(), "screenshot returned empty content");

    // JPEG format.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"screenshot","arguments":{"format":"jpeg","quality":70}}
    }));
    let resp = read_response(&mut stdout);
    assert!(resp["error"].is_null(), "Protocol error from screenshot jpeg: {resp:?}");
    if !resp["result"]["isError"].as_bool().unwrap_or(false) {
        let content = resp["result"]["content"].as_array().expect("content array");
        let has_jpeg = content.iter().any(|c| {
            c["type"] == "image" && c["mimeType"].as_str().unwrap_or("") == "image/jpeg"
                && c["data"].as_str().map(|s| s.len() > 10).unwrap_or(false)
        });
        assert!(has_jpeg, "Expected image/jpeg in screenshot response: {content:?}");
        let sc = &resp["result"]["structuredContent"];
        assert!(sc["width"].as_f64().unwrap_or(0.0) > 0.0);
        assert!(sc["height"].as_f64().unwrap_or(0.0) > 0.0);
        assert_eq!(sc["format"].as_str().unwrap_or(""), "jpeg");
    }

    child.kill().ok();
}

#[test]
#[cfg(target_os = "windows")]
fn test_zoom_tool_returns_jpeg_windows() {
    let binary = binary_path_windows();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"list_windows","arguments":{"on_screen_only":true}}
    }));
    let resp = read_response(&mut stdout);
    let windows = resp["result"]["structuredContent"]["windows"]
        .as_array().cloned().unwrap_or_default();

    let mut found_jpeg = false;
    let mut tried = 0usize;
    for win in windows.iter().take(5) {
        let Some(wid) = win["window_id"].as_u64() else { continue; };
        tried += 1;
        send_request(stdin, &serde_json::json!({
            "jsonrpc":"2.0","id": 2 + tried as u64,"method":"tools/call",
            "params":{"name":"zoom","arguments":{
                "window_id":wid,"x1":0,"y1":0,"x2":100,"y2":100
            }}
        }));
        let r = read_response(&mut stdout);
        if r["result"]["isError"].as_bool().unwrap_or(false) { continue; }
        let content = r["result"]["content"].as_array().expect("content array");
        found_jpeg = content.iter().any(|c| {
            c["type"] == "image" && c["mimeType"].as_str().unwrap_or("") == "image/jpeg"
                && c["data"].as_str().map(|s| s.len() > 10).unwrap_or(false)
        });
        if found_jpeg {
            let sc = &r["result"]["structuredContent"];
            assert!(sc["width"].as_f64().unwrap_or(0.0) > 0.0);
            assert!(sc["height"].as_f64().unwrap_or(0.0) > 0.0);
            assert_eq!(sc["format"].as_str().unwrap_or(""), "jpeg");
            break;
        }
    }

    if tried == 0 {
        eprintln!("No on-screen windows found — skipping zoom test");
        child.kill().ok();
        return;
    }
    assert!(found_jpeg, "Expected at least one window to return a valid JPEG from zoom");

    child.kill().ok();
}

#[test]
#[cfg(target_os = "windows")]
fn test_overlay_move_cursor_stays_alive_windows() {
    let binary = binary_path_windows();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    for (id, (x, y)) in [(2u64, (100.0_f64, 200.0_f64)), (3, (400.0, 300.0)), (4, (800.0, 600.0))] {
        send_request(stdin, &serde_json::json!({
            "jsonrpc":"2.0","id":id,"method":"tools/call",
            "params":{"name":"move_cursor","arguments":{"x":x,"y":y}}
        }));
        let resp = read_response(&mut stdout);
        assert_eq!(resp["id"], id);
        assert!(!resp["result"]["isError"].as_bool().unwrap_or(false), "move_cursor failed: {:?}", resp);
    }

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":5,"method":"tools/call",
        "params":{"name":"get_agent_cursor_state","arguments":{}}
    }));
    let resp = read_response(&mut stdout);
    assert_eq!(resp["id"], 5);
    assert!(!resp["result"]["isError"].as_bool().unwrap_or(false));

    std::thread::sleep(Duration::from_millis(100));

    assert!(
        child.try_wait().expect("try_wait").is_none(),
        "cua-driver crashed during overlay move_cursor test"
    );

    child.kill().ok();
}

#[test]
#[cfg(target_os = "windows")]
fn test_multi_cursor_instance_state_windows() {
    let binary = binary_path_windows();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"move_cursor","arguments":{"x":100.0,"y":200.0,"cursor_id":"agent1"}}
    }));
    assert!(!read_response(&mut stdout)["result"]["isError"].as_bool().unwrap_or(false));

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"move_cursor","arguments":{"x":300.0,"y":400.0,"cursor_id":"agent2"}}
    }));
    assert!(!read_response(&mut stdout)["result"]["isError"].as_bool().unwrap_or(false));

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":4,"method":"tools/call",
        "params":{"name":"set_agent_cursor_motion","arguments":{
            "cursor_id":"agent1","cursor_color":"#FF0000","cursor_label":"AI-1"
        }}
    }));
    assert!(!read_response(&mut stdout)["result"]["isError"].as_bool().unwrap_or(false));

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":5,"method":"tools/call",
        "params":{"name":"set_agent_cursor_enabled","arguments":{"enabled":false,"cursor_id":"agent2"}}
    }));
    assert!(!read_response(&mut stdout)["result"]["isError"].as_bool().unwrap_or(false));

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":6,"method":"tools/call",
        "params":{"name":"get_agent_cursor_state","arguments":{}}
    }));
    let resp = read_response(&mut stdout);
    assert!(!resp["result"]["isError"].as_bool().unwrap_or(false));
    let cursors = &resp["result"]["structuredContent"]["cursors"];
    assert!(cursors.as_array().map(|a| a.len() >= 2).unwrap_or(false),
        "Expected at least 2 cursor instances: {cursors:?}");

    std::thread::sleep(Duration::from_millis(50));
    assert!(child.try_wait().expect("try_wait").is_none(), "cua-driver crashed during multi-cursor test");

    child.kill().ok();
}

#[test]
#[cfg(target_os = "windows")]
fn test_press_key_harmless_windows() {
    let binary = binary_path_windows();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"list_windows","arguments":{}}
    }));
    let resp = read_response(&mut stdout);
    let wins = resp["result"]["structuredContent"]["windows"].as_array();
    let Some(win) = wins.and_then(|a| a.iter().find(|w| w["pid"].as_i64().is_some())) else {
        eprintln!("No windows found — skipping press_key test");
        child.kill().ok();
        return;
    };
    let pid = win["pid"].as_i64().unwrap();
    let wid = win["window_id"].as_u64().unwrap();

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"press_key","arguments":{"pid":pid,"window_id":wid,"key":"F24"}}
    }));
    let resp = read_response(&mut stdout);
    assert!(resp["error"].is_null(), "Protocol error from press_key: {resp:?}");
    assert!(!resp["result"]["content"].as_array().map(|a| a.is_empty()).unwrap_or(true));

    child.kill().ok();
}

#[test]
#[cfg(target_os = "windows")]
fn test_scroll_tool_windows() {
    let binary = binary_path_windows();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    // Windows list_apps returns "processes".
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"list_apps","arguments":{}}
    }));
    let resp = read_response(&mut stdout);
    let procs = resp["result"]["structuredContent"]["processes"].as_array();
    let Some(proc_) = procs.and_then(|a| a.iter().find(|p| p["pid"].as_i64().is_some())) else {
        eprintln!("No processes found — skipping scroll test");
        child.kill().ok();
        return;
    };
    let pid = proc_["pid"].as_i64().unwrap();

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"scroll","arguments":{"pid":pid,"direction":"down","amount":1}}
    }));
    let resp = read_response(&mut stdout);
    assert!(resp["error"].is_null(), "Protocol error from scroll: {resp:?}");
    assert!(!resp["result"]["content"].as_array().map(|a| a.is_empty()).unwrap_or(true));

    child.kill().ok();
}

#[test]
#[cfg(target_os = "windows")]
fn test_click_pixel_path_windows() {
    let binary = binary_path_windows();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"list_windows","arguments":{"on_screen_only":true}}
    }));
    let resp = read_response(&mut stdout);
    let wins = resp["result"]["structuredContent"]["windows"].as_array();
    let Some(win) = wins.and_then(|a| a.iter().find(|w| w["pid"].as_i64().is_some())) else {
        eprintln!("No on-screen windows — skipping click pixel path test");
        child.kill().ok();
        return;
    };
    let pid = win["pid"].as_i64().unwrap();
    let wid = win["window_id"].as_u64().unwrap();

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"click","arguments":{"pid":pid,"window_id":wid,"x":5.0,"y":5.0}}
    }));
    let resp = read_response(&mut stdout);
    assert!(resp["error"].is_null(), "Protocol error from click: {resp:?}");
    assert!(!resp["result"]["content"].as_array().map(|a| a.is_empty()).unwrap_or(true));

    child.kill().ok();
}

#[test]
#[cfg(target_os = "windows")]
fn test_double_click_and_right_click_windows() {
    let binary = binary_path_windows();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"list_windows","arguments":{"on_screen_only":true}}
    }));
    let resp = read_response(&mut stdout);
    let wins = resp["result"]["structuredContent"]["windows"].as_array();
    let Some(win) = wins.and_then(|a| a.iter().find(|w| w["pid"].as_i64().is_some())) else {
        eprintln!("No on-screen windows — skipping double/right click test");
        child.kill().ok();
        return;
    };
    let pid = win["pid"].as_i64().unwrap();
    let wid = win["window_id"].as_u64().unwrap();

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"double_click","arguments":{"pid":pid,"window_id":wid,"x":5.0,"y":5.0}}
    }));
    let resp = read_response(&mut stdout);
    assert!(resp["error"].is_null(), "Protocol error from double_click: {resp:?}");
    assert!(!resp["result"]["content"].as_array().map(|a| a.is_empty()).unwrap_or(true));

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":4,"method":"tools/call",
        "params":{"name":"right_click","arguments":{"pid":pid,"window_id":wid,"x":5.0,"y":5.0}}
    }));
    let resp = read_response(&mut stdout);
    assert!(resp["error"].is_null(), "Protocol error from right_click: {resp:?}");
    assert!(!resp["result"]["content"].as_array().map(|a| a.is_empty()).unwrap_or(true));

    child.kill().ok();
}

#[test]
#[cfg(target_os = "windows")]
fn test_hotkey_keys_array_windows() {
    let binary = binary_path_windows();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"list_windows","arguments":{}}
    }));
    let resp = read_response(&mut stdout);
    let windows = resp["result"]["structuredContent"]["windows"].as_array();
    let Some(win) = windows.and_then(|a| a.iter().find(|w| w["pid"].as_i64().is_some())) else {
        eprintln!("No windows found — skipping hotkey test");
        child.kill().ok();
        return;
    };
    let pid = win["pid"].as_i64().unwrap();
    let wid = win["window_id"].as_u64().unwrap();

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"hotkey","arguments":{"pid":pid,"window_id":wid,"keys":["ctrl","a"]}}
    }));
    let resp = read_response(&mut stdout);
    assert!(resp["error"].is_null(), "Protocol error from hotkey: {resp:?}");
    assert!(!resp["result"]["content"].as_array().map(|a| a.is_empty()).unwrap_or(true));

    child.kill().ok();
}

#[test]
#[cfg(target_os = "windows")]
fn test_get_window_state_ax_mode_windows() {
    let binary = binary_path_windows();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"list_windows","arguments":{}}
    }));
    let resp = read_response(&mut stdout);
    let windows = resp["result"]["structuredContent"]["windows"].as_array();
    let Some(win) = windows.and_then(|a| a.iter().find(|w| {
        w["pid"].as_i64().is_some() && w["window_id"].as_u64().is_some()
    })) else {
        eprintln!("No windows found — skipping get_window_state ax test");
        child.kill().ok();
        return;
    };
    let pid = win["pid"].as_i64().unwrap();
    let wid = win["window_id"].as_u64().unwrap();

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"get_window_state","arguments":{"pid":pid,"window_id":wid,"capture_mode":"ax"}}
    }));
    let resp = read_response(&mut stdout);
    assert!(resp["error"].is_null(), "Protocol error: {resp:?}");
    let content = resp["result"]["content"].as_array().expect("content array");
    let has_image = content.iter().any(|c| c["type"] == "image");
    assert!(!has_image, "capture_mode=ax should not return an image");
    assert!(!content.is_empty());

    let sc = &resp["result"]["structuredContent"];
    assert_eq!(sc["window_id"].as_u64().unwrap_or(0), wid);
    assert_eq!(sc["pid"].as_i64().unwrap_or(0), pid);
    assert!(sc["element_count"].is_number(), "expected element_count: {sc:?}");

    // vision mode should include screenshot dimensions.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":4,"method":"tools/call",
        "params":{"name":"get_window_state","arguments":{"pid":pid,"window_id":wid,"capture_mode":"vision"}}
    }));
    let vision_resp = read_response(&mut stdout);
    assert!(vision_resp["error"].is_null(), "Protocol error from vision get_window_state: {vision_resp:?}");
    if !vision_resp["result"]["isError"].as_bool().unwrap_or(false) {
        let vsc = &vision_resp["result"]["structuredContent"];
        assert!(vsc["screenshot_width"].as_f64().unwrap_or(0.0) > 0.0, "vision mode should have screenshot_width: {vsc:?}");
    }

    child.kill().ok();
}

#[test]
#[cfg(target_os = "windows")]
fn test_type_text_chars_schema_windows() {
    let binary = binary_path_windows();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}));
    let list_resp = read_response(&mut stdout);
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

    child.kill().ok();
}

#[test]
#[cfg(target_os = "windows")]
fn test_type_text_notepad_windows() {
    //! Launch Notepad, type a short string via type_text. Skips if Notepad unavailable.
    let binary = binary_path_windows();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    // Launch Notepad.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"launch_app","arguments":{"name":"notepad.exe"}}
    }));
    read_response(&mut stdout);
    std::thread::sleep(std::time::Duration::from_millis(1000));

    // Find Notepad in running processes.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"list_apps","arguments":{}}
    }));
    let resp = read_response(&mut stdout);
    let procs = resp["result"]["structuredContent"]["processes"].as_array();
    let Some(proc_) = procs.and_then(|a| {
        a.iter().find(|p| {
            p["name"].as_str().map(|n| n.to_lowercase().contains("notepad")).unwrap_or(false)
        })
    }) else {
        eprintln!("Notepad not found — skipping type_text test");
        child.kill().ok();
        return;
    };
    let pid = proc_["pid"].as_i64().unwrap();

    // Find Notepad's window.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":4,"method":"tools/call",
        "params":{"name":"list_windows","arguments":{"pid":pid,"on_screen_only":true}}
    }));
    let resp = read_response(&mut stdout);
    let wins = resp["result"]["structuredContent"]["windows"].as_array();
    let Some(win) = wins.and_then(|a| a.first()) else {
        eprintln!("Notepad has no on-screen windows — skipping type_text test");
        child.kill().ok();
        return;
    };
    let wid = win["window_id"].as_u64().unwrap();

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":5,"method":"tools/call",
        "params":{"name":"type_text","arguments":{"pid":pid,"window_id":wid,"text":"cua-test"}}
    }));
    let resp = read_response(&mut stdout);
    assert!(resp["error"].is_null(), "Protocol error from type_text: {resp:?}");
    assert!(!resp["result"]["content"].as_array().map(|a| a.is_empty()).unwrap_or(true));

    child.kill().ok();
}

#[test]
#[cfg(target_os = "windows")]
fn test_recording_session_windows() {
    let binary = binary_path_windows();
    if !binary.exists() { return; }

    let tmp_dir = std::env::temp_dir().join(format!("cua-driver-rs-rec-test-{}", std::process::id()));
    let tmp_str = tmp_dir.to_string_lossy().to_string();

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"set_recording","arguments":{"enabled":true,"output_dir":tmp_str}}
    }));
    let resp = read_response(&mut stdout);
    assert!(!resp["result"]["isError"].as_bool().unwrap_or(false), "set_recording failed: {resp:?}");
    assert!(resp["result"]["structuredContent"]["enabled"].as_bool().unwrap_or(false));

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"move_cursor","arguments":{"x":100.0,"y":200.0}}
    }));
    read_response(&mut stdout);

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":4,"method":"tools/call",
        "params":{"name":"get_recording_state","arguments":{}}
    }));
    let resp = read_response(&mut stdout);
    let next_turn = resp["result"]["structuredContent"]["next_turn"].as_u64().unwrap_or(0);
    assert!(next_turn >= 2, "expected next_turn >= 2, got {next_turn}");

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":5,"method":"tools/call",
        "params":{"name":"set_recording","arguments":{"enabled":false}}
    }));
    let resp = read_response(&mut stdout);
    assert!(!resp["result"]["structuredContent"]["enabled"].as_bool().unwrap_or(true));
    child.kill().ok();

    std::thread::sleep(Duration::from_millis(50));

    let action_path = tmp_dir.join("turn-00001").join("action.json");
    assert!(action_path.exists(), "Expected action.json at {action_path:?}");
    let content: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&action_path).unwrap()
    ).unwrap();
    assert_eq!(content["tool"].as_str().unwrap_or(""), "move_cursor");
    assert_eq!(content["arguments"]["x"].as_f64().unwrap_or(0.0), 100.0);

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
#[cfg(target_os = "windows")]
fn test_replay_trajectory_windows() {
    let binary = binary_path_windows();
    if !binary.exists() { return; }

    let traj_dir = std::env::temp_dir().join(format!("cua-driver-rs-replay-{}", std::process::id()));
    let turn_dir = traj_dir.join("turn-00001");
    std::fs::create_dir_all(&turn_dir).unwrap();
    std::fs::write(
        turn_dir.join("action.json"),
        r#"{"tool":"move_cursor","arguments":{"x":50.0,"y":60.0}}"#,
    ).unwrap();

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"replay_trajectory","arguments":{
            "dir":traj_dir.to_string_lossy(),"delay_ms":0
        }}
    }));
    let resp = read_response(&mut stdout);
    assert!(!resp["result"]["isError"].as_bool().unwrap_or(false), "replay error: {resp:?}");
    let sc = &resp["result"]["structuredContent"];
    assert_eq!(sc["attempted"].as_u64().unwrap_or(0), 1);
    assert_eq!(sc["succeeded"].as_u64().unwrap_or(0), 1);
    assert_eq!(sc["failed"].as_u64().unwrap_or(99), 0);

    child.kill().ok();
    let _ = std::fs::remove_dir_all(&traj_dir);
}

#[test]
#[cfg(target_os = "windows")]
fn test_browser_eval_no_cdp_error_windows() {
    let binary = binary_path_windows();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"browser_eval","arguments":{"expression":"1+1","cdp_port":19722}}
    }));
    let resp = read_response(&mut stdout);
    assert!(
        resp["result"]["isError"].as_bool().unwrap_or(false),
        "expected isError=true when CDP port not listening: {resp:?}"
    );
    let text = resp["result"]["content"][0]["text"].as_str().unwrap_or("");
    assert!(text.contains("browser_eval"), "error text should mention browser_eval: {text}");

    child.kill().ok();
}

#[test]
#[cfg(target_os = "windows")]
fn test_set_config_screenshot_resize_windows() {
    let binary = binary_path_windows();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"set_config","arguments":{"max_image_dimension":200}}
    }));
    let resp = read_response(&mut stdout);
    assert!(resp["error"].is_null(), "set_config failed: {resp:?}");
    assert!(!resp["result"]["isError"].as_bool().unwrap_or(false));

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"screenshot","arguments":{}}
    }));
    let resp = read_response(&mut stdout);
    assert!(resp["error"].is_null(), "screenshot failed: {resp:?}");

    let w = resp["result"]["structuredContent"]["width"].as_u64().unwrap_or(9999);
    let h = resp["result"]["structuredContent"]["height"].as_u64().unwrap_or(9999);
    assert!(w <= 200, "screenshot width {w} should be ≤ 200 after max_image_dimension=200");
    assert!(h <= 200, "screenshot height {h} should be ≤ 200 after max_image_dimension=200");

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":4,"method":"tools/call",
        "params":{"name":"get_config","arguments":{}}
    }));
    let resp = read_response(&mut stdout);
    assert_eq!(resp["result"]["structuredContent"]["max_image_dimension"].as_u64(), Some(200));

    child.kill().ok();
}

#[test]
#[cfg(target_os = "windows")]
fn test_zoom_from_zoom_click_round_trip_windows() {
    let binary = binary_path_windows();
    if !binary.exists() { return; }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"list_windows","arguments":{"on_screen_only":true}}
    }));
    let resp = read_response(&mut stdout);
    let wins = resp["result"]["structuredContent"]["windows"].as_array().cloned().unwrap_or_default();
    let Some(win) = wins.iter().find(|w| w["pid"].as_i64().is_some()) else {
        eprintln!("No on-screen windows — skipping from_zoom test");
        child.kill().ok();
        return;
    };
    let window_id = win["window_id"].as_u64().unwrap();
    let pid = win["pid"].as_i64().unwrap();

    // click with from_zoom=true and no prior zoom context — expect error.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"click","arguments":{"pid":pid,"window_id":window_id,"x":10.0,"y":10.0,"from_zoom":true}}
    }));
    let resp = read_response(&mut stdout);
    let is_err = resp["result"]["isError"].as_bool().unwrap_or(false);
    let err_text = resp["result"]["content"][0]["text"].as_str().unwrap_or("");
    assert!(is_err, "Expected error when from_zoom=true with no context: {resp:?}");
    assert!(err_text.contains("no zoom context"), "Expected 'no zoom context': {err_text}");

    // Call zoom to store context.
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":4,"method":"tools/call",
        "params":{"name":"zoom","arguments":{"window_id":window_id,"pid":pid,"x1":0,"y1":0,"x2":50,"y2":50}}
    }));
    let resp = read_response(&mut stdout);
    if resp["result"]["isError"].as_bool().unwrap_or(false) {
        eprintln!("zoom failed — skipping from_zoom click test");
        child.kill().ok();
        return;
    }

    // click with from_zoom=true should NOT say "no zoom context".
    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":5,"method":"tools/call",
        "params":{"name":"click","arguments":{"pid":pid,"window_id":window_id,"x":10.0,"y":10.0,"from_zoom":true}}
    }));
    let resp = read_response(&mut stdout);
    let err_text = resp["result"]["content"][0]["text"].as_str().unwrap_or("");
    assert!(!err_text.contains("no zoom context"),
        "After zoom(), from_zoom click should not say 'no zoom context': {err_text}");

    child.kill().ok();
}

#[test]
#[cfg(target_os = "windows")]
fn test_set_recording_video_experimental_accepted_windows() {
    let binary = binary_path_windows();
    if !binary.exists() { return; }

    let tmp_dir = std::env::temp_dir().join(format!("cua-driver-rs-videxp-{}", std::process::id()));
    let tmp_str = tmp_dir.to_string_lossy().to_string();
    std::fs::create_dir_all(&tmp_dir).unwrap();

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn");
    let stdin = child.stdin.as_mut().unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

    send_request(stdin, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout);

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"set_recording","arguments":{
            "enabled":true,"output_dir":tmp_str,"video_experimental":true
        }}
    }));
    let resp = read_response(&mut stdout);
    assert!(resp["error"].is_null(), "Expected no JSON-RPC error: {resp:?}");
    assert!(!resp["result"]["isError"].as_bool().unwrap_or(false),
        "set_recording with video_experimental should succeed: {resp:?}");

    send_request(stdin, &serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"set_recording","arguments":{"enabled":false}}
    }));
    read_response(&mut stdout);
    child.kill().ok();
    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
#[cfg(target_os = "windows")]
fn test_concurrent_multi_driver_isolation_windows() {
    let binary = binary_path_windows();
    if !binary.exists() { return; }

    let mut child_a = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn driver A");
    let mut child_b = Command::new(&binary)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn driver B");

    let stdin_a = child_a.stdin.as_mut().unwrap();
    let mut stdout_a = BufReader::new(child_a.stdout.as_mut().unwrap());
    let stdin_b = child_b.stdin.as_mut().unwrap();
    let mut stdout_b = BufReader::new(child_b.stdout.as_mut().unwrap());

    send_request(stdin_a, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    send_request(stdin_b, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    read_response(&mut stdout_a);
    read_response(&mut stdout_b);

    send_request(stdin_a, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"set_agent_cursor_enabled","arguments":{"cursor_id":"alpha","enabled":true}}
    }));
    send_request(stdin_b, &serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"set_agent_cursor_enabled","arguments":{"cursor_id":"beta","enabled":false}}
    }));
    let resp_a = read_response(&mut stdout_a);
    let resp_b = read_response(&mut stdout_b);
    assert!(resp_a["error"].is_null(), "Driver A failed: {resp_a:?}");
    assert!(resp_b["error"].is_null(), "Driver B failed: {resp_b:?}");

    send_request(stdin_a, &serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"get_agent_cursor_state","arguments":{}}
    }));
    send_request(stdin_b, &serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"get_agent_cursor_state","arguments":{}}
    }));
    let state_a = read_response(&mut stdout_a);
    let state_b = read_response(&mut stdout_b);
    assert!(state_a["error"].is_null());
    assert!(state_b["error"].is_null());

    let cursors_a = state_a["result"]["structuredContent"]["cursors"].as_array();
    let cursors_b = state_b["result"]["structuredContent"]["cursors"].as_array();

    let alpha_enabled = cursors_a.and_then(|arr| arr.iter().find(|c| {
        c["config"]["cursor_id"].as_str() == Some("alpha")
    })).and_then(|c| c["config"]["enabled"].as_bool());
    let beta_enabled = cursors_b.and_then(|arr| arr.iter().find(|c| {
        c["config"]["cursor_id"].as_str() == Some("beta")
    })).and_then(|c| c["config"]["enabled"].as_bool());

    assert_eq!(alpha_enabled, Some(true), "Cursor alpha should be enabled in driver A");
    assert_eq!(beta_enabled, Some(false), "Cursor beta should be disabled in driver B");

    child_a.kill().ok();
    child_b.kill().ok();
}
