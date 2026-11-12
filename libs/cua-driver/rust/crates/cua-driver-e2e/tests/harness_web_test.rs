// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Integration tests against the CuaTestHarness.WebView (WPF + WebView2)
//! and CuaTestHarness.Electron hosts. Both load the same
//! `tests/fixtures/shared/web/index.html`.
//!
//! Run via:
//!   cargo test --test harness_web_test -- --ignored --nocapture

#![cfg(target_os = "windows")]

use std::cell::RefCell;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cua_driver_testkit::e2e::{
    execute_case, recording_evidence, CaseSpec, Delivery, DriverRoute, Evidence, Observation,
    OracleKind, RefusalCode, Scope, Targeting,
};
use cua_driver_testkit::observer::TargetWindow;
use cua_driver_testkit::sentinel::run_with_background_oracles;
use cua_driver_testkit::{
    ax, harness_app, spawn_in_job, Driver, FixtureJournal, McpDriver, ToolResponse,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    IsIconic, ShowWindow, SW_MINIMIZE, SW_SHOWNOACTIVATE,
};

// ── workspace paths ──────────────────────────────────────────────────────────

fn electron_exe() -> PathBuf {
    if let Ok(p) = std::env::var("HARNESS_ELECTRON_EXE") {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return pb;
        }
    }
    harness_app("harness-electron", "CuaTestHarness.Electron.exe")
}

// ── shared session helper ────────────────────────────────────────────────────

fn allocate_loopback_port() -> u16 {
    let listener =
        std::net::TcpListener::bind(("127.0.0.1", 0)).expect("allocate an ephemeral CDP port");
    listener.local_addr().expect("read CDP port").port()
}

