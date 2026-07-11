//! Windows launch behavior against the repo-local Electron harness.

#![cfg(target_os = "windows")]

use std::collections::HashSet;
use std::time::{Duration, Instant};

use cua_driver_testkit::e2e::OracleKind;
use cua_driver_testkit::sentinel::ForegroundSentinel;
use cua_driver_testkit::{harness_app, Driver, McpDriver};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::IsIconic;

#[test]
#[ignore]
fn launch_app_minimized_preserves_foreground() {
    let executable = harness_app("harness-electron", "CuaTestHarness.Electron.exe");
    assert!(
        executable.exists(),
        "required Electron launch harness is missing: {}",
        executable.display()
    );
    let mut driver = McpDriver::spawn_named("windows-launch-minimized")
        .expect("required source-built driver did not start");
    let before = window_ids(&mut driver);
    let sentinel = ForegroundSentinel::launch(&mut driver);

    let ((pid, window_id), passed) = sentinel
        .observe_background(sentinel.target(), || {
            let response = driver.call(
                "launch_app",
                serde_json::json!({
                    "path": executable.to_string_lossy(),
                    "start_minimized": true
                }),
            );
            assert!(
                !response.is_error(),
                "launch_app(start_minimized=true) failed: {}",
                response.text()
            );
            wait_for_new_window(&mut driver, &before)
        })
        .unwrap_or_else(|error| panic!("minimized launch disturbed the desktop: {error}"));
    assert_required_background_oracles(&passed);
    driver.reaper().track_pid(pid);
    let minimized = unsafe { IsIconic(HWND(window_id as *mut _)).as_bool() };
    assert!(
        minimized,
        "launch_app(start_minimized=true) created a restored window: HWND 0x{window_id:x}"
    );
}

fn window_ids(driver: &mut McpDriver) -> HashSet<u64> {
    driver
        .call("list_windows", serde_json::json!({}))
        .structured()["windows"]
        .as_array()
        .map(|windows| {
            windows
                .iter()
                .filter_map(|window| window["window_id"].as_u64())
                .collect()
        })
        .unwrap_or_default()
}

fn wait_for_new_window(driver: &mut McpDriver, before: &HashSet<u64>) -> (u32, u64) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let response = driver.call("list_windows", serde_json::json!({}));
        if let Some(found) = response.structured()["windows"]
            .as_array()
            .and_then(|windows| {
                windows.iter().find_map(|window| {
                    let id = window["window_id"].as_u64()?;
                    let title = window["title"].as_str().unwrap_or("");
                    (!before.contains(&id) && title.contains("CuaTestHarness Electron"))
                        .then(|| (window["pid"].as_u64().unwrap_or(0) as u32, id))
                })
            })
        {
            assert_ne!(found.0, 0, "launched Electron window has no process id");
            return found;
        }
        assert!(
            Instant::now() < deadline,
            "launch_app succeeded but no Electron harness window appeared"
        );
        std::thread::sleep(Duration::from_millis(150));
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
            "minimized launch omitted required {required:?} oracle"
        );
    }
}
