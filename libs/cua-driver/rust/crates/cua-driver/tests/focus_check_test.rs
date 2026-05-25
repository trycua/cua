//! Integration test: verify that background automation does NOT steal focus.
//!
//! Test strategy:
//! 1. Open a "focus check window" (Terminal) and confirm it is focused.
//! 2. Start a cua-driver-rs MCP server process.
//! 3. Send automation actions (click, type_text) to Calculator (a different app).
//! 4. After each action, verify the Terminal window is STILL the active window.
//!
//! This test requires:
//! - macOS (or Linux/Windows equivalents)
//! - Calculator app installed (com.apple.calculator on macOS)
//! - Accessibility permission granted
//!
//! Run with: cargo test --test focus_check_test -- --nocapture
//! Requires: `cargo build` first to produce the binary.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::Duration;
use std::thread;

#[cfg(target_os = "macos")]
mod macos_focus_tests {
    use super::*;

    fn binary_path() -> std::path::PathBuf {
        let manifest = std::env::var("CARGO_MANIFEST_DIR")
            .expect("CARGO_MANIFEST_DIR not set");
        std::path::PathBuf::from(manifest)
            .parent().unwrap()  // crates/
            .parent().unwrap()  // workspace root
            .join("target/debug/cua-driver")
    }

    fn send(stdin: &mut impl Write, req: serde_json::Value) {
        writeln!(stdin, "{}", serde_json::to_string(&req).unwrap()).unwrap();
    }

    fn recv(stdout: &mut impl BufRead) -> serde_json::Value {
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        serde_json::from_str(line.trim()).unwrap()
    }

