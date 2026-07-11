//! Integration tests against the CuaTestHarness.WebView (WPF + WebView2)
//! and CuaTestHarness.Electron hosts. Both load the same
//! `tests/fixtures/shared/web/index.html`, so the same `page` tool flows
//! are exercised against two Chromium-based hosts.
//!
//! Run via:
//!   cargo test --test harness_web_test -- --ignored --nocapture
//!
//! ## Known cua-driver gaps these tests document
//!
//! - **CDP `/json` HTTP read uses `read_to_end`** — `mcp-server/src/cdp.rs`
//!   sends `Connection: close` and then calls `stream.read_to_end()`, but
//!   Chromium's CDP HTTP server ignores `Connection: close` and keeps the
//!   socket alive, so `read_to_end` hangs until the 10 s discovery timeout.
//!   Confirmed against Electron 31 on port 9223 (verified manually via
//!   curl: instant 200, JSON body present). Fix: parse `Content-Length`
//!   and `read_exact` that many bytes, or honour `Transfer-Encoding:
//!   chunked`. Tracked in this test as a structural assertion (window
//!   discoverable) rather than a behavioural one (page tool round-trip).
//!
//! - **WebView2 `--remote-debugging-port` ignored** — passing
//!   `AdditionalBrowserArguments = "--remote-debugging-port=9222"` via
//!   `CoreWebView2EnvironmentOptions` does not open a CDP listener on the
//!   WebView2 helper processes. WebView2 may be filtering the flag.
//!   Tracked here as a TODO for the harness rather than a cua-driver
//!   issue (since this is a WebView2 configuration concern).

#![cfg(target_os = "windows")]

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use cua_driver_testkit::{harness_app, spawn_in_job, Driver, McpDriver};

// ── workspace paths ──────────────────────────────────────────────────────────

fn webview_exe() -> PathBuf {
    if let Ok(p) = std::env::var("HARNESS_WEBVIEW_EXE") {
        let pb = PathBuf::from(p);
        if pb.exists() { return pb; }
    }
    harness_app("harness-webview", "CuaTestHarness.WebView.exe")
}
fn electron_exe() -> PathBuf {
    if let Ok(p) = std::env::var("HARNESS_ELECTRON_EXE") {
        let pb = PathBuf::from(p);
        if pb.exists() { return pb; }
    }
    harness_app("harness-electron", "CuaTestHarness.Electron.exe")
}

// ── shared session helper ────────────────────────────────────────────────────

/// Wait (up to ~5s) for `port` to become free. These web tests use FIXED CDP
/// ports (9222/9223) and a process-global `CUA_DRIVER_CDP_PORT`, so they must
/// run serially (`--test-threads=1`). A previous test's host can still be
/// releasing its port when the next launches; reusing it before then makes the
/// daemon discover the OLD host's page (`pages[0]`), so the click lands on a
/// stale window and the counter check fails. This guard closes that teardown
/// overlap — belt-and-braces on top of serial execution.
fn wait_port_free(port: u16) {
    for _ in 0..50 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    eprintln!("warning: CDP port {port} still bound after 5s — prior host may not have released it");
}

