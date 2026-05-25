//! Integration test against the CuaTestHarness.Wpf .NET 8 app.
//!
//! Pairs of test apps live under `libs/cua-driver/test-harness/`. They
//! publish into `libs/cua-driver/rust/test-apps/harness-{wpf,winui3}/` and
//! get mapped into the sandbox via the existing run-tests-in-sandbox.ps1
//! mapped folder.
//!
//! Each scenario covers a Win32 hosting pattern the agent should handle:
//!   - counter        : UIA Invoke on a plain WPF button
//!   - text_body      : get_window_state extracts known marker text
//!   - message_box    : modal MessageBox enumeration
//!   - bottom_strip   : Save/Cancel buttons present in the UIA tree
//!                      (regression guard for the GetClientRect-vs-
//!                      GetWindowRect capture bug fixed in #1696)
//!   - owned_popup    : owned secondary window discovered via list_windows
//!   - layered_popup  : WS_EX_LAYERED window enumerated and captured
//!   - child_hwnd     : native Win32 BUTTON child HWND visible in tree
//!
//! Run via the sandbox runner:
//!   .\sandbox\run-tests-in-sandbox.ps1 harness_wpf
//!
//! Or locally (requires .NET 8 SDK + `test-harness/build.ps1`):
//!   cargo test --test harness_wpf_test -- --ignored --nocapture
//!
//! Tests are `#[ignore]` so they don't run in plain `cargo test`. The
//! sandbox runner unignores them explicitly via the `--ignored` arg.

#![cfg(target_os = "windows")]

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Duration;

// ── path helpers ─────────────────────────────────────────────────────────────

fn workspace_root() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest).parent().unwrap().parent().unwrap().to_owned()
}

fn driver_binary() -> PathBuf {
    workspace_root().join("target/debug/cua-driver.exe")
}

fn harness_exe() -> PathBuf {
    if let Ok(p) = std::env::var("HARNESS_WPF_EXE") {
        let pb = PathBuf::from(p);
        if pb.exists() { return pb; }
    }
    workspace_root().join("test-apps/harness-wpf/CuaTestHarness.Wpf.exe")
}

// ── JSON-RPC helpers ─────────────────────────────────────────────────────────

fn send(stdin: &mut ChildStdin, req: serde_json::Value) {
    writeln!(stdin, "{}", serde_json::to_string(&req).unwrap()).unwrap();
}

fn recv(stdout: &mut BufReader<&mut ChildStdout>) -> serde_json::Value {
    let mut line = String::new();
    stdout.read_line(&mut line).expect("read response");
    serde_json::from_str(line.trim()).expect("parse json")
}

