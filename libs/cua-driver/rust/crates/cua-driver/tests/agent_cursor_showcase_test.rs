//! Cross-platform semantic cursor showcase for release evidence.

use std::time::Duration;

use cua_driver_testkit::e2e::{
    execute_case, recording_evidence, CaseSpec, Delivery, DriverRoute, Evidence, Observation,
    Scope, Targeting,
};
use cua_driver_testkit::{Driver, McpDriver};

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
        Vec::new(),
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

        let screen = driver.call("get_screen_size", serde_json::json!({}));
        assert!(
            !screen.is_error(),
            "get_screen_size failed: {}",
            screen.text()
        );
        let width = screen.structured()["width"].as_f64().expect("screen width");
        let height = screen.structured()["height"]
            .as_f64()
            .expect("screen height");
        assert!(
            width >= 640.0 && height >= 480.0,
            "showcase requires a normal desktop, got {width}x{height}"
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
                "y": center_y - 80.0,
                "scope": "desktop"
            }),
        );
        settle(900);

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

        Observation::delivered(Vec::new(), Evidence::default())
    });
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
