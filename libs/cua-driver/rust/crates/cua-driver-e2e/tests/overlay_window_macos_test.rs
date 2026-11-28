// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The macOS cursor overlay is one display-sized window per display. While it
//! stayed on screen with no cursor in it, Mission Control's Desktop thumbnails
//! showed only wallpaper until the daemon stopped (#7212). The overlay must be
//! on screen only while a cursor paints in it. The oracle is CoreGraphics'
//! on-screen window list for the daemon's process tree.
//!
//! Run with:
//! `cargo test -p cua-driver-e2e --test overlay_window_macos_test -- --ignored --nocapture --test-threads=1`

#![cfg(target_os = "macos")]

use std::ffi::c_void;
use std::process::Command;
use std::time::{Duration, Instant};

use core_foundation::{
    array::{CFArray, CFArrayRef},
    base::{CFGetTypeID, CFTypeRef, TCFType},
    dictionary::CFDictionary,
    number::CFNumber,
    string::CFString,
};
use cua_driver_testkit::{Driver, McpDriver};

const SESSION: &str = "overlay-window";

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGWindowListCopyWindowInfo(option: u32, relative_to_window: u32) -> CFArrayRef;
}

/// `pid` and its descendants. Outside embedded mode `serve` relaunches
/// itself, so the overlay may belong to a child process.
fn process_tree(pid: u32) -> Vec<u32> {
    let mut tree = vec![pid];
    let mut index = 0;
    while index < tree.len() {
        let output = Command::new("pgrep")
            .args(["-P", &tree[index].to_string()])
            .output()
            .expect("run pgrep");
        tree.extend(
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter_map(|line| line.trim().parse::<u32>().ok()),
        );
        index += 1;
    }
    tree
}

/// Ids of the on-screen windows owned by any of `pids`.
fn on_screen_windows(pids: &[u32]) -> Vec<i64> {
    // kCGWindowListOptionOnScreenOnly
    let raw = unsafe { CGWindowListCopyWindowInfo(1, 0) };
    assert!(!raw.is_null(), "CGWindowListCopyWindowInfo returned null");
    let array: CFArray<CFTypeRef> = unsafe { CFArray::wrap_under_create_rule(raw) };
    let mut windows = Vec::new();
    for item in array.iter() {
        let item = *item;
        if unsafe { CFGetTypeID(item) } != CFDictionary::<*const c_void, *const c_void>::type_id() {
            continue;
        }
        let dictionary: CFDictionary<*const c_void, *const c_void> =
            unsafe { CFDictionary::wrap_under_get_rule(item as _) };
        let number = |key: &str| -> Option<i64> {
            let key = CFString::new(key);
            dictionary
                .find(key.as_concrete_TypeRef() as *const c_void)
                .and_then(|value| unsafe {
                    let value = *value;
                    (CFGetTypeID(value) == CFNumber::type_id())
                        .then(|| CFNumber::wrap_under_get_rule(value as _).to_i64())
                        .flatten()
                })
        };
        let owner = number("kCGWindowOwnerPID").and_then(|pid| u32::try_from(pid).ok());
        if owner.is_some_and(|owner| pids.contains(&owner)) {
            windows.extend(number("kCGWindowNumber"));
        }
    }
    windows
}

fn wait_until(what: &str, daemon: u32, mut done: impl FnMut(&[i64]) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let windows = on_screen_windows(&process_tree(daemon));
        if done(&windows) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{what}; on-screen daemon windows: {windows:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn call_ok(driver: &mut McpDriver, tool: &str, args: serde_json::Value) {
    let response = driver.call(tool, args);
    assert!(!response.is_error(), "{tool} failed: {}", response.text());
}

#[test]
#[ignore = "needs a logged-in macOS desktop; shows the agent cursor"]
fn overlay_window_is_on_screen_only_while_a_cursor_paints() {
    let mut driver = McpDriver::spawn_named_with_overlay(SESSION)
        .expect("start a cua-driver daemon with the cursor overlay");
    let daemon = driver.daemon_pid().expect("daemon pid");

    // The overlay creates its surfaces as AppKit starts. None may reach the
    // screen before a cursor is shown.
    let until = Instant::now() + Duration::from_secs(3);
    while Instant::now() < until {
        let windows = on_screen_windows(&process_tree(daemon));
        assert!(
            windows.is_empty(),
            "the overlay is on screen with no cursor: {windows:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    call_ok(
        &mut driver,
        "move_cursor",
        serde_json::json!({"session": SESSION, "x": 240, "y": 180}),
    );
    wait_until(
        "the shown cursor's overlay never reached the screen",
        daemon,
        |windows| !windows.is_empty(),
    );

    call_ok(
        &mut driver,
        "end_session",
        serde_json::json!({"session": SESSION}),
    );
    wait_until(
        "the overlay stayed on screen after the session ended",
        daemon,
        |windows| windows.is_empty(),
    );
}
