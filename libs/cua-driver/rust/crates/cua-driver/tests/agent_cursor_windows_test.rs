//! Windows agent-cursor rendering and desktop-side-effect contract.

#![cfg(target_os = "windows")]

use std::ffi::OsStr;
use std::net::TcpListener;
use std::os::windows::ffi::OsStrExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cua_driver_testkit::e2e::{
    execute_case, recording_evidence, CaseSpec, Delivery, DriverRoute, Evidence, Observation,
    OracleKind, Scope, Targeting,
};
use cua_driver_testkit::observer::TargetWindow;
use cua_driver_testkit::sentinel::ForegroundSentinel;
use cua_driver_testkit::{ax, harness_app, spawn_in_job, Driver, McpDriver};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetCursorPos, GetForegroundWindow, GetWindow, GetWindowLongPtrW, GetWindowRect,
    GWL_EXSTYLE, GW_HWNDPREV, WS_EX_TOPMOST,
};

#[test]
#[ignore]
fn agent_cursor_overlay_obeys_untargeted_and_targeted_z_order() {
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
        ],
    );
    execute_case(case, |evidence| {
        let mut driver = McpDriver::spawn_named_with_overlay("windows-desktop-agent-cursor-px")
            .expect("required source-built driver with cursor overlay did not start");
        *evidence = recording_evidence(driver.recording_dir());
        let target = launch_wpf_window(&mut driver);
        bring_to_front(&mut driver, target);
        driver.start_behavior_recording();
        let (x, y) = window_center(target.native_id);
        let cursor_id = "windows-agent-cursor-e2e";
        let foreground_before = unsafe { GetForegroundWindow() };
        assert_eq!(foreground_before.0 as u64, target.native_id);
        let real_cursor_before = real_cursor_position();

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
                serde_json::json!({
                    "x": x,
                    "y": y,
                    "cursor_id": cursor_id
                }),
            ),
        ] {
            let response = driver.call(tool, arguments);
            assert!(!response.is_error(), "{tool} failed: {}", response.text());
        }
        std::thread::sleep(Duration::from_millis(350));

        let overlay = wait_for_overlay();
        assert_eq!(
            unsafe { GetForegroundWindow() },
            foreground_before,
            "move-only cursor stole focus"
        );
        assert_eq!(
            real_cursor_position(),
            real_cursor_before,
            "move-only agent cursor moved the real pointer"
        );
        assert_not_topmost(overlay);
        assert!(
            window_is_above(overlay, HWND(target.native_id as *mut _)),
            "move-only overlay did not reach the top of the ordinary band"
        );

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

        // Capture the target while it is still foreground. Electron may prune
        // parts of its UIA tree once another full-size window covers it, but
        // the snapshot remains a stable handle for the later background click.
        let target_state = wait_for_window_text(&mut driver, target, "btn-increment");
        let button_index = ax::element_index_by_id(target_state.tree_text(), "btn-increment")
            .unwrap_or_else(|| {
                panic!(
                    "target increment button missing from foreground UIA snapshot: {}",
                    target_state.tree_text()
                )
            });
        assert!(target_state.tree_text().contains("counter=0"));

        // A targeted cursor remains exactly above its target while an
        // independent ordinary foreground window stays above both. This
        // guards the pre-existing PinAbove semantics while exercising the new
        // no-target branch in the same hosted desktop.
        let (_foreground_profile, foreground) = launch_normal_window(&mut driver, "foreground");
        bring_to_front(&mut driver, foreground);
        assert!(
            window_is_above(
                HWND(foreground.native_id as *mut _),
                HWND(target.native_id as *mut _)
            ),
            "independent foreground window did not cover target"
        );
        let targeted = driver.call(
            "click",
            serde_json::json!({
                "pid": target.pid,
                "window_id": target.native_id,
                "element_index": button_index,
                "snapshot_id": target_state.snapshot_id(),
                "delivery_mode": "background",
                "cursor_id": cursor_id
            }),
        );
        assert!(
            !targeted.is_error(),
            "targeted background click failed: {}",
            targeted.text()
        );
        let target_state_after = wait_for_window_text(&mut driver, target, "counter=1");
        assert!(
            target_state_after.tree_text().contains("counter=1"),
            "targeted click did not change the background target: {}",
            target_state_after.tree_text()
        );
        let foreground_state_after = window_state(&mut driver, foreground);
        assert!(
            foreground_state_after.tree_text().contains("counter=0"),
            "targeted click leaked into the independent foreground window: {}",
            foreground_state_after.tree_text()
        );
        wait_until(
            Duration::from_secs(2),
            "target-bound overlay z-order",
            || {
                window_is_above(overlay, HWND(target.native_id as *mut _))
                    && window_is_above(HWND(foreground.native_id as *mut _), overlay)
            },
        );
        assert_eq!(
            unsafe { GetForegroundWindow().0 as u64 },
            foreground.native_id,
            "target-bound overlay or action stole foreground"
        );
        assert_not_topmost(overlay);

        let disabled = driver.call(
            "set_agent_cursor_enabled",
            serde_json::json!({"enabled": false, "cursor_id": cursor_id}),
        );
        assert!(
            !disabled.is_error(),
            "failed to disable agent cursor: {}",
            disabled.text()
        );
        let passed = vec![
            OracleKind::Pixels,
            OracleKind::Focus,
            OracleKind::ZOrder,
            OracleKind::Cursor,
        ];
        Observation::delivered(passed, Evidence::default())
    });
}

