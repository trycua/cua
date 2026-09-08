//! Focused GTK4 process-level AT-SPI selection coverage on Linux/X11.
//!
//! The GTK3 fixture starts first and proves a foreign registry application is
//! already accessible before the GTK4 target registers. The public driver must
//! still resolve the later target PID and return its actionable GTK4 subtree.

#![cfg(target_os = "linux")]

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cua_driver_testkit::e2e::{
    execute_case, native_readonly_case, recording_evidence, DriverRoute, Evidence, Observation,
    OracleKind, Targeting,
};
use cua_driver_testkit::{harness_app, Driver, McpDriver, ToolResponse};

fn launch_fixture(
    driver: &mut McpDriver,
    fixture: &str,
    executable: &str,
    title: &str,
) -> (u32, u64) {
    let path = harness_app(fixture, executable);
    assert!(path.exists(), "required fixture is missing: {path:?}");
    driver
        .reaper()
        .spawn(
            Command::new(&path)
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit()),
        )
        .unwrap_or_else(|error| panic!("launch fixture {path:?}: {error}"));

    let deadline = Instant::now() + Duration::from_secs(12);
    while Instant::now() < deadline {
        let response = driver.call("list_windows", serde_json::json!({}));
        if let Some(window) = response.structured()["windows"]
            .as_array()
            .and_then(|windows| {
                windows.iter().find(|window| {
                    window["title"]
                        .as_str()
                        .is_some_and(|value| value.contains(title))
                })
            })
        {
            let pid = window["pid"].as_u64().unwrap_or(0) as u32;
            let window_id = window["window_id"].as_u64().unwrap_or(0);
            if pid != 0 && window_id != 0 {
                driver.reaper().track_pid(pid);
                return (pid, window_id);
            }
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    panic!("fixture window {title:?} never appeared");
}

fn settled_snapshot(
    driver: &mut McpDriver,
    pid: u32,
    window_id: u64,
    marker: &str,
) -> ToolResponse {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut response = driver.call(
        "get_window_state",
        serde_json::json!({
            "pid": pid as i64,
            "window_id": window_id,
            "include_screenshot": false
        }),
    );
    while Instant::now() < deadline && !response.tree_text().contains(marker) {
        std::thread::sleep(Duration::from_millis(300));
        response = driver.call(
            "get_window_state",
            serde_json::json!({
                "pid": pid as i64,
                "window_id": window_id,
                "include_screenshot": false
            }),
        );
    }
    response
}

#[test]
#[ignore]
fn gtk4_tree_resolves_after_foreign_gtk3_application() {
    let case = native_readonly_case(
        "gtk4",
        "foreign_app_target_selection",
        Targeting::Ax,
        DriverRoute::AxRead,
        vec![OracleKind::AxState],
    );
    execute_case(case, |evidence| {
        let mut driver = McpDriver::spawn_named("linux-gtk4-foreign-app-target-selection")
            .expect("start source-built Linux driver");
        *evidence = recording_evidence(driver.recording_dir());

        let (gtk3_pid, gtk3_window) = launch_fixture(
            &mut driver,
            "harness-gtk3",
            "CuaTestHarness.Gtk3",
            "CuaTestHarness GTK3",
        );
        let gtk3 = settled_snapshot(&mut driver, gtk3_pid, gtk3_window, "btn-increment");
        assert!(
            gtk3.tree_text().contains("btn-increment"),
            "foreign GTK3 application never exposed its AT-SPI tree: {}",
            gtk3.tree_text()
        );

        let (gtk4_pid, gtk4_window) = launch_fixture(
            &mut driver,
            "harness-gtk4",
            "CuaTestHarness.Gtk4",
            "CuaTestHarness GTK4",
        );
        driver.start_behavior_recording();
        assert_ne!(gtk4_pid, gtk3_pid, "fixtures must be distinct processes");
        let gtk4 = settled_snapshot(&mut driver, gtk4_pid, gtk4_window, "GTK4 actionable target");
        assert!(
            !gtk4.is_error(),
            "GTK4 get_window_state failed: {}",
            gtk4.raw
        );
        assert_eq!(
            gtk4.structured()["degraded"].as_bool(),
            None,
            "GTK4 snapshot degraded: {}",
            gtk4.structured()
        );
        assert!(
            gtk4.tree_text().contains("GTK4 actionable target"),
            "GTK4 actionable subtree missing: {}",
            gtk4.tree_text()
        );
        assert!(
            gtk4.structured()["element_count"].as_u64().unwrap_or(0) > 0,
            "GTK4 snapshot contained no actionable elements: {}",
            gtk4.structured()
        );
        Observation::delivered(vec![OracleKind::AxState], Evidence::default())
    });
}
