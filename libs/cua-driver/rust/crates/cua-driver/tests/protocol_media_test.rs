//! Image / capture / recording tool tests.
//!
//! Removed-tool refusal, zoom + the zoom→from_zoom click round trip, the
//! recording session lifecycle (start/stop + action.json), recording
//! screenshot capture, the `record_video` flag, trajectory replay, click
//! `debug_image_out`, and the `set_config` screenshot-resize pipeline. Split
//! out of the old monolithic `mcp_protocol_test.rs`; mac/windows pairs merge
//! and branch only where assertions differ.

#![cfg(any(target_os = "macos", target_os = "windows"))]

use cua_driver_testkit::RawDriver;

#[test]
fn standalone_screenshot_tool_is_removed() {
    let Some(mut d) = RawDriver::spawn() else {
        return;
    };

    d.send(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    d.recv();

    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"screenshot","arguments":{}}
    }));
    let resp = d.recv();
    assert_eq!(resp["result"]["isError"], serde_json::json!(true), "{resp:?}");
    let text = resp["result"]["content"]
        .as_array()
        .and_then(|items| items.iter().find_map(|item| item["text"].as_str()))
        .unwrap_or_default();
    assert_eq!(text, "Unknown tool: screenshot");
}

#[test]
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn zoom_tool_returns_jpeg() {
    //! Call zoom on a visible window and verify the result contains a JPEG image.
    //! Skips gracefully if no windows are visible (headless environment).
    let Some(mut d) = RawDriver::spawn() else {
        return;
    };

    d.send(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    d.recv();

    // List on-screen windows only — off-screen/minimized windows may not be capturable.
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"list_windows","arguments":{"on_screen_only": true}}
    }));
    let resp = d.recv();
    let windows = resp["result"]["structuredContent"]["windows"]
        .as_array()
        .cloned()
        .unwrap_or_default();

    // Try windows until one captures successfully.
    let mut found_jpeg = false;
    let mut tried = 0usize;
    let mut zoom_errors: Vec<String> = Vec::new();
    let mut successful_without_jpeg = false;
    for win in windows.iter().take(5) {
        let Some(wid) = win["window_id"].as_u64() else {
            continue;
        };
        tried += 1;

        d.send(&serde_json::json!({
            "jsonrpc":"2.0","id": 2 + tried as u64,"method":"tools/call",
            "params":{"name":"zoom","arguments":{
                "window_id": wid,
                "x1": 0, "y1": 0, "x2": 100, "y2": 100
            }}
        }));
        let r = d.recv();
        if r["result"]["isError"].as_bool().unwrap_or(false) {
            // This window might be off-screen or not capturable — try the next.
            let text = r["result"]["content"]
                .as_array()
                .and_then(|items| items.iter().find_map(|c| c["text"].as_str()))
                .unwrap_or("<no error text>")
                .to_owned();
            zoom_errors.push(format!("window_id={wid}: {text}"));
            continue;
        }
        let content = r["result"]["content"].as_array().expect("content array");
        found_jpeg = content.iter().any(|c| {
            c["type"] == "image"
                && c["mimeType"].as_str().unwrap_or("") == "image/jpeg"
                && c["data"].as_str().map(|s| s.len() > 10).unwrap_or(false)
        });
        if !found_jpeg {
            successful_without_jpeg = true;
        }
        if found_jpeg {
            // Also verify structuredContent.
            let sc = &r["result"]["structuredContent"];
            assert!(
                sc["width"].as_f64().unwrap_or(0.0) > 0.0,
                "Expected positive width: {sc:?}"
            );
            assert!(
                sc["height"].as_f64().unwrap_or(0.0) > 0.0,
                "Expected positive height: {sc:?}"
            );
            assert_eq!(
                sc["format"].as_str().unwrap_or(""),
                "jpeg",
                "Expected format=jpeg: {sc:?}"
            );
            break;
        }
    }

    if tried == 0 {
        eprintln!("No on-screen windows found — skipping zoom test");
        return;
    }
    if !found_jpeg
        && !successful_without_jpeg
        && cfg!(target_os = "macos")
        && !zoom_errors.is_empty()
        && zoom_errors
            .iter()
            .all(|e| e.contains("screencapture failed"))
    {
        eprintln!(
            "No visible windows were capturable by the raw unbundled test process — skipping zoom test. \
             Errors: {}",
            zoom_errors.join(" | ")
        );
        return;
    }
    assert!(
        found_jpeg,
        "Expected at least one window to return a valid JPEG from zoom. Errors: {}",
        zoom_errors.join(" | ")
    );
}

