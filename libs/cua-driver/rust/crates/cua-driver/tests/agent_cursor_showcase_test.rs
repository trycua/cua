//! Cross-platform semantic cursor showcase for release evidence.

use std::time::Duration;

use base64::Engine as _;
use cua_driver_testkit::e2e::{
    execute_case, recording_evidence, CaseSpec, Delivery, DriverRoute, Evidence, Observation,
    OracleKind, Scope, Targeting,
};
use cua_driver_testkit::{Driver, McpDriver};
use cursor_overlay::{BADGE_CURSOR_GAP, BADGE_HEIGHT, BADGE_MAX_WIDTH};
use image::RgbaImage;

const CELL_ID: &str = "desktop-agent-cursor-showcase-px";
const SESSION: &str = "Cursor showcase";
// MoveTo offsets the cursor artwork by 16 points so its tip lands on the
// requested coordinate. The session badge follows that artwork anchor.
const MAX_CURSOR_ANCHOR_OFFSET: f64 = 16.0;

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

        // Capture the empty desktop before declaring the explicit session.
        // `start_session` may revive and materialize the session-owned overlay
        // at the current pointer position, which can otherwise put the badge
        // in both frames when a previous showcase left the pointer at this
        // deterministic target.
        let (baseline_png, width, height) = capture_desktop_png(&mut driver);
        let baseline = image::load_from_memory(&baseline_png)
            .expect("decode baseline desktop screenshot")
            .to_rgba8();
        assert!(
            width >= 640.0 && height >= 480.0,
            "showcase requires a normal desktop, got {width}x{height}"
        );

        call_ok(
            &mut driver,
            "start_session",
            serde_json::json!({
                "session": SESSION
            }),
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
        // move_cursor waits for the configured glide, but X11/Wayland capture
        // still needs a compositor round-trip before the overlay is guaranteed
        // to appear in the driver-owned screenshot. The badge remains fully
        // visible for two seconds, so this settle stays inside that window.
        settle(900);

        let cursor_png = capture_cursor_oracle_png(&mut driver);
        let cursor_frame = image::load_from_memory(&cursor_png)
            .expect("decode cursor desktop screenshot")
            .to_rgba8();
        assert_cursor_and_badge_pixels_changed(
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
                "target": {"kind": "desktop", "display_id": "primary"},
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
                "target": {"kind": "desktop", "display_id": "primary"},
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
                "target": {"kind": "desktop", "display_id": "primary"},
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
                "target": {"kind": "desktop", "display_id": "primary"},
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

fn assert_cursor_and_badge_pixels_changed(
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

    // The pointer is anchored at the requested coordinate (with at most the
    // renderer's small click offset). The session badge is a separate pill
    // centered below it. Keep the regions disjoint so a visible pointer alone
    // cannot satisfy the badge oracle, which was possible with the previous
    // single large region/count check.
    let pointer_radius_x = (48.0 * scale_x).ceil() as i64;
    let pointer_radius_y = (48.0 * scale_y).ceil() as i64;
    let pointer_pixels = changed_pixels_in_rect(
        baseline,
        cursor_frame,
        center_x - pointer_radius_x,
        center_y - pointer_radius_y,
        center_x + pointer_radius_x,
        center_y + (f64::from(BADGE_CURSOR_GAP) * scale_y).floor() as i64,
    );

    let badge_half_width = (f64::from(BADGE_MAX_WIDTH) * 0.5 * scale_x).ceil() as i64;
    let badge_cursor_exclusion = (34.0 * scale_x).ceil() as i64;
    let badge_top = center_y + (f64::from(BADGE_CURSOR_GAP) * scale_y).floor() as i64;
    let badge_bottom = center_y
        + ((f64::from(BADGE_CURSOR_GAP + BADGE_HEIGHT) + MAX_CURSOR_ANCHOR_OFFSET) * scale_y).ceil()
            as i64;
    // Ignore the center corridor where the pointer's lower edge or glow could
    // overlap the pill. Requiring changed pixels in the badge's outer wings
    // makes this an independent badge assertion.
    let badge_pixels = changed_pixels_in_rect(
        baseline,
        cursor_frame,
        center_x - badge_half_width,
        badge_top,
        center_x - badge_cursor_exclusion,
        badge_bottom,
    ) + changed_pixels_in_rect(
        baseline,
        cursor_frame,
        center_x + badge_cursor_exclusion,
        badge_top,
        center_x + badge_half_width,
        badge_bottom,
    );

    assert!(
        pointer_pixels >= 12 && badge_pixels >= 24,
        "agent cursor overlay was incomplete near ({logical_x:.0},{logical_y:.0}): \
         pointer region changed {pointer_pixels} pixels (minimum 12), \
         badge region changed {badge_pixels} pixels (minimum 24)"
    );
}

fn changed_pixels_in_rect(
    baseline: &RgbaImage,
    cursor_frame: &RgbaImage,
    x0: i64,
    y0: i64,
    x1: i64,
    y1: i64,
) -> usize {
    let x0 = x0.clamp(0, i64::from(baseline.width())) as u32;
    let x1 = x1.clamp(0, i64::from(baseline.width())) as u32;
    let y0 = y0.clamp(0, i64::from(baseline.height())) as u32;
    let y1 = y1.clamp(0, i64::from(baseline.height())) as u32;
    (y0..y1)
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
        .count()
}

fn capture_desktop_png(driver: &mut McpDriver) -> (Vec<u8>, f64, f64) {
    let response = driver.call("get_desktop_state", serde_json::json!({}));
    assert!(
        !response.is_error(),
        "driver-owned desktop capture failed: {}",
        response.text()
    );
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
        .expect("driver-owned desktop capture returned no PNG image");
    let png = base64::engine::general_purpose::STANDARD
        .decode(image_base64)
        .expect("decode driver-owned desktop screenshot");
    let width = response.structured()["screen_width"]
        .as_f64()
        .or_else(|| response.structured()["screenshot_width"].as_f64())
        .expect("desktop capture returned no logical width");
    let height = response.structured()["screen_height"]
        .as_f64()
        .or_else(|| response.structured()["screenshot_height"].as_f64())
        .expect("desktop capture returned no logical height");
    (png, width, height)
}

fn capture_cursor_oracle_png(driver: &mut McpDriver) -> Vec<u8> {
    #[cfg(target_os = "linux")]
    {
        // A compositor-less X11 root read can omit the overlay client's own
        // shaped window even while an independent display capture sees the
        // complete cursor and badge. The action evidence recorder is the
        // canonical external behavior oracle and has already captured the
        // move_cursor after-frame before this call returns.
        let path = driver
            .recording_dir()
            .expect("showcase recording directory")
            .join("turn-00001/after.png");
        for _ in 0..20 {
            if let Ok(png) = std::fs::read(&path) {
                if !png.is_empty() {
                    return png;
                }
            }
            settle(50);
        }
        panic!(
            "action evidence did not produce the cursor after-frame at {}",
            path.display()
        );
    }

    #[cfg(not(target_os = "linux"))]
    {
        capture_desktop_png(driver).0
    }
}

fn call_ok(driver: &mut McpDriver, tool: &str, arguments: serde_json::Value) {
    let response = driver.call(tool, arguments);
    assert!(!response.is_error(), "{tool} failed: {}", response.text());
}

fn settle(milliseconds: u64) {
    std::thread::sleep(Duration::from_millis(milliseconds));
}

#[cfg(test)]
mod pixel_oracle_tests {
    use super::*;
    use image::Rgba;

    const WIDTH: u32 = 400;
    const HEIGHT: u32 = 300;
    const CURSOR_X: f64 = 200.0;
    const CURSOR_Y: f64 = 150.0;

    #[test]
    fn accepts_independent_pointer_and_badge_changes() {
        let baseline = RgbaImage::new(WIDTH, HEIGHT);
        let mut overlay = baseline.clone();
        paint_changed_rect(&mut overlay, 196, 146, 200, 150);
        paint_changed_rect(&mut overlay, 150, 180, 156, 186);

        assert_cursor_and_badge_pixels_changed(
            &baseline,
            &overlay,
            CURSOR_X,
            CURSOR_Y,
            f64::from(WIDTH),
            f64::from(HEIGHT),
        );
    }

    #[test]
    fn accepts_badge_at_shifted_cursor_artwork_anchor() {
        let baseline = RgbaImage::new(WIDTH, HEIGHT);
        let mut overlay = baseline.clone();
        paint_changed_rect(&mut overlay, 196, 146, 200, 150);
        paint_changed_rect(&mut overlay, 250, 204, 256, 210);

        assert_cursor_and_badge_pixels_changed(
            &baseline,
            &overlay,
            CURSOR_X,
            CURSOR_Y,
            f64::from(WIDTH),
            f64::from(HEIGHT),
        );
    }

    #[test]
    #[should_panic(expected = "badge region changed 0 pixels")]
    fn rejects_pointer_without_badge() {
        let baseline = RgbaImage::new(WIDTH, HEIGHT);
        let mut pointer_only = baseline.clone();
        paint_changed_rect(&mut pointer_only, 196, 146, 200, 150);

        assert_cursor_and_badge_pixels_changed(
            &baseline,
            &pointer_only,
            CURSOR_X,
            CURSOR_Y,
            f64::from(WIDTH),
            f64::from(HEIGHT),
        );
    }

    fn paint_changed_rect(image: &mut RgbaImage, x0: u32, y0: u32, x1: u32, y1: u32) {
        for y in y0..y1 {
            for x in x0..x1 {
                image.put_pixel(x, y, Rgba([94, 192, 232, 255]));
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn spawn_driver() -> McpDriver {
    McpDriver::spawn_macos_daemon_proxy_named(CELL_ID).expect("start installed macOS daemon proxy")
}

#[cfg(not(target_os = "macos"))]
fn spawn_driver() -> McpDriver {
    McpDriver::spawn_named_with_overlay(CELL_ID)
        .expect("start source-built driver with native cursor overlay")
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
