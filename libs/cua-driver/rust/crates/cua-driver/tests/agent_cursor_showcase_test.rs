//! Cross-platform semantic cursor showcase for release evidence.

use std::time::Duration;

use cua_driver_testkit::e2e::{
    execute_case, recording_evidence, CaseSpec, Delivery, DriverRoute, Evidence, Observation,
    OracleKind, Scope, Targeting,
};
use cua_driver_testkit::{Driver, McpDriver};
use image::RgbaImage;

const CELL_ID: &str = "desktop-agent-cursor-showcase-px";
const SESSION: &str = "Cursor showcase";

#[test]
#[ignore]
fn semantic_cursor_showcase_records_session_and_action_states() {
    let case = CaseSpec::delivered(
        CELL_ID,
        "desktop",
        platform_toolkit(),
        "agent_cursor_showcase",
        Targeting::Px,
        Delivery::Foreground,
        Scope::Desktop,
        platform_route(),
        vec![OracleKind::Pixels],
    );
    execute_case(case, |evidence| {
        let mut driver = spawn_driver();
        *evidence = recording_evidence(driver.recording_dir());

        call_ok(
            &mut driver,
            "start_session",
            serde_json::json!({
                "session": SESSION,
                "capture_scope": "desktop"
            }),
        );

        let baseline_png = capture_desktop_png();
        let baseline = image::load_from_memory(&baseline_png)
            .expect("decode baseline desktop screenshot")
            .to_rgba8();
        let screen = driver.call("get_screen_size", serde_json::json!({}));
        let (width, height) = if screen.is_error() {
            (f64::from(baseline.width()), f64::from(baseline.height()))
        } else {
            (
                screen.structured()["width"].as_f64().expect("screen width"),
                screen.structured()["height"]
                    .as_f64()
                    .expect("screen height"),
            )
        };
        assert!(
            width >= 640.0 && height >= 480.0,
            "showcase requires a normal desktop, got {width}x{height}"
        );

        call_ok(
            &mut driver,
            "set_agent_cursor_enabled",
            serde_json::json!({
                "session": SESSION,
                "enabled": true
            }),
        );
        call_ok(
            &mut driver,
            "set_agent_cursor_motion",
            serde_json::json!({
                "session": SESSION,
                "glide_duration_ms": 420,
                "idle_hide_ms": 0
            }),
        );

        let center_x = width * 0.55;
        let center_y = height * 0.45;
        driver.start_behavior_recording();

        call_ok(
            &mut driver,
            "move_cursor",
            serde_json::json!({
                "session": SESSION,
                "x": center_x - 180.0,
                "y": center_y - 80.0
            }),
        );
        settle(900);

        let cursor_png = capture_desktop_png();
        let cursor_frame = image::load_from_memory(&cursor_png)
            .expect("decode cursor desktop screenshot")
            .to_rgba8();
        assert_cursor_pixels_changed(
            &baseline,
            &cursor_frame,
            center_x - 180.0,
            center_y - 80.0,
            width,
            height,
        );
        let screenshot_path = driver
            .recording_dir()
            .expect("showcase recording directory")
            .join("cursor-oracle.png");
        std::fs::write(&screenshot_path, cursor_png).expect("write cursor oracle screenshot");
        evidence.screenshot = Some(screenshot_path.display().to_string());

        call_ok(
            &mut driver,
            "click",
            serde_json::json!({
                "session": SESSION,
                "scope": "desktop",
                "x": center_x,
                "y": center_y,
                "delivery_mode": "foreground"
            }),
        );
        settle(900);

        call_ok(
            &mut driver,
            "type_text",
            serde_json::json!({
                "session": SESSION,
                "scope": "desktop",
                "text": "cua",
                "delivery_mode": "foreground"
            }),
        );
        settle(900);

        call_ok(
            &mut driver,
            "scroll",
            serde_json::json!({
                "session": SESSION,
                "scope": "desktop",
                "x": center_x,
                "y": center_y,
                "direction": "down",
                "amount": 4,
                "delivery_mode": "foreground"
            }),
        );
        settle(900);

        call_ok(
            &mut driver,
            "drag",
            serde_json::json!({
                "session": SESSION,
                "scope": "desktop",
                "from_x": center_x - 90.0,
                "from_y": center_y + 80.0,
                "to_x": center_x + 120.0,
                "to_y": center_y + 20.0,
                "duration_ms": 700,
                "steps": 28,
                "delivery_mode": "foreground"
            }),
        );
        settle(1_100);

        Observation::delivered(vec![OracleKind::Pixels], Evidence::default())
    });
}

