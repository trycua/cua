// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Focused GTK4 coverage on Linux/X11.
//!
//! The GTK3 fixture starts first and proves a foreign registry application is
//! already accessible before the GTK4 target registers. The public driver must
//! still resolve the later target PID and return its actionable GTK4 subtree.
//!
//! The GTK4 fixture draws a client-side header bar, so its screenshot includes
//! the title bar. A pixel click grounded on that screenshot must land where
//! the screenshot shows the target (#3237).

#![cfg(target_os = "linux")]

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cua_driver_testkit::e2e::{
    execute_case, native_background_case, native_foreground_case, native_readonly_case,
    recording_evidence, Delivery, DriverRoute, Evidence, Observation, OracleKind, Targeting,
};
use cua_driver_testkit::observer::TargetWindow;
use cua_driver_testkit::sentinel::run_with_background_oracles;
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

fn screenshot_rgba(response: &ToolResponse) -> image::RgbaImage {
    use base64::Engine as _;
    let png = response.raw["result"]["content"]
        .as_array()
        .and_then(|content| {
            content
                .iter()
                .find(|entry| entry["type"].as_str() == Some("image"))
        })
        .and_then(|entry| entry["data"].as_str())
        .unwrap_or_else(|| panic!("response carries no screenshot: {}", response.raw));
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(png)
        .expect("screenshot is base64");
    image::load_from_memory(&bytes)
        .expect("screenshot decodes")
        .to_rgba8()
}

/// The magenta pixel target's bounds `(left, top, right, bottom)` in
/// screenshot pixels, found from the image alone.
fn magenta_bounds(image: &image::RgbaImage) -> Option<(u32, u32, u32, u32)> {
    let mut bounds: Option<(u32, u32, u32, u32)> = None;
    for (x, y, pixel) in image.enumerate_pixels() {
        let [r, g, b, _] = pixel.0;
        if r > 200 && g < 60 && b > 200 {
            bounds = Some(match bounds {
                None => (x, y, x, y),
                Some((left, top, right, bottom)) => {
                    (left.min(x), top.min(y), right.max(x), bottom.max(y))
                }
            });
        }
    }
    bounds
}

fn pixel_target_center(driver: &mut McpDriver, pid: u32, window_id: u64) -> (f64, f64) {
    settled_snapshot(driver, pid, window_id, "GTK4_PIXEL_CLICKS=0");
    let state = driver.call(
        "get_window_state",
        serde_json::json!({ "pid": pid as i64, "window_id": window_id }),
    );
    assert!(!state.is_error(), "GTK4 capture failed: {}", state.text());
    let image = screenshot_rgba(&state);
    let (left, top, right, bottom) = magenta_bounds(&image).unwrap_or_else(|| {
        panic!(
            "pixel target is not in the {}x{} screenshot",
            image.width(),
            image.height()
        )
    });
    (f64::from(left + right) / 2.0, f64::from(top + bottom) / 2.0)
}

fn wait_for_pixel_click(driver: &mut McpDriver, pid: u32, window_id: u64) -> String {
    settled_snapshot(driver, pid, window_id, "GTK4_PIXEL_CLICKS=1").tree_text()
}

fn click_screenshot_pixel(delivery: Delivery) {
    let case = match delivery {
        Delivery::Background => native_background_case(
            "gtk4",
            "left_click_header_bar_window",
            Targeting::Px,
            DriverRoute::LinuxAtSpiAction,
        ),
        _ => native_foreground_case(
            "gtk4",
            "left_click_header_bar_window",
            Targeting::Px,
            DriverRoute::LinuxXTest,
        ),
    };
    let cell_id = case.cell_id.clone();
    execute_case(case, |evidence| {
        let mut driver = McpDriver::spawn_named(&cell_id).expect("start source-built Linux driver");
        *evidence = recording_evidence(driver.recording_dir());
        let (pid, window_id) = launch_fixture(
            &mut driver,
            "harness-gtk4",
            "CuaTestHarness.Gtk4",
            "CuaTestHarness GTK4",
        );
        let (x, y) = pixel_target_center(&mut driver, pid, window_id);
        let mode = if delivery == Delivery::Background {
            "background"
        } else {
            "foreground"
        };
        let click = |driver: &mut McpDriver| {
            let clicked = driver.call(
                "click",
                serde_json::json!({
                    "pid": pid as i64, "window_id": window_id,
                    "x": x, "y": y, "delivery_mode": mode
                }),
            );
            assert!(
                !clicked.is_error(),
                "pixel click failed: {}",
                clicked.text()
            );
        };
        let passed = if delivery == Delivery::Background {
            let ((), passed) = run_with_background_oracles(
                &mut driver,
                TargetWindow {
                    pid,
                    native_id: window_id,
                },
                click,
            )
            .unwrap_or_else(|error| panic!("background desktop contract failed: {error}"));
            passed
        } else {
            driver.start_behavior_recording();
            click(&mut driver);
            Vec::new()
        };
        let tree = wait_for_pixel_click(&mut driver, pid, window_id);
        assert!(
            tree.contains("GTK4_PIXEL_CLICKS=1"),
            "click at screenshot pixel ({x}, {y}) missed the target: {tree}"
        );
        Observation::delivered_with_fixture_state(passed)
    });
}

#[test]
#[ignore]
fn gtk4_screenshot_pixel_click_lands_in_header_bar_window_background() {
    click_screenshot_pixel(Delivery::Background);
}

#[test]
#[ignore]
fn gtk4_screenshot_pixel_click_lands_in_header_bar_window_foreground() {
    click_screenshot_pixel(Delivery::Foreground);
}
