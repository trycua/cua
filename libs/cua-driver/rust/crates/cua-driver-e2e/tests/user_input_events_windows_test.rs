// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Windows `get_user_input_events`: the driver's low-level hooks see system
//! input and tell the driver's own input from input another process injects.
//!
//! Needs a real desktop session. Run explicitly:
//!   cargo test -p cua-driver-e2e --test user_input_events_windows_test -- --ignored --nocapture --test-threads=1

#![cfg(target_os = "windows")]

use std::time::{Duration, Instant};

use cua_driver_testkit::{Driver, McpDriver};
use serde_json::{json, Value};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_MOVE, MOUSEINPUT,
};

/// Read events after `cursor` until `done` holds for them or a few seconds
/// pass. Returns the events and the cursor the last read gave.
fn read_until(
    driver: &mut McpDriver,
    cursor: &str,
    done: impl Fn(&[Value]) -> bool,
) -> (Vec<Value>, String) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let response = driver.call("get_user_input_events", json!({ "since": cursor }));
        assert!(
            !response.is_error(),
            "get_user_input_events failed: {}",
            response.text()
        );
        let structured = response.structured();
        assert_eq!(structured["history_lost"], false);
        let events = structured["events"].as_array().cloned().unwrap_or_default();
        let next = structured["cursor"].as_str().expect("cursor").to_owned();
        if done(&events) || Instant::now() > deadline {
            return (events, next);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn has(events: &[Value], kind: &str, source: &str) -> bool {
    events
        .iter()
        .any(|event| event["kind"] == kind && event["source"] == source)
}

/// Move the real cursor by a few pixels with `SendInput` from this process,
/// which is not the driver.
fn nudge_cursor_from_another_process() {
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 3,
                dy: 3,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_MOVE,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let sent = unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
    assert_eq!(sent, 1, "SendInput from the test process was refused");
}

#[test]
#[ignore]
fn the_drivers_input_and_another_processes_input_are_told_apart() {
    let mut driver = McpDriver::spawn_named("windows-user-input-events")
        .expect("required source-built driver did not start");

    let start = driver.call("get_user_input_events", json!({}));
    assert!(!start.is_error(), "start failed: {}", start.text());
    assert_eq!(start.structured()["events"], json!([]));
    let cursor = start.structured()["cursor"]
        .as_str()
        .expect("cursor")
        .to_owned();

    // SendInput: every event the driver sends carries its tag.
    let scrolled = driver.call(
        "scroll",
        json!({"scope": "desktop", "x": 10, "y": 10, "direction": "down", "amount": 1}),
    );
    assert!(!scrolled.is_error(), "scroll failed: {}", scrolled.text());
    let (events, cursor) = read_until(&mut driver, &cursor, |events| {
        has(events, "wheel", "driver")
    });
    assert!(
        has(&events, "wheel", "driver"),
        "no driver wheel: {events:?}"
    );
    assert!(
        !has(&events, "wheel", "injected"),
        "the driver's wheel was reported as injected: {events:?}"
    );

    // SetCursorPos carries no tag; a move it makes is still the driver's.
    let moved = driver.call("move_cursor", json!({"scope": "desktop", "x": 40, "y": 40}));
    assert!(!moved.is_error(), "move_cursor failed: {}", moved.text());
    let (events, cursor) = read_until(&mut driver, &cursor, |_| false);
    assert!(
        !has(&events, "mouse_move", "injected"),
        "the driver's cursor placement was reported as injected: {events:?}"
    );

    nudge_cursor_from_another_process();
    let (events, _) = read_until(&mut driver, &cursor, |events| {
        has(events, "mouse_move", "injected")
    });
    assert!(
        has(&events, "mouse_move", "injected"),
        "this process's SendInput move was not reported as injected: {events:?}"
    );
    assert!(
        !has(&events, "mouse_move", "driver"),
        "another process's move was reported as the driver's: {events:?}"
    );
}
