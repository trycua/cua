// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Calculator's result is display-only text, so it must be readable and
//! verifiable without an action handle (#2958).
//!
//! `4 + 2 =` goes in through element-token clicks. The result `6` then has to
//! appear in `get_window_state.observations` (and not in `elements`), and
//! `verify_state` has to satisfy a unique selector on its raw value. The
//! oracle is AppleScript reading Calculator's own accessibility tree through
//! System Events, independent of the driver.
//!
//! Run with:
//! `cargo test -p cua-driver-e2e --test installed_app_calculator_macos_test -- --ignored --nocapture --test-threads=1`
//! against a running installed daemon (`CUA_E2E_MACOS_DAEMON_SOCKET`).

#![cfg(target_os = "macos")]

use std::process::Command;
use std::time::{Duration, Instant};

use cua_driver_testkit::{Driver, McpDriver};

const CALCULATOR_BUNDLE: &str = "com.apple.calculator";

struct QuitCalculator;

impl Drop for QuitCalculator {
    fn drop(&mut self) {
        let _ = Command::new("pkill").args(["-x", "Calculator"]).status();
    }
}

/// Every static text in Calculator's front window, read by System Events.
fn oracle_static_texts() -> Vec<String> {
    let output = Command::new("osascript")
        .args([
            "-e",
            r#"tell application "System Events" to tell process "Calculator" to get value of every static text of entire contents of front window"#,
        ])
        .output()
        .expect("run osascript");
    assert!(
        output.status.success(),
        "System Events could not read Calculator: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .split(", ")
        .map(str::to_owned)
        .collect()
}

fn read_state(driver: &mut McpDriver, pid: i64, window_id: u64) -> serde_json::Value {
    let state = driver.call(
        "get_window_state",
        serde_json::json!({
            "pid": pid,
            "window_id": window_id,
            "include_screenshot": false,
            "tree_format": "elements",
        }),
    );
    assert!(
        !state.is_error(),
        "get_window_state failed: {}",
        state.text()
    );
    state.structured().clone()
}

/// Launch Calculator and find its keypad window. WindowServer may list
/// helper windows, and titles are unavailable without Screen Recording, so
/// select the window whose tree holds the Equals button.
fn launch_calculator(driver: &mut McpDriver) -> (i64, u64) {
    let launch = driver.call(
        "launch_app",
        serde_json::json!({ "bundle_id": CALCULATOR_BUNDLE }),
    );
    assert!(
        !launch.is_error(),
        "could not launch Calculator: {}",
        launch.text()
    );
    let pid = launch.structured()["pid"].as_i64().expect("Calculator pid");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let listed = driver.call("list_windows", serde_json::json!({ "pid": pid }));
        let candidates: Vec<u64> = listed.structured()["windows"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|window| window["is_on_screen"] == true)
            .filter_map(|window| window["window_id"].as_u64())
            .collect();
        for window_id in candidates {
            let state = read_state(driver, pid, window_id);
            let has_keypad = state["elements"].as_array().is_some_and(|elements| {
                elements
                    .iter()
                    .any(|element| element["role"] == "AXButton" && element["label"] == "Equals")
            });
            if has_keypad {
                return (pid, window_id);
            }
        }
        assert!(
            Instant::now() < deadline,
            "Calculator opened no window with its keypad: {}",
            listed.text()
        );
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn click_button(driver: &mut McpDriver, pid: i64, window_id: u64, label: &str) {
    let state = read_state(driver, pid, window_id);
    let token = state["elements"]
        .as_array()
        .and_then(|elements| {
            elements
                .iter()
                .find(|element| element["role"] == "AXButton" && element["label"] == label)
        })
        .and_then(|element| element["element_token"].as_str())
        .unwrap_or_else(|| panic!("Calculator has no {label:?} button: {state}"))
        .to_owned();
    let click = driver.call(
        "click",
        serde_json::json!({ "pid": pid, "window_id": window_id, "element_token": token }),
    );
    assert!(
        !click.is_error(),
        "click {label:?} failed: {}",
        click.text()
    );
}

fn verify(driver: &mut McpDriver, pid: i64, window_id: u64, label: &str, value: &str) -> String {
    let verified = driver.call(
        "verify_state",
        serde_json::json!({
            "pid": pid,
            "window_id": window_id,
            "timeout_ms": 2000,
            "expect": [{"element": {
                "selector": {"role": "AXStaticText", "label_contains": label},
                "value_equals": value,
            }}],
        }),
    );
    assert!(
        !verified.is_error(),
        "verify_state failed: {}",
        verified.text()
    );
    verified.structured()["status"]
        .as_str()
        .expect("verify_state status")
        .to_owned()
}

#[test]
#[ignore = "needs a logged-in macOS desktop with Calculator and System Events access"]
fn calculator_result_is_an_observation_that_verify_state_can_check() {
    // A fresh Calculator: an earlier run's history must not duplicate the result.
    drop(QuitCalculator);
    std::thread::sleep(Duration::from_millis(500));
    // The installed, TCC-authorized daemon: a raw test binary has no
    // Accessibility grant of its own.
    let mut driver = McpDriver::spawn_macos_daemon_proxy_named("installed-app-calculator")
        .expect("start installed macOS daemon proxy");
    let _quit = QuitCalculator;
    let (pid, window_id) = launch_calculator(&mut driver);

    for label in ["All Clear", "4", "Add", "2", "Equals"] {
        click_button(&mut driver, pid, window_id, label);
    }

    // Calculator wraps each display run in U+200E, which the driver keeps.
    let result = "\u{200e}6";
    let deadline = Instant::now() + Duration::from_secs(5);
    while !oracle_static_texts().iter().any(|text| text == result) {
        assert!(
            Instant::now() < deadline,
            "Calculator never displayed 6: {:?}",
            oracle_static_texts()
        );
        std::thread::sleep(Duration::from_millis(200));
    }

    let state = read_state(&mut driver, pid, window_id);
    let elements = state["elements"].as_array().expect("elements");
    assert!(
        elements
            .iter()
            .all(|element| element["role"] != "AXStaticText"),
        "display text must stay out of the addressable elements: {state}"
    );
    let observations = state["observations"]
        .as_array()
        .unwrap_or_else(|| panic!("macOS must publish observations: {state}"));
    let shown = observations
        .iter()
        .find(|row| row["role"] == "AXStaticText" && row["value"] == result)
        .unwrap_or_else(|| panic!("the result is not an observation: {state}"));
    assert!(shown.get("element_index").is_none());
    assert!(shown.get("element_token").is_none());

    assert_eq!(
        verify(&mut driver, pid, window_id, result, result),
        "satisfied"
    );
    assert_eq!(
        verify(&mut driver, pid, window_id, result, "\u{200e}7"),
        "unsatisfied"
    );
    // Values compare exactly: the bare digit is not the displayed text.
    assert_eq!(
        verify(&mut driver, pid, window_id, result, "6"),
        "unsatisfied"
    );
}
