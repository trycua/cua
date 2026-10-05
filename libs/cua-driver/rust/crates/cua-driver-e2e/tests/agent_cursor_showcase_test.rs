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
// MoveTo anchors the artwork `POINTER_ANCHOR_OFFSET` points from the requested
// coordinate at 45 degrees so the cursor tip lands on it. Each axis moves by
// that offset over sqrt(2), and the session badge follows the anchor.
const CURSOR_ANCHOR_OFFSET_PER_AXIS: f64 =
    cursor_overlay::POINTER_ANCHOR_OFFSET * std::f64::consts::FRAC_1_SQRT_2;
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

        // The cursor and pill keep resting there, yet the capture the Driver
        // hands the agent must not contain them (D-WL5).
        assert_driver_capture_excludes_overlay(
            &mut driver,
            &baseline,
            &cursor_frame,
            center_x - 180.0,
            center_y - 80.0,
            width,
            height,
        );

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

const KEYBOARD_FIRST_CELL_ID: &str = "window-agent-cursor-keyboard-first-placement";
/// Largest distance, per axis and in screen units, between the reported cursor
/// point and the centre of the window's `list_windows` bounds. Each platform
/// reads the centre from its own native geometry (Windows `GetWindowRect`
/// includes the invisible resize border that the DWM frame bounds leave out),
/// so the two can differ by a few units; a cursor left at the pointer, at a
/// stale session point, or unplaced is far outside it.
const WINDOW_CENTRE_TOLERANCE: f64 = 12.0;

/// A named session whose first action is an untargeted `press_key` shows its
/// agent cursor on the target window before the key is delivered: no element,
/// no pixel target, and no remembered position leave the shared keyboard
/// placement policy with the window centre.
#[test]
#[ignore]
fn keyboard_first_session_places_its_cursor_on_the_target_window() {
    let fixture = native_fixture();
    let case = CaseSpec::delivered(
        KEYBOARD_FIRST_CELL_ID,
        fixture.toolkit,
        fixture.toolkit,
        "press_key_cursor_placement",
        Targeting::NotApplicable,
        Delivery::Foreground,
        Scope::Window,
        platform_route(),
        vec![OracleKind::Protocol],
    );
    execute_case(case, |evidence| {
        let mut driver = spawn_driver_named(KEYBOARD_FIRST_CELL_ID);
        *evidence = recording_evidence(driver.recording_dir());
        let (pid, window_id) = launch_native_fixture(&mut driver, &fixture);

        // A per-run name keeps a long-lived daemon from handing this row a
        // cursor position that an earlier run left behind.
        let session = format!("Keyboard-first cursor {}", std::process::id());
        call_ok(
            &mut driver,
            "start_session",
            serde_json::json!({"session": session}),
        );
        call_ok(
            &mut driver,
            "set_agent_cursor_enabled",
            serde_json::json!({"session": session, "enabled": true}),
        );
        let before = agent_cursor_state(&mut driver, &session);
        assert!(
            before["position"].is_null(),
            "the session cursor must start unplaced so press_key is its first placement: {before}"
        );

        driver.start_behavior_recording();
        call_ok(
            &mut driver,
            "press_key",
            serde_json::json!({
                "session": session,
                "pid": pid,
                "window_id": window_id,
                "key": "escape",
                "delivery_mode": "foreground"
            }),
        );

        let after = agent_cursor_state(&mut driver, &session);
        assert_eq!(
            after["enabled"], true,
            "keyboard-first cursor is hidden: {after}"
        );
        let (Some(x), Some(y)) = (
            after["position"]["x"].as_f64(),
            after["position"]["y"].as_f64(),
        ) else {
            panic!("a keyboard-first press_key left the session cursor unplaced: {after}");
        };
        let (centre_x, centre_y) = window_centre(&mut driver, pid, window_id);
        assert!(
            (x - centre_x).abs() <= WINDOW_CENTRE_TOLERANCE
                && (y - centre_y).abs() <= WINDOW_CENTRE_TOLERANCE,
            "keyboard-first cursor landed at ({x:.1}, {y:.1}), expected the target window \
             centre ({centre_x:.1}, {centre_y:.1}) within {WINDOW_CENTRE_TOLERANCE}: {after}"
        );

        call_ok(
            &mut driver,
            "end_session",
            serde_json::json!({"session": session}),
        );
        Observation::delivered(vec![OracleKind::Protocol], Evidence::default())
    });
}