/// Launch the harness exe + a cua-driver child. Polls list_windows until the
/// host's window appears.
fn run_web_case_with_preparation<P, F, R, O>(
    case: cua_driver_testkit::e2e::CaseSpec,
    toolkit: &str,
    host_exe: PathBuf,
    title_substr: &str,
    prepare: P,
    f: F,
    observe: O,
) where
    P: FnOnce(i64, u64, &mut McpDriver, &FixtureJournal),
    F: FnOnce(i64, u64, &mut McpDriver, &FixtureJournal) -> R,
    O: FnOnce(R, Vec<OracleKind>) -> Observation,
{
    let cell_id = case.cell_id.clone();
    execute_case(case, |evidence| {
        assert!(
            host_exe.exists(),
            "required {toolkit} host is missing at {host_exe:?}"
        );
        let cdp_port = allocate_loopback_port();
        let cdp_port_string = cdp_port.to_string();
        let journal = FixtureJournal::start();
        let mut driver = McpDriver::spawn_named_with_env(&cell_id, &[])
            .expect("required source-built Windows driver did not start");
        *evidence = recording_evidence(driver.recording_dir());

        let env_var = if toolkit == "webview2" {
            "CUA_WEBVIEW_CDP_PORT"
        } else {
            "CUA_ELECTRON_CDP_PORT"
        };
        let mut cmd = Command::new(&host_exe);
        cmd.env(env_var, &cdp_port_string)
            .env("CUA_E2E_FIXTURE_JOURNAL_URL", journal.url())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut app = spawn_in_job(&mut cmd).expect("spawn web harness");
        let pid = app.id() as i64;
        // WebView2's first CoreWebView2Environment creation can exceed 12s on a
        // cold hosted runner. Keep polling the externally visible ready title;
        // process exit and the final deadline still fail closed.
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        let mut observed_titles = Vec::new();
        let (wid, _) = 'ready: loop {
            if let Some(status) = app.try_wait().expect("poll web harness process") {
                let mut stderr = String::new();
                if let Some(mut pipe) = app.stderr.take() {
                    let _ = pipe.read_to_string(&mut stderr);
                }
                panic!(
                    "{toolkit} fixture exited before readiness with {status}: {}",
                    stderr.trim()
                );
            }
            let response = driver.call("list_windows", serde_json::json!({ "pid": pid }));
            observed_titles.clear();
            if let Some(windows) = response.structured()["windows"].as_array() {
                for window in windows {
                    let title = window["title"].as_str().unwrap_or("");
                    observed_titles.push(title.to_owned());
                    if title.contains(title_substr) {
                        if let Some(wid) = window["window_id"].as_u64() {
                            break 'ready (wid, title.to_owned());
                        }
                    }
                }
            }
            if std::time::Instant::now() >= deadline {
                let _ = app.kill();
                let _ = app.wait();
                let mut stderr = String::new();
                if let Some(mut pipe) = app.stderr.take() {
                    let _ = pipe.read_to_string(&mut stderr);
                }
                panic!(
                    "{toolkit} window with title containing {title_substr:?} did not become ready; \
                     observed titles={observed_titles:?}; stderr={:?}",
                    stderr.trim()
                );
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        driver.reaper().push(app);
        let journal_deadline = Instant::now() + Duration::from_secs(5);
        while !journal.contains("WEB_HARNESS_MARKER_v1") {
            assert!(
                Instant::now() < journal_deadline,
                "{toolkit} fixture journal did not become ready: {}",
                journal.snapshot()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        prepare(pid, wid, &mut driver, &journal);
        let (result, passed) = run_with_background_oracles(
            &mut driver,
            TargetWindow {
                pid: pid as u32,
                native_id: wid,
            },
            |driver| f(pid, wid, driver, &journal),
        )
        .unwrap_or_else(|error| panic!("background desktop contract failed: {error}"));
        observe(result, passed)
    });
}

fn snapshot(driver: &mut McpDriver, pid: i64, wid: u64) -> ToolResponse {
    driver.call(
        "get_window_state",
        serde_json::json!({
            "pid": pid,
            "window_id": wid,
            "capture_mode": "ax"
        }),
    )
}

fn wait_for_journal_text(journal: &FixtureJournal, id: &str, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if journal.text(id).as_deref() == Some(expected) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "WebView2 fixture journal {id:?} did not reach {expected:?}: {}",
            journal.snapshot()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

// ── WebView2 page tool ──────────────────────────────────────────────────────

// ── Electron minimized snapshot ──────────────────────────────────────────────

fn wait_for_iconic(hwnd: HWND, iconic: bool) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while unsafe { IsIconic(hwnd) }.as_bool() != iconic {
        assert!(
            Instant::now() < deadline,
            "Electron window did not reach iconic={iconic}"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[test]
#[ignore]
fn harness_electron_minimized_snapshot_click_refuses_without_side_effects() {
    let case = CaseSpec::delivered(
        "windows-electron-minimized-snapshot-click-refusal",
        "electron",
        "electron",
        "left_click",
        Targeting::Ax,
        Delivery::Background,
        Scope::Window,
        DriverRoute::PostMessage,
        vec![
            OracleKind::FixtureState,
            OracleKind::Focus,
            OracleKind::ZOrder,
            OracleKind::Cursor,
            OracleKind::NoLeakedInput,
            OracleKind::Protocol,
        ],
    )
    .expecting_refusal(vec![RefusalCode::ElementNotVisible]);
    let token = RefCell::new(None);
    run_web_case_with_preparation(
        case,
        "electron",
        electron_exe(),
        "CuaTestHarness Electron",
        |pid, wid, driver, journal| {
            wait_for_journal_text(journal, "lbl-counter", "counter=0");
            // A visible read first, so the minimized read below sees the
            // renderer's accessibility tree.
            let _ = snapshot(driver, pid, wid);
            let hwnd = HWND(wid as *mut _);
            let _ = unsafe { ShowWindow(hwnd, SW_MINIMIZE) };
            wait_for_iconic(hwnd, true);
            let minimized = snapshot(driver, pid, wid);
            let idx = ax::element_index_by_id(minimized.tree_text(), "btn-increment")
                .expect("btn-increment not in the minimized snapshot");
            token.replace(Some(minimized.element_token(idx).to_owned()));
            let _ = unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
            wait_for_iconic(hwnd, false);
        },
        |pid, wid, driver, journal| {
            let token = token
                .take()
                .expect("minimized snapshot did not set a token");
            let response = driver.call(
                "click",
                serde_json::json!({
                    "pid": pid,
                    "window_id": wid,
                    "element_token": token,
                    "delivery_mode": "background"
                }),
            );
            assert!(
                response.is_error(),
                "click with a minimized-snapshot token reported success: {}",
                response.text()
            );
            assert_eq!(response.structured()["status"], "refused");
            assert_eq!(
                response.structured()["refusal"]["code"],
                "element_not_visible"
            );
            std::thread::sleep(Duration::from_millis(300));
            wait_for_journal_text(journal, "lbl-counter", "counter=0");
            response.text().to_owned()
        },
        |message, mut passed| {
            passed.extend([OracleKind::FixtureState, OracleKind::Protocol]);
            passed.sort();
            passed.dedup();
            Observation::refused(
                RefusalCode::ElementNotVisible,
                passed,
                message,
                Evidence::default(),
            )
        },
    );
}