#[test]
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn zoom_from_zoom_click_round_trip() {
    //! Verify the zoom → from_zoom click pipeline:
    //! 1. click(from_zoom=true) with no zoom context returns the expected error.
    //! 2. zoom() stores context; subsequent click(from_zoom=true) does NOT return
    //!    that error (translation succeeded, even if the click itself may fail).
    let Some(mut d) = RawDriver::spawn() else {
        return;
    };

    d.send(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    d.recv();

    // List on-screen windows to find a target window and pid.
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"list_windows","arguments":{"on_screen_only":true}}
    }));
    let resp = d.recv();
    let wins = resp["result"]["structuredContent"]["windows"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let Some(win) = wins.iter().find(|w| w["pid"].as_i64().is_some()) else {
        eprintln!("No on-screen windows — skipping from_zoom test");
        return;
    };
    let window_id = win["window_id"].as_u64().unwrap();
    let pid = win["pid"].as_i64().unwrap();

    // Step 1: click with from_zoom=true and NO prior zoom context — expect error.
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"click","arguments":{"pid": pid, "window_id": window_id, "x":10.0,"y":10.0,"from_zoom":true}}
    }));
    let resp = d.recv();
    let is_err = resp["result"]["isError"].as_bool().unwrap_or(false);
    let err_text = resp["result"]["content"][0]["text"].as_str().unwrap_or("");
    assert!(
        is_err,
        "Expected error when from_zoom=true with no context, got: {resp:?}"
    );
    assert!(
        err_text.contains("no zoom context"),
        "Expected 'no zoom context' error, got: {err_text}"
    );

    // Step 2: call zoom on that window to store context.
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":4,"method":"tools/call",
        "params":{"name":"zoom","arguments":{"window_id": window_id, "pid": pid, "x1":0,"y1":0,"x2":50,"y2":50}}
    }));
    let resp = d.recv();
    if resp["result"]["isError"].as_bool().unwrap_or(false) {
        // Window may not be capturable (e.g. system UI) — skip.
        eprintln!("zoom failed — skipping from_zoom click test");
        return;
    }
    if !cfg!(target_os = "windows") {
        // Verify zoom returned structured content with width/height.
        let sc = &resp["result"]["structuredContent"];
        assert!(
            sc["width"].as_u64().unwrap_or(0) > 0,
            "zoom should return positive width: {sc:?}"
        );
    }

    // Step 3: click with from_zoom=true — should NOT return the "no zoom context" error.
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":5,"method":"tools/call",
        "params":{"name":"click","arguments":{"pid": pid, "window_id": window_id, "x":10.0,"y":10.0,"from_zoom":true}}
    }));
    let resp = d.recv();
    let err_text = resp["result"]["content"][0]["text"].as_str().unwrap_or("");
    // Translation should succeed — if click fails it's for another reason (target app state), not missing context.
    assert!(
        !err_text.contains("no zoom context"),
        "After zoom(), from_zoom click should not say 'no zoom context', got: {err_text}"
    );
}

