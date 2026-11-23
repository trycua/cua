// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! An Electron app started without `--force-renderer-accessibility` exposes
//! its page on the driver's first snapshot (#4768).
//!
//! Chromium keeps its AT-SPI tree off until a client reads an object's
//! attributes, then builds it asynchronously. The fixture is launched with no
//! accessibility flag, and its own journal proves the page has loaded before
//! the driver looks. The oracle is that the first `get_window_state` of a
//! fresh driver already contains the page marker. Twenty fresh launches run,
//! because a walk racing the tree build failed about one launch in ten.
//!
//! #[ignore] (needs a desktop session with AT-SPI and the staged Electron
//! fixture). Run:
//!   cargo test -p cua-driver-e2e --test chromium_tree_wake_linux_test -- --ignored --nocapture --test-threads=1

#![cfg(target_os = "linux")]

use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use cua_driver_testkit::{harness_app, spawn_in_job, Driver, FixtureJournal, McpDriver};

const MARKER: &str = "WEB_HARNESS_MARKER_v1";
const LAUNCHES: usize = 20;

fn allocate_loopback_port() -> u16 {
    let listener =
        std::net::TcpListener::bind(("127.0.0.1", 0)).expect("allocate an ephemeral CDP port");
    listener.local_addr().expect("read CDP port").port()
}

fn fixture_window(driver: &mut McpDriver, pid: u32) -> Option<u64> {
    driver
        .call("list_windows", serde_json::json!({ "pid": pid }))
        .structured()["windows"]
        .as_array()?
        .iter()
        .find(|window| {
            window["title"]
                .as_str()
                .is_some_and(|title| title.contains("CuaTestHarness Electron"))
        })
        .and_then(|window| window["window_id"].as_u64())
}

fn first_snapshot_after_launch(launch: usize) {
    let electron = harness_app("harness-electron", "CuaTestHarness.Electron");
    assert!(
        electron.exists(),
        "the Electron fixture is not staged at {electron:?}"
    );
    let mut driver = McpDriver::spawn_unrecorded().expect("start source-built Linux driver");
    let journal = FixtureJournal::start();
    let mut command = Command::new(&electron);
    command
        .args(["--no-sandbox", "--disable-gpu"])
        .env(
            "CUA_ELECTRON_CDP_PORT",
            allocate_loopback_port().to_string(),
        )
        .env("CUA_E2E_FIXTURE_JOURNAL_URL", journal.url())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    let child = spawn_in_job(&mut command).expect("launch the Electron fixture");
    let pid = child.id();
    driver.reaper().push(child);

    let deadline = Instant::now() + Duration::from_secs(30);
    let window_id = loop {
        if journal.contains(MARKER) {
            if let Some(window_id) = fixture_window(&mut driver, pid) {
                break window_id;
            }
        }
        assert!(
            Instant::now() < deadline,
            "launch {launch}: the Electron fixture never loaded its page and showed its window"
        );
        thread::sleep(Duration::from_millis(250));
    };

    let state = driver.call(
        "get_window_state",
        serde_json::json!({
            "pid": pid,
            "window_id": window_id,
            "capture_mode": "ax",
            "timeout_ms": 10_000
        }),
    );
    assert!(
        !state.is_error(),
        "launch {launch}: get_window_state failed: {}",
        state.text()
    );
    assert!(
        state.tree_text().contains(MARKER),
        "launch {launch}: the first snapshot of an Electron app started without \
         --force-renderer-accessibility lacks its page content:\n{}",
        state.tree_text()
    );
}

#[test]
#[ignore]
fn electron_page_is_in_the_first_snapshot_without_renderer_accessibility_flag() {
    for launch in 1..=LAUNCHES {
        first_snapshot_after_launch(launch);
    }
}