    /// Get the bundle ID of the current frontmost app via osascript.
    fn frontmost_bundle_id() -> String {
        let out = Command::new("osascript")
            .arg("-e")
            .arg(r#"tell application "System Events" to bundle identifier of first application process whose frontmost is true"#)
            .output()
            .expect("osascript");
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    /// Bring Terminal to the front (our "focus check window").
    fn focus_terminal() {
        let _ = Command::new("osascript")
            .arg("-e")
            .arg(r#"tell application "Terminal" to activate"#)
            .output();
        thread::sleep(Duration::from_millis(300));
    }

    /// Open Calculator in background.
    fn open_calculator_background() {
        let _ = Command::new("open")
            .args(["-g", "-b", "com.apple.calculator"])
            .output();
        thread::sleep(Duration::from_secs(1));
    }

    fn find_calculator_pid(stdout: &mut impl BufRead, stdin: &mut impl Write) -> Option<i32> {
        send(stdin, serde_json::json!({
            "jsonrpc": "2.0", "id": 100, "method": "tools/call",
            "params": { "name": "list_apps", "arguments": {} }
        }));
        let resp = recv(stdout);
        // Use structuredContent.apps array (preferred over text parsing).
        if let Some(apps) = resp["result"]["structuredContent"]["apps"].as_array() {
            for app in apps {
                let bundle = app["bundle_id"].as_str().unwrap_or("");
                let name = app["name"].as_str().unwrap_or("");
                if bundle.contains("calculator") || name.eq_ignore_ascii_case("calculator") {
                    if let Some(pid) = app["pid"].as_i64() {
                        return Some(pid as i32);
                    }
                }
            }
        }
        // Fallback: parse text content for older binary versions.
        let text = resp["result"]["content"][0]["text"].as_str()?;
        for line in text.lines() {
            if line.contains("com.apple.calculator") || line.contains("Calculator") {
                if let Some(pid_str) = line.split("(pid ").nth(1)
                    .and_then(|s| s.split(')').next())
                {
                    return pid_str.trim().parse().ok();
                }
            }
        }
        None
    }

    #[test]
    #[ignore] // Run explicitly: cargo test --test focus_check_test focus_not_stolen -- --ignored --nocapture
    fn focus_not_stolen_during_calculator_click() {
        let binary = binary_path();
        if !binary.exists() {
            eprintln!("Binary not built. Run `cargo build` first.");
            return;
        }

        // Setup: open Calculator in background, bring Terminal to front.
        open_calculator_background();
        focus_terminal();

        let initial_focus = frontmost_bundle_id();
        println!("Initial focus: {}", initial_focus);
        // Terminal or our test runner should be focused.

        // Start the MCP driver.
        let mut child = Command::new(&binary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn cua-driver-rs");

        let stdin = child.stdin.as_mut().unwrap();
        let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());

        // Initialize.
        send(stdin, serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
        recv(&mut stdout);

        // Find Calculator.
        let calc_pid = find_calculator_pid(&mut stdout, stdin);
        let calc_pid = match calc_pid {
            Some(p) => p,
            None => {
                eprintln!("Calculator not found in running apps");
                child.kill().ok();
                return;
            }
        };
        println!("Calculator pid: {}", calc_pid);

        // Get Calculator's window.
        send(stdin, serde_json::json!({
            "jsonrpc": "2.0", "id": 200, "method": "tools/call",
            "params": {
                "name": "list_windows",
                "arguments": { "pid": calc_pid }
            }
        }));
        let resp = recv(&mut stdout);
        let windows = resp["result"]["structuredContent"]["windows"].as_array()
            .expect("windows array");
        if windows.is_empty() {
            eprintln!("No windows for Calculator");
            child.kill().ok();
            return;
        }
        let window_id = windows[0]["window_id"].as_u64().unwrap() as u32;
        println!("Calculator window_id: {}", window_id);

        // Walk AX tree.
        send(stdin, serde_json::json!({
            "jsonrpc": "2.0", "id": 300, "method": "tools/call",
            "params": {
                "name": "get_window_state",
                "arguments": { "pid": calc_pid, "window_id": window_id, "capture_mode": "tree" }
            }
        }));
        let resp = recv(&mut stdout);
        println!("get_window_state: {}", resp["result"]["content"][0]["text"].as_str().unwrap_or("(none)").chars().take(200).collect::<String>());

        // Verify focus hasn't been stolen yet.
        let focus_after_get = frontmost_bundle_id();
        println!("Focus after get_window_state: {}", focus_after_get);
        assert_eq!(
            focus_after_get, initial_focus,
            "get_window_state STOLE FOCUS! Was: {}, now: {}", initial_focus, focus_after_get
        );

        // Find a button element to click (element_index 1 is the Delete/AC button
        // in Calculator's AX tree and supports AXPress).
        // We use element_index=1 because [0] is the AXWindow itself (only raises).
        send(stdin, serde_json::json!({
            "jsonrpc": "2.0", "id": 400, "method": "tools/call",
            "params": {
                "name": "click",
                "arguments": { "pid": calc_pid, "window_id": window_id, "element_index": 1 }
            }
        }));
        let resp = recv(&mut stdout);
        let click_text = resp["result"]["content"][0]["text"].as_str().unwrap_or("?");
        println!("click result: {}", click_text);
        // Accept either a successful click or a supported AX error — what matters is no crash/focus steal.

        thread::sleep(Duration::from_millis(200));

        // CRITICAL: verify focus was not stolen.
        let focus_after_click = frontmost_bundle_id();
        println!("Focus after click: {}", focus_after_click);
        assert_eq!(
            focus_after_click, initial_focus,
            "click() STOLE FOCUS! Expected {} to remain focused, got {}", initial_focus, focus_after_click
        );

        // type_text also must not steal focus.
        send(stdin, serde_json::json!({
            "jsonrpc": "2.0", "id": 500, "method": "tools/call",
            "params": {
                "name": "type_text",
                "arguments": { "pid": calc_pid, "text": "5" }
            }
        }));
        let resp = recv(&mut stdout);
        println!("type_text result: {}", resp["result"]["content"][0]["text"].as_str().unwrap_or("?"));
        thread::sleep(Duration::from_millis(200));
        let focus_after_type = frontmost_bundle_id();
        println!("Focus after type_text: {}", focus_after_type);
        assert_eq!(
            focus_after_type, initial_focus,
            "type_text() STOLE FOCUS! Expected {} to remain focused, got {}", initial_focus, focus_after_type
        );

        println!("✅ Focus was NOT stolen by click or type_text. Background automation confirmed.");
        child.kill().ok();
    }
}