#[test]
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn recording_session() {
    //! Enable recording, invoke a non-read-only tool (recorded), disable, verify action.json written.
    let Some(mut d) = RawDriver::spawn() else {
        return;
    };

    let tmp_dir =
        std::env::temp_dir().join(format!("cua-driver-rs-rec-test-{}", std::process::id()));
    let tmp_str = tmp_dir.to_string_lossy().to_string();

    d.send(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    d.recv();

    // Enable recording.
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"start_recording","arguments":{"output_dir":tmp_str,"record_video":false}}
    }));
    let resp = d.recv();
    assert!(
        !resp["result"]["isError"].as_bool().unwrap_or(false),
        "start_recording failed: {resp:?}"
    );
    assert!(resp["result"]["structuredContent"]["enabled"]
        .as_bool()
        .unwrap_or(false));

    // Invoke a non-read-only tool — should be recorded.
    if cfg!(target_os = "windows") {
        d.send(&serde_json::json!({
            "jsonrpc":"2.0","id":3,"method":"tools/call",
            "params":{"name":"move_cursor","arguments":{"x":100.0,"y":200.0}}
        }));
    } else {
        d.send(&serde_json::json!({
            "jsonrpc":"2.0","id":3,"method":"tools/call",
            "params":{"name":"set_agent_cursor_enabled","arguments":{"enabled":false}}
        }));
    }
    d.recv();

    // get_recording_state — next_turn should advance after one recorded action.
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":4,"method":"tools/call",
        "params":{"name":"get_recording_state","arguments":{}}
    }));
    let resp = d.recv();
    let next_turn = resp["result"]["structuredContent"]["next_turn"]
        .as_u64()
        .unwrap_or(0);
    assert!(next_turn >= 2, "expected next_turn >= 2, got {next_turn}");

    // Disable recording.
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":5,"method":"tools/call",
        "params":{"name":"stop_recording","arguments":{}}
    }));
    let resp = d.recv();
    assert!(!resp["result"]["structuredContent"]["enabled"]
        .as_bool()
        .unwrap_or(true));
    drop(d);

    // Small sync wait for the write to flush.
    std::thread::sleep(std::time::Duration::from_millis(50));

    // Verify turn-00001/action.json was written.
    let action_path = tmp_dir.join("turn-00001").join("action.json");
    assert!(action_path.exists(), "Expected {action_path:?} to exist");
    let content: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&action_path).unwrap()).unwrap();
    if cfg!(target_os = "windows") {
        assert_eq!(content["tool"].as_str().unwrap_or(""), "move_cursor");
        assert_eq!(content["arguments"]["x"].as_f64().unwrap_or(0.0), 100.0);
    } else {
        assert_eq!(
            content["tool"].as_str().unwrap_or(""),
            "set_agent_cursor_enabled"
        );
        assert_eq!(content["arguments"]["enabled"].as_bool(), Some(false));
    }

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
#[cfg(target_os = "macos")]
fn recording_screenshot_capture() {
    //! When recording is active and a tool call includes a window_id, a screenshot.png
    //! should appear alongside action.json in the turn folder.
    let Some(mut d) = RawDriver::spawn() else {
        return;
    };

    let tmp_dir = std::env::temp_dir().join(format!("cua-driver-rs-rec-ss-{}", std::process::id()));
    let tmp_str = tmp_dir.to_string_lossy().to_string();

    d.send(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    d.recv();

    // Find a capturable on-screen window.
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"list_windows","arguments":{"on_screen_only":true}}
    }));
    let resp = d.recv();
    let wins = resp["result"]["structuredContent"]["windows"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    // Prefer windows with non-empty titles (more likely capturable).
    let win = wins
        .iter()
        .find(|w| w["pid"].as_i64().is_some() && !w["title"].as_str().unwrap_or("").is_empty())
        .or_else(|| wins.iter().find(|w| w["pid"].as_i64().is_some()));
    let Some(win) = win else {
        eprintln!("No on-screen windows — skipping recording screenshot test");
        return;
    };
    let window_id = win["window_id"].as_u64().unwrap();
    let pid = win["pid"].as_i64().unwrap();

    // Enable recording.
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"start_recording","arguments":{"output_dir":tmp_str,"record_video":false}}
    }));
    let resp = d.recv();
    assert!(
        !resp["result"]["isError"].as_bool().unwrap_or(false),
        "start_recording failed: {resp:?}"
    );

    // Invoke click (non-read-only) with window_id + pid — recording should capture screenshot.
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":4,"method":"tools/call",
        "params":{"name":"click","arguments":{"pid": pid, "window_id": window_id, "x":0.0,"y":0.0}}
    }));
    d.recv(); // result may be error if coords are off-window — that's ok

    // Disable recording.
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":5,"method":"tools/call",
        "params":{"name":"stop_recording","arguments":{}}
    }));
    d.recv();
    drop(d);

    std::thread::sleep(std::time::Duration::from_millis(100));

    // action.json must exist.
    let turn_dir = tmp_dir.join("turn-00001");
    let action_path = turn_dir.join("action.json");
    assert!(
        action_path.exists(),
        "Expected action.json at {action_path:?}"
    );
    let content: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&action_path).unwrap()).unwrap();
    assert_eq!(content["tool"].as_str().unwrap_or(""), "click");

    // screenshot.png should exist if screencapture succeeded for this window.
    let ss_path = turn_dir.join("screenshot.png");
    if !ss_path.exists() {
        eprintln!("screenshot.png not written for window {window_id} (may be uncapturable) — skipping PNG check");
    } else {
        let ss_bytes = std::fs::read(&ss_path).unwrap();
        assert!(!ss_bytes.is_empty(), "screenshot.png should not be empty");
        assert_eq!(
            &ss_bytes[..4],
            b"\x89PNG",
            "screenshot.png should be a valid PNG"
        );

        // click.png should also exist (click at 0,0 with crosshair marker).
        let click_path = turn_dir.join("click.png");
        if !click_path.exists() {
            eprintln!("click.png not written — skipping click PNG check");
        } else {
            let click_bytes = std::fs::read(&click_path).unwrap();
            assert!(!click_bytes.is_empty(), "click.png should not be empty");
            assert_eq!(
                &click_bytes[..4],
                b"\x89PNG",
                "click.png should be a valid PNG"
            );
        }
    }

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn start_recording_record_video_flag_accepted() {
    //! `record_video` is the new on/off flag (replaces the old
    //! `video_experimental`). Default is true. This test passes
    //! `record_video: false` to bypass the ffmpeg dependency in CI;
    //! we only verify that the schema accepts the call and the
    //! session enters the enabled state. Full video lifecycle is
    //! covered by the manual demo + the per-platform smoke when
    //! ffmpeg is installed.
    let Some(mut d) = RawDriver::spawn() else {
        return;
    };

    let tmp_dir = std::env::temp_dir().join(format!("cua-driver-rs-recvid-{}", std::process::id()));
    let tmp_str = tmp_dir.to_string_lossy().to_string();
    std::fs::create_dir_all(&tmp_dir).unwrap();

    d.send(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    d.recv();

    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"start_recording","arguments":{
            "output_dir": tmp_str, "record_video": false
        }}
    }));
    let resp = d.recv();
    assert!(
        resp["error"].is_null(),
        "Expected no JSON-RPC error, got: {resp:?}"
    );
    let is_err = resp["result"]["isError"].as_bool().unwrap_or(false);
    assert!(
        !is_err,
        "start_recording with record_video:false should succeed: {resp:?}"
    );

    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"stop_recording","arguments":{}}
    }));
    d.recv();
    drop(d);
    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn replay_trajectory() {
    //! Write a minimal trajectory (one move_cursor turn), replay it, verify succeeded=1.
    let Some(mut d) = RawDriver::spawn() else {
        return;
    };

    let traj_dir =
        std::env::temp_dir().join(format!("cua-driver-rs-replay-{}", std::process::id()));
    let turn_dir = traj_dir.join("turn-00001");
    std::fs::create_dir_all(&turn_dir).unwrap();
    std::fs::write(
        turn_dir.join("action.json"),
        r#"{"tool":"move_cursor","arguments":{"x":50.0,"y":60.0}}"#,
    )
    .unwrap();

    d.send(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    d.recv();

    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"replay_trajectory","arguments":{
            "dir": traj_dir.to_string_lossy(),
            "delay_ms": 0
        }}
    }));
    let resp = d.recv();
    assert!(
        !resp["result"]["isError"].as_bool().unwrap_or(false),
        "replay error: {resp:?}"
    );
    let sc = &resp["result"]["structuredContent"];
    assert_eq!(
        sc["attempted"].as_u64().unwrap_or(0),
        1,
        "expected attempted=1"
    );
    assert_eq!(
        sc["succeeded"].as_u64().unwrap_or(0),
        1,
        "expected succeeded=1"
    );
    assert_eq!(sc["failed"].as_u64().unwrap_or(99), 0, "expected failed=0");

    drop(d);
    let _ = std::fs::remove_dir_all(&traj_dir);
}

