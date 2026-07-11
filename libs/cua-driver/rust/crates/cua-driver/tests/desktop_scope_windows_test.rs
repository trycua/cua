//! Harness integration test for the **desktop-scope** modality (#1968 / #2019).
//!
//! Desktop-scope is cua-driver's *foreground*, vision-only, **screen-absolute**
//! loop (the "Computer-Use 1.0" mode), the complement to the default per-window
//! background model that `harness_bg_modality_test` / `e2e_windows_bg_input_test`
//! cover. This test exercises the Windows Phase-1 actuator end-to-end against a
//! real harness app:
//!
//!   1. `set_config capture_scope=desktop` → `get_desktop_state` returns a
//!      full-display capture with true `screen_width/height` (no downscale).
//!   2. A **window-less** screen-absolute `click` / `scroll` (no pid/window_id)
//!      lands via `WindowFromPoint` while in desktop scope.
//!   3. Negative gate: the same window-less `click` under `capture_scope=window`
//!      is rejected with the structured `desktop_scope_disabled` error.
//!
//! Note: `set_config` is a *session* override — it persists for the lifetime of
//! the one MCP server we spawn here (not across separate `cua-driver call`
//! processes), which is exactly why this test drives a single long-lived server.
//!
//! All tests are `#[ignore]` (need a real desktop session). Run explicitly:
//!   cargo test -p cua-driver --test desktop_scope_windows_test -- --ignored --nocapture --test-threads=1

#![cfg(target_os = "windows")]

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cua_driver_testkit::ax;
use cua_driver_testkit::e2e::{
    execute_case, recording_evidence, CaseSpec, Delivery, DriverRoute, Evidence, Observation,
    OracleKind, Scope, Targeting,
};
use cua_driver_testkit::{harness_app, Driver, McpDriver};

/// WPF harness app (built by `tests/fixtures/build/windows.ps1`). Path mirrors
/// `shared/scenarios.json`'s `wpf.exe_relative_path`.
fn harness_wpf_exe() -> std::path::PathBuf {
    harness_app("harness-wpf", "CuaTestHarness.Wpf.exe")
}

