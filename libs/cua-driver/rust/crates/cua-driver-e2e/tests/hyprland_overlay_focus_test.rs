// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Live Hyprland regression for the native cursor overlay's focus contract:
//! one `cua-agent-cursor` layer-shell surface per output, no focusable client,
//! and no workspace or active-window change during cursor activity. Run only
//! in a disposable native Hyprland desktop.
#![cfg(target_os = "linux")]

use std::process::Command;
use std::time::Duration;

use cua_driver_testkit::{Driver, McpDriver};
use serde_json::{json, Value};

const SESSION: &str = "hyprland-overlay-focus";

#[derive(Debug, PartialEq)]
struct HyprState {
    workspace: String,
    active_window: String,
}

fn hypr_json(query: &str) -> Value {
    let output = Command::new("hyprctl")
        .args(["-j", query])
        .output()
        .unwrap_or_else(|error| panic!("run hyprctl -j {query}: {error}"));
    assert!(output.status.success(), "hyprctl -j {query} failed");
    serde_json::from_slice(&output.stdout).expect("parse hyprctl JSON")
}

fn hypr_state() -> HyprState {
    HyprState {
        workspace: hypr_json("activeworkspace")["name"]
            .as_str()
            .expect("active workspace name")
            .to_owned(),
        active_window: hypr_json("activewindow")["address"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
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
    let before = hypr_state();
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

    let after = hypr_state();
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

    assert!(
        daemon_clients.is_empty(),
        "overlay daemon mapped a focusable Hyprland client: {daemon_clients:#?}"
    );
    assert_eq!(
        after, before,
        "cursor activity changed the Hyprland workspace or active client"
    );
    assert_eq!(
        overlay_layers, monitor_count,
        "native layer-shell overlay did not map exactly once per enabled output"
    );
}
