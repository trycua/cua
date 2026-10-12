// SPDX-License-Identifier: MIT

//! #5012: a synchronous modal UIA Invoke must not hold the MCP response open.
//! Requires the repository WinForms modal fixture and an interactive desktop.
#![cfg(target_os = "windows")]

use cua_driver_testkit::{ax, harness_app, spawn_in_job, Driver, McpDriver, ToolResponse};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn counts(path: &Path) -> BTreeMap<String, u32> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            Some((key.into(), value.parse().ok()?))
        })
        .collect()
}

fn wait_count(path: &Path, key: &str, expected: u32) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if counts(path).get(key) == Some(&expected) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{key} did not reach {expected}: {:?}",
            counts(path)
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn snapshot(driver: &mut McpDriver, pid: u32, wid: u64) -> ToolResponse {
    let response = driver.call(
        "get_window_state",
        json!({"pid": pid, "window_id": wid, "include_screenshot": false}),
    );
    assert!(!response.is_error(), "snapshot failed: {}", response.text());
    response
}

fn click_args(snap: &ToolResponse, label: &str, pid: u32, wid: u64) -> serde_json::Value {
    let idx = ax::element_index_containing(snap.tree_text(), label)
        .unwrap_or_else(|| panic!("missing fixture control {label:?}: {}", snap.tree_text()));
    json!({"pid": pid, "window_id": wid, "element_token": snap.element_token(idx), "delivery_mode": "background"})
}

#[test]
#[ignore]
fn synchronous_modal_click_times_out_without_replay_and_recovers() {
    let mut driver = McpDriver::spawn_named("semantic-modal-deadline").expect("built driver");
    let temp = tempfile::tempdir().unwrap();
    let journal = temp.path().join("fixture.txt");
    let exe = harness_app("harness-winforms-modal", "CuaTestHarness.WinFormsModal.exe");
    assert!(
        exe.exists(),
        "build tests/fixtures/apps/windows/winforms-modal/build.ps1 first: {exe:?}"
    );
    let app = spawn_in_job(
        Command::new(exe)
            .arg(&journal)
            .stdout(Stdio::null())
            .stderr(Stdio::null()),
    )
    .unwrap();
    let pid = app.id();
    driver.reaper().push(app);
    let (wid, _) = driver
        .find_window(pid.into(), "CUA semantic deadline fixture")
        .expect("owned main window");
    let session = driver.call("start_session", json!({}));
    assert!(!session.is_error(), "{}", session.text());
    driver.start_behavior_recording();

    let snap = snapshot(&mut driver, pid, wid);
    let normal = driver.call(
        "click",
        click_args(&snap, "Normal semantic action", pid, wid),
    );
    assert!(!normal.is_error(), "{}", normal.text());
    assert!(normal.text().contains("UIA Invoke"), "{}", normal.text());
    wait_count(&journal, "normal", 1);

    let snap = snapshot(&mut driver, pid, wid);
    let args = click_args(&snap, "Open synchronous modal", pid, wid);
    let start = Instant::now();
    let response = driver.call("click", args.clone());
    println!("modal_response_ms={}", start.elapsed().as_millis());
    assert!(
        start.elapsed() < Duration::from_secs(6),
        "semantic response was not bounded: {}",
        response.text()
    );
    assert!(
        response.is_error(),
        "modal Invoke should exceed the response budget"
    );
    assert_eq!(response.structured()["code"], "background_unavailable");
    assert_eq!(response.structured()["uia_status"], "timeout");
    assert_eq!(response.structured()["effect"], "unverifiable");
    wait_count(&journal, "opened", 1);
    assert_eq!(
        counts(&journal).get("closed"),
        Some(&0),
        "modal must still be open at timeout"
    );

    // Reuse the same admitted token. No second worker or activation is allowed.
    let retry_start = Instant::now();
    let busy = driver.call("click", args);
    println!("busy_response_ms={}", retry_start.elapsed().as_millis());
    assert!(retry_start.elapsed() < Duration::from_secs(1));
    assert!(busy.is_error());
    assert_eq!(busy.structured()["code"], "background_unavailable");
    assert_eq!(busy.structured()["uia_status"], "busy");
    assert_eq!(busy.structured()["effect"], "refused");
    assert_eq!(counts(&journal).get("opened"), Some(&1));
    assert_eq!(
        counts(&journal).get("mouse_down"),
        Some(&0),
        "no coordinate fallback"
    );

    // Read-only Win32 enumeration remains usable while the UIA permit is held.
    let (dialog, _) = driver
        .find_window(pid.into(), "CUA synchronous modal")
        .expect("owned modal window");
    let focus = driver.call("bring_to_front", json!({"pid": pid, "window_id": dialog}));
    assert!(!focus.is_error(), "{}", focus.text());
    assert_eq!(focus.structured()["landed_on_target"], true);
    let dismiss = driver.call(
        "press_key",
        json!({"pid": pid, "window_id": dialog, "key": "Escape", "delivery_mode": "foreground"}),
    );
    assert!(!dismiss.is_error(), "{}", dismiss.text());
    wait_count(&journal, "closed", 1);
    std::thread::sleep(Duration::from_millis(250));

    let snap = snapshot(&mut driver, pid, wid);
    let normal = driver.call(
        "click",
        click_args(&snap, "Normal semantic action", pid, wid),
    );
    assert!(
        !normal.is_error(),
        "gate did not recover: {}",
        normal.text()
    );
    wait_count(&journal, "normal", 2);
    let snap = snapshot(&mut driver, pid, wid);
    let toggle = driver.call(
        "click",
        click_args(&snap, "Toggle semantic action", pid, wid),
    );
    assert!(!toggle.is_error(), "Toggle regression: {}", toggle.text());
    assert_eq!(toggle.structured()["route"], "accessibility");
    println!("checkbox_summary={}", toggle.text());
    wait_count(&journal, "toggled", 1);
    let snap = snapshot(&mut driver, pid, wid);
    let select = driver.call(
        "click",
        click_args(&snap, "Select semantic action", pid, wid),
    );
    assert!(
        !select.is_error(),
        "Selection regression: {}",
        select.text()
    );
    assert_eq!(select.structured()["route"], "accessibility");
    println!("radio_summary={}", select.text());
    wait_count(&journal, "selected", 1);
    let snap = snapshot(&mut driver, pid, wid);
    let fallback = driver.call("click", click_args(&snap, "Coordinate fallback", pid, wid));
    assert!(
        !fallback.is_error(),
        "completed miss must permit coordinate fallback: {}",
        fallback.text()
    );
    wait_count(&journal, "fallback", 1);
    assert_eq!(fallback.structured()["route"], "synthetic_events");
    let snap = snapshot(&mut driver, pid, wid);
    let expand = driver.call(
        "click",
        click_args(&snap, "Expand semantic action", pid, wid),
    );
    assert!(!expand.is_error(), "Expand regression: {}", expand.text());
    // The legacy WinForms combo has no usable click pattern; a completed
    // semantic miss must retain its existing coordinate fallback.
    assert_eq!(expand.structured()["route"], "synthetic_events");
    println!("combo_summary={}", expand.text());
    wait_count(&journal, "expanded", 1);
    assert_eq!(
        counts(&journal).get("opened"),
        Some(&1),
        "late completion must not replay activation"
    );
    assert_eq!(counts(&journal).get("mouse_down"), Some(&0));
    println!("final_fixture_counts={:?}", counts(&journal));
}
