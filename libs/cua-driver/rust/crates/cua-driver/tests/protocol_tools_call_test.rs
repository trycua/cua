//! Per-tool `tools/call` integration tests.
//!
//! Drives the real tools end-to-end over JSON-RPC: list_apps, get_config /
//! check_permissions, the accessibility tree, list_windows, screen size /
//! cursor position, window-state AX mode, scroll, type_text, hotkey,
//! press_key, click / double_click / right_click, and set_value. Split out
//! of the old monolithic `mcp_protocol_test.rs`; mac/windows pairs merge into
//! one fn and branch only where the platforms differ (list_apps `apps` vs
//! `processes`, get_config platform string, check_permissions keys, type_text
//! target app, hotkey modifier, etc.).

#![cfg(any(target_os = "macos", target_os = "windows"))]

use cua_driver_testkit::RawDriver;

#[test]
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn tools_call_list_apps() {
    let Some(mut d) = RawDriver::spawn() else {
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
    let resp = d.recv();
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
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn get_config_and_check_permissions() {
    let Some(mut d) = RawDriver::spawn() else {
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
        "params":{"name":"check_permissions","arguments":{}}
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
    let Some(mut d) = RawDriver::spawn() else {
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
    let Some(mut d) = RawDriver::spawn() else {
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
    let Some(mut d) = RawDriver::spawn() else {
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
    let Some(mut d) = RawDriver::spawn() else {
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
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn scroll_tool() {
    //! scroll with direction=down, by=line, amount=1 against the first available window.
    //! Verifies the tool is accepted and returns content, not a protocol error.
    let Some(mut d) = RawDriver::spawn() else {
        return;
    };

    d.send(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    d.recv();

    // Find a pid with a window.
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"list_apps","arguments":{}}
    }));
    let resp = d.recv();
    let pid = if cfg!(target_os = "windows") {
        // Windows list_apps returns "processes".
        let procs = resp["result"]["structuredContent"]["processes"].as_array();
        let Some(proc_) = procs.and_then(|a| a.iter().find(|p| p["pid"].as_i64().is_some())) else {
            eprintln!("No processes found — skipping scroll test");
            return;
        };
        proc_["pid"].as_i64().unwrap()
    } else {
        let apps = resp["result"]["structuredContent"]["apps"].as_array();
        let Some(app) = apps.and_then(|a| a.iter().find(|a| a["pid"].as_i64().is_some())) else {
            eprintln!("No apps found — skipping scroll test");
            return;
        };
        app["pid"].as_i64().unwrap()
    };

    // scroll with minimal parameters — window_id is auto-resolved from pid.
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"scroll","arguments":{"pid": pid, "direction": "down", "amount": 1}}
    }));
    let resp = d.recv();
    assert!(
        resp["error"].is_null(),
        "Protocol error from scroll: {resp:?}"
    );
    let content = resp["result"]["content"].as_array().expect("content array");
    assert!(!content.is_empty(), "scroll returned empty content");
}

#[test]
#[cfg(target_os = "macos")]
fn type_text_tool() {
    //! Opens TextEdit (or reuses it if already running) and types a short string via type_text.
    //! Skips gracefully if TextEdit is not available or has no window.
    let Some(mut d) = RawDriver::spawn() else {
        return;
    };

    d.send(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    d.recv();

    // Launch TextEdit (no-op if already running).
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"launch_app","arguments":{"name":"TextEdit"}}
    }));
    d.recv();
    std::thread::sleep(std::time::Duration::from_millis(800));

    // Find TextEdit's pid.
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"list_apps","arguments":{}}
    }));
    let resp = d.recv();
    let apps = resp["result"]["structuredContent"]["apps"].as_array();
    let Some(app) = apps.and_then(|a| {
        a.iter().find(|a| {
            a["name"]
                .as_str()
                .map(|n| n.eq_ignore_ascii_case("textedit"))
                .unwrap_or(false)
        })
    }) else {
        eprintln!("TextEdit not found — skipping type_text test");
        return;
    };
    let pid = app["pid"].as_i64().unwrap();

    // Find TextEdit's first on-screen window.
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":4,"method":"tools/call",
        "params":{"name":"list_windows","arguments":{"pid": pid, "on_screen_only": true}}
    }));
    let resp = d.recv();
    let wins = resp["result"]["structuredContent"]["windows"].as_array();
    let Some(win) = wins.and_then(|a| a.first()) else {
        eprintln!("TextEdit has no on-screen windows — skipping type_text test");
        return;
    };
    let wid = win["window_id"].as_u64().unwrap();

    // type_text into TextEdit's window.
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":5,"method":"tools/call",
        "params":{"name":"type_text","arguments":{"pid": pid, "window_id": wid, "text": "cua-test"}}
    }));
    let resp = d.recv();
    assert!(
        resp["error"].is_null(),
        "Protocol error from type_text: {resp:?}"
    );
    let content = resp["result"]["content"].as_array().expect("content array");
    assert!(!content.is_empty(), "type_text returned empty content");
}

