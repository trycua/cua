//! Live Hyprland regression: a background element click must reveal the
//! agent-cursor overlay at the element's *proven output coordinates* (#3554).
//!
//! The failure this guards against: window-local accessibility bounds being
//! used as output-space coordinates, which draws the agent cursor near the
//! output origin instead of over the target control. The check paints the
//! overlay's debug square (`CUA_OVERLAY_DEBUG=1`), captures the desktop
//! through the driver, locates the square, and asserts it lands on the
//! element's absolute frame center reported by the same snapshot.
//!
//! Preconditions: a live Hyprland session (native Wayland), the GTK3 harness
//! (`CUA_TEST_APPS_ROOT` / `test-apps/harness-gtk3`), and a fixture window
//! placed away from the output origin. Run under the shared desktop lock:
//! `flock /tmp/cua-hyprland-live.lock -c '<cargo test command>'`.

#![cfg(target_os = "linux")]

use std::process::Command;
use std::time::{Duration, Instant};

use base64::Engine as _;
use cua_driver_testkit::{harness_app, Driver, McpDriver, ToolResponse};
use image::RgbaImage;

const SESSION: &str = "hyprland-element-reveal";
const CELL: &str = "hyprland-element-reveal";
/// Point tolerance between the measured cursor center and the element frame
/// center. The overlay anchors the artwork ~16 points off the exact point and
/// the debug square is 60 points wide, so a small slack is expected; a wrong
/// coordinate space is hundreds of points off.
const TOLERANCE_POINTS: f64 = 32.0;

/// Center of the overlay's magenta debug square, in capture pixels.
fn magenta_centroid(image: &RgbaImage) -> Option<(f64, f64)> {
    let mut xs = 0f64;
    let mut ys = 0f64;
    let mut count = 0u32;
    for (x, y, pixel) in image.enumerate_pixels() {
        let [r, g, b, _a] = pixel.0;
        if r > 200 && g < 90 && b > 200 {
            xs += f64::from(x);
            ys += f64::from(y);
            count += 1;
        }
    }
    (count > 0).then(|| (xs / f64::from(count), ys / f64::from(count)))
}

/// Capture the desktop through the driver.
fn capture_desktop(driver: &mut McpDriver) -> RgbaImage {
    let response = driver.call("get_desktop_state", serde_json::json!({}));
    assert!(
        !response.is_error(),
        "driver-owned desktop capture failed: {}",
        response.text()
    );
    image_from_response(&response)
}

/// Logical geometry of the single Hyprland output, used to map capture pixels
/// back to the compositor's global layout coordinates.
struct OutputGeometry {
    x: f64,
    y: f64,
    logical_w: f64,
    logical_h: f64,
}

fn single_output() -> OutputGeometry {
    let output = Command::new("hyprctl")
        .args(["-j", "monitors"])
        .output()
        .expect("run hyprctl -j monitors");
    let monitors: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("parse hyprctl monitors JSON");
    let monitors = monitors.as_array().expect("monitors array");
    assert_eq!(
        monitors.len(),
        1,
        "this regression expects a single-output Hyprland desktop"
    );
    let monitor = &monitors[0];
    let scale = monitor["scale"].as_f64().expect("monitor scale");
    OutputGeometry {
        x: monitor["x"].as_f64().expect("monitor x"),
        y: monitor["y"].as_f64().expect("monitor y"),
        logical_w: monitor["width"].as_f64().expect("monitor width") / scale,
        logical_h: monitor["height"].as_f64().expect("monitor height") / scale,
    }
}

fn image_from_response(response: &ToolResponse) -> RgbaImage {
    let image_base64 = response.raw["result"]["content"]
        .as_array()
        .and_then(|content| {
            content.iter().find_map(|item| {
                (item["type"].as_str() == Some("image")
                    && item["mimeType"].as_str() == Some("image/png"))
                .then(|| item["data"].as_str())
                .flatten()
            })
        })
        .expect("desktop capture returned no PNG image");
    let png = base64::engine::general_purpose::STANDARD
        .decode(image_base64)
        .expect("decode desktop screenshot");
    image::load_from_memory(&png)
        .expect("decode desktop screenshot PNG")
        .to_rgba8()
}

fn centroid_in_points(image: &RgbaImage, output: &OutputGeometry) -> Option<(f64, f64)> {
    let (cx, cy) = magenta_centroid(image)?;
    let (w, h) = image.dimensions();
    let scale_x = f64::from(w) / output.logical_w;
    let scale_y = f64::from(h) / output.logical_h;
    Some((cx / scale_x + output.x, cy / scale_y + output.y))
}

