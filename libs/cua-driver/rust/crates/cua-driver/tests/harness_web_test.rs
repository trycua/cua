//! Integration tests against the CuaTestHarness.WebView (WPF + WebView2)
//! and CuaTestHarness.Electron hosts. Both load the same
//! `tests/fixtures/shared/web/index.html`, so the same `page` tool flows
//! are exercised against two Chromium-based hosts.
//!
//! Run via:
//!   cargo test --test harness_web_test -- --ignored --nocapture
//!
//! The page-tool tests cover CDP discovery and a DOM round-trip together.
//! WebView2 can expose its listener before its first page target is ready, so
//! the driver must tolerate a briefly empty `/json` response.

#![cfg(target_os = "windows")]

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use cua_driver_testkit::e2e::{
    execute_case, native_background_case, recording_evidence, DriverRoute, Observation, Targeting,
};
use cua_driver_testkit::observer::TargetWindow;
use cua_driver_testkit::sentinel::run_with_background_oracles;
use cua_driver_testkit::{harness_app, spawn_in_job, Driver, McpDriver};

// ── workspace paths ──────────────────────────────────────────────────────────

fn webview_exe() -> PathBuf {
    if let Ok(p) = std::env::var("HARNESS_WEBVIEW_EXE") {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return pb;
        }
    }
    harness_app("harness-webview", "CuaTestHarness.WebView.exe")
}
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