fn agent_cursor_state(driver: &mut McpDriver, session: &str) -> serde_json::Value {
    let response = driver.call(
        "get_agent_cursor_state",
        serde_json::json!({"session": session}),
    );
    assert!(
        !response.is_error(),
        "get_agent_cursor_state failed: {}",
        response.text()
    );
    response.structured().clone()
}

/// Centre of the target window's `list_windows` bounds.
fn window_centre(driver: &mut McpDriver, pid: u32, window_id: u64) -> (f64, f64) {
    let windows = driver.call("list_windows", serde_json::json!({"pid": pid}));
    let window = windows.structured()["windows"]
        .as_array()
        .and_then(|windows| {
            windows
                .iter()
                .find(|window| window["window_id"].as_u64() == Some(window_id))
        })
        .unwrap_or_else(|| panic!("target window {window_id} is not listed: {}", windows.raw))
        .clone();
    let bounds = &window["bounds"];
    let (Some(x), Some(y), Some(width), Some(height)) = (
        bounds["x"].as_f64(),
        bounds["y"].as_f64(),
        bounds["width"].as_f64(),
        bounds["height"].as_f64(),
    ) else {
        panic!("target window has no bounds: {window}");
    };
    assert!(
        width > 0.0 && height > 0.0,
        "target window is empty: {window}"
    );
    (x + width / 2.0, y + height / 2.0)
}

/// The repo-local native harness the platform's native lane stages.
struct NativeFixture {
    toolkit: &'static str,
    executable: std::path::PathBuf,
    title: &'static str,
}

#[cfg(target_os = "macos")]
fn native_fixture() -> NativeFixture {
    let app = std::env::var("HARNESS_APPKIT_APP")
        .map(std::path::PathBuf::from)
        .ok()
        .filter(|path| path.exists())
        .unwrap_or_else(|| {
            cua_driver_testkit::harness_app("harness-appkit", "CuaTestHarness.AppKit.app")
        });
    NativeFixture {
        toolkit: "appkit",
        executable: app.join("Contents/MacOS/CuaTestHarness.AppKit"),
        title: "CuaTestHarness AppKit",
    }
}

#[cfg(target_os = "windows")]
fn native_fixture() -> NativeFixture {
    NativeFixture {
        toolkit: "wpf",
        executable: cua_driver_testkit::harness_app("harness-wpf", "CuaTestHarness.Wpf.exe"),
        title: "CuaTestHarness WPF",
    }
}

#[cfg(target_os = "linux")]
fn native_fixture() -> NativeFixture {
    NativeFixture {
        toolkit: "gtk3",
        executable: std::env::var("HARNESS_GTK3_EXE")
            .map(std::path::PathBuf::from)
            .ok()
            .filter(|path| path.exists())
            .unwrap_or_else(|| {
                cua_driver_testkit::harness_app("harness-gtk3", "CuaTestHarness.Gtk3")
            }),
        title: "CuaTestHarness GTK3",
    }
}

