//! Per-tool `tools/call` integration tests.
//!
//! Drives the real tools end-to-end over JSON-RPC: list_apps, get_config /
//! check_permissions, the accessibility tree, list_windows, screen size /
//! cursor position, window-state AX mode, and the hidden `type_text_chars`
//! alias. Split out of the old monolithic `mcp_protocol_test.rs`; mac/windows
//! pairs merge into one fn and branch only where the platforms differ
//! (list_apps `apps` vs `processes`, get_config platform string,
//! check_permissions keys, etc.).
//!
//! Action delivery (click, double/right click, scroll, type_text, press_key,
//! hotkey, set_value) is owned by the state-verified fixture suites:
//! `cross_platform_behavior_test` and the `harness_*` tests. Tests here that
//! dispatch real input are `#[ignore]`d and never run under a plain
//! `cargo test`. Run them with `--ignored` only on a disposable desktop.

#![cfg(any(target_os = "macos", target_os = "windows"))]

use cua_driver_testkit::RawDriver;
#[cfg(target_os = "windows")]
use cua_driver_testkit::{Driver, McpDriver};

fn spawn_unrestricted() -> Option<RawDriver> {
    RawDriver::spawn_with_env(&[
        ("CUA_DRIVER_PERMISSION_MODE", "unrestricted"),
        ("CUA_DRIVER_DANGEROUSLY_BYPASS_APPROVALS", "1"),
    ])
}

#[test]
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn tools_call_list_apps() {
    let Some(mut d) = spawn_unrestricted() else {
        return;
    };

    // Initialize.
    d.send(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    d.recv();

    // Call list_apps.
    d.send(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": { "name": "list_apps", "arguments": {} }
    }));
    let started = std::time::Instant::now();
    let resp = d.recv();
    assert!(
        started.elapsed() < std::time::Duration::from_secs(8),
        "list_apps exceeded its optional installed-app discovery budget: {:?}",
        started.elapsed()
    );
    assert_eq!(resp["id"], 2);
    assert!(resp["result"]["content"].is_array());
    if cfg!(target_os = "windows") {
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("Found"),
            "Expected process list text, got: {}",
            text
        );
        // Windows backend returns "processes" key.
        assert!(
            resp["result"]["structuredContent"]["processes"].is_array(),
            "Expected processes array: {:?}",
            resp["result"]["structuredContent"]
        );
    } else {
        let content = &resp["result"]["content"][0];
        assert_eq!(content["type"], "text");
        let text = content["text"].as_str().unwrap();
        // Should contain some running apps.
        assert!(
            text.contains("Found"),
            "Expected app list text, got: {}",
            text
        );
    }
}

#[test]
#[cfg(target_os = "windows")]
fn launch_unknown_app_is_bounded_and_keeps_mcp_session_responsive() {
    let Some(mut driver) = McpDriver::spawn_with_env(&[
        ("CUA_DRIVER_PERMISSION_MODE", "unrestricted"),
        ("CUA_DRIVER_DANGEROUSLY_BYPASS_APPROVALS", "1"),
    ]) else {
        return;
    };
    let missing_name = format!("CuaMissingAppIssue2856_{}.exe", std::process::id());

    let launch_started = std::time::Instant::now();
    let launch = driver.call("launch_app", serde_json::json!({ "name": missing_name }));
    let launch_elapsed = launch_started.elapsed();
    assert!(
        launch_elapsed < std::time::Duration::from_secs(10),
        "unknown-app launch exceeded its hard response budget: {launch_elapsed:?}; response={:?}",
        launch.raw
    );
    assert!(
        launch.is_error(),
        "unknown app should fail: {:?}",
        launch.raw
    );
    let error = launch.text().to_ascii_lowercase();
    assert!(
        error.contains("not found") || error.contains("lookup") && error.contains("unavailable"),
        "expected an explicit not-found or lookup-unavailable error, got: {}",
        launch.text()
    );

    let follow_up_started = std::time::Instant::now();
    let windows = driver.call("list_windows", serde_json::json!({}));
    let follow_up_elapsed = follow_up_started.elapsed();
    assert!(
        follow_up_elapsed < std::time::Duration::from_secs(5),
        "follow-up list_windows call was not responsive: {follow_up_elapsed:?}; response={:?}",
        windows.raw
    );
    assert!(
        !windows.is_error(),
        "follow-up list_windows failed after unknown launch: {:?}",
        windows.raw
    );
    assert!(
        windows.structured()["windows"].is_array(),
        "follow-up list_windows returned no windows array: {:?}",
        windows.raw
    );
}

