// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Owner of the KWin desktop frame contract: a pixel read off a desktop
//! screenshot, when clicked, lands on that pixel, on any output layout, or the
//! call refuses. Run it in a disposable KWin session, in each layout worth
//! checking (scaled, several outputs, mixed scales, away from the origin). The
//! portal asks for consent on first use, so someone or something must answer.
#![cfg(target_os = "linux")]

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

use base64::Engine;
use cua_driver_testkit::{spawn_in_job, Driver, McpDriver};
use serde_json::{json, Value};

/// One fullscreen window per output, with a marker in its own color. It prints
/// every output's logical geometry as GDK sees it, then every press it gets.
const FIXTURE: &str = r#"
import gi, json
gi.require_version('Gtk', '3.0'); gi.require_version('Gdk', '3.0')
from gi.repository import Gtk, Gdk
display = Gdk.Display.get_default()
css = Gtk.CssProvider()
rules = 'eventbox{background:#202020}'
colors = ['ff00ff', '00ffff', 'ffff00', '00ff00']
monitors = []
windows = []
def press(index):
    def handler(_, event):
        print(json.dumps({'monitor': index, 'x': event.x, 'y': event.y}), flush=True)
    return handler
for index in range(display.get_n_monitors()):
    geometry = display.get_monitor(index).get_geometry()
    monitors.append({'x': geometry.x, 'y': geometry.y, 'width': geometry.width, 'height': geometry.height})
    rules += '#marker%d{background:#%s}' % (index, colors[index])
    window = Gtk.Window(title='Cua KWin frame fixture %d' % index)
    panel = Gtk.EventBox(); fixed = Gtk.Fixed(); marker = Gtk.Box()
    marker.set_name('marker%d' % index); marker.set_size_request(60, 60)
    fixed.put(marker, 300, 200); panel.add(fixed); window.add(panel)
    panel.connect('button-press-event', press(index))
    window.show_all(); window.fullscreen_on_monitor(window.get_screen(), index)
    windows.append(window)
css.load_from_data(rules.encode())
Gtk.StyleContext.add_provider_for_screen(Gdk.Screen.get_default(), css, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION)
print(json.dumps(monitors), flush=True)
Gtk.main()
"#;

const MARKER_COLORS: [[u8; 3]; 4] = [[255, 0, 255], [0, 255, 255], [255, 255, 0], [0, 255, 0]];
const MARKER: (f64, f64, f64) = (300.0, 200.0, 60.0);

fn next_line(lines: &Receiver<String>) -> Value {
    let line = lines
        .recv_timeout(Duration::from_secs(20))
        .expect("the fixture answers");
    serde_json::from_str(&line).expect("fixture JSON")
}

fn screenshot(driver: &mut McpDriver) -> (Value, image::RgbaImage) {
    let response = driver.call("get_desktop_state", json!({"max_image_dimension": 0}));
    assert!(!response.is_error(), "{}", response.text());
    let png = response.raw["result"]["content"]
        .as_array()
        .expect("content")
        .iter()
        .find(|item| item["type"] == "image")
        .expect("desktop PNG")["data"]
        .as_str()
        .expect("PNG data");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(png)
        .expect("base64");
    let image = image::load_from_memory(&bytes).expect("PNG").to_rgba8();
    (response.structured().clone(), image)
}

/// Center of the pixels of this color. The screenshot is resized on a scaled
/// output, so edges blend and an exact match would shrink the marker.
fn marker_center(image: &image::RgbaImage, color: [u8; 3]) -> (u32, u32) {
    let (mut min, mut max) = ((u32::MAX, u32::MAX), (0, 0));
    for (x, y, pixel) in image.enumerate_pixels() {
        if (0..3).all(|c| pixel[c].abs_diff(color[c]) <= 16) {
            min = (min.0.min(x), min.1.min(y));
            max = (max.0.max(x), max.1.max(y));
        }
    }
    assert!(min.0 != u32::MAX, "no {color:?} marker in the screenshot");
    ((min.0 + max.0) / 2, (min.1 + max.1) / 2)
}

fn click(driver: &mut McpDriver, x: i64, y: i64) -> cua_driver_testkit::ToolResponse {
    driver.call(
        "click",
        json!({"target": {"kind": "desktop", "display_id": "primary"}, "x": x, "y": y}),
    )
}

/// Click a screenshot pixel on an output and return the press it delivered. A
/// layout point left of or above the origin has no libei address, so there the
/// exact refusal is the contract and `None` comes back.
fn click_on_output(
    driver: &mut McpDriver,
    lines: &Receiver<String>,
    frame: (i64, i64),
    (x, y): (i64, i64),
) -> Option<Value> {
    let response = click(driver, x, y);
    let (layout_x, layout_y) = (frame.0 + x, frame.1 + y);
    if layout_x < 0 || layout_y < 0 {
        let expected = format!(
            "libei: ({layout_x}, {layout_y}) is left of or above the layout origin, which libei cannot address"
        );
        assert!(response.text().contains(&expected), "{}", response.text());
        return None;
    }
    assert!(!response.is_error(), "{}", response.text());
    Some(next_line(lines))
}

