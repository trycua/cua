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
    window.add(panel)
    window.show_all()
    window.fullscreen()
    windows.append(window)
def command(source, condition):
    line = sys.stdin.readline().strip()
    if line == 'close-first': windows[0].destroy()
    elif line == 'raise-first': windows[0].present()
    elif line == 'minimize-second': windows[1].iconify()
    elif line == 'resize-second': windows[1].unfullscreen(); windows[1].resize(400, 240)
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

#[test]
#[ignore = "requires native KWin Wayland and Python with GTK3; temporarily opens two fullscreen fixtures"]
fn same_pid_same_title_windows_capture_exact_surfaces_without_a_helper() {
    assert_eq!(std::env::var("XDG_SESSION_TYPE").as_deref(), Ok("wayland"));
    let mut driver = McpDriver::spawn_named_with_env(
        "kwin-native-capture",
        &[("CUA_DRIVER_RS_ENABLE_WAYLAND", "1")],
    )
    .expect("exact-candidate Driver");
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
        if windows.len() == 2
            && windows
                .iter()
                .all(|w| w["width"].as_u64().unwrap_or(0) > 320)
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
    for (window, color) in windows.iter().zip(&colors) {
        assert_eq!(capture(&mut driver, pid, window), *color);
    }
    // Bringing the previously covered red sibling forward must update the
    // compositor rank without changing either driver window ID.
    let red = colors
        .iter()
        .position(|color| color == &[255, 0, 0, 255])
        .unwrap();
    let blue = 1 - red;
    writeln!(input, "raise-first").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let response = driver.call("list_windows", json!({"pid": pid}));
        let refreshed = response.structured()["windows"].as_array().unwrap();
        let ranks: Vec<_> = windows
            .iter()
            .map(|window| {
                refreshed
                    .iter()
                    .find(|fresh| fresh["window_id"] == window["window_id"])
                    .and_then(|fresh| fresh["z_index"].as_u64())
            })
            .collect();
        if matches!((ranks[red], ranks[blue]), (Some(red), Some(blue)) if red > blue) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "fixture stacking: {}",
            response.text()
        );
        std::thread::sleep(Duration::from_millis(100));
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
    let first = colors
        .iter()
        .position(|color| color == &[255, 0, 0, 255])
        .unwrap();
    writeln!(input, "close-first").unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let closed = driver.call("get_window_state", json!({
        "pid": pid, "window_id": windows[first]["window_id"], "include_accessibility_tree": false,
    }));
    assert!(
        closed.is_error() && closed.text().ends_with("KWin window no longer exists"),
        "{}",
        closed.text()
    );
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
