// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Live Hyprland regression for the native cursor overlay's focus contract:
//! one `cua-agent-cursor` layer-shell surface per output, no focusable client,
//! and no workspace or active-window change during cursor activity. Run only
//! in a disposable native Hyprland desktop.
#![cfg(target_os = "linux")]

use std::io::{BufRead, BufReader};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::Command;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use cua_driver_testkit::{Driver, McpDriver};
use serde_json::{json, Value};

const SESSION: &str = "hyprland-overlay-focus";

/// Hyprland event-socket events that mean focus, workspace or a client moved.
const FOCUS_EVENTS: &[&str] = &[
    "activewindow",
    "activewindowv2",
    "workspace",
    "workspacev2",
    "focusedmon",
    "focusedmonv2",
    "openwindow",
];

fn hypr_json(query: &str) -> Value {
    let output = Command::new("hyprctl")
        .args(["-j", query])
        .output()
        .unwrap_or_else(|error| panic!("run hyprctl -j {query}: {error}"));
    assert!(output.status.success(), "hyprctl -j {query} failed");
    serde_json::from_slice(&output.stdout).expect("parse hyprctl JSON")
}

/// Records every focus event Hyprland broadcasts until `finish`, so a change
/// that reverts before the end still fails the test.
struct FocusEvents {
    stream: UnixStream,
    reader: JoinHandle<Vec<String>>,
}

impl FocusEvents {
    fn start() -> Self {
        let socket = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR"))
            .join("hypr")
            .join(std::env::var("HYPRLAND_INSTANCE_SIGNATURE").unwrap())
            .join(".socket2.sock");
        let stream = UnixStream::connect(&socket).expect("connect to Hyprland event socket");
        let reader = BufReader::new(stream.try_clone().expect("clone event socket"));
        let reader = thread::spawn(move || {
            reader
                .lines()
                .map_while(Result::ok)
                .filter(|line| {
                    line.split_once(">>")
                        .is_some_and(|(event, _)| FOCUS_EVENTS.contains(&event))
                })
                .collect::<Vec<_>>()
        });
        Self { stream, reader }
    }

    fn finish(self) -> Vec<String> {
        let _ = self.stream.shutdown(Shutdown::Both);
        self.reader.join().expect("event reader")
    }
}

fn call_ok(driver: &mut McpDriver, tool: &str, args: Value) {
    let response = driver.call(tool, args);
    assert!(!response.is_error(), "{tool} failed: {}", response.text());
}

#[test]
#[ignore = "requires disposable native Hyprland and exact-candidate Driver"]
fn native_overlay_never_maps_an_active_hyprland_client() {
    assert!(std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some());
    assert_eq!(
        std::env::var("CUA_DRIVER_RS_ENABLE_WAYLAND").as_deref(),
        Ok("1")
    );
    let events = FocusEvents::start();
    let mut driver = McpDriver::spawn_named_with_overlay("hyprland-overlay-focus")
        .expect("exact-candidate Driver is required");
    let pid = u64::from(driver.daemon_pid().expect("test-owned daemon"));

    call_ok(&mut driver, "start_session", json!({"session": SESSION}));
    call_ok(
        &mut driver,
        "set_agent_cursor_motion",
        json!({"session": SESSION, "glide_duration_ms": 250, "idle_hide_ms": 0}),
    );
    call_ok(
        &mut driver,
        "set_agent_cursor_enabled",
        json!({"session": SESSION, "enabled": true}),
    );
    call_ok(
        &mut driver,
        "move_cursor",
        json!({"session": SESSION, "x": 500.0, "y": 500.0}),
    );
    std::thread::sleep(Duration::from_secs(1));

    let monitor_count = hypr_json("monitors")
        .as_array()
        .expect("Hyprland monitors array")
        .len();
    let daemon_clients = hypr_json("clients")
        .as_array()
        .expect("Hyprland clients array")
        .iter()
        .filter(|client| client["pid"].as_u64() == Some(pid))
        .cloned()
        .collect::<Vec<_>>();
    let overlay_layers = hypr_json("layers")
        .as_object()
        .expect("Hyprland layer map")
        .values()
        .filter_map(|output| output["levels"].as_object())
        .flat_map(|levels| levels.values())
        .flat_map(|level| level.as_array().into_iter().flatten())
        .filter(|layer| layer["pid"].as_u64() == Some(pid))
        .filter(|layer| layer["namespace"] == "cua-agent-cursor")
        .count();
    drop(driver);
    let focus_events = events.finish();

    assert!(
        daemon_clients.is_empty(),
        "overlay daemon mapped a focusable Hyprland client: {daemon_clients:#?}"
    );
    assert!(
        focus_events.is_empty(),
        "overlay startup or cursor activity moved Hyprland focus: {focus_events:#?}"
    );
    assert_eq!(
        overlay_layers, monitor_count,
        "native layer-shell overlay did not map exactly once per enabled output"
    );
}