fn wait_for_page_text(
    driver: &mut McpDriver,
    pid: i64,
    wid: u64,
    javascript: &str,
    expected: &str,
) -> String {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let response = driver.call(
            "page",
            serde_json::json!({
                "pid": pid,
                "window_id": wid,
                "action": "execute_javascript",
                "javascript": javascript,
            }),
        );
        let text = response.text().to_owned();
        if text.contains(expected) {
            return text;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "page state did not reach {expected:?}: {text:?}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Launch the harness exe + a cua-driver child with `CUA_DRIVER_CDP_PORT`
/// pointing at the harness's CDP endpoint. Polls list_windows until the
/// host's window appears.
fn run_web_case<F>(toolkit: &str, action: &str, host_exe: PathBuf, title_substr: &str, f: F)
where
    F: FnOnce(i64, u64, &mut McpDriver),
{
    if !host_exe.exists() {
        if std::env::var_os("CUA_TEST_REQUIRE_FIXTURES").is_some() {
            panic!("required {toolkit} host is missing at {host_exe:?}");
        }
        eprintln!("{toolkit} host exe not found at {host_exe:?}; skipping");
        return;
    }
    let case = native_background_case(toolkit, action, Targeting::Page, DriverRoute::Cdp);
    let cell_id = case.cell_id.clone();
    execute_case(case, |evidence| {
        let cdp_port = allocate_loopback_port();
        let cdp_port_string = cdp_port.to_string();
        let mut driver = McpDriver::spawn_named_with_env(
            &cell_id,
            &[("CUA_DRIVER_CDP_PORT", cdp_port_string.as_str())],
        )
        .expect("required source-built Windows driver did not start");
        *evidence = recording_evidence(driver.recording_dir());

        let env_var = if toolkit == "webview2" {
            "CUA_WEBVIEW_CDP_PORT"
        } else {
            "CUA_ELECTRON_CDP_PORT"
        };
        let mut cmd = Command::new(&host_exe);
        cmd.env(env_var, &cdp_port_string)
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut app = spawn_in_job(&mut cmd).expect("spawn web harness");
        let pid = app.id() as i64;
        let deadline = std::time::Instant::now() + Duration::from_secs(12);
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
        let (_, passed) = run_with_background_oracles(
            &mut driver,
            TargetWindow {
                pid: pid as u32,
                native_id: wid,
            },
            |driver| f(pid, wid, driver),
        )
        .unwrap_or_else(|error| panic!("background desktop contract failed: {error}"));
        Observation::delivered_with_fixture_state(passed)
    });
}

// ── WebView2 page tool ──────────────────────────────────────────────────────

#[test]
#[ignore]
fn harness_webview_page_tool() {
    // Regression guard for WebView2 CDP exposure via
    // CoreWebView2EnvironmentOptions.AdditionalBrowserArguments.
    // Combined with the `/json` Content-Length fix in mcp-server/src/cdp.rs,
    // the page tool now reaches WebView2's DOM via CDP just like Electron.
    run_web_case(
        "webview2",
        "page_roundtrip",
        webview_exe(),
        "CuaTestHarness WebView [ready",
        |pid, wid, driver| {
            let marker = driver.call("page", serde_json::json!({
            "pid": pid, "window_id": wid, "action": "execute_javascript",
            "javascript": "document.querySelector('[data-cua-id=\"page-marker\"]').textContent"
        })).text().to_string();
            assert!(
                marker.contains("WEB_HARNESS_MARKER_v1"),
                "WebView2 CDP execute_javascript marker fetch: {marker:?}"
            );

            // click_element via DOM selector + counter readback.
            let _ = driver.call(
                "page",
                serde_json::json!({
                    "pid": pid, "window_id": wid, "action": "click_element",
                    "selector": "#btn-increment"
                }),
            );
            wait_for_page_text(
                driver,
                pid,
                wid,
                "document.getElementById('lbl-counter').textContent",
                "counter=1",
            );
            println!("✅ harness_webview_page_tool: CDP+execute_javascript+click_element green");
        },
    );
}

// ── Electron page tool ───────────────────────────────────────────────────────

#[test]
#[ignore]
fn harness_electron_page_tool() {
    // Regression guard for the CDP /json discovery fix (parse
    // Content-Length / Transfer-Encoding instead of read_to_end).
    // cua-driver's page tool now reaches Electron's CDP successfully.
    run_web_case(
        "electron",
        "page_execute",
        electron_exe(),
        "CuaTestHarness Electron",
        |pid, wid, driver| {
            // 1. execute_javascript via CDP.
            let marker = driver.call("page", serde_json::json!({
            "pid": pid, "window_id": wid, "action": "execute_javascript",
            "javascript": "document.querySelector('[data-cua-id=\"page-marker\"]').textContent"
        })).text().to_string();
            assert!(
                marker.contains("WEB_HARNESS_MARKER_v1"),
                "Electron CDP execute_javascript marker fetch: {marker:?}"
            );

            // 2. Increment counter via direct execute_javascript (the
            //    click_element path has a separate probe-JSON-parsing gap
            //    documented below — track separately).
            let click = driver.call(
                "page",
                serde_json::json!({
                    "pid": pid, "window_id": wid, "action": "execute_javascript",
                    "javascript": "document.getElementById('btn-increment').click()"
                }),
            );
            assert!(
                !click.is_error(),
                "Electron execute_javascript click failed: {}",
                click.text()
            );
            wait_for_page_text(
                driver,
                pid,
                wid,
                "document.getElementById('lbl-counter').textContent",
                "counter=1",
            );
            println!("✅ harness_electron_page_tool: CDP+execute_javascript green");
        },
    );
}

/// Regression guard for the page.click_element double-encode fix.
///
/// Originally the CDP runtime.evaluate response for the probe JS came
/// back as a JSON-encoded string containing the actual `{vx,vy,...}`
/// object. The page tool's `serde_json::from_str(&probe_json).or_else(...)`
/// only fell into the inner-decode branch on a hard parse error, but
/// `from_str` happily parses a JSON-string into a `Value::String`, so the
/// inner-decode branch never ran and `parsed.get("vx")` returned None.
/// Fix: match on Value::String and re-decode explicitly.
#[test]
#[ignore]
fn harness_electron_click_element() {
    run_web_case(
        "electron",
        "click_element_probe",
        electron_exe(),
        "CuaTestHarness Electron",
        |pid, wid, driver| {
            let resp = driver.call(
                "page",
                serde_json::json!({
                    "pid": pid, "window_id": wid, "action": "click_element",
                    "selector": "#btn-increment"
                }),
            );
            // Prefer the tool text; fall back to a JSON-RPC error message.
            let text = if resp.text().is_empty() {
                resp.raw["error"]["message"]
                    .as_str()
                    .unwrap_or("")
                    .to_string()
            } else {
                resp.text().to_string()
            };
            assert!(
                !text.contains("probe JSON missing") && !text.contains("required field"),
                "click_element probe parse regressed: {text:?}"
            );
            // Verify the click actually fired in the DOM.
            wait_for_page_text(
                driver,
                pid,
                wid,
                "document.getElementById('lbl-counter').textContent",
                "counter=1",
            );
            println!("✅ harness_electron_click_element: probe parsed, click fired, counter=1");
        },
    );
}
