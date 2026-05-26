//! Integration tests against the CuaTestHarness.WebView (WPF + WebView2)
//! and CuaTestHarness.Electron hosts. Both load the same
//! `test-harness/shared-web/index.html`, so the same `page` tool flows
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

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Duration;

// ── workspace paths ──────────────────────────────────────────────────────────

fn workspace_root() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest).parent().unwrap().parent().unwrap().to_owned()
}
fn driver_binary() -> PathBuf { workspace_root().join("target/debug/cua-driver.exe") }

fn webview_exe() -> PathBuf {
    if let Ok(p) = std::env::var("HARNESS_WEBVIEW_EXE") {
        let pb = PathBuf::from(p);
        if pb.exists() { return pb; }
    }
    workspace_root().join("test-apps/harness-webview/CuaTestHarness.WebView.exe")
}
fn electron_exe() -> PathBuf {
    if let Ok(p) = std::env::var("HARNESS_ELECTRON_EXE") {
        let pb = PathBuf::from(p);
        if pb.exists() { return pb; }
    }
    workspace_root().join("test-apps/harness-electron/CuaTestHarness.Electron.exe")
}

// ── JSON-RPC plumbing ────────────────────────────────────────────────────────

fn send(stdin: &mut ChildStdin, req: serde_json::Value) {
    writeln!(stdin, "{}", serde_json::to_string(&req).unwrap()).unwrap();
}
fn recv(stdout: &mut BufReader<&mut ChildStdout>) -> serde_json::Value {
    let mut line = String::new();
    stdout.read_line(&mut line).expect("read");
    serde_json::from_str(line.trim()).expect("json")
}
fn init(stdin: &mut ChildStdin, stdout: &mut BufReader<&mut ChildStdout>) {
    send(stdin, serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    let _ = recv(stdout);
}
fn tools_call(stdin: &mut ChildStdin, stdout: &mut BufReader<&mut ChildStdout>,
              id: u32, name: &str, args: serde_json::Value) -> serde_json::Value {
    send(stdin, serde_json::json!({
        "jsonrpc":"2.0","id":id,"method":"tools/call",
        "params":{"name":name,"arguments":args}
    }));
    recv(stdout)
}

fn find_window_by_title(stdin: &mut ChildStdin, stdout: &mut BufReader<&mut ChildStdout>,
                        pid: u32, title_substr: &str) -> Option<u64> {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let mut id = 10u32;
    loop {
        let resp = tools_call(stdin, stdout, id, "list_windows", serde_json::json!({"pid": pid as i64}));
        id = id.wrapping_add(1);
        if let Some(wins) = resp["result"]["structuredContent"]["windows"].as_array() {
            for w in wins {
                if w["pid"].as_u64() != Some(pid as u64) { continue; }
                if w["title"].as_str().unwrap_or("").contains(title_substr) {
                    if let Some(wid) = w["window_id"].as_u64() { return Some(wid); }
                }
            }
        }
        if std::time::Instant::now() >= deadline { return None; }
        std::thread::sleep(Duration::from_millis(200));
    }
}

// ── shared session helper ────────────────────────────────────────────────────

/// Launch the harness exe + a cua-driver child with `CUA_DRIVER_CDP_PORT`
/// pointing at the harness's CDP endpoint. Polls list_windows until the
/// host's window appears.
struct WebSession {
    _app: Child,
    driver: Child,
}

impl Drop for WebSession {
    fn drop(&mut self) {
        let _ = self.driver.kill();
        let _ = self.driver.wait();
        let _ = self._app.kill();
        let _ = self._app.wait();
        // settle so the next test's launch doesn't see leftover windows
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn run_with_session<F>(label: &str, host_exe: PathBuf, title_substr: &str, cdp_port: u16, f: F)
where F: FnOnce(u32, u64, &mut ChildStdin, &mut BufReader<&mut ChildStdout>) {
    if !driver_binary().exists() {
        eprintln!("cua-driver.exe not built — run `cargo build` first"); return;
    }
    if !host_exe.exists() {
        eprintln!("{label} host exe not found at {host_exe:?} — run test-harness/build.ps1"); return;
    }
    // Set the CDP port the host should use so the daemon can find it.
    let env_var = if label == "webview" { "CUA_WEBVIEW_CDP_PORT" } else { "CUA_ELECTRON_CDP_PORT" };
    let app = Command::new(&host_exe)
        .env(env_var, cdp_port.to_string())
        .stdout(Stdio::null()).stderr(Stdio::null())
        .spawn().expect("spawn host");
    let pid = app.id();
    println!("{label} pid={pid} cdp_port={cdp_port}");
    std::thread::sleep(Duration::from_secs(2));   // small cold-start for runtime spin-up

    let mut driver = Command::new(driver_binary())
        .env("CUA_DRIVER_CDP_PORT", cdp_port.to_string())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn cua-driver");
    let mut stdin = driver.stdin.take().unwrap();
    let mut raw_stdout = driver.stdout.take().unwrap();
    let mut stdout = BufReader::new(&mut raw_stdout);
    init(&mut stdin, &mut stdout);

    let wid = find_window_by_title(&mut stdin, &mut stdout, pid, title_substr)
        .unwrap_or_else(|| panic!("{label} window with title containing {title_substr:?} not found"));

    let session = WebSession { _app: app, driver };
    f(pid, wid, &mut stdin, &mut stdout);
    drop(session);
}

// ── WebView2 structural ──────────────────────────────────────────────────────

#[test]
#[ignore]
fn harness_webview_window_discoverable() {
    // Smoke test: WebView2 harness launches, window appears via list_windows.
    // Behavioural page-tool tests are deferred until WebView2 actually opens
    // its CDP listener — see the module docstring TODO.
    run_with_session("webview", webview_exe(), "CuaTestHarness WebView", 9222,
        |pid, wid, _stdin, _stdout| {
        println!("✅ harness_webview_window_discoverable: pid={pid} wid={wid}");
    });
}

// ── Electron structural + page tool ──────────────────────────────────────────

#[test]
#[ignore]
fn harness_electron_window_discoverable() {
    run_with_session("electron", electron_exe(), "CuaTestHarness Electron", 9223,
        |pid, wid, _stdin, _stdout| {
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
        |pid, wid, stdin, stdout| {

        // 1. execute_javascript via CDP.
        let marker_resp = tools_call(stdin, stdout, 30, "page", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "action": "execute_javascript",
            "javascript": "document.querySelector('[data-cua-id=\"page-marker\"]').textContent"
        }));
        let marker = marker_resp["result"]["content"][0]["text"].as_str().unwrap_or("");
        assert!(marker.contains("WEB_HARNESS_MARKER_v1"),
            "Electron CDP execute_javascript marker fetch: {marker:?}");

        // 2. Increment counter via direct execute_javascript (the
        //    click_element path has a separate probe-JSON-parsing gap
        //    documented below — track separately).
        let _ = tools_call(stdin, stdout, 31, "page", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "action": "execute_javascript",
            "javascript": "document.getElementById('btn-increment').click()"
        }));
        std::thread::sleep(Duration::from_millis(300));

        let counter_resp = tools_call(stdin, stdout, 32, "page", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "action": "execute_javascript",
            "javascript": "document.getElementById('lbl-counter').textContent"
        }));
        let post = counter_resp["result"]["content"][0]["text"].as_str().unwrap_or("");
        assert!(post.contains("counter=1"),
            "Electron counter did not advance via execute_javascript: {post:?}");
        println!("✅ harness_electron_page_tool: CDP+execute_javascript green");
    });
}