/// Launch the harness exe + a cua-driver child with `CUA_DRIVER_CDP_PORT`
/// pointing at the harness's CDP endpoint. Polls list_windows until the
/// host's window appears.
fn run_with_session<F>(label: &str, host_exe: PathBuf, title_substr: &str, cdp_port: u16, f: F)
where
    F: FnOnce(i64, u64, &mut McpDriver),
{
    if !host_exe.exists() {
        eprintln!("{label} host exe not found at {host_exe:?} — run tests/fixtures/build/windows.ps1");
        return;
    }
    // A prior test's host may still hold this fixed CDP port — wait for it to
    // free so the daemon doesn't discover the stale host's page.
    wait_port_free(cdp_port);
    // Set the CDP port the daemon should probe; the spawned cua-driver child
    // inherits it from this process's environment.
    std::env::set_var("CUA_DRIVER_CDP_PORT", cdp_port.to_string());
    let Some(mut driver) = McpDriver::spawn() else { return };

    // Set the CDP port the host should use so the daemon can find it.
    let env_var = if label == "webview" { "CUA_WEBVIEW_CDP_PORT" } else { "CUA_ELECTRON_CDP_PORT" };
    let mut cmd = Command::new(&host_exe);
    cmd.env(env_var, cdp_port.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let app = spawn_in_job(&mut cmd).expect("spawn host");
    let pid = app.id() as i64;
    driver.reaper().push(app);
    println!("{label} pid={pid} cdp_port={cdp_port}");
    std::thread::sleep(Duration::from_secs(2)); // small cold-start for runtime spin-up

    let (wid, _title) = driver
        .find_window(pid, title_substr)
        .unwrap_or_else(|| panic!("{label} window with title containing {title_substr:?} not found"));

    f(pid, wid, &mut driver);
}

// ── WebView2 structural + page tool ─────────────────────────────────────────

#[test]
#[ignore]
fn harness_webview_window_discoverable() {
    run_with_session("webview", webview_exe(), "CuaTestHarness WebView", 9222,
        |pid, wid, _driver| {
        println!("✅ harness_webview_window_discoverable: pid={pid} wid={wid}");
    });
}

#[test]
#[ignore]
fn harness_webview_page_tool() {
    // Regression guard for WebView2 CDP exposure via
    // CoreWebView2EnvironmentOptions.AdditionalBrowserArguments.
    // Combined with the `/json` Content-Length fix in mcp-server/src/cdp.rs,
    // the page tool now reaches WebView2's DOM via CDP just like Electron.
    run_with_session("webview", webview_exe(), "CuaTestHarness WebView", 9222,
        |pid, wid, driver| {

        let marker = driver.call("page", serde_json::json!({
            "pid": pid, "window_id": wid, "action": "execute_javascript",
            "javascript": "document.querySelector('[data-cua-id=\"page-marker\"]').textContent"
        })).text().to_string();
        assert!(marker.contains("WEB_HARNESS_MARKER_v1"),
            "WebView2 CDP execute_javascript marker fetch: {marker:?}");

        // click_element via DOM selector + counter readback.
        let _ = driver.call("page", serde_json::json!({
            "pid": pid, "window_id": wid, "action": "click_element",
            "selector": "#btn-increment"
        }));
        std::thread::sleep(Duration::from_millis(500));

        let post = driver.call("page", serde_json::json!({
            "pid": pid, "window_id": wid, "action": "execute_javascript",
            "javascript": "document.getElementById('lbl-counter').textContent"
        })).text().to_string();
        assert!(post.contains("counter=1"),
            "WebView2 counter didn't advance via page.click_element: {post:?}");
        println!("✅ harness_webview_page_tool: CDP+execute_javascript+click_element green");
    });
}

// ── Electron structural + page tool ──────────────────────────────────────────

#[test]
#[ignore]
fn harness_electron_window_discoverable() {
    run_with_session("electron", electron_exe(), "CuaTestHarness Electron", 9223,
        |pid, wid, _driver| {
        println!("✅ harness_electron_window_discoverable: pid={pid} wid={wid}");
    });
}

#[test]
#[ignore]
fn harness_electron_page_tool() {
    // Regression guard for the CDP /json discovery fix (parse
    // Content-Length / Transfer-Encoding instead of read_to_end).
    // cua-driver's page tool now reaches Electron's CDP successfully.
    run_with_session("electron", electron_exe(), "CuaTestHarness Electron", 9223,
        |pid, wid, driver| {

        // 1. execute_javascript via CDP.
        let marker = driver.call("page", serde_json::json!({
            "pid": pid, "window_id": wid, "action": "execute_javascript",
            "javascript": "document.querySelector('[data-cua-id=\"page-marker\"]').textContent"
        })).text().to_string();
        assert!(marker.contains("WEB_HARNESS_MARKER_v1"),
            "Electron CDP execute_javascript marker fetch: {marker:?}");

        // 2. Increment counter via direct execute_javascript (the
        //    click_element path has a separate probe-JSON-parsing gap
        //    documented below — track separately).
        let _ = driver.call("page", serde_json::json!({
            "pid": pid, "window_id": wid, "action": "execute_javascript",
            "javascript": "document.getElementById('btn-increment').click()"
        }));
        std::thread::sleep(Duration::from_millis(300));

        let post = driver.call("page", serde_json::json!({
            "pid": pid, "window_id": wid, "action": "execute_javascript",
            "javascript": "document.getElementById('lbl-counter').textContent"
        })).text().to_string();
        assert!(post.contains("counter=1"),
            "Electron counter did not advance via execute_javascript: {post:?}");
        println!("✅ harness_electron_page_tool: CDP+execute_javascript green");
    });
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
    run_with_session("electron", electron_exe(), "CuaTestHarness Electron", 9223,
        |pid, wid, driver| {
        let resp = driver.call("page", serde_json::json!({
            "pid": pid, "window_id": wid, "action": "click_element",
            "selector": "#btn-increment"
        }));
        // Prefer the tool text; fall back to a JSON-RPC error message.
        let text = if resp.text().is_empty() {
            resp.raw["error"]["message"].as_str().unwrap_or("").to_string()
        } else {
            resp.text().to_string()
        };
        assert!(!text.contains("probe JSON missing") && !text.contains("required field"),
            "click_element probe parse regressed: {text:?}");
        std::thread::sleep(Duration::from_millis(400));

        // Verify the click actually fired in the DOM.
        let post = driver.call("page", serde_json::json!({
            "pid": pid, "window_id": wid, "action": "execute_javascript",
            "javascript": "document.getElementById('lbl-counter').textContent"
        })).text().to_string();
        assert!(post.contains("counter=1"),
            "Counter didn't advance after page.click_element: {post:?}");
        println!("✅ harness_electron_click_element: probe parsed, click fired, counter=1");
    });
}