/// Launch the fixture and return the pid and id of its main window.
fn launch_native_fixture(driver: &mut McpDriver, fixture: &NativeFixture) -> (u32, u64) {
    assert!(
        fixture.executable.exists(),
        "required {} harness is missing at {:?}; run the fixture build",
        fixture.toolkit,
        fixture.executable
    );
    let listed_window_ids = |driver: &mut McpDriver| -> std::collections::HashSet<u64> {
        driver
            .call("list_windows", serde_json::json!({}))
            .structured()["windows"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|window| window["window_id"].as_u64())
            .collect()
    };
    let earlier_windows = listed_window_ids(driver);
    let child = cua_driver_testkit::spawn_in_job(
        std::process::Command::new(&fixture.executable)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null()),
    )
    .unwrap_or_else(|error| panic!("launch {} harness: {error}", fixture.toolkit));
    driver.reaper().push(child);

    // Take the new window with the fixture title, whatever process owns it:
    // a toolkit launcher can hand the window to another process, and an
    // earlier fixture window must not be mistaken for this one.
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let windows = driver.call("list_windows", serde_json::json!({}));
        let found = windows.structured()["windows"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|window| {
                window["title"]
                    .as_str()
                    .is_some_and(|title| title.contains(fixture.title))
                    && window["window_id"]
                        .as_u64()
                        .is_some_and(|id| !earlier_windows.contains(&id))
            })
            .and_then(|window| {
                Some((
                    u32::try_from(window["pid"].as_u64()?).ok()?,
                    window["window_id"].as_u64()?,
                ))
            });
        if let Some((pid, window_id)) =
            found.filter(|(pid, window_id)| *pid != 0 && *window_id != 0)
        {
            driver.reaper().track_pid(pid);
            #[cfg(target_os = "macos")]
            while !cua_driver_testkit::observer::macos::application_presented(pid) {
                assert!(
                    std::time::Instant::now() < deadline,
                    "AppKit harness pid {pid} did not finish launching"
                );
                settle(50);
            }
            settle(500);
            return (pid, window_id);
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{} harness window never appeared",
            fixture.toolkit
        );
        settle(250);
    }
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

/// Check the Driver's own desktop capture against the external cursor frame:
/// every pixel the overlay changed on screen must show the desktop there.
///
/// Native Wayland compositors draw the cursor into the captured output; there
/// the Driver must say so instead of claiming a clean capture.
fn assert_driver_capture_excludes_overlay(
    driver: &mut McpDriver,
    baseline: &RgbaImage,
    cursor_frame: &RgbaImage,
    logical_x: f64,
    logical_y: f64,
    logical_width: f64,
    logical_height: f64,
) {
    let (driver_png, _, _, report) = capture_desktop_png_with_report(driver);
    let status = report["status"].as_str().unwrap_or("missing");
    if wayland_session() {
        assert_eq!(
            status, "not_excluded",
            "a Wayland desktop capture must report that it could not exclude the overlay: \
             {report}"
        );
        return;
    }
    assert_eq!(
        status, "excluded",
        "desktop capture did not exclude the agent cursor overlay: {report}"
    );
    let driver_frame = image::load_from_memory(&driver_png)
        .expect("decode driver-owned desktop screenshot")
        .to_rgba8();
    assert_eq!(
        driver_frame.dimensions(),
        cursor_frame.dimensions(),
        "driver capture and cursor oracle are not in the same pixel frame"
    );
    let regions = cursor_oracle_regions(
        baseline.width(),
        baseline.height(),
        logical_x,
        logical_y,
        logical_width,
        logical_height,
    );
    let mut overlay_pixels = 0usize;
    let mut desktop_pixels = 0usize;
    for rect in [regions.pointer, regions.badge_left, regions.badge_right] {
        let (overlay, desktop) =
            overlay_pixels_resolved_to_desktop(baseline, cursor_frame, &driver_frame, rect);
        overlay_pixels += overlay;
        desktop_pixels += desktop;
    }
    assert!(
        overlay_pixels >= 36,
        "the cursor oracle no longer shows the overlay ({overlay_pixels} pixels)"
    );
    assert!(
        desktop_pixels * 10 >= overlay_pixels * 9,
        "driver desktop capture still shows the agent cursor overlay: only {desktop_pixels} \
         of {overlay_pixels} overlay pixels show the desktop; report={report}"
    );
}