#[test]
#[cfg(target_os = "macos")]
fn click_debug_image_out() {
    //! click with debug_image_out writes a PNG crosshair file and then proceeds.
    //! Verifies the debug capture path works end-to-end.
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
    // Prefer a window with a non-empty title to avoid uncapturable overlay/system windows.
    let Some(win) = wins.and_then(|a| {
        a.iter()
            .find(|w| w["pid"].as_i64().is_some() && !w["title"].as_str().unwrap_or("").is_empty())
            .or_else(|| a.iter().find(|w| w["pid"].as_i64().is_some()))
    }) else {
        eprintln!("No on-screen windows — skipping debug_image_out test");
        return;
    };
    let pid = win["pid"].as_i64().unwrap();
    let wid = win["window_id"].as_u64().unwrap();

    let dbg_path =
        std::env::temp_dir().join(format!("cua-driver-rs-dbg-{}.png", std::process::id()));
    let dbg_path_str = dbg_path.to_string_lossy().to_string();

    // Click with debug_image_out — should write the PNG and then proceed.
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"click","arguments":{
            "pid": pid, "window_id": wid,
            "x": 10.0, "y": 10.0,
            "debug_image_out": dbg_path_str
        }}
    }));
    let resp = d.recv();
    assert!(
        resp["error"].is_null(),
        "Protocol error from click+debug_image_out: {resp:?}"
    );

    // The tool may return isError if screencapture can't capture this particular window
    // (e.g., overlay windows, system windows). If that happens, skip the PNG check.
    let is_error = resp["result"]["isError"].as_bool().unwrap_or(false);
    if is_error {
        let msg = resp["result"]["content"][0]["text"].as_str().unwrap_or("?");
        eprintln!("debug_image_out screenshot failed for this window ({msg}) — skipping PNG check");
        return;
    }

    // Verify the PNG was actually written.
    assert!(
        dbg_path.exists(),
        "debug_image_out PNG was not written to {dbg_path:?}"
    );
    let meta = std::fs::metadata(&dbg_path).expect("metadata");
    assert!(
        meta.len() > 100,
        "debug_image_out PNG is suspiciously small"
    );

    let _ = std::fs::remove_file(&dbg_path);
}