fn assert_cursor_pixels_changed(
    baseline: &RgbaImage,
    cursor_frame: &RgbaImage,
    logical_x: f64,
    logical_y: f64,
    logical_width: f64,
    logical_height: f64,
) {
    assert_eq!(
        baseline.dimensions(),
        cursor_frame.dimensions(),
        "desktop dimensions changed while checking the cursor overlay"
    );
    let scale_x = f64::from(baseline.width()) / logical_width;
    let scale_y = f64::from(baseline.height()) / logical_height;
    let center_x = (logical_x * scale_x).round() as i64;
    let center_y = (logical_y * scale_y).round() as i64;
    let radius_x = (170.0 * scale_x).round() as i64;
    let radius_y = (130.0 * scale_y).round() as i64;
    let x0 = (center_x - radius_x).max(0) as u32;
    let x1 = (center_x + radius_x)
        .min(i64::from(baseline.width()))
        .max(0) as u32;
    let y0 = (center_y - radius_y).max(0) as u32;
    let y1 = (center_y + radius_y)
        .min(i64::from(baseline.height()))
        .max(0) as u32;
    let changed_pixels = (y0..y1)
        .flat_map(|pixel_y| (x0..x1).map(move |pixel_x| (pixel_x, pixel_y)))
        .filter(|(pixel_x, pixel_y)| {
            let before = baseline.get_pixel(*pixel_x, *pixel_y).0;
            let after = cursor_frame.get_pixel(*pixel_x, *pixel_y).0;
            before
                .iter()
                .zip(after.iter())
                .map(|(left, right)| u16::from(left.abs_diff(*right)))
                .sum::<u16>()
                >= 80
        })
        .count();
    assert!(
        changed_pixels >= 24,
        "agent cursor and session badge were not externally visible near \
         ({logical_x:.0},{logical_y:.0}): only {changed_pixels} pixels changed"
    );
}

#[cfg(target_os = "linux")]
fn capture_desktop_png() -> Vec<u8> {
    platform_linux::capture::screenshot_display_bytes().expect("capture Linux desktop")
}

#[cfg(target_os = "macos")]
fn capture_desktop_png() -> Vec<u8> {
    platform_macos::capture::screenshot_display_bytes().expect("capture macOS desktop")
}

#[cfg(target_os = "windows")]
fn capture_desktop_png() -> Vec<u8> {
    platform_windows::capture::screenshot_display_bytes().expect("capture Windows desktop")
}

fn call_ok(driver: &mut McpDriver, tool: &str, arguments: serde_json::Value) {
    let response = driver.call(tool, arguments);
    assert!(!response.is_error(), "{tool} failed: {}", response.text());
}

fn settle(milliseconds: u64) {
    std::thread::sleep(Duration::from_millis(milliseconds));
}

#[cfg(target_os = "macos")]
fn spawn_driver() -> McpDriver {
    McpDriver::spawn_macos_daemon_proxy_named(CELL_ID).expect("start installed macOS daemon proxy")
}

#[cfg(not(target_os = "macos"))]
fn spawn_driver() -> McpDriver {
    McpDriver::spawn_named(CELL_ID).expect("start source-built driver")
}

#[cfg(target_os = "macos")]
fn platform_toolkit() -> &'static str {
    "appkit"
}

#[cfg(target_os = "windows")]
fn platform_toolkit() -> &'static str {
    "win32"
}

#[cfg(target_os = "linux")]
fn platform_toolkit() -> &'static str {
    "gtk3"
}

#[cfg(target_os = "macos")]
fn platform_route() -> DriverRoute {
    DriverRoute::Composite
}

#[cfg(target_os = "windows")]
fn platform_route() -> DriverRoute {
    DriverRoute::WindowsOverlay
}

#[cfg(target_os = "linux")]
fn platform_route() -> DriverRoute {
    if std::env::var_os("CUA_INJECT_SOCKET").is_some() {
        DriverRoute::LinuxCuaCompositorInject
    } else if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        DriverRoute::LinuxWaylandVirtualPointer
    } else {
        DriverRoute::LinuxXTest
    }
}