/// Launch the WPF harness app and return its pid and native window id.
/// Skips (returns None) if the harness app isn't built.
fn launch_wpf(driver: &mut McpDriver) -> Option<(u32, u64)> {
    let exe = harness_wpf_exe();
    if !exe.exists() {
        if std::env::var_os("CUA_TEST_REQUIRE_FIXTURES").is_some() {
            panic!("required WPF harness is missing at {exe:?}");
        }
        eprintln!("[desktop-scope] WPF harness not built ({exe:?}) — skipping window-target tests");
        return None;
    }
    driver
        .reaper()
        .spawn(
            Command::new(&exe)
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
        )
        .ok()?;

    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        let r = driver.call("list_windows", serde_json::json!({}));
        if let Some(arr) = r.structured()["windows"].as_array() {
            for w in arr {
                let title = w["title"].as_str().unwrap_or("");
                if !title.contains("CuaTestHarness") {
                    continue;
                }
                let pid = w["pid"].as_u64().unwrap_or(0) as u32;
                let wid = w["window_id"].as_u64().unwrap_or(0);
                if pid != 0 && wid != 0 {
                    driver.reaper().track_pid(pid);
                    return Some((pid, wid));
                }
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    if std::env::var_os("CUA_TEST_REQUIRE_FIXTURES").is_some() {
        panic!("required WPF harness window never appeared");
    }
    eprintln!("[desktop-scope] WPF harness window never appeared — skipping");
    None
}

fn snapshot(driver: &mut McpDriver, pid: u32, wid: u64) -> cua_driver_testkit::ToolResponse {
    driver.call(
        "get_window_state",
        serde_json::json!({ "pid": pid as i64, "window_id": wid, "capture_mode": "ax" }),
    )
}

fn element_center(state: &cua_driver_testkit::ToolResponse, id: &str) -> (i64, i64) {
    let index = ax::element_index_by_id(state.tree_text(), id)
        .unwrap_or_else(|| panic!("missing WPF element {id:?}: {}", state.tree_text()));
    let element = state.structured()["elements"]
        .as_array()
        .and_then(|elements| {
            elements
                .iter()
                .find(|element| element["element_index"].as_u64() == Some(index))
        })
        .unwrap_or_else(|| panic!("WPF element {id:?} has no structured frame"));
    let frame = &element["frame"];
    (
        (frame["x"].as_f64().expect("frame x") + frame["w"].as_f64().expect("frame w") / 2.0)
            as i64,
        (frame["y"].as_f64().expect("frame y") + frame["h"].as_f64().expect("frame h") / 2.0)
            as i64,
    )
}

fn set_scope(driver: &mut McpDriver, scope: &str) {
    let r = driver.call(
        "set_config",
        serde_json::json!({ "key": "capture_scope", "value": scope }),
    );
    assert!(
        !r.is_error(),
        "set_config capture_scope={scope} failed: {}",
        r.text()
    );
    assert_eq!(
        r.structured()["capture_scope"].as_str(),
        Some(scope),
        "set_config did not report capture_scope={scope}: {}",
        r.text()
    );
}

fn run_desktop_fixture_case(
    action: &str,
    route: DriverRoute,
    test: impl FnOnce(u32, u64, &mut McpDriver),
) {
    let cell_id = format!("windows-wpf-desktop-{action}-px-foreground").replace('_', "-");
    let case = CaseSpec::delivered(
        cell_id.clone(),
        "wpf",
        "wpf",
        action,
        Targeting::Px,
        Delivery::Foreground,
        Scope::Desktop,
        route,
        vec![OracleKind::FixtureState],
    );
    execute_case(case, |evidence| {
        let mut driver =
            McpDriver::spawn_named(&cell_id).expect("required source-built driver did not start");
        *evidence = recording_evidence(driver.recording_dir());
        let (pid, wid) = launch_wpf(&mut driver).expect("required WPF harness did not launch");
        set_scope(&mut driver, "desktop");
        test(pid, wid, &mut driver);
        Observation::delivered_with_fixture_state(Vec::new())
    });
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// `get_desktop_state` in desktop scope returns a full-display capture with
/// real screen dimensions (the Session-0 `handle is invalid` case is only the
/// service-session wall; this needs a real interactive desktop).
#[test]
#[ignore]
fn desktop_scope_capture_returns_screen_dims() {
    let case = CaseSpec::delivered(
        "windows-desktop-state-px-not-applicable",
        "desktop",
        "win32",
        "get_desktop_state",
        Targeting::Px,
        Delivery::NotApplicable,
        Scope::Desktop,
        DriverRoute::WindowState,
        vec![OracleKind::Pixels],
    );
    execute_case(case, |evidence| {
        let mut driver = McpDriver::spawn_named("windows-desktop-state-px-not-applicable")
            .expect("required source-built driver did not start");
        *evidence = recording_evidence(driver.recording_dir());
        set_scope(&mut driver, "desktop");
        let response = driver.call("get_desktop_state", serde_json::json!({}));
        assert!(
            !response.is_error(),
            "get_desktop_state errored: {}",
            response.text()
        );
        let width = response.structured()["screen_width"].as_u64().unwrap_or(0);
        let height = response.structured()["screen_height"].as_u64().unwrap_or(0);
        assert!(
            width > 0 && height > 0,
            "get_desktop_state returned no screen size"
        );
        Observation::delivered(vec![OracleKind::Pixels], Evidence::default())
    });
}

/// In desktop scope, a window-less screen-absolute click + scroll succeed and
/// resolve a real window via WindowFromPoint (no pid/window_id supplied).
#[test]
#[ignore]
fn desktop_scope_windowless_click_lands_on_control() {
    run_desktop_fixture_case(
        "left_click",
        DriverRoute::WindowsSendInput,
        |pid, wid, driver| {
            let pre = snapshot(driver, pid, wid);
            let (x, y) = element_center(&pre, "border-click-target");
            let response = driver.call("click", serde_json::json!({ "x": x, "y": y }));
            assert!(
                !response.is_error(),
                "desktop click failed: {}",
                response.text()
            );
            std::thread::sleep(Duration::from_millis(400));
            let after = snapshot(driver, pid, wid);
            assert!(
                after.tree_text().contains("last_action=left_click"),
                "desktop click did not update the WPF click oracle: {}",
                after.tree_text()
            );
        },
    );
}

#[test]
#[ignore]
fn desktop_scope_windowless_scroll_lands_on_control() {
    run_desktop_fixture_case(
        "scroll",
        DriverRoute::WindowsSendInput,
        |pid, wid, driver| {
            let pre = snapshot(driver, pid, wid);
            // The fixture's outer ScrollViewer is visible while the nested
            // scroll-tall region begins below a 768px CI desktop. Wheel over a
            // visible child and verify the outer viewport moved by observing a
            // lower AX element's fresh screen coordinate.
            let (x, y) = element_center(&pre, "border-click-target");
            let (_, before_y) = element_center(&pre, "btn-increment");
            let response = driver.call(
                "scroll",
                serde_json::json!({ "x": x, "y": y, "direction": "down", "amount": 5 }),
            );
            assert!(
                !response.is_error(),
                "desktop scroll failed: {}",
                response.text()
            );
            std::thread::sleep(Duration::from_millis(500));
            let after = snapshot(driver, pid, wid);
            let (_, after_y) = element_center(&after, "btn-increment");
            assert!(
                after_y < before_y,
                "desktop scroll did not move the WPF outer viewport: before_y={before_y}, after_y={after_y}"
            );
        },
    );
}

/// Negative gate: a window-less screen-absolute click under `capture_scope=window`
/// must be rejected (the `desktop_scope_disabled` contract), not silently retargeted.
#[test]
#[ignore]
fn window_scope_rejects_windowless_click() {
    let case = CaseSpec::delivered(
        "windows-window-scope-gate-px-not-applicable",
        "desktop",
        "win32",
        "window_scope_gate",
        Targeting::Px,
        Delivery::NotApplicable,
        Scope::Window,
        DriverRoute::Composite,
        vec![OracleKind::Protocol],
    );
    execute_case(case, |evidence| {
        let mut driver = McpDriver::spawn_named("windows-window-scope-gate-px-not-applicable")
            .expect("required source-built driver did not start");
        *evidence = recording_evidence(driver.recording_dir());
        set_scope(&mut driver, "window");
        let response = driver.call("click", serde_json::json!({ "x": 100, "y": 100 }));
        let text = response.text().to_lowercase();
        assert!(
            response.is_error()
                || text.contains("desktop scope")
                || text.contains("desktop_scope_disabled"),
            "window-scope window-less click was not rejected: {}",
            response.text()
        );
        Observation::delivered(vec![OracleKind::Protocol], Evidence::default())
    });
}