#[test]
#[cfg(target_os = "windows")]
fn type_text_notepad() {
    //! Launch Notepad, type a short string via type_text. Skips if Notepad unavailable.
    let Some(mut d) = RawDriver::spawn() else {
        return;
    };

    d.send(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    d.recv();

    // Launch Notepad.
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"launch_app","arguments":{"name":"notepad.exe"}}
    }));
    d.recv();
    std::thread::sleep(std::time::Duration::from_millis(1000));

    // Find Notepad in running processes.
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"list_apps","arguments":{}}
    }));
    let resp = d.recv();
    let procs = resp["result"]["structuredContent"]["processes"].as_array();
    let Some(proc_) = procs.and_then(|a| {
        a.iter().find(|p| {
            p["name"]
                .as_str()
                .map(|n| n.to_lowercase().contains("notepad"))
                .unwrap_or(false)
        })
    }) else {
        eprintln!("Notepad not found — skipping type_text test");
        return;
    };
    let pid = proc_["pid"].as_i64().unwrap();

    // Find Notepad's window.
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":4,"method":"tools/call",
        "params":{"name":"list_windows","arguments":{"pid":pid,"on_screen_only":true}}
    }));
    let resp = d.recv();
    let wins = resp["result"]["structuredContent"]["windows"].as_array();
    let Some(win) = wins.and_then(|a| a.first()) else {
        eprintln!("Notepad has no on-screen windows — skipping type_text test");
        return;
    };
    let wid = win["window_id"].as_u64().unwrap();

    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":5,"method":"tools/call",
        "params":{"name":"type_text","arguments":{"pid":pid,"window_id":wid,"text":"cua-test"}}
    }));
    let resp = d.recv();
    assert!(
        resp["error"].is_null(),
        "Protocol error from type_text: {resp:?}"
    );
    assert!(!resp["result"]["content"]
        .as_array()
        .map(|a| a.is_empty())
        .unwrap_or(true));
}

#[test]
#[cfg(target_os = "macos")]
fn type_text_chars_tool() {
    //! Verify type_text_chars with delay_ms is accepted without error (dry-run via TextEdit or
    //! a pid that accepts WM_CHAR). We just verify the tool responds with a non-error.
    //! Skips gracefully if no visible TextEdit window.
    let Some(mut d) = RawDriver::spawn() else {
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

    // type_text_chars with delay_ms=5 — just verify the tool invocation is accepted.
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"type_text_chars","arguments":{
            "pid": pid, "text": "hi", "delay_ms": 5
        }}
    }));
    let resp = d.recv();
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

#[test]
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn hotkey_keys_array() {
    //! Verify hotkey accepts a keys array without a protocol error.
    let Some(mut d) = RawDriver::spawn() else {
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
    let first_win = windows.and_then(|a| a.iter().find(|w| w["pid"].as_i64().is_some()));

    let Some(win) = first_win else {
        eprintln!("No windows found — skipping hotkey keys array test");
        return;
    };
    let pid = win["pid"].as_i64().unwrap();
    let wid = win["window_id"].as_u64().unwrap();

    // keys array should be parsed and the tool invoked; result may succeed or fail depending
    // on whether the target app accepts the hotkey, but there must be no protocol error.
    // Windows uses ["ctrl","a"]; macOS uses ["cmd","a"].
    let keys = if cfg!(target_os = "windows") {
        serde_json::json!(["ctrl", "a"])
    } else {
        serde_json::json!(["cmd", "a"])
    };
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"hotkey","arguments":{
            "pid": pid, "window_id": wid, "keys": keys
        }}
    }));
    let resp = d.recv();
    assert!(
        resp["error"].is_null(),
        "Protocol error from hotkey: {resp:?}"
    );
    let content = resp["result"]["content"].as_array().expect("content array");
    assert!(!content.is_empty(), "hotkey should return content");
}

