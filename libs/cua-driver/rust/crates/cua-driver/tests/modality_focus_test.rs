//! Integration test: verify that background automation does NOT steal focus.
//!
//! Test strategy:
//! 1. Open a "focus check window" (Terminal) and confirm it is focused.
//! 2. Start a cua-driver MCP server (via the shared `McpDriver` testkit).
//! 3. Send automation actions (click, type_text) to Calculator (a different app).
//! 4. After each action, verify the Terminal window is STILL the active window.
//!
//! Requires: macOS, Calculator (com.apple.calculator), Accessibility permission,
//! and a built `cua-driver` (`cargo build` first).
//!
//! Run with: cargo test --test focus_check_test focus_not_stolen -- --ignored --nocapture

#[cfg(target_os = "macos")]
mod macos_focus_tests {
    use cua_driver_testkit::{Driver, McpDriver};
    use std::process::Command;
    use std::thread;
    use std::time::Duration;

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

    fn find_calculator_pid(driver: &mut McpDriver) -> Option<i32> {
        let resp = driver.call("list_apps", serde_json::json!({}));
        // Use structuredContent.apps array (preferred over text parsing).
        if let Some(apps) = resp.structured()["apps"].as_array() {
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
        for line in resp.text().lines() {
            if line.contains("com.apple.calculator") || line.contains("Calculator") {
                if let Some(pid_str) = line.split("(pid ").nth(1).and_then(|s| s.split(')').next()) {
                    return pid_str.trim().parse().ok();
                }
            }
        }
        None
    }

    #[test]
    #[ignore] // Run explicitly: cargo test --test focus_check_test focus_not_stolen -- --ignored --nocapture
    fn focus_not_stolen_during_calculator_click() {
        // Setup: open Calculator in background, bring Terminal to front.
        open_calculator_background();
        focus_terminal();

        let initial_focus = frontmost_bundle_id();
        println!("Initial focus: {}", initial_focus);

        // Precondition: the decoy (Terminal) must REALLY be the frontmost,
        // non-harness window before we touch the background app. Without this
        // guard the test passes vacuously: if `focus_terminal()` silently
        // failed (Terminal not installed / activation blocked) and Calculator
        // were frontmost instead, every "focus unchanged" assertion below would
        // still hold — even though the harness owns the foreground, which is
        // exactly the regression these tests exist to catch. Mirrors the
        // sentinel-is-up precondition the Windows background tests get for free
        // from the focus-monitor-win pid/hwnd files.
        assert!(
            !initial_focus.is_empty()
                && !initial_focus.eq_ignore_ascii_case("com.apple.calculator"),
            "decoy precondition failed: frontmost app is {initial_focus:?}, expected a \
             non-harness control window (Terminal) to hold the foreground before the \
             background action. Calculator (the harness) must NOT be frontmost, otherwise \
             the no-focus-steal assertions below are vacuous."
        );

        // Start the MCP driver (skips if the binary isn't built).
        let Some(mut driver) = McpDriver::spawn() else { return };

        // Find Calculator.
        let calc_pid = match find_calculator_pid(&mut driver) {
            Some(p) => p,
            None => {
                eprintln!("Calculator not found in running apps");
                return;
            }
        };
        println!("Calculator pid: {}", calc_pid);

        // Get Calculator's window.
        let resp = driver.call("list_windows", serde_json::json!({ "pid": calc_pid }));
        let windows = resp.structured()["windows"].as_array().expect("windows array");
        if windows.is_empty() {
            eprintln!("No windows for Calculator");
            return;
        }
        let window_id = windows[0]["window_id"].as_u64().unwrap() as u32;
        println!("Calculator window_id: {}", window_id);

        // Walk AX tree.
        let resp = driver.call(
            "get_window_state",
            serde_json::json!({ "pid": calc_pid, "window_id": window_id, "capture_mode": "tree" }),
        );
        println!(
            "get_window_state: {}",
            resp.text().chars().take(200).collect::<String>()
        );

        // Verify focus hasn't been stolen yet.
        let focus_after_get = frontmost_bundle_id();
        println!("Focus after get_window_state: {}", focus_after_get);
        assert_eq!(
            focus_after_get, initial_focus,
            "get_window_state STOLE FOCUS! Was: {}, now: {}", initial_focus, focus_after_get
        );

        // Click element_index 1 (a Calculator button supporting AXPress; [0] is
        // the AXWindow itself, which only raises).
        let resp = driver.call(
            "click",
            serde_json::json!({ "pid": calc_pid, "window_id": window_id, "element_index": 1 }),
        );
        println!("click result: {}", resp.text());
        thread::sleep(Duration::from_millis(200));

        // CRITICAL: verify focus was not stolen.
        let focus_after_click = frontmost_bundle_id();
        println!("Focus after click: {}", focus_after_click);
        assert_eq!(
            focus_after_click, initial_focus,
            "click() STOLE FOCUS! Expected {} to remain focused, got {}", initial_focus, focus_after_click
        );

        // type_text also must not steal focus.
        let resp = driver.call("type_text", serde_json::json!({ "pid": calc_pid, "text": "5" }));
        println!("type_text result: {}", resp.text());
        thread::sleep(Duration::from_millis(200));
        let focus_after_type = frontmost_bundle_id();
        println!("Focus after type_text: {}", focus_after_type);
        assert_eq!(
            focus_after_type, initial_focus,
            "type_text() STOLE FOCUS! Expected {} to remain focused, got {}", initial_focus, focus_after_type
        );

        println!("✅ Focus was NOT stolen by click or type_text. Background automation confirmed.");
    }
}