#[test]
#[ignore = "requires native KWin Wayland, portal consent and Python with GTK3; temporarily opens a fullscreen fixture on every output"]
fn a_screenshot_pixel_lands_on_target_on_every_output() {
    assert_eq!(std::env::var("XDG_SESSION_TYPE").as_deref(), Ok("wayland"));
    let mut driver = McpDriver::spawn_named_with_env(
        "kwin-logical-frame",
        &[("CUA_DRIVER_RS_ENABLE_WAYLAND", "1")],
    )
    .expect("exact-candidate Driver");
    let python = std::env::var("CUA_KWIN_FIXTURE_PYTHON").unwrap_or_else(|_| "python3".into());
    let mut child = spawn_in_job(
        Command::new(python)
            .args(["-c", FIXTURE])
            .env("GDK_BACKEND", "wayland")
            .stdout(Stdio::piped()),
    )
    .expect("GTK3 fixture");
    let stdout = child.stdout.take().expect("fixture stdout");
    driver.reaper().push(child);
    let (send, lines) = channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if send.send(line).is_err() {
                return;
            }
        }
    });
    let monitors = next_line(&lines);
    let monitors = monitors.as_array().expect("monitor list");
    std::thread::sleep(Duration::from_secs(2));

    // The frame is the bounding box GDK sees, whatever its origin.
    let field = |monitor: &Value, name: &str| monitor[name].as_i64().expect("geometry");
    let origin_x = monitors.iter().map(|m| field(m, "x")).min().expect("x");
    let origin_y = monitors.iter().map(|m| field(m, "y")).min().expect("y");
    let right = monitors.iter().map(|m| field(m, "x") + field(m, "width"));
    let bottom = monitors.iter().map(|m| field(m, "y") + field(m, "height"));
    let width = right.max().expect("right") - origin_x;
    let height = bottom.max().expect("bottom") - origin_y;

    let size = driver.call("get_screen_size", json!({}));
    assert!(!size.is_error(), "{}", size.text());
    let size = size.structured().clone();
    assert_eq!(
        (size["width"].as_i64(), size["height"].as_i64()),
        (Some(width), Some(height))
    );
    let reported = size["monitors"].as_array().expect("monitor report");
    assert_eq!(reported.len(), monitors.len(), "{size}");
    for monitor in monitors {
        let corner = (
            field(monitor, "x") - origin_x,
            field(monitor, "y") - origin_y,
        );
        assert!(
            reported.iter().any(|r| {
                (r["frame_x"].as_i64(), r["frame_y"].as_i64()) == (Some(corner.0), Some(corner.1))
            }),
            "{monitor} is not at {corner:?} in {size}"
        );
    }

    // Mixed scales report the largest one, whichever output came first.
    let largest = reported
        .iter()
        .map(|r| r["scale"].as_f64().expect("monitor scale"))
        .fold(f64::MIN, f64::max);
    assert_eq!(size["scale_factor"].as_f64(), Some(largest), "{size}");

    // The screenshot is that same frame, described with the same numbers.
    let (state, image) = screenshot(&mut driver);
    assert_eq!(i64::from(image.width()), width);
    assert_eq!(i64::from(image.height()), height);
    assert_eq!(state["screen_width"].as_i64(), Some(width));
    assert_eq!(state["screen_height"].as_i64(), Some(height));
    assert_eq!(state["scale_factor"], size["scale_factor"]);

    let frame = (origin_x, origin_y);
    let distorted = state["distorted_outputs"].as_array().cloned();
    let distorted = distorted.unwrap_or_default();
    for (index, monitor) in monitors.iter().enumerate() {
        // An output KWin's capture distorts names itself, and refuses input.
        let corner = (
            field(monitor, "x") - origin_x,
            field(monitor, "y") - origin_y,
        );
        let name = &reported
            .iter()
            .find(|r| {
                (r["frame_x"].as_i64(), r["frame_y"].as_i64()) == (Some(corner.0), Some(corner.1))
            })
            .expect("reported monitor")["name"];
        if distorted.contains(name) {
            let refusal = click(&mut driver, corner.0, corner.1);
            let expected = format!(
                "desktop point ({}, {}) is on {}, a rotated output that KWin",
                corner.0,
                corner.1,
                name.as_str().expect("monitor name")
            );
            assert!(refusal.text().contains(&expected), "{}", refusal.text());
            continue;
        }

        // Its marker, read off the screenshot.
        let (x, y) = marker_center(&image, MARKER_COLORS[index]);
        let point = (i64::from(x), i64::from(y));
        if let Some(press) = click_on_output(&mut driver, &lines, frame, point) {
            assert_eq!(press["monitor"], index, "{press}");
            let (left, top, side) = MARKER;
            for (axis, start) in [("x", left), ("y", top)] {
                let value = press[axis].as_f64().expect("press coordinate");
                assert!((start..start + side).contains(&value), "{press}");
            }
            let cursor = driver.call("get_cursor_position", json!({}));
            let cursor = cursor.structured();
            assert_eq!(
                (cursor["x"].as_i64(), cursor["y"].as_i64()),
                (Some(point.0), Some(point.1))
            );
        }

        // The output's first pixel, which is pixel (0, 0) of a frame it starts.
        if let Some(press) = click_on_output(&mut driver, &lines, frame, corner) {
            assert_eq!(press["monitor"], index, "{press}");
            for axis in ["x", "y"] {
                assert!(press[axis].as_f64().expect("press") < 2.0, "{press}");
            }
        }
    }

    // A point past the frame is refused, not moved to somewhere else.
    let (x, y) = (width + 10, height + 10);
    let refusal = click(&mut driver, x, y);
    assert!(refusal.is_error(), "{}", refusal.text());
    let expected = if monitors.len() > 1 {
        format!("desktop point ({x}, {y}) is not on a powered monitor in the current layout; call get_desktop_state again")
    } else {
        format!(
            "libei: ({}, {}) lies outside every announced pointer region",
            origin_x + x,
            origin_y + y
        )
    };
    assert!(refusal.text().contains(&expected), "{}", refusal.text());
}
