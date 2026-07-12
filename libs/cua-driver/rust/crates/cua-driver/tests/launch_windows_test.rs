//! Windows launch behavior against the repo-local Electron harness.

#![cfg(target_os = "windows")]

use std::collections::HashSet;
use std::process::{Command, Stdio};
use std::time::Duration;

use cua_driver_testkit::e2e::{
    execute_case, recording_evidence, CaseSpec, Delivery, DriverRoute, Evidence, Observation,
    OracleKind, RefusalCode, Scope, Targeting,
};
use cua_driver_testkit::sentinel::ForegroundSentinel;
use cua_driver_testkit::{harness_app, spawn_in_job, Driver, McpDriver};
use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM, TRUE};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic,
    ShowWindow, SW_MINIMIZE,
};

#[test]
#[ignore]
fn minimized_window_is_listed_and_bring_to_front_restores_it() {
    let executable = harness_app("harness-electron", "CuaTestHarness.Electron.exe");
    assert!(
        executable.exists(),
        "required Electron launch harness is missing: {}",
        executable.display()
    );
    let mut driver = McpDriver::spawn_named("windows-electron-minimized-window-restore")
        .expect("required source-built driver did not start");
    let mut command = Command::new(&executable);
    command.stdout(Stdio::null()).stderr(Stdio::null());
    let app = spawn_in_job(&mut command).expect("Electron harness did not start");
    let pid = app.id();
    driver.reaper().push(app);

    let (window_id, _) = wait_for_window(pid);
    let hwnd = HWND(window_id as *mut _);
    unsafe {
        let _ = ShowWindow(hwnd, SW_MINIMIZE);
    }
    wait_until(Duration::from_secs(2), || {
        unsafe { IsIconic(hwnd) }.as_bool()
    });

    let listed = driver.call("list_windows", serde_json::json!({"pid": pid as i64}));
    let window = listed.structured()["windows"]
        .as_array()
        .and_then(|windows| {
            windows
                .iter()
                .find(|window| window["window_id"].as_u64() == Some(window_id))
        })
        .expect("minimized target window missing from list_windows");
    assert_eq!(window["minimized"].as_bool(), Some(true));
    assert_eq!(window["is_on_screen"].as_bool(), Some(false));

    let on_screen = driver.call(
        "list_windows",
        serde_json::json!({"pid": pid as i64, "on_screen_only": true}),
    );
    assert!(
        on_screen.structured()["windows"]
            .as_array()
            .is_some_and(|windows| windows.iter().all(|window| {
                window["window_id"].as_u64() != Some(window_id)
            })),
        "on_screen_only retained minimized target"
    );

    let restored = driver.call(
        "bring_to_front",
        serde_json::json!({"pid": pid as i64, "window_id": window_id}),
    );
    assert!(!restored.is_error(), "restore failed: {}", restored.text());
    assert_eq!(restored.structured()["restored"].as_bool(), Some(true));
    wait_until(Duration::from_secs(2), || {
        !unsafe { IsIconic(hwnd) }.as_bool()
    });

    let listed = driver.call("list_windows", serde_json::json!({"pid": pid as i64}));
    let window = listed.structured()["windows"]
        .as_array()
        .and_then(|windows| {
            windows
                .iter()
                .find(|window| window["window_id"].as_u64() == Some(window_id))
        })
        .expect("restored target window missing from list_windows");
    assert_eq!(window["minimized"].as_bool(), Some(false));
    assert_eq!(window["is_on_screen"].as_bool(), Some(true));
}

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
    )
    .expecting_refusal(vec![RefusalCode::BackgroundUnavailable]);
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
        let sentinel = ForegroundSentinel::launch(&mut driver);
        let before = window_ids();
        driver.start_behavior_recording();

        let (response, mut passed) = sentinel
            .observe_desktop(|| {
                driver.call(
                    "launch_app",
                    serde_json::json!({
                        "path": executable.to_string_lossy(),
                        "start_minimized": true
                    }),
                )
            })
            .unwrap_or_else(|error| panic!("minimized launch disturbed the desktop: {error}"));
        assert_required_background_oracles(&passed);
        assert!(response.is_error(), "minimized launch unexpectedly proceeded");
        assert_eq!(
            response.structured()["code"].as_str(),
            Some("background_unavailable"),
            "minimized launch returned the wrong refusal: {}",
            response.text()
        );
        std::thread::sleep(Duration::from_millis(500));
        assert!(
            window_ids().is_subset(&before),
            "refused minimized launch created a new desktop window"
        );
        passed.push(OracleKind::FixtureState);
        Observation::refused(
            RefusalCode::BackgroundUnavailable,
            passed,
            response.text(),
            Evidence::default(),
        )
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

fn wait_for_window(pid: u32) -> (u64, String) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some((_, window_id, title)) = native_windows()
            .into_iter()
            .find(|(window_pid, _, _)| *window_pid == pid)
        {
            return (window_id, title);
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for pid {pid} to create a window"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn wait_until(timeout: Duration, predicate: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + timeout;
    while !predicate() {
        assert!(
            std::time::Instant::now() < deadline,
            "condition did not become true within {timeout:?}"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn window_ids() -> HashSet<u64> {
    native_windows()
        .into_iter()
        .map(|(_, window_id, _)| window_id)
        .collect()
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