/// Documents a separate gap in page.click_element: its CDP probe runs JS
/// that returns a JSON object (vx/vy/sx/sy/dpr) but the result is wrapped
/// as a CDP `runtime.evaluate.user_gesture` string by the time the page
/// tool's JSON parser sees it — causing "probe JSON missing required
/// field 'vx'". Fix: unwrap the CDP result's `.value` before parsing,
/// or send the probe with `returnByValue:true` in the Runtime.evaluate
/// params. Manually unwrapping the harness's verified JSON string works.
#[test]
#[ignore]
fn harness_electron_click_element_DOCUMENTED_wrapper_bug() {
    run_with_session("electron", electron_exe(), "CuaTestHarness Electron", 9223,
        |pid, wid, stdin, stdout| {
        let resp = tools_call(stdin, stdout, 30, "page", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "action": "click_element",
            "selector": "#btn-increment"
        }));
        // Safe traversal — error responses from `page` can vary in shape
        // (sometimes `result.isError: true` + `content[0].text`, sometimes
        // `error.message`). Don't blow up on a missing array entry.
        let text = resp.get("result")
            .and_then(|r| r.get("content"))
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.get(0))
            .and_then(|item| item.get("text"))
            .and_then(|t| t.as_str())
            .or_else(|| resp.get("error").and_then(|e| e.get("message")).and_then(|m| m.as_str()))
            .unwrap_or("");
        // Expect the gap: parser fails on the CDP-wrapped probe response.
        let expected_gap = text.contains("probe JSON missing") || text.contains("required field");
        assert!(expected_gap,
            "Expected click_element probe-wrapper gap. Got: {text:?}. \
             If this asserts on a success message, the wrapper-unwrap fix is in — \
             flip this assertion to assert success.");
        println!("⚠️  harness_electron_click_element_DOCUMENTED_wrapper_bug: probe JSON parse gap present");
    });
}