#[test]
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn press_key_harmless() {
    //! press_key with a harmless key (F24 — virtually no app responds to it) sent to the
    //! first available window. Just verifies the tool doesn't return a protocol error.
    let Some(mut d) = RawDriver::spawn() else {
        return;
    };

    d.send(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    d.recv();

    // Find a window to target.
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"list_windows","arguments":{}}
    }));
    let resp = d.recv();
    let wins = resp["result"]["structuredContent"]["windows"].as_array();
    let Some(win) = wins.and_then(|a| a.iter().find(|w| w["pid"].as_i64().is_some())) else {
        eprintln!("No windows found — skipping press_key test");
        return;
    };
    let pid = win["pid"].as_i64().unwrap();
    let wid = win["window_id"].as_u64().unwrap();

    // F24 is a function key that virtually no app binds — safe to send.
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"press_key","arguments":{"pid": pid, "window_id": wid, "key": "F24"}}
    }));
    let resp = d.recv();
    assert!(
        resp["error"].is_null(),
        "Protocol error from press_key: {resp:?}"
    );
    // Tool may succeed or return an app error, but must NOT be a protocol-level error.
    let content = resp["result"]["content"].as_array().expect("content array");
    assert!(!content.is_empty(), "press_key returned empty content");
}

#[test]
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn click_pixel_path() {
    //! click at window-local (5, 5) — title bar area, safe to click without disrupting UI.
    //! Verifies the pixel-coordinate path for click works without a protocol error.
    let Some(mut d) = RawDriver::spawn() else {
        return;
    };

    d.send(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    d.recv();

    // Find an on-screen window.
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"list_windows","arguments":{"on_screen_only": true}}
    }));
    let resp = d.recv();
    let wins = resp["result"]["structuredContent"]["windows"].as_array();
    let Some(win) = wins.and_then(|a| a.iter().find(|w| w["pid"].as_i64().is_some())) else {
        eprintln!("No on-screen windows — skipping click pixel path test");
        return;
    };
    let pid = win["pid"].as_i64().unwrap();
    let wid = win["window_id"].as_u64().unwrap();

    // Click at (5, 5) in window-local coords — title bar, harmless.
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"click","arguments":{"pid": pid, "window_id": wid, "x": 5.0, "y": 5.0}}
    }));
    let resp = d.recv();
    assert!(
        resp["error"].is_null(),
        "Protocol error from click: {resp:?}"
    );
    let content = resp["result"]["content"].as_array().expect("content array");
    assert!(!content.is_empty(), "click returned empty content");
}

#[test]
#[cfg(target_os = "windows")]
fn double_click_and_right_click() {
    let Some(mut d) = RawDriver::spawn() else {
        return;
    };

    d.send(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    d.recv();

    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"list_windows","arguments":{"on_screen_only":true}}
    }));
    let resp = d.recv();
    let wins = resp["result"]["structuredContent"]["windows"].as_array();
    let Some(win) = wins.and_then(|a| a.iter().find(|w| w["pid"].as_i64().is_some())) else {
        eprintln!("No on-screen windows — skipping double/right click test");
        return;
    };
    let pid = win["pid"].as_i64().unwrap();
    let wid = win["window_id"].as_u64().unwrap();

    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"double_click","arguments":{"pid":pid,"window_id":wid,"x":5.0,"y":5.0}}
    }));
    let resp = d.recv();
    assert!(
        resp["error"].is_null(),
        "Protocol error from double_click: {resp:?}"
    );
    assert!(!resp["result"]["content"]
        .as_array()
        .map(|a| a.is_empty())
        .unwrap_or(true));

    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":4,"method":"tools/call",
        "params":{"name":"right_click","arguments":{"pid":pid,"window_id":wid,"x":5.0,"y":5.0}}
    }));
    let resp = d.recv();
    assert!(
        resp["error"].is_null(),
        "Protocol error from right_click: {resp:?}"
    );
    assert!(!resp["result"]["content"]
        .as_array()
        .map(|a| a.is_empty())
        .unwrap_or(true));
}