#[test]
#[ignore]
fn agent_cursor_move_does_not_leak_input() {
    let case = CaseSpec::delivered(
        "windows-desktop-agent-cursor-no-input-leak",
        "desktop",
        "win32",
        "agent_cursor",
        Targeting::Px,
        Delivery::NotApplicable,
        Scope::Desktop,
        DriverRoute::WindowsOverlay,
        vec![
            OracleKind::Focus,
            OracleKind::ZOrder,
            OracleKind::Cursor,
            OracleKind::NoLeakedInput,
        ],
    );
    execute_case(case, |evidence| {
        let mut driver =
            McpDriver::spawn_named_with_overlay("windows-desktop-agent-cursor-no-input-leak")
                .expect("required source-built driver with cursor overlay did not start");
        *evidence = recording_evidence(driver.recording_dir());
        let sentinel = ForegroundSentinel::launch(&mut driver);
        driver.start_behavior_recording();
        let screen = driver.call("get_screen_size", serde_json::json!({}));
        assert!(
            !screen.is_error(),
            "get_screen_size failed: {}",
            screen.text()
        );
        let x = screen.structured()["width"].as_f64().unwrap_or(0.0) / 2.0;
        let y = screen.structured()["height"].as_f64().unwrap_or(0.0) / 2.0;
        let cursor_id = "windows-agent-cursor-no-input-leak";
        let (_, passed) = sentinel
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
                        serde_json::json!({
                            "x": x,
                            "y": y,
                            "cursor_id": cursor_id
                        }),
                    ),
                ] {
                    let response = driver.call(tool, arguments);
                    assert!(!response.is_error(), "{tool} failed: {}", response.text());
                }
                std::thread::sleep(Duration::from_millis(350));
            })
            .unwrap_or_else(|error| panic!("agent cursor disturbed the real desktop: {error}"));
        assert_required_background_oracles(&passed);
        Observation::delivered(passed, Evidence::default())
    });
}

fn window_state(driver: &mut McpDriver, target: TargetWindow) -> cua_driver_testkit::ToolResponse {
    let response = driver.call(
        "get_window_state",
        serde_json::json!({
            "pid": target.pid,
            "window_id": target.native_id,
            "capture_mode": "ax"
        }),
    );
    assert!(
        !response.is_error(),
        "window snapshot failed: {}",
        response.text()
    );
    response
}