#[test]
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn set_config_screenshot_resize() {
    //! set_config(max_image_dimension=200) then get_window_state — the returned
    //! window image must have both dimensions ≤ 200.
    let Some(mut d) = RawDriver::spawn() else {
        return;
    };

    d.send(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    d.recv();

    // Set a tiny max_image_dimension so the window screenshot will be resized.
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"set_config","arguments":{"max_image_dimension":200}}
    }));
    let resp = d.recv();
    assert!(resp["error"].is_null(), "set_config failed: {resp:?}");
    assert!(
        !resp["result"]["isError"].as_bool().unwrap_or(false),
        "set_config returned error: {resp:?}"
    );

    // Find a visible window to capture through the canonical state tool.
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"list_windows","arguments":{}}
    }));
    let listed = d.recv();
    let target = listed["result"]["structuredContent"]["windows"]
        .as_array()
        .and_then(|windows| windows.iter().find(|window| {
            window["is_on_screen"].as_bool() == Some(true)
                && window["pid"].as_u64().is_some()
                && window["window_id"].as_u64().is_some()
        }));
    if let Some(target) = target {
        let pid = target["pid"].as_u64().unwrap();
        let window_id = target["window_id"].as_u64().unwrap();
        d.send(&serde_json::json!({
            "jsonrpc":"2.0","id":4,"method":"tools/call",
            "params":{"name":"get_window_state","arguments":{
                "pid":pid,"window_id":window_id,"max_elements":10,"max_depth":2
            }}
        }));
        let state = d.recv();
        if !state["result"]["isError"].as_bool().unwrap_or(false) {
            let dimensions = (
                state["result"]["structuredContent"]["screenshot_width"].as_u64(),
                state["result"]["structuredContent"]["screenshot_height"].as_u64(),
            );
            if let (Some(w), Some(h)) = dimensions {
                assert!(w <= 200, "screenshot width {w} should be ≤ 200");
                assert!(h <= 200, "screenshot height {h} should be ≤ 200");
            }
        }
    }

    // get_config should reflect the updated value even in a headless session.
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":5,"method":"tools/call",
        "params":{"name":"get_config","arguments":{}}
    }));
    let resp = d.recv();
    let dim = resp["result"]["structuredContent"]["max_image_dimension"].as_u64();
    assert_eq!(
        dim,
        Some(200),
        "get_config should reflect max_image_dimension=200"
    );
}