#[test]
#[cfg(target_os = "macos")]
fn double_click_and_right_click_pixel_path() {
    //! double_click and right_click at title-bar coords — verifies both tools accept pixel path.
    let Some(mut d) = RawDriver::spawn() else {
        return;
    };

    d.send(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    d.recv();

    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"list_windows","arguments":{"on_screen_only": true}}
    }));
    let resp = d.recv();
    let wins = resp["result"]["structuredContent"]["windows"].as_array();
    let Some(win) = wins.and_then(|a| a.iter().find(|w| w["pid"].as_i64().is_some())) else {
        eprintln!("No on-screen windows — skipping double/right click test");
        return;
    };
    let pid = win["pid"].as_i64().unwrap();
    let wid = win["window_id"].as_u64().unwrap();

    // double_click at safe coords.
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"double_click","arguments":{"pid": pid, "window_id": wid, "x": 5.0, "y": 5.0}}
    }));
    let resp = d.recv();
    assert!(
        resp["error"].is_null(),
        "Protocol error from double_click: {resp:?}"
    );
    assert!(
        !resp["result"]["content"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(true),
        "double_click returned empty content"
    );

    // right_click at same coords.
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":4,"method":"tools/call",
        "params":{"name":"right_click","arguments":{"pid": pid, "window_id": wid, "x": 5.0, "y": 5.0}}
    }));
    let resp = d.recv();
    assert!(
        resp["error"].is_null(),
        "Protocol error from right_click: {resp:?}"
    );
    assert!(
        !resp["result"]["content"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(true),
        "right_click returned empty content"
    );
}

#[test]
#[cfg(target_os = "macos")]
fn set_value_via_element_index() {
    //! get_window_state on TextEdit → find a text-area element → set_value on it.
    //! Skips gracefully if TextEdit is not available.
    let Some(mut d) = RawDriver::spawn() else {
        return;
    };

    d.send(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    d.recv();

    // Ensure TextEdit is running.
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"launch_app","arguments":{"name":"TextEdit"}}
    }));
    d.recv();
    std::thread::sleep(std::time::Duration::from_millis(800));

    // Find TextEdit.
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"list_apps","arguments":{}}
    }));
    let resp = d.recv();
    let apps = resp["result"]["structuredContent"]["apps"].as_array();
    let Some(app) = apps.and_then(|a| {
        a.iter().find(|a| {
            a["name"]
                .as_str()
                .map(|n| n.eq_ignore_ascii_case("textedit"))
                .unwrap_or(false)
        })
    }) else {
        eprintln!("TextEdit not found — skipping set_value test");
        return;
    };
    let pid = app["pid"].as_i64().unwrap();

    // Get first window.
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":4,"method":"tools/call",
        "params":{"name":"list_windows","arguments":{"pid": pid, "on_screen_only": true}}
    }));
    let resp = d.recv();
    let wins = resp["result"]["structuredContent"]["windows"].as_array();
    let Some(win) = wins.and_then(|a| a.first()) else {
        eprintln!("TextEdit has no on-screen windows — skipping set_value test");
        return;
    };
    let wid = win["window_id"].as_u64().unwrap();

    // Walk AX tree to get element_count — we'll target element_index 0.
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":5,"method":"tools/call",
        "params":{"name":"get_window_state","arguments":{"pid": pid, "window_id": wid, "capture_mode": "ax"}}
    }));
    let resp = d.recv();
    assert!(
        resp["error"].is_null(),
        "Protocol error from get_window_state: {resp:?}"
    );
    let element_count = resp["result"]["structuredContent"]["element_count"]
        .as_u64()
        .unwrap_or(0);
    if element_count == 0 {
        eprintln!("No AX elements — skipping set_value test");
        return;
    }

    // set_value on element 0 — this is the document / text area in a new TextEdit document.
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":6,"method":"tools/call",
        "params":{"name":"set_value","arguments":{"pid": pid, "window_id": wid, "element_index": 0, "value": "cua-test-value"}}
    }));
    let resp = d.recv();
    // set_value may fail if element 0 is not settable; accept tool-level errors but not protocol errors.
    assert!(
        resp["error"].is_null(),
        "Protocol error from set_value: {resp:?}"
    );
    let content = resp["result"]["content"].as_array().expect("content array");
    assert!(!content.is_empty(), "set_value returned empty content");
}
