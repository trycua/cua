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
// MoveTo offsets the artwork centre by a 16-point vector at 45 degrees so the
// cursor tip lands on the requested coordinate. Each axis moves by 16/sqrt(2),
// and the session badge follows that artwork centre.
const CURSOR_ANCHOR_OFFSET_MAGNITUDE: f64 = 16.0;
const CURSOR_ANCHOR_OFFSET_PER_AXIS: f64 =
    CURSOR_ANCHOR_OFFSET_MAGNITUDE * std::f64::consts::FRAC_1_SQRT_2;
const POINTER_ORACLE_RADIUS: f64 = 24.0;
const BADGE_CURSOR_EXCLUSION: f64 = 34.0;

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

        let cursor_png = capture_cursor_oracle_png(&mut driver, width, height);
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
    let regions = cursor_oracle_regions(
        baseline.width(),
        baseline.height(),
        logical_x,
        logical_y,
        logical_width,
        logical_height,
    );
    let pointer_pixels = changed_pixels_in_rect(
        baseline,
        cursor_frame,
        regions.pointer.x0,
        regions.pointer.y0,
        regions.pointer.x1,
        regions.pointer.y1,
    );
    // Ignore the center corridor where the pointer's lower edge or glow could
    // overlap the pill. Requiring changed pixels in the badge's outer wings
    // makes this an independent badge assertion.
    let badge_pixels = changed_pixels_in_rect(
        baseline,
        cursor_frame,
        regions.badge_left.x0,
        regions.badge_left.y0,
        regions.badge_left.x1,
        regions.badge_left.y1,
    ) + changed_pixels_in_rect(
        baseline,
        cursor_frame,
        regions.badge_right.x0,
        regions.badge_right.y0,
        regions.badge_right.x1,
        regions.badge_right.y1,
    );

    assert!(
        pointer_pixels >= 12 && badge_pixels >= 24,
        "agent cursor overlay was incomplete near ({logical_x:.0},{logical_y:.0}): \
         pointer region changed {pointer_pixels} pixels (minimum 12), \
         badge region changed {badge_pixels} pixels (minimum 24); \
         image={}x{}, logical={}x{}, scale={:.3}x{:.3}, \
         pointer_rect=({},{}..{},{}), badge_rect=({},{}..{},{}), \
         badge_exclusion={}",
        baseline.width(),
        baseline.height(),
        logical_width,
        logical_height,
        regions.scale_x,
        regions.scale_y,
        regions.pointer.x0,
        regions.pointer.y0,
        regions.pointer.x1,
        regions.pointer.y1,
        regions.badge_left.x0,
        regions.badge_left.y0,
        regions.badge_right.x1,
        regions.badge_right.y1,
        (BADGE_CURSOR_EXCLUSION * regions.scale_x).ceil() as i64,
    );
}

#[derive(Clone, Copy, Debug)]
struct PixelRect {
    x0: i64,
    y0: i64,
    x1: i64,
    y1: i64,
}

#[derive(Clone, Copy, Debug)]
struct CursorOracleRegions {
    anchor: (i64, i64),
    scale_x: f64,
    scale_y: f64,
    pointer: PixelRect,
    badge_left: PixelRect,
    badge_right: PixelRect,
}