/// Pixels in `rect` the overlay changed (baseline vs cursor frame), and how
/// many of those the driver capture shows closer to the desktop than to the
/// overlay. "Closer" keeps the check independent of small color differences
/// between capture pipelines.
fn overlay_pixels_resolved_to_desktop(
    baseline: &RgbaImage,
    cursor_frame: &RgbaImage,
    driver_frame: &RgbaImage,
    rect: PixelRect,
) -> (usize, usize) {
    let distance = |a: &image::Rgba<u8>, b: &image::Rgba<u8>| -> u16 {
        a.0.iter()
            .zip(b.0.iter())
            .take(3)
            .map(|(a, b)| u16::from(a.abs_diff(*b)))
            .sum()
    };
    let x0 = rect.x0.clamp(0, i64::from(baseline.width())) as u32;
    let x1 = rect.x1.clamp(0, i64::from(baseline.width())) as u32;
    let y0 = rect.y0.clamp(0, i64::from(baseline.height())) as u32;
    let y1 = rect.y1.clamp(0, i64::from(baseline.height())) as u32;
    let mut overlay = 0;
    let mut desktop = 0;
    for pixel_y in y0..y1 {
        for pixel_x in x0..x1 {
            let before = baseline.get_pixel(pixel_x, pixel_y);
            let on_screen = cursor_frame.get_pixel(pixel_x, pixel_y);
            if distance(before, on_screen) < 80 {
                continue;
            }
            overlay += 1;
            let captured = driver_frame.get_pixel(pixel_x, pixel_y);
            if distance(captured, before) < distance(captured, on_screen) {
                desktop += 1;
            }
        }
    }
    (overlay, desktop)
}

fn wayland_session() -> bool {
    cfg!(target_os = "linux") && std::env::var_os("WAYLAND_DISPLAY").is_some()
}

fn capture_desktop_png(driver: &mut McpDriver) -> (Vec<u8>, f64, f64) {
    let (png, width, height, _) = capture_desktop_png_with_report(driver);
    (png, width, height)
}

fn capture_desktop_png_with_report(
    driver: &mut McpDriver,
) -> (Vec<u8>, f64, f64, serde_json::Value) {
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
    let report = response.structured()["agent_overlay_capture"].clone();
    (png, width, height, report)
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

    // The Driver's own desktop capture leaves the overlay out, so the oracle
    // for what a user sees comes from another capture path: a BitBlt from this
    // (different) process, which the overlay's temporary capture exclusion
    // does not apply to.
    #[cfg(target_os = "windows")]
    {
        let _ = (driver, width, height);
        platform_windows::capture::screenshot_display_bytes()
            .expect("external Windows display capture failed")
    }

    // Only the Driver holds Screen Recording permission on the macOS lanes.
    // Its per-turn recording capture deliberately keeps the overlay, so the
    // move's post-action screenshot is what a user saw.
    #[cfg(target_os = "macos")]
    {
        let _ = (width, height);
        recorded_turn_screenshot(driver, "move_cursor")
    }
}

/// The newest recorded turn's post-action screenshot for `tool`.
#[cfg(target_os = "macos")]
fn recorded_turn_screenshot(driver: &McpDriver, tool: &str) -> Vec<u8> {
    let recording_dir = driver
        .recording_dir()
        .expect("showcase recording directory")
        .to_path_buf();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let mut turns: Vec<_> = std::fs::read_dir(&recording_dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("turn-"))
            })
            .collect();
        turns.sort();
        for turn in turns.iter().rev() {
            let Ok(action) = std::fs::read(turn.join("action.json")) else {
                continue;
            };
            let Ok(action) = serde_json::from_slice::<serde_json::Value>(&action) else {
                continue;
            };
            if action["tool"].as_str() != Some(tool) {
                continue;
            }
            if let Ok(png) = std::fs::read(turn.join("after.png")) {
                return png;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no recorded {tool} turn with an after.png under {}",
            recording_dir.display()
        );
        settle(100);
    }
}

fn call_ok(driver: &mut McpDriver, tool: &str, arguments: serde_json::Value) {
    let response = driver.call(tool, arguments);
    assert!(!response.is_error(), "{tool} failed: {}", response.text());
}

fn settle(milliseconds: u64) {
    std::thread::sleep(Duration::from_millis(milliseconds));
}

fn spawn_driver() -> McpDriver {
    spawn_driver_named(CELL_ID)
}

#[cfg(target_os = "macos")]
fn spawn_driver_named(cell_id: &str) -> McpDriver {
    McpDriver::spawn_macos_daemon_proxy_named(cell_id).expect("start installed macOS daemon proxy")
}

#[cfg(not(target_os = "macos"))]
fn spawn_driver_named(cell_id: &str) -> McpDriver {
    McpDriver::spawn_named_with_overlay(cell_id)
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