#[test]
#[cfg(target_os = "windows")]
#[ignore = "requires an interactive Windows desktop with Microsoft Edge installed"]
fn launch_edge_from_apps_folder_registration() {
    let Some(mut driver) = McpDriver::spawn_with_env(&[
        ("CUA_DRIVER_PERMISSION_MODE", "unrestricted"),
        ("CUA_DRIVER_DANGEROUSLY_BYPASS_APPROVALS", "1"),
    ]) else {
        return;
    };

    let launch = driver.call(
        "launch_app",
        serde_json::json!({
            "name": "Microsoft Edge",
            "urls": ["https://example.com"]
        }),
    );
    assert!(
        !launch.is_error(),
        "Edge AppsFolder launch failed: {:?}",
        launch.raw
    );
    assert!(
        launch.structured()["pid"]
            .as_u64()
            .is_some_and(|pid| pid > 0),
        "Edge AppsFolder launch returned no process id: {:?}",
        launch.raw
    );
}

#[test]
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn get_config_and_check_permissions() {
    let Some(mut d) = spawn_unrestricted() else {
        return;
    };

    d.send(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    d.recv();

    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"get_config","arguments":{}}
    }));
    let resp = d.recv();
    assert!(!resp["result"]["isError"].as_bool().unwrap_or(false));
    if cfg!(target_os = "windows") {
        assert_eq!(resp["result"]["structuredContent"]["platform"], "windows");
    } else {
        assert_eq!(resp["result"]["structuredContent"]["platform"], "macos");
    }

    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"check_permissions","arguments":{"prompt":false}}
    }));
    let resp = d.recv();
    assert!(!resp["result"]["isError"].as_bool().unwrap_or(false));
    if cfg!(target_os = "windows") {
        let sc = &resp["result"]["structuredContent"];
        // Windows returns elevated, uia, post_message booleans.
        assert!(
            sc["uia"].as_bool().unwrap_or(false),
            "uia should be true: {sc:?}"
        );
        assert!(
            sc["post_message"].as_bool().unwrap_or(false),
            "post_message should be true: {sc:?}"
        );
        assert!(
            sc["elevated"].is_boolean(),
            "elevated should be a boolean: {sc:?}"
        );
    } else {
        // Returns structured content with accessibility and screen_recording booleans.
        assert!(resp["result"]["structuredContent"]["accessibility"].is_boolean());
    }

    // set_config — change max_image_dimension and verify get_config reflects it.
    // capture_mode / capture_scope are no longer persistent settings on macOS
    // (capture_scope is per-session); max_image_dimension is the
    // cross-platform persisted field this exercises.
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":4,"method":"tools/call",
        "params":{"name":"set_config","arguments":{"max_image_dimension": 1920}}
    }));
    let resp = d.recv();
    assert!(
        resp["error"].is_null(),
        "Protocol error from set_config: {resp:?}"
    );
    if !cfg!(target_os = "windows") {
        let content = resp["result"]["content"].as_array().expect("content array");
        assert!(!content.is_empty(), "set_config returned empty content");
    }

    // get_config should now reflect the updated value.
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":5,"method":"tools/call",
        "params":{"name":"get_config","arguments":{}}
    }));
    let resp = d.recv();
    assert_eq!(
        resp["result"]["structuredContent"]["max_image_dimension"], 1920,
        "get_config should reflect max_image_dimension change"
    );
}

#[test]
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn get_accessibility_tree() {
    //! get_accessibility_tree returns a lightweight process+window snapshot.
    let Some(mut d) = spawn_unrestricted() else {
        return;
    };

    d.send(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    d.recv();

    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"get_accessibility_tree","arguments":{}}
    }));
    let resp = d.recv();
    assert!(resp["error"].is_null(), "Protocol error: {resp:?}");
    assert!(
        !resp["result"]["isError"].as_bool().unwrap_or(false),
        "get_accessibility_tree returned error: {resp:?}"
    );

    let sc = &resp["result"]["structuredContent"];
    if cfg!(target_os = "windows") {
        // Windows backend returns both "processes" and "windows" keys.
        assert!(
            sc["processes"].is_array(),
            "Expected processes array: {sc:?}"
        );
        assert!(sc["windows"].is_array(), "Expected windows array: {sc:?}");
    } else {
        // macOS backend returns "apps"; Linux returns "processes". Either key is valid.
        let has_apps = sc["apps"].is_array() || sc["processes"].is_array();
        assert!(
            has_apps,
            "Expected apps or processes array in structured content, got: {sc:?}"
        );
    }

    let content = resp["result"]["content"].as_array().expect("content array");
    assert!(!content.is_empty(), "Expected non-empty content");
}

