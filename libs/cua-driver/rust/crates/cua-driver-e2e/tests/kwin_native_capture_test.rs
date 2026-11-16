// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

#![cfg(target_os = "linux")]

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use base64::Engine;
use cua_driver_testkit::{spawn_in_job, Driver, McpDriver};
use serde_json::{json, Value};

const FIXTURE: &str = r#"
import gi, sys
gi.require_version('Gtk', '3.0')
from gi.repository import Gtk, Gdk, GLib
windows = []
for color in [(1, 0, 0), (0, 0, 1)]:
    window = Gtk.Window(title='Cua KWin capture fixture')
    window.set_default_size(320, 200)
    panel = Gtk.EventBox()
    panel.override_background_color(Gtk.StateFlags.NORMAL, Gdk.RGBA(*color, 1))
    text = Gtk.TextView()
    text.get_buffer().set_text('no semantic action')
    text.set_halign(Gtk.Align.START)
    text.set_valign(Gtk.Align.START)
    panel.add(text)
    window.add(panel)
    window.show_all()
    window.fullscreen()
    windows.append(window)
def command(source, condition):
    line = sys.stdin.readline().strip()
    if line == 'close-first': windows[0].destroy()
    elif line == 'minimize-second': windows[1].iconify()
    elif line == 'resize-second': windows[1].unfullscreen(); windows[1].resize(400, 240)
    elif line == 'raise-first': windows[0].present()
    elif line == 'quit': Gtk.main_quit(); return False
    return True
GLib.io_add_watch(sys.stdin, GLib.IO_IN, command)
Gtk.main()
"#;

fn capture(driver: &mut McpDriver, pid: u32, window: &Value) -> [u8; 4] {
    let response = driver.call(
        "get_window_state",
        json!({
            "pid": pid, "window_id": window["window_id"],
            "include_accessibility_tree": false, "max_image_dimension": 0,
        }),
    );
    assert!(!response.is_error(), "{}", response.text());
    assert_eq!(response.structured()["screenshot_frame_valid"], true);
    let png = response.raw["result"]["content"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["type"] == "image")
        .expect("window PNG")["data"]
        .as_str()
        .unwrap();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(png)
        .unwrap();
    let image = image::load_from_memory(&bytes).unwrap().to_rgba8();
    assert_eq!(u64::from(image.width()), window["width"].as_u64().unwrap());
    assert_eq!(
        u64::from(image.height()),
        window["height"].as_u64().unwrap()
    );
    image.get_pixel(image.width() / 2, image.height() / 2).0
}

/// Save the operator's KWin switch in the test driver's own home. The driver
/// reads it on every call, as `cua-driver kwin-capture on|off` leaves it.
fn set_kwin_capture(driver: &McpDriver, value: &str) {
    let dir = driver
        .state_root()
        .expect("isolated driver home")
        .join(".cua-driver");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("kwin-window-capture"), value).unwrap();
}

/// A capture that returned the tree with the typed soft refusal, no pixels.
fn capture_unproven(response: &cua_driver_testkit::ToolResponse) -> bool {
    !response.is_error()
        && response.structured()["screenshot_error"]["code"] == "surface_identity_unproven"
}

/// A refusal with exactly this code: some routes report it as a structured
/// field, others as the prefix of a generic tool failure.
fn refused_with(response: &cua_driver_testkit::ToolResponse, code: &str) -> bool {
    response.is_error()
        && (response.structured()["code"] == code
            || response.text().starts_with(&format!("{code}:")))
}