#[test]
#[ignore]
fn background_element_click_reveals_cursor_at_element_output_coordinates() {
    assert!(
        std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some(),
        "live Hyprland session required"
    );
    let exe = harness_app("harness-gtk3", "CuaTestHarness.Gtk3");
    assert!(exe.exists(), "required GTK3 harness is missing: {exe:?}");

    let mut driver = McpDriver::spawn_named_with_overlay_and_env(
        CELL,
        &[
            ("CUA_OVERLAY_DEBUG", "1"),
            ("CUA_DRIVER_RS_ENABLE_WAYLAND", "1"),
        ],
    )
    .expect("start source-built driver with the native cursor overlay");

    driver
        .reaper()
        .spawn(
            Command::new(&exe)
                .env("GDK_BACKEND", "wayland")
                .stdout(std::process::Stdio::inherit())
                .stderr(std::process::Stdio::inherit()),
        )
        .unwrap_or_else(|error| panic!("launch GTK3 harness {exe:?}: {error}"));

    // Locate the fixture window and its bounds.
    let deadline = Instant::now() + Duration::from_secs(12);
    let (pid, window_id, window) = loop {
        assert!(
            Instant::now() < deadline,
            "GTK3 harness window never appeared"
        );
        let response = driver.call("list_windows", serde_json::json!({}));
        let found = response.structured()["windows"]
            .as_array()
            .and_then(|windows| {
                windows.iter().find_map(|window| {
                    window["title"]
                        .as_str()
                        .unwrap_or("")
                        .contains("CuaTestHarness GTK3")
                        .then(|| window.clone())
                })
            });
        if let Some(window) = found {
            let pid = window["pid"].as_u64().unwrap_or(0) as u32;
            let window_id = window["window_id"].as_u64().unwrap_or(0);
            if pid != 0 && window_id != 0 {
                driver.reaper().track_pid(pid);
                break (pid, window_id, window);
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    let bounds = &window["bounds"];
    let (wx, wy, ww, wh) = (
        bounds["x"].as_f64().expect("window x"),
        bounds["y"].as_f64().expect("window y"),
        bounds["width"].as_f64().expect("window width"),
        bounds["height"].as_f64().expect("window height"),
    );
    assert!(
        wx.abs() + wy.abs() > 200.0,
        "this regression needs the fixture away from the output origin \
         (window at ({wx:.0}, {wy:.0})) so a window-local leak cannot pass"
    );

    // Cursor settings + a neutral starting position inside the window.
    let call_ok = |driver: &mut McpDriver, name: &str, args: serde_json::Value| {
        let response = driver.call(name, args);
        assert!(!response.is_error(), "{name} failed: {}", response.text());
        response
    };
    call_ok(
        &mut driver,
        "start_session",
        serde_json::json!({"session": SESSION}),
    );
    call_ok(
        &mut driver,
        "set_agent_cursor_motion",
        serde_json::json!({"session": SESSION, "glide_duration_ms": 100, "idle_hide_ms": 0}),
    );
    call_ok(
        &mut driver,
        "set_agent_cursor_enabled",
        serde_json::json!({"session": SESSION, "enabled": true}),
    );
    let neutral = (wx + ww / 2.0, wy + wh / 2.0);
    call_ok(
        &mut driver,
        "move_cursor",
        serde_json::json!({"session": SESSION, "x": neutral.0, "y": neutral.1}),
    );
    std::thread::sleep(Duration::from_millis(400));

    // Snapshot for the exact element token + its absolute frame.
    let snapshot = call_ok(
        &mut driver,
        "get_window_state",
        serde_json::json!({
            "pid": pid,
            "window_id": window_id,
            "include_accessibility_tree": true,
            "include_screenshot": false,
        }),
    );
    let element = snapshot.structured()["elements"]
        .as_array()
        .and_then(|elements| {
            elements.iter().find_map(|element| {
                (element["label"].as_str() == Some("btn-increment")).then(|| element.clone())
            })
        })
        .expect("btn-increment element missing from the snapshot");
    let token = element["element_token"].as_str().expect("element token");
    let frame = &element["frame"];
    let expected = (
        frame["x"].as_f64().expect("frame x") + frame["w"].as_f64().expect("frame w") / 2.0,
        frame["y"].as_f64().expect("frame y") + frame["h"].as_f64().expect("frame h") / 2.0,
    );
    assert!(
        expected.0 >= wx && expected.0 <= wx + ww && expected.1 >= wy && expected.1 <= wy + wh,
        "element frame {expected:?} is not inside the window bounds \
         ({wx:.0},{wy:.0} {ww:.0}x{wh:.0})"
    );

    let output = single_output();
    let baseline_image = capture_desktop(&mut driver);
    let baseline = centroid_in_points(&baseline_image, &output)
        .expect("baseline: overlay debug square not found — is the overlay painting?");

    // Background element click: the reveal must glide the agent cursor to the
    // element's proven output coordinates.
    call_ok(
        &mut driver,
        "click",
        serde_json::json!({
            "pid": pid,
            "session": SESSION,
            "element_token": token,
            "delivery_mode": "background",
        }),
    );
    std::thread::sleep(Duration::from_millis(1200));
    let after_image = capture_desktop(&mut driver);
    let after =
        centroid_in_points(&after_image, &output).expect("after: overlay debug square not found");

    let moved = ((after.0 - baseline.0).powi(2) + (after.1 - baseline.1).powi(2)).sqrt();
    assert!(
        moved > TOLERANCE_POINTS,
        "click did not move the revealed cursor (baseline {baseline:?}, after {after:?})"
    );
    let distance = ((after.0 - expected.0).powi(2) + (after.1 - expected.1).powi(2)).sqrt();
    assert!(
        distance <= TOLERANCE_POINTS,
        "revealed cursor at {after:?} is {distance:.1} points from the element's \
         absolute frame center {expected:?} (window at ({wx:.0},{wy:.0})) — \
         window-local bounds are being used as output coordinates"
    );
    assert!(
        after.0 >= wx && after.0 <= wx + ww && after.1 >= wy && after.1 <= wy + wh,
        "revealed cursor {after:?} is outside the target window bounds \
         ({wx:.0},{wy:.0} {ww:.0}x{wh:.0})"
    );
}