#[test]
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn list_windows_structured_content() {
    //! Verify list_windows returns structuredContent.windows array with expected fields.
    let Some(mut d) = spawn_unrestricted() else {
        return;
    };

    d.send(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    d.recv();

    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"list_windows","arguments":{}}
    }));
    let resp = d.recv();
    assert!(resp["error"].is_null(), "Protocol error: {resp:?}");
    assert!(
        !resp["result"]["isError"].as_bool().unwrap_or(false),
        "list_windows returned error: {resp:?}"
    );

    let sc = &resp["result"]["structuredContent"];
    assert!(
        sc["windows"].is_array(),
        "Expected windows array in structuredContent: {sc:?}"
    );

    // If there are any windows, verify the expected fields are present.
    if let Some(wins) = sc["windows"].as_array() {
        if let Some(w) = wins.first() {
            assert!(
                w["window_id"].is_number(),
                "window_id missing from window: {w:?}"
            );
            assert!(w["pid"].is_number(), "pid missing from window: {w:?}");
        }
    }
}

#[test]
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn get_screen_size_and_cursor_position() {
    let Some(mut d) = spawn_unrestricted() else {
        return;
    };

    d.send(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    d.recv();

    // get_screen_size
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"get_screen_size","arguments":{}}
    }));
    let resp = d.recv();
    assert!(
        !resp["result"]["isError"].as_bool().unwrap_or(false),
        "get_screen_size failed: {resp:?}"
    );
    let sc = &resp["result"]["structuredContent"];
    assert!(
        sc["width"].as_f64().unwrap_or(0.0) > 0.0,
        "width should be positive"
    );
    assert!(
        sc["height"].as_f64().unwrap_or(0.0) > 0.0,
        "height should be positive"
    );

    // get_cursor_position
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"get_cursor_position","arguments":{}}
    }));
    let resp = d.recv();
    assert!(
        !resp["result"]["isError"].as_bool().unwrap_or(false),
        "get_cursor_position failed: {resp:?}"
    );
    // x and y may be 0,0 or any value — just verify the keys exist and are numbers.
    assert!(
        resp["result"]["structuredContent"]["x"].is_number(),
        "x should be a number"
    );
    assert!(
        resp["result"]["structuredContent"]["y"].is_number(),
        "y should be a number"
    );
}

#[test]
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn get_window_state_returns_both_with_opt_out() {
    //! Perception is mode-agnostic: get_window_state returns BOTH the tree AND a
    //! screenshot by default (the deprecated `capture_mode` arg is ignored), and
    //! `include_screenshot:false` is the opt-out that returns the tree only.
    let Some(mut d) = spawn_unrestricted() else {
        return;
    };

    d.send(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    d.recv();

    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"list_windows","arguments":{}}
    }));
    let resp = d.recv();
    let windows = resp["result"]["structuredContent"]["windows"].as_array();
    let first_win = windows.and_then(|a| {
        a.iter()
            .find(|w| w["pid"].as_i64().is_some() && w["window_id"].as_u64().is_some())
    });

    let Some(win) = first_win else {
        eprintln!("No windows found — skipping get_window_state perception test");
        return;
    };
    let pid = win["pid"].as_i64().unwrap();
    let wid = win["window_id"].as_u64().unwrap();

    // DEFAULT (no capture_mode): both tree AND a screenshot come back.
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"get_window_state","arguments":{
            "pid": pid, "window_id": wid
        }}
    }));
    let resp = d.recv();
    assert!(
        resp["error"].is_null(),
        "Protocol error from get_window_state: {resp:?}"
    );

    let content = resp["result"]["content"].as_array().expect("content array");
    assert!(!content.is_empty(), "Expected at least one content item");
    let sc = &resp["result"]["structuredContent"];
    assert_eq!(
        sc["window_id"].as_u64().unwrap_or(0),
        wid,
        "structuredContent.window_id mismatch"
    );
    assert_eq!(
        sc["pid"].as_i64().unwrap_or(0),
        pid,
        "structuredContent.pid mismatch"
    );
    assert!(
        sc["element_count"].is_number(),
        "expected element_count in structuredContent: {sc:?}"
    );
    // The screenshot is delivered by default (skip the strict image assertion on
    // Windows, whose capture can vary, and when screen-recording isn't granted).
    if !cfg!(target_os = "windows") && !sc["screenshot_width"].is_null() {
        let has_image = content.iter().any(|c| c["type"] == "image");
        assert!(
            has_image,
            "default get_window_state should deliver a screenshot image: {content:?}"
        );
        assert!(
            sc["screenshot_width"].as_f64().unwrap_or(0.0) > 0.0,
            "default mode should report screenshot_width: {sc:?}"
        );
    }

    // OPT-OUT: include_screenshot:false → tree only, NO image / no screenshot_width.
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":4,"method":"tools/call",
        "params":{"name":"get_window_state","arguments":{
            "pid": pid, "window_id": wid, "include_screenshot": false
        }}
    }));
    let tree_only = d.recv();
    assert!(
        tree_only["error"].is_null(),
        "Protocol error from tree-only get_window_state: {tree_only:?}"
    );
    let to_content = tree_only["result"]["content"]
        .as_array()
        .expect("content array");
    let to_has_image = to_content.iter().any(|c| c["type"] == "image");
    assert!(
        !to_has_image,
        "include_screenshot:false must NOT return an image, got: {to_content:?}"
    );
    assert!(
        tree_only["result"]["structuredContent"]["screenshot_width"].is_null(),
        "include_screenshot:false must not report screenshot_width: {tree_only:?}"
    );

    // DEPRECATED `capture_mode:"vision"` must be IGNORED — still returns the tree.
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":5,"method":"tools/call",
        "params":{"name":"get_window_state","arguments":{
            "pid": pid, "window_id": wid, "capture_mode": "vision"
        }}
    }));
    let dep = d.recv();
    assert!(
        dep["error"].is_null(),
        "Protocol error: deprecated capture_mode must be accepted, not rejected: {dep:?}"
    );
    assert!(
        dep["result"]["structuredContent"]["element_count"].is_number(),
        "capture_mode=vision is ignored — the tree must still be present: {dep:?}"
    );
}

