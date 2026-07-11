//! Windows agent-cursor rendering and desktop-side-effect contract.

#![cfg(target_os = "windows")]

use std::time::Duration;

use cua_driver_testkit::e2e::{
    execute_case, recording_evidence, CaseSpec, Delivery, DriverRoute, Evidence, Observation,
    OracleKind, Scope, Targeting,
};
use cua_driver_testkit::sentinel::ForegroundSentinel;
use cua_driver_testkit::{Driver, McpDriver};

#[test]
#[ignore]
fn agent_cursor_overlay_is_visible_without_moving_real_cursor() {
    let case = CaseSpec::delivered(
        "windows-desktop-agent-cursor-px",
        "desktop",
        "win32",
        "agent_cursor",
        Targeting::Px,
        Delivery::NotApplicable,
        Scope::Desktop,
        DriverRoute::WindowsOverlay,
        vec![
            OracleKind::Pixels,
            OracleKind::Focus,
            OracleKind::ZOrder,
            OracleKind::Cursor,
            OracleKind::NoLeakedInput,
        ],
    );
    execute_case(case, |evidence| {
        let mut driver = McpDriver::spawn_named("windows-desktop-agent-cursor-px")
            .expect("required source-built driver did not start");
        *evidence = recording_evidence(driver.recording_dir());
        let sentinel = ForegroundSentinel::launch(&mut driver);
        let screen = driver.call("get_screen_size", serde_json::json!({}));
        assert!(
            !screen.is_error(),
            "get_screen_size failed: {}",
            screen.text()
        );
        let width = screen.structured()["width"].as_f64().unwrap_or(0.0);
        let height = screen.structured()["height"].as_f64().unwrap_or(0.0);
        assert!(
            width >= 80.0 && height >= 80.0,
            "invalid screen size {width}x{height}"
        );
        let (x, y) = (width / 2.0, height / 2.0);
        let cursor_id = "windows-agent-cursor-e2e";

        let (_, mut passed) = sentinel
            .observe_desktop(|| {
                for (tool, arguments) in [
                    (
                        "set_agent_cursor_enabled",
                        serde_json::json!({"enabled": true, "cursor_id": cursor_id}),
                    ),
                    (
                        "set_agent_cursor_motion",
                        serde_json::json!({
                            "cursor_id": cursor_id,
                            "glide_duration_ms": 100,
                            "idle_hide_ms": 0
                        }),
                    ),
                    (
                        "move_cursor",
                        serde_json::json!({"x": x, "y": y, "cursor_id": cursor_id}),
                    ),
                ] {
                    let response = driver.call(tool, arguments);
                    assert!(!response.is_error(), "{tool} failed: {}", response.text());
                }
                std::thread::sleep(Duration::from_millis(350));
            })
            .unwrap_or_else(|error| panic!("agent cursor disturbed the real desktop: {error}"));
        assert_required_background_oracles(&passed);

        let png = platform_windows::capture::screenshot_display_bytes()
            .expect("screenshot_display_bytes failed");
        let image = image::load_from_memory(&png)
            .expect("decode display screenshot")
            .to_rgba8();
        let (image_width, image_height) = image.dimensions();
        let half = 20u32;
        let center_x = x
            .round()
            .clamp(0.0, f64::from(image_width.saturating_sub(1))) as u32;
        let center_y = y
            .round()
            .clamp(0.0, f64::from(image_height.saturating_sub(1))) as u32;
        let x0 = center_x.saturating_sub(half);
        let x1 = center_x.saturating_add(half).min(image_width);
        let y0 = center_y.saturating_sub(half);
        let y1 = center_y.saturating_add(half).min(image_height);
        let visible_pixels = (y0..y1)
            .flat_map(|pixel_y| (x0..x1).map(move |pixel_x| (pixel_x, pixel_y)))
            .filter(|(pixel_x, pixel_y)| {
                let [red, green, blue, alpha] = image.get_pixel(*pixel_x, *pixel_y).0;
                if alpha < 10 {
                    return false;
                }
                let brightness = u32::from(red) + u32::from(green) + u32::from(blue);
                let saturation = u32::from(red.max(green).max(blue) - red.min(green).min(blue));
                brightness > 60 && (saturation > 30 || brightness > 600)
            })
            .count();
        assert!(
            visible_pixels >= 5,
            "agent cursor not visible at ({x:.0},{y:.0}): only {visible_pixels} qualifying pixels"
        );

        let disabled = driver.call(
            "set_agent_cursor_enabled",
            serde_json::json!({"enabled": false, "cursor_id": cursor_id}),
        );
        assert!(
            !disabled.is_error(),
            "failed to disable agent cursor: {}",
            disabled.text()
        );
        passed.push(OracleKind::Pixels);
        Observation::delivered(passed, Evidence::default())
    });
}

fn assert_required_background_oracles(passed: &[OracleKind]) {
    for required in [
        OracleKind::Focus,
        OracleKind::ZOrder,
        OracleKind::Cursor,
        OracleKind::NoLeakedInput,
    ] {
        assert!(
            passed.contains(&required),
            "agent cursor test omitted required {required:?} oracle"
        );
    }
}
