//! Windows launch behavior against the repo-local Electron harness.

#![cfg(target_os = "windows")]

use std::collections::HashSet;
use std::time::{Duration, Instant};

use cua_driver_testkit::e2e::{
    execute_case, recording_evidence, CaseSpec, Delivery, DriverRoute, Evidence, Observation,
    OracleKind, Scope, Targeting,
};
use cua_driver_testkit::sentinel::ForegroundSentinel;
use cua_driver_testkit::{harness_app, Driver, McpDriver};
use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM, TRUE};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic,
};

#[test]
#[ignore]
fn launch_app_minimized_preserves_foreground() {
    let case = CaseSpec::delivered(
        "windows-electron-launch-app-background",
        "electron",
        "chromium",
        "launch_app",
        Targeting::NotApplicable,
        Delivery::Background,
        Scope::Window,
        DriverRoute::WindowsShellExecute,
        vec![
            OracleKind::FixtureState,
            OracleKind::Focus,
            OracleKind::ZOrder,
            OracleKind::Cursor,
            OracleKind::NoLeakedInput,
        ],
    );
    execute_case(case, |evidence| {
        let executable = harness_app("harness-electron", "CuaTestHarness.Electron.exe");
        assert!(
            executable.exists(),
            "required Electron launch harness is missing: {}",
            executable.display()
        );
        let mut driver = McpDriver::spawn_named("windows-electron-launch-app-background")
            .expect("required source-built driver did not start");
        *evidence = recording_evidence(driver.recording_dir());
        let before = window_ids();
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
                wait_for_new_window(&before)
            })
            .unwrap_or_else(|error| panic!("minimized launch disturbed the desktop: {error}"));
        assert_required_background_oracles(&passed);
        driver.reaper().track_pid(pid);
        let minimized = unsafe { IsIconic(HWND(window_id as *mut _)).as_bool() };
        assert!(
            minimized,
            "launch_app(start_minimized=true) created a restored window: HWND 0x{window_id:x}"
        );
        let mut passed = passed;
        passed.push(OracleKind::FixtureState);
        Observation::delivered(passed, Evidence::default())
    });
}

fn native_windows() -> Vec<(u32, u64, String)> {
    unsafe extern "system" fn callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let windows = &mut *(lparam.0 as *mut Vec<(u32, u64, String)>);
        let title_len = GetWindowTextLengthW(hwnd);
        if title_len > 0 {
            let mut title = vec![0u16; title_len as usize + 1];
            let copied = GetWindowTextW(hwnd, &mut title);
            if copied > 0 {
                let mut pid = 0u32;
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
                windows.push((
                    pid,
                    hwnd.0 as u64,
                    String::from_utf16_lossy(&title[..copied as usize]),
                ));
            }
        }
        TRUE
    }

    let mut windows = Vec::new();
    unsafe {
        let _ = EnumWindows(
            Some(callback),
            LPARAM(&mut windows as *mut Vec<(u32, u64, String)> as isize),
        );
    }
    windows
}

fn window_ids() -> HashSet<u64> {
    native_windows()
        .into_iter()
        .map(|(_, window_id, _)| window_id)
        .collect()
}

fn wait_for_new_window(before: &HashSet<u64>) -> (u32, u64) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some((pid, window_id, _)) = native_windows().into_iter().find(|(_, id, title)| {
            !before.contains(id) && title.contains("CuaTestHarness Electron")
        }) {
            let found = (pid, window_id);
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
