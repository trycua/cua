// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Window policy: only the notch panel is a borderless, transparent overlay.
//! Every other surface (the main window with the Spaces list, New Space,
//! Settings and onboarding; the teleport picker and its consent screens; the
//! Space viewers) is a standard, opaque, decorated window.

use std::path::Path;

fn manifest_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn config() -> serde_json::Value {
    let text = std::fs::read_to_string(manifest_dir().join("tauri.conf.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

#[test]
fn only_the_notch_portal_is_a_transparent_overlay() {
    let config = config();
    let windows = config["app"]["windows"].as_array().expect("windows");
    let labels: Vec<&str> = windows
        .iter()
        .map(|w| w["label"].as_str().unwrap())
        .collect();
    assert_eq!(labels, ["portal", "main"]);
    for window in windows {
        let label = window["label"].as_str().unwrap();
        let transparent = window["transparent"].as_bool().unwrap_or(false);
        let decorated = window["decorations"].as_bool().unwrap_or(true);
        if label == "portal" {
            assert!(
                transparent && !decorated,
                "the notch panel draws its own shape"
            );
        } else {
            assert!(!transparent, "{label} must be opaque");
            assert!(decorated, "{label} must have the system title bar");
        }
    }
}

#[test]
fn the_main_window_is_an_ordinary_app_window() {
    let config = config();
    let main = config["app"]["windows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["label"] == "main")
        .expect("a main window");
    assert_eq!(main["visible"], true, "the app opens on its main window");
    assert_eq!(main["resizable"], true);
    assert!(main["minWidth"].as_f64().unwrap() >= 700.0);
    assert!(main["width"].as_f64().unwrap() >= 1000.0);
    assert_eq!(main["titleBarStyle"], "Overlay");
    assert_eq!(main["hiddenTitle"], true);
}

#[test]
fn runtime_windows_are_opaque_and_never_force_fullscreen() {
    let source = std::fs::read_to_string(manifest_dir().join("src/viewer_windows.rs")).unwrap();
    assert!(
        !source.contains(".transparent(true)"),
        "no transparent runtime windows"
    );
    assert!(
        !source.contains(".fullscreen(true)"),
        "viewers open windowed"
    );
    assert!(
        !source.contains("set_fullscreen(true)"),
        "viewers never force fullscreen"
    );
    assert!(
        !source.contains(".decorations(false)"),
        "runtime windows keep the title bar"
    );
    let commands = std::fs::read_to_string(manifest_dir().join("src/commands.rs")).unwrap();
    assert!(!commands.contains("set_fullscreen(true)"));
}

#[test]
fn the_main_window_capability_allows_dragging_and_the_open_panel() {
    let text = std::fs::read_to_string(manifest_dir().join("capabilities/main.json")).unwrap();
    let cap: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(cap["windows"], serde_json::json!(["main"]));
    let perms: Vec<&str> = cap["permissions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap())
        .collect();
    assert!(perms.contains(&"core:window:allow-start-dragging"));
    assert!(perms.contains(&"dialog:allow-open"));
}