fn init(stdin: &mut ChildStdin, stdout: &mut BufReader<&mut ChildStdout>) {
    send(stdin, serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    let _ = recv(stdout);
}

fn tools_call(stdin: &mut ChildStdin, stdout: &mut BufReader<&mut ChildStdout>,
              id: u32, name: &str, args: serde_json::Value) -> serde_json::Value {
    send(stdin, serde_json::json!({
        "jsonrpc": "2.0", "id": id, "method": "tools/call",
        "params": { "name": name, "arguments": args }
    }));
    recv(stdout)
}

// ── harness fixture ──────────────────────────────────────────────────────────

struct Harness {
    _app: Child,
    pid: u32,
}

impl Harness {
    fn launch() -> Option<Self> {
        let exe = harness_exe();
        if !exe.exists() {
            eprintln!("harness exe not found at {exe:?} — run test-harness/build.ps1 first");
            return None;
        }
        let app = Command::new(&exe)
            .stdout(Stdio::null()).stderr(Stdio::null())
            .spawn().ok()?;
        let pid = app.id();
        // Settle. WPF cold-start in sandbox is slow.
        std::thread::sleep(Duration::from_secs(3));
        Some(Self { _app: app, pid })
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self._app.kill();
    }
}

// Look up the harness's main window via list_windows so the test doesn't
// depend on title strings that callers might localize later. Returns
// (window_id, title).
fn find_harness_window(stdin: &mut ChildStdin, stdout: &mut BufReader<&mut ChildStdout>,
                       pid: u32, title_substr: &str) -> Option<(u64, String)> {
    let resp = tools_call(stdin, stdout, 10, "list_windows",
        serde_json::json!({ "pid": pid as i64 }));
    let wins = resp["result"]["structuredContent"]["windows"].as_array()?;
    for w in wins {
        let title = w["title"].as_str().unwrap_or("");
        if title.contains(title_substr) {
            return Some((w["window_id"].as_u64()?, title.to_string()));
        }
    }
    None
}

// Get a UIA snapshot's flat element list — used to assert AutomationIds
// exist in the tree without depending on tree-walk order.
fn snapshot_elements(stdin: &mut ChildStdin, stdout: &mut BufReader<&mut ChildStdout>,
                     pid: u32, window_id: u64) -> serde_json::Value {
    let resp = tools_call(stdin, stdout, 20, "get_window_state",
        serde_json::json!({
            "pid": pid as i64,
            "window_id": window_id,
            "capture_mode": "tree"
        }));
    resp
}

fn snapshot_text(snapshot: &serde_json::Value) -> &str {
    snapshot["result"]["content"][0]["text"].as_str().unwrap_or("")
}

fn elements_have_aid(snapshot: &serde_json::Value, aid: &str) -> bool {
    // UIA tree is rendered as markdown lines like:
    //   `  - [3] Button "Increment" [id=btn-increment actions=[click]]`
    // so the substring `id=<aid>` is a reliable presence marker.
    snapshot_text(snapshot).contains(&format!("id={aid}"))
}

/// Parse the UIA markdown snapshot for the line matching `id=<aid>` and
/// extract the leading `[<index>]` element_index token.
fn find_element_index_by_aid(snapshot: &serde_json::Value, aid: &str) -> Option<u64> {
    let needle = format!("id={aid}");
    for line in snapshot_text(snapshot).lines() {
        if !line.contains(&needle) { continue; }
        let start = line.find('[')? + 1;
        let end   = line[start..].find(']')? + start;
        return line[start..end].trim().parse().ok();
    }
    None
}

// ── tests ────────────────────────────────────────────────────────────────────

#[test]
#[ignore]
fn harness_wpf_smoke() {
    let driver = driver_binary();
    if !driver.exists() {
        eprintln!("cua-driver.exe not built — run `cargo build` first");
        return;
    }
    let harness = match Harness::launch() {
        Some(h) => h,
        None => { eprintln!("harness not built — skipping"); return; }
    };
    println!("harness pid={}", harness.pid);

    let mut child = Command::new(&driver)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn cua-driver");
    let mut stdin = child.stdin.take().unwrap();
    let mut raw_stdout = child.stdout.take().unwrap();
    let mut stdout = BufReader::new(&mut raw_stdout);

    init(&mut stdin, &mut stdout);

    let (wid, title) = find_harness_window(&mut stdin, &mut stdout, harness.pid, "CuaTestHarness WPF")
        .expect("main window not found via list_windows");
    println!("main window: id={} title={:?}", wid, title);

    let snap = snapshot_elements(&mut stdin, &mut stdout, harness.pid, wid);
    let text = snapshot_text(&snap);

    // Buttons appear with explicit id=<aid> tags in the UIA markdown.
    for aid in [
        "btn-increment", "btn-reset",
        "btn-open-msgbox",
        "btn-save", "btn-cancel",                       // regression guard for #1696
        "btn-open-owned", "btn-open-layered",
        "btn-exit",
    ] {
        assert!(elements_have_aid(&snap, aid), "missing AutomationId {aid} in WPF UIA snapshot");
    }

    // TextBlocks are reported as bare Text nodes (no UIA Invoke/Value pattern,
    // no AutomationId in the rendered tree). Assert on their content instead.
    assert!(text.contains("HARNESS_TEXT_MARKER_v1"), "text_body marker not in snapshot");
    assert!(text.contains("counter=0"),             "initial counter label not in snapshot");
    assert!(text.contains("accel_fired=0"),         "initial accel label not in snapshot");

    // HwndHost child should surface the native Win32 BUTTON as a UIA Button.
    assert!(text.contains("\"Native Win32 Child\""), "native HWND child button not in snapshot");

    println!("✅ harness_wpf_smoke: all expected scenarios present in UIA tree");

    child.kill().ok();
}

#[test]
#[ignore]
fn harness_wpf_counter_invoke() {
    let driver = driver_binary();
    if !driver.exists() { return; }
    let harness = match Harness::launch() { Some(h) => h, None => return };

    let mut child = Command::new(&driver)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn cua-driver");
    let mut stdin = child.stdin.take().unwrap();
    let mut raw_stdout = child.stdout.take().unwrap();
    let mut stdout = BufReader::new(&mut raw_stdout);
    init(&mut stdin, &mut stdout);

    let (wid, _) = find_harness_window(&mut stdin, &mut stdout, harness.pid, "CuaTestHarness WPF")
        .expect("main window");
    // Pre-snapshot so element_cache has indices we can address.
    let pre = snapshot_elements(&mut stdin, &mut stdout, harness.pid, wid);
    let idx = find_element_index_by_aid(&pre, "btn-increment")
        .expect("btn-increment not in pre-snapshot");

    let click = tools_call(&mut stdin, &mut stdout, 30, "click",
        serde_json::json!({
            "pid": harness.pid as i64,
            "window_id": wid,
            "element_index": idx
        }));
    println!("click [{idx}] btn-increment: {}", click["result"]["content"][0]["text"]);

    std::thread::sleep(Duration::from_millis(300));

    let post = snapshot_elements(&mut stdin, &mut stdout, harness.pid, wid);
    let text = snapshot_text(&post);
    assert!(
        text.contains("counter=1"),
        "counter label did not advance after click — snapshot text: {}",
        text.chars().take(400).collect::<String>()
    );
    println!("✅ harness_wpf_counter_invoke: counter advanced to 1");

    child.kill().ok();
}