#[test]
#[cfg(target_os = "macos")]
#[ignore = "sends real input to a live desktop window; run with --ignored only on a disposable desktop"]
fn type_text_chars_tool() {
    //! Verify type_text_chars with delay_ms is accepted without error (dry-run via TextEdit or
    //! a pid that accepts WM_CHAR). We just verify the tool responds with a non-error.
    //! Skips gracefully if no visible TextEdit window.
    let Some(mut d) = spawn_unrestricted() else {
        return;
    };

    d.send(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    d.recv();

    // list_apps to find a target (e.g. TextEdit).
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"list_apps","arguments":{}}
    }));
    let resp = d.recv();
    let apps = resp["result"]["structuredContent"]["apps"].as_array();
    let textedit_pid = apps.and_then(|arr| {
        arr.iter()
            .find(|a| a["name"].as_str().unwrap_or("").contains("TextEdit"))
            .and_then(|a| a["pid"].as_i64())
    });

    let Some(pid) = textedit_pid else {
        eprintln!("TextEdit not running — skipping type_text_chars test");
        return;
    };

    // Resolve an exact on-screen window. PID-only targeting is deliberately
    // refused when an app owns multiple eligible windows.
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"list_windows","arguments":{"pid":pid,"on_screen_only":true}}
    }));
    let resp = d.recv();
    let windows = resp["result"]["structuredContent"]["windows"].as_array();
    let Some(window_id) = windows
        .and_then(|windows| windows.first())
        .and_then(|window| window["window_id"].as_u64())
    else {
        eprintln!("TextEdit has no on-screen windows — skipping type_text_chars test");
        return;
    };

    // type_text_chars with delay_ms=5 — just verify the tool invocation is accepted.
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":4,"method":"tools/call",
        "params":{"name":"type_text_chars","arguments":{
            "pid": pid, "window_id": window_id, "text": "hi", "delay_ms": 5
        }}
    }));
    let resp = d.recv();
    if resp["result"]["isError"].as_bool().unwrap_or(false)
        && matches!(
            resp["result"]["structuredContent"]["code"].as_str(),
            Some("off_space_or_ax_unresolved" | "window_target_not_found")
        )
    {
        eprintln!(
            "TextEdit has no safe AX-resolved input target in this desktop session — skipping type_text_chars test"
        );
        return;
    }
    assert!(
        !resp["result"]["isError"].as_bool().unwrap_or(false),
        "type_text_chars returned error: {resp:?}"
    );
    let msg = resp["result"]["content"][0]["text"].as_str().unwrap_or("");
    // type_text reports "Inserted" when an AX read-back verified the text, and
    // "Sent (unverified)" when it dispatched but couldn't confirm (e.g. the field
    // wasn't focused/frontmost) — both are accepted invocations. "Typed" covers
    // the standalone type_text_chars wording.
    assert!(
        msg.contains("Typed") || msg.contains("Inserted") || msg.contains("Sent"),
        "Unexpected message: {msg}"
    );
}