fn wait_for_window_text(
    driver: &mut McpDriver,
    target: TargetWindow,
    expected: &str,
) -> cua_driver_testkit::ToolResponse {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let state = window_state(driver, target);
        if state.tree_text().contains(expected) {
            return state;
        }
        assert!(
            Instant::now() < deadline,
            "window state did not contain {expected:?}: {}",
            state.tree_text()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
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

fn launch_normal_window(driver: &mut McpDriver, label: &str) -> (tempfile::TempDir, TargetWindow) {
    let executable = harness_app("harness-electron", "CuaTestHarness.Electron.exe");
    assert!(
        executable.exists(),
        "Electron harness missing at {}; run the Windows harness build first",
        executable.display()
    );
    let profile = tempfile::Builder::new()
        .prefix(&format!("cua-agent-cursor-{label}-"))
        .tempdir()
        .expect("create Electron profile");
    let port = TcpListener::bind(("127.0.0.1", 0))
        .and_then(|listener| listener.local_addr())
        .expect("allocate Electron CDP port")
        .port();
    let mut command = Command::new(executable);
    command
        .env("CUA_ELECTRON_CDP_PORT", port.to_string())
        .env("CUA_E2E_USER_DATA_DIR", profile.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let child = spawn_in_job(&mut command).expect("launch ordinary Electron harness window");
    let pid = child.id();
    driver.reaper().push(child);
    let (native_id, _) = driver
        .find_window(pid as i64, "CuaTestHarness Electron")
        .expect("ordinary Electron harness window did not appear");
    (profile, TargetWindow { pid, native_id })
}

fn launch_wpf_window(driver: &mut McpDriver) -> TargetWindow {
    let executable = harness_app("harness-wpf", "CuaTestHarness.Wpf.exe");
    assert!(
        executable.exists(),
        "WPF harness missing at {}; run the Windows harness build first",
        executable.display()
    );
    let mut command = Command::new(executable);
    command.stdout(Stdio::null()).stderr(Stdio::null());
    let child = spawn_in_job(&mut command).expect("launch ordinary WPF harness window");
    let pid = child.id();
    driver.reaper().push(child);
    let (native_id, _) = driver
        .find_window(pid as i64, "CuaTestHarness WPF")
        .expect("ordinary WPF harness window did not appear");
    TargetWindow { pid, native_id }
}

fn bring_to_front(driver: &mut McpDriver, target: TargetWindow) {
    let response = driver.call(
        "bring_to_front",
        serde_json::json!({"pid": target.pid, "window_id": target.native_id}),
    );
    assert!(
        !response.is_error(),
        "bring_to_front failed: {}",
        response.text()
    );
    wait_until(
        Duration::from_secs(2),
        "ordinary window foreground",
        || unsafe { GetForegroundWindow().0 as u64 == target.native_id },
    );
}

fn wait_for_overlay() -> HWND {
    let class_name: Vec<u16> = OsStr::new("Cua.AgentCursorOverlay\0")
        .encode_wide()
        .collect();
    let mut overlay = HWND::default();
    wait_until(
        Duration::from_secs(2),
        "agent cursor overlay window",
        || {
            overlay = unsafe { FindWindowW(PCWSTR(class_name.as_ptr()), PCWSTR::null()) }
                .unwrap_or_default();
            !overlay.0.is_null()
        },
    );
    overlay
}

fn window_center(window_id: u64) -> (f64, f64) {
    let mut rect = RECT::default();
    unsafe { GetWindowRect(HWND(window_id as *mut _), &mut rect) }
        .expect("read ordinary foreground window bounds");
    (
        f64::from(rect.left + (rect.right - rect.left) / 2),
        f64::from(rect.top + (rect.bottom - rect.top) / 2),
    )
}

fn real_cursor_position() -> (i32, i32) {
    let mut point = POINT::default();
    unsafe { GetCursorPos(&mut point) }.expect("read real cursor position");
    (point.x, point.y)
}

fn assert_not_topmost(window: HWND) {
    let ex_style = unsafe { GetWindowLongPtrW(window, GWL_EXSTYLE) } as u32;
    assert_eq!(
        ex_style & WS_EX_TOPMOST.0,
        0,
        "agent cursor overlay remained persistently topmost"
    );
}

fn window_is_above(upper: HWND, lower: HWND) -> bool {
    let mut cursor = lower;
    loop {
        cursor = unsafe { GetWindow(cursor, GW_HWNDPREV) }.unwrap_or_default();
        if cursor.0.is_null() {
            return false;
        }
        if cursor == upper {
            return true;
        }
    }
}

fn wait_until(timeout: Duration, description: &str, mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !predicate() {
        assert!(
            Instant::now() < deadline,
            "timed out after {timeout:?} waiting for {description}"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}
