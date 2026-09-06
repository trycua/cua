//! Focused native Hyprland regression; supplements, never replaces, the
//! canonical desktop matrix. Run only in a disposable Hyprland desktop with
//! the repository GTK3 fixture and exact-candidate Driver installed.
#![cfg(target_os = "linux")]

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cua_driver_testkit::e2e::OracleKind;
use cua_driver_testkit::observer::{DesktopObserver, NativeObserver, TargetWindow, TargetZ};
use cua_driver_testkit::{harness_app, spawn_in_job, Driver, McpDriver, ToolResponse};
use serde_json::json;

fn launch(driver: &mut McpDriver) -> TargetWindow {
    let child = spawn_in_job(
        Command::new(harness_app("harness-gtk3", "CuaTestHarness.Gtk3"))
            .stdout(Stdio::null())
            .stderr(Stdio::inherit()),
    )
    .expect("launch repository GTK3 fixture");
    let pid = child.id();
    driver.reaper().push(child);
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let windows = driver.call("list_windows", json!({"pid": pid}));
        if let Some(window) = windows.structured()["windows"].as_array().and_then(|ws| {
            ws.iter().find(|w| {
                w["pid"].as_u64() == Some(u64::from(pid))
                    && w["title"].as_str() == Some("CuaTestHarness GTK3")
            })
        }) {
            return TargetWindow {
                pid,
                native_id: window["window_id"].as_u64().unwrap(),
            };
        }
        assert!(
            Instant::now() < deadline,
            "GTK3 fixture did not map: {}",
            windows.text()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn snapshot(driver: &mut McpDriver, target: TargetWindow) -> ToolResponse {
    let state = driver.call(
        "get_window_state",
        json!({"pid": target.pid, "window_id": target.native_id}),
    );
    assert!(!state.is_error(), "{}", state.text());
    assert!(
        state.structured().get("screenshot_error").is_none(),
        "{}",
        state.structured()
    );
    assert!(state.structured()["screenshot_width"].as_u64().unwrap_or(0) > 0);
    assert!(
        state.tree_text().contains("HARNESS_TEXT_MARKER_v1"),
        "{}",
        state.tree_text()
    );
    state
}

#[test]
#[ignore = "requires disposable native Hyprland, GTK3 fixture, and exact-candidate Driver"]
fn native_window_capture_and_background_ax_keep_exact_identity() {
    assert!(std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some());
    assert_eq!(
        std::env::var("CUA_DRIVER_RS_ENABLE_WAYLAND").as_deref(),
        Ok("1")
    );
    let mut driver = McpDriver::spawn_named_with_overlay("hyprland-native-observation")
        .expect("exact-candidate Driver is required");
    let target = launch(&mut driver);
    snapshot(&mut driver, target);
    let foreground = launch(&mut driver);
    snapshot(&mut driver, foreground);
    std::thread::sleep(Duration::from_millis(500));

    let before = snapshot(&mut driver, target);
    let token = before.structured()["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|element| element["label"].as_str() == Some("btn-increment"))
        .and_then(|element| element["element_token"].as_str())
        .expect("exact-window increment token")
        .to_owned();
    let mut observer = DesktopObserver::new(NativeObserver::new(), target);
    let baseline = observer.snapshot().expect("independent Hyprland observer");
    assert_eq!(baseline.foreground, Some(foreground.native_id));
    assert!(matches!(
        baseline.target_z,
        TargetZ::BackgroundVisible | TargetZ::BackgroundOccluded
    ));
    let (clicked, delta) = observer
        .observe(
            &[OracleKind::Focus, OracleKind::ZOrder, OracleKind::Cursor],
            || driver.call("click", json!({"pid": target.pid, "element_token": token})),
        )
        .expect("independent action observation");
    assert!(!clicked.is_error(), "{}", clicked.text());
    delta.ensure_supported().unwrap();
    assert!(delta.violations().is_empty(), "{:?}", delta.violations());
    assert_eq!(delta.before.cursor_pos, delta.after.cursor_pos);
    let after = snapshot(&mut driver, target);
    assert!(
        after.tree_text().contains("counter=1"),
        "{}",
        after.tree_text()
    );
    assert!(snapshot(&mut driver, foreground)
        .tree_text()
        .contains("counter=0"));

    // An explicit owner mismatch and a token paired with a sibling address
    // must refuse before any accessibility actuation.
    let wrong_owner = driver.call(
        "get_window_state",
        json!({
            "pid": foreground.pid, "window_id": target.native_id,
        }),
    );
    assert!(wrong_owner.is_error());
    let fresh = snapshot(&mut driver, target);
    let token = fresh.structured()["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|element| element["label"].as_str() == Some("btn-increment"))
        .and_then(|element| element["element_token"].as_str())
        .unwrap();
    let conflict = driver.call(
        "click",
        json!({
            "pid": target.pid, "window_id": foreground.native_id, "element_token": token,
        }),
    );
    assert!(conflict.is_error());
    assert!(snapshot(&mut driver, target)
        .tree_text()
        .contains("counter=1"));
}