fn cursor_oracle_regions(
    image_width: u32,
    image_height: u32,
    logical_x: f64,
    logical_y: f64,
    logical_width: f64,
    logical_height: f64,
) -> CursorOracleRegions {
    let scale_x = f64::from(image_width) / logical_width;
    let scale_y = f64::from(image_height) / logical_height;
    let anchor_x = (logical_x + CURSOR_ANCHOR_OFFSET_PER_AXIS) * scale_x;
    let anchor_y = (logical_y + CURSOR_ANCHOR_OFFSET_PER_AXIS) * scale_y;

    // The production artwork is 42 points across. A 24-point radius includes
    // its outline while remaining one logical point above the badge. Floor the
    // pointer bottom and ceil the badge top so fractional and unequal scales
    // cannot round the two regions onto the same pixel row.
    let pointer = PixelRect {
        x0: (anchor_x - POINTER_ORACLE_RADIUS * scale_x).floor() as i64,
        y0: (anchor_y - POINTER_ORACLE_RADIUS * scale_y).floor() as i64,
        x1: (anchor_x + POINTER_ORACLE_RADIUS * scale_x).ceil() as i64,
        y1: (anchor_y + POINTER_ORACLE_RADIUS * scale_y).floor() as i64,
    };
    let badge_half_width = f64::from(BADGE_MAX_WIDTH) * 0.5 * scale_x;
    let badge_exclusion = BADGE_CURSOR_EXCLUSION * scale_x;
    let badge_top = (anchor_y + f64::from(BADGE_CURSOR_GAP) * scale_y).ceil() as i64;
    let badge_bottom =
        (anchor_y + f64::from(BADGE_CURSOR_GAP + BADGE_HEIGHT) * scale_y).ceil() as i64;
    let badge_left = PixelRect {
        x0: (anchor_x - badge_half_width).floor() as i64,
        y0: badge_top,
        x1: (anchor_x - badge_exclusion).floor() as i64,
        y1: badge_bottom,
    };
    let badge_right = PixelRect {
        x0: (anchor_x + badge_exclusion).ceil() as i64,
        y0: badge_top,
        x1: (anchor_x + badge_half_width).ceil() as i64,
        y1: badge_bottom,
    };

    CursorOracleRegions {
        anchor: (anchor_x.round() as i64, anchor_y.round() as i64),
        scale_x,
        scale_y,
        pointer,
        badge_left,
        badge_right,
    }
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

fn capture_cursor_oracle_png(driver: &mut McpDriver, width: f64, height: f64) -> Vec<u8> {
    #[cfg(target_os = "linux")]
    {
        // Native Wayland has no X11 DISPLAY to hand to x11grab. The driver's
        // display capture is the composed-screen oracle for that lane; keep
        // ffmpeg only for the canonical X11 path where it is available.
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            return capture_desktop_png(driver).0;
        }
        // XGetImage root reads can omit a shaped overlay client's pixels on a
        // compositor-less X11 server. Capture the composed display exactly as
        // the behavioral recording does so the oracle observes what a user sees.
        let display = std::env::var("DISPLAY").expect("Linux cursor showcase requires DISPLAY");
        let video_size = format!("{}x{}", width.round() as u32, height.round() as u32);
        let output = std::process::Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "x11grab",
                "-draw_mouse",
                "0",
                "-video_size",
                &video_size,
                "-i",
                &display,
                "-frames:v",
                "1",
                "-f",
                "image2pipe",
                "-vcodec",
                "png",
                "pipe:1",
            ])
            .output()
            .expect("launch ffmpeg X11 display capture");
        assert!(
            output.status.success() && !output.stdout.is_empty(),
            "ffmpeg X11 display capture failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = (width, height);
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

    const CURSOR_X: f64 = 200.0;
    const CURSOR_Y: f64 = 150.0;

    #[test]
    fn accepts_colocated_pointer_and_badge_at_1x() {
        assert_colocated_overlay(400, 300, 400.0, 300.0, (211, 161));
    }

    #[test]
    fn accepts_colocated_pointer_and_badge_at_2x() {
        assert_colocated_overlay(800, 600, 400.0, 300.0, (423, 323));
    }

    #[test]
    fn accepts_colocated_pointer_and_badge_at_fractional_unequal_scale() {
        assert_colocated_overlay(500, 525, 400.0, 300.0, (264, 282));
    }

    #[test]
    fn rejects_meaningfully_desynchronized_pointer() {
        let (baseline, mut overlay, regions) = oracle_images(500, 525, 400.0, 300.0);
        paint_badge(&mut overlay, regions);
        paint_changed_rect_i64(
            &mut overlay,
            regions.anchor.0 + (30.0 * regions.scale_x).round() as i64,
            regions.anchor.1 - 2,
            4,
            4,
        );

        let failure = std::panic::catch_unwind(|| {
            assert_cursor_and_badge_pixels_changed(
                &baseline, &overlay, CURSOR_X, CURSOR_Y, 400.0, 300.0,
            );
        });
        assert!(
            failure.is_err(),
            "desynchronized pointer unexpectedly passed"
        );
    }

    #[test]
    fn rejects_meaningfully_desynchronized_badge() {
        let (baseline, mut overlay, regions) = oracle_images(800, 600, 400.0, 300.0);
        paint_pointer(&mut overlay, regions);
        paint_changed_rect_i64(
            &mut overlay,
            regions.badge_left.x0 + 2,
            regions.badge_left.y1 + 8,
            6,
            4,
        );

        let failure = std::panic::catch_unwind(|| {
            assert_cursor_and_badge_pixels_changed(
                &baseline, &overlay, CURSOR_X, CURSOR_Y, 400.0, 300.0,
            );
        });
        assert!(failure.is_err(), "desynchronized badge unexpectedly passed");
    }

    #[test]
    fn badge_cannot_satisfy_pointer_oracle_at_supported_scales() {
        for (width, height, logical_width, logical_height) in [
            (400, 300, 400.0, 300.0),
            (800, 600, 400.0, 300.0),
            (500, 525, 400.0, 300.0),
        ] {
            let (baseline, mut badge_only, regions) =
                oracle_images(width, height, logical_width, logical_height);
            assert!(regions.pointer.y1 < regions.badge_left.y0);
            paint_badge(&mut badge_only, regions);

            let failure = std::panic::catch_unwind(|| {
                assert_cursor_and_badge_pixels_changed(
                    &baseline,
                    &badge_only,
                    CURSOR_X,
                    CURSOR_Y,
                    logical_width,
                    logical_height,
                );
            });
            assert!(failure.is_err(), "badge-only overlay unexpectedly passed");
        }
    }

    fn assert_colocated_overlay(
        width: u32,
        height: u32,
        logical_width: f64,
        logical_height: f64,
        expected_anchor: (i64, i64),
    ) {
        let (baseline, mut overlay, regions) =
            oracle_images(width, height, logical_width, logical_height);
        assert_eq!(regions.anchor, expected_anchor);
        assert!(regions.pointer.y1 < regions.badge_left.y0);
        paint_pointer(&mut overlay, regions);
        paint_badge(&mut overlay, regions);

        assert_cursor_and_badge_pixels_changed(
            &baseline,
            &overlay,
            CURSOR_X,
            CURSOR_Y,
            logical_width,
            logical_height,
        );
    }

    fn oracle_images(
        width: u32,
        height: u32,
        logical_width: f64,
        logical_height: f64,
    ) -> (RgbaImage, RgbaImage, CursorOracleRegions) {
        let baseline = RgbaImage::new(width, height);
        let overlay = baseline.clone();
        let regions = cursor_oracle_regions(
            width,
            height,
            CURSOR_X,
            CURSOR_Y,
            logical_width,
            logical_height,
        );
        (baseline, overlay, regions)
    }

    fn paint_pointer(image: &mut RgbaImage, regions: CursorOracleRegions) {
        paint_changed_rect_i64(image, regions.anchor.0 - 2, regions.anchor.1 - 2, 4, 4);
    }

    fn paint_badge(image: &mut RgbaImage, regions: CursorOracleRegions) {
        paint_changed_rect_i64(
            image,
            regions.badge_left.x0 + 2,
            regions.badge_left.y0 + 2,
            6,
            4,
        );
    }

    fn paint_changed_rect_i64(image: &mut RgbaImage, x: i64, y: i64, width: u32, height: u32) {
        let x = u32::try_from(x).expect("test rectangle x");
        let y = u32::try_from(y).expect("test rectangle y");
        paint_changed_rect(image, x, y, x + width, y + height);
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