#[test]
#[ignore = "requires native KWin Wayland and Python with GTK3; temporarily opens two fullscreen fixtures"]
fn same_pid_same_title_windows_capture_exact_surfaces_without_a_helper() {
    assert_eq!(std::env::var("XDG_SESSION_TYPE").as_deref(), Ok("wayland"));
    let mut driver = McpDriver::spawn_named_with_env(
        "kwin-native-capture",
        &[("CUA_DRIVER_RS_ENABLE_WAYLAND", "1")],
    )
    .expect("exact-candidate Driver");
    set_kwin_capture(&driver, "on");
    let python = std::env::var("CUA_KWIN_FIXTURE_PYTHON").unwrap_or_else(|_| "python3".into());
    let mut child = spawn_in_job(
        Command::new(python)
            .args(["-c", FIXTURE])
            .env("GDK_BACKEND", "wayland")
            .stdin(Stdio::piped()),
    )
    .expect("GTK3 fixtures");
    let pid = child.id();
    let mut input = child.stdin.take().unwrap();
    driver.reaper().push(child);
    let deadline = Instant::now() + Duration::from_secs(15);
    let windows = loop {
        let response = driver.call("list_windows", json!({"pid": pid}));
        let windows = response.structured()["windows"]
            .as_array()
            .expect("window list");
        // KWin announces a new window before it enters the stacking order.
        if windows.len() == 2
            && windows
                .iter()
                .all(|w| w["width"].as_u64().unwrap_or(0) > 320 && w["z_index"].is_u64())
        {
            break windows.clone();
        }
        assert!(
            Instant::now() < deadline,
            "fixture enumeration: {}",
            response.text()
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(windows[0]["title"], windows[1]["title"]);
    assert_ne!(windows[0]["window_id"], windows[1]["window_id"]);
    assert!(windows
        .iter()
        .all(|w| (0xE000_0000..0xF000_0000).contains(&w["window_id"].as_u64().unwrap())));
    let colors: Vec<_> = windows
        .iter()
        .map(|window| capture(&mut driver, pid, window))
        .collect();
    assert!(colors.contains(&[255, 0, 0, 255]), "{colors:?}");
    assert!(colors.contains(&[0, 0, 255, 255]), "{colors:?}");
    let first = colors
        .iter()
        .position(|color| color == &[255, 0, 0, 255])
        .unwrap();
    assert!(
        windows[first]["z_index"].as_u64().unwrap()
            < windows[1 - first]["z_index"].as_u64().unwrap()
    );
    let topmost = driver.call(
        "get_window_state",
        json!({
            "pid": pid, "include_accessibility_tree": false,
        }),
    );
    assert!(!topmost.is_error(), "{}", topmost.text());
    assert_eq!(
        topmost.structured()["window_id"],
        windows[1 - first]["window_id"]
    );
    writeln!(input, "raise-first").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let response = driver.call("list_windows", json!({"pid": pid}));
        let current = response.structured()["windows"].as_array().unwrap();
        let red = current
            .iter()
            .find(|window| window["window_id"] == windows[first]["window_id"])
            .unwrap();
        let blue = current
            .iter()
            .find(|window| window["window_id"] == windows[1 - first]["window_id"])
            .unwrap();
        if red["z_index"].as_u64().unwrap() > blue["z_index"].as_u64().unwrap() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "fixture stacking: {}",
            response.text()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    let topmost = driver.call(
        "get_window_state",
        json!({
            "pid": pid, "include_accessibility_tree": false,
        }),
    );
    assert!(!topmost.is_error(), "{}", topmost.text());
    assert_eq!(
        topmost.structured()["window_id"],
        windows[first]["window_id"]
    );
    for (window, color) in windows.iter().zip(&colors) {
        assert_eq!(capture(&mut driver, pid, window), *color);
    }
    let wrong_owner = driver.call("get_window_state", json!({
        "pid": std::process::id(), "window_id": windows[0]["window_id"], "include_accessibility_tree": false,
    }));
    assert!(
        wrong_owner.is_error()
            && wrong_owner
                .text()
                .contains("is stale or no longer running; refresh list_windows"),
        "{}",
        wrong_owner.text()
    );
    let covered = &windows[1 - first];
    let z_index = |driver: &mut McpDriver| {
        let response = driver.call("list_windows", json!({"pid": pid}));
        response.structured()["windows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|window| window["window_id"] == covered["window_id"])
            .unwrap()["z_index"]
            .clone()
    };
    let covered_z = z_index(&mut driver);
    // KWin input is focus-bound. Every raw route refuses, and none raises the window.
    let point = json!({"x": 100, "y": 100});
    for (tool, mut request) in [
        ("press_key", json!({"key": "a"})),
        ("type_text", json!({"text": "a"})),
        ("type_text_chars", json!({"text": "a"})),
        ("hotkey", json!({"keys": ["ctrl", "a"]})),
        ("click", point.clone()),
        ("right_click", point.clone()),
        ("double_click", point.clone()),
        (
            "scroll",
            json!({"x": 100, "y": 100, "direction": "down", "amount": 1}),
        ),
        (
            "drag",
            json!({"from_x": 100, "from_y": 100, "to_x": 120, "to_y": 120}),
        ),
    ] {
        request["pid"] = json!(pid);
        request["window_id"] = covered["window_id"].clone();
        request["delivery_mode"] = json!("foreground");
        let response = driver.call(tool, request);
        assert!(
            refused_with(&response, "foreground_unavailable"),
            "{tool}: {}",
            response.text()
        );
        assert_eq!(z_index(&mut driver), covered_z, "{tool} raised the window");
    }
    // An element with no semantic action has no pointer fallback on this handle.
    let state = driver.call(
        "get_window_state",
        json!({"pid": pid, "window_id": covered["window_id"], "include_screenshot": false}),
    );
    let snapshot = state.structured()["snapshot_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let tree = state.structured()["tree_markdown"].as_str().unwrap();
    let row = tree
        .lines()
        .find(|line| line.contains("no semantic action"))
        .and_then(|line| line.split_once('[')?.1.split_once(']'))
        .and_then(|(index, _)| index.parse::<u32>().ok())
        .unwrap_or_else(|| panic!("no text row in {tree}"));
    for (delivery, code) in [
        ("background", "background_unavailable"),
        ("foreground", "foreground_unavailable"),
    ] {
        let response = driver.call(
            "click",
            json!({
                "pid": pid, "window_id": covered["window_id"], "delivery_mode": delivery,
                "element_token": format!("{snapshot}:{row}"),
            }),
        );
        assert!(
            refused_with(&response, code),
            "{delivery}: {}",
            response.text()
        );
        assert_eq!(
            z_index(&mut driver),
            covered_z,
            "{delivery} semantic fallback raised the window"
        );
    }
    // bring_to_front is the one call that changes focus, and only for the real owner.
    let response = driver.call(
        "bring_to_front",
        json!({"pid": std::process::id(), "window_id": covered["window_id"]}),
    );
    assert!(response.is_error(), "{}", response.text());
    assert_eq!(
        response.text(),
        format!(
            "foreground_unavailable: window {} no longer exists or belongs to another process",
            covered["window_id"]
        )
    );
    assert_eq!(
        z_index(&mut driver),
        covered_z,
        "wrong owner raised the window"
    );
    let response = driver.call(
        "bring_to_front",
        json!({"pid": pid, "window_id": covered["window_id"]}),
    );
    assert!(!response.is_error(), "{}", response.text());
    assert!(
        z_index(&mut driver).as_u64().unwrap() > covered_z.as_u64().unwrap(),
        "bring_to_front did not raise the window"
    );
    writeln!(input, "raise-first").unwrap();
    std::thread::sleep(Duration::from_millis(300));
    writeln!(input, "close-first").unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let closed = driver.call("get_window_state", json!({
        "pid": pid, "window_id": windows[first]["window_id"], "include_accessibility_tree": false,
    }));
    assert!(capture_unproven(&closed), "{}", closed.text());
    let remaining = driver.call("list_windows", json!({"pid": pid}));
    assert_eq!(
        remaining.structured()["windows"].as_array().unwrap().len(),
        1
    );
    assert_eq!(
        capture(&mut driver, pid, &windows[1 - first]),
        [0, 0, 255, 255]
    );
    writeln!(input, "resize-second").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let resized = loop {
        let response = driver.call("list_windows", json!({"pid": pid}));
        let window = &response.structured()["windows"][0];
        if window["width"].as_u64().unwrap_or(0) < windows[1 - first]["width"].as_u64().unwrap() {
            break window.clone();
        }
        assert!(
            Instant::now() < deadline,
            "fixture resize: {}",
            response.text()
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(resized["window_id"], windows[1 - first]["window_id"]);
    assert_eq!(capture(&mut driver, pid, &resized), [0, 0, 255, 255]);
    // Off from the very next call, and back on, with no restart: off is the
    // driver without this adapter, so the listed window can't be captured.
    set_kwin_capture(&driver, "off");
    let off = driver.call(
        "get_window_state",
        json!({
            "pid": pid, "window_id": resized["window_id"], "include_accessibility_tree": false,
        }),
    );
    assert!(capture_unproven(&off), "{}", off.text());
    set_kwin_capture(&driver, "on");
    assert_eq!(capture(&mut driver, pid, &resized), [0, 0, 255, 255]);
    writeln!(input, "minimize-second").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let response = driver.call("list_windows", json!({"pid": pid}));
        if response.structured()["windows"][0]["is_on_screen"] == false {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "fixture minimize: {}",
            response.text()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    writeln!(input, "quit").unwrap();
}
