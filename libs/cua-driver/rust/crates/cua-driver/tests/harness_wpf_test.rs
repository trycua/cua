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
//!
//! **Foreground-lock caveat:** a handful of these tests (`double_click`,
//! `right_click`, `type_text`) rely on `dispatch:"foreground"` to reach
//! WPF's input chain reliably. Windows' system-wide foreground-lock
//! kicks in after ~30s with no real user input — once that happens,
//! `SetForegroundWindow` is denied for non-UIAccess processes and the
//! tests fail. Run the WPF and WinUI3 suites as **separate** `cargo test`
//! invocations (`--test harness_wpf_test`, then `--test harness_winui3_test`)
//! rather than combined: each binary's first ~30s falls inside the
//! foreground-lock window and stays green. The sandbox runner does this
//! automatically.

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
        // Short fixed settle for cold-start (window-creation + initial
        // foreground hand-off after spawn) — the rest of the readiness
        // wait happens via polling in find_harness_window. A 200ms-only
        // wait turned out to be too short for the harness to establish
        // foreground reliably under test-batch load, which caused
        // SetForegroundWindow-needing tests (dispatch:foreground) to
        // fail with a foreground-lock rejection.
        std::thread::sleep(Duration::from_millis(800));
        Some(Self { _app: app, pid })
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        // Child::kill on Windows uses TerminateProcess, which signals exit
        // but doesn't synchronously reap. Without wait() the next test's
        // Harness::launch can briefly see TWO CuaTestHarness.Wpf windows —
        // and find_harness_window may pick the dying one, returning stale
        // element-cache coords (off-screen click failures).
        let _ = self._app.kill();
        let _ = self._app.wait();
        // Extra settle for the OS-level window destruction sweep.
        std::thread::sleep(Duration::from_millis(300));
    }
}

// Look up the harness's main window via list_windows, filtered to the
// exact spawned pid — defensive against the corner case where a previous
// test's harness hasn't been fully reaped and there are briefly two
// CuaTestHarness.Wpf windows on the desktop. Polls with a deadline rather
// than relying on a fixed pre-sleep, so cold-start in sandbox (where the
// WPF runtime + harness exe both pay first-launch JIT cost) doesn't time
// out the harness with a too-short fixed sleep.
fn find_harness_window(stdin: &mut ChildStdin, stdout: &mut BufReader<&mut ChildStdout>,
                       pid: u32, title_substr: &str) -> Option<(u64, String)> {
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let mut id = 10u32;
    loop {
        let resp = tools_call(stdin, stdout, id, "list_windows",
            serde_json::json!({ "pid": pid as i64 }));
        id = id.wrapping_add(1);
        if let Some(wins) = resp["result"]["structuredContent"]["windows"].as_array() {
            for w in wins {
                if w["pid"].as_u64() != Some(pid as u64) { continue; }
                let title = w["title"].as_str().unwrap_or("");
                if title.contains(title_substr) {
                    return Some((w["window_id"].as_u64()?, title.to_string()));
                }
            }
        }
        if std::time::Instant::now() >= deadline { return None; }
        std::thread::sleep(Duration::from_millis(150));
    }
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

// ── shared driver session helper ─────────────────────────────────────────────

/// Spin up a fresh harness + cua-driver pair, run the closure, then tear
/// everything down. Returns whatever the closure returns. The closure
/// receives the harness pid, the cua-driver stdin/stdout and a pre-resolved
/// main window_id.
fn with_session<F, R>(f: F) -> Option<R>
where F: FnOnce(u32, u64, &mut ChildStdin, &mut BufReader<&mut ChildStdout>) -> R {
    if !driver_binary().exists() { eprintln!("cua-driver.exe not built"); return None; }
    let harness = Harness::launch()?;
    let mut child = Command::new(&driver_binary())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn cua-driver");
    let mut stdin = child.stdin.take().unwrap();
    let mut raw_stdout = child.stdout.take().unwrap();
    let mut stdout = BufReader::new(&mut raw_stdout);
    init(&mut stdin, &mut stdout);
    let (wid, _) = find_harness_window(&mut stdin, &mut stdout, harness.pid, "CuaTestHarness WPF")
        .expect("main window");
    let out = f(harness.pid, wid, &mut stdin, &mut stdout);
    drop(stdout);
    drop(stdin);
    child.kill().ok();
    drop(harness);
    Some(out)
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

#[test]
#[ignore]
fn harness_wpf_type_text() {
    with_session(|pid, wid, stdin, stdout| {
        let snap = snapshot_elements(stdin, stdout, pid, wid);
        let idx = find_element_index_by_aid(&snap, "txt-input")
            .expect("txt-input not in snapshot");

        // WPF's TextBox needs *keyboard focus* for WM_CHAR delivery — and
        // PostMessage(WM_LBUTTONDOWN) doesn't reliably transfer keyboard
        // focus (WPF's input system treats posted events differently from
        // real ones). Use dispatch:"foreground" → SendInput synthesizes
        // an OS-level click that WPF treats identically to a user mouse,
        // landing actual keyboard focus on the TextBox.
        let _ = tools_call(stdin, stdout, 28, "bring_to_front", serde_json::json!({
            "pid": pid as i64, "window_id": wid
        }));
        std::thread::sleep(Duration::from_millis(300));

        let _ = tools_call(stdin, stdout, 29, "click", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "element_index": idx,
            "dispatch": "foreground"
        }));
        std::thread::sleep(Duration::from_millis(400));

        // SendInput's restore_foreground_polling_best_effort may yank
        // foreground back from the harness window between click and
        // type_text. Re-assert foreground so PostMessage WM_CHAR finds
        // the TextBox with keyboard focus.
        let _ = tools_call(stdin, stdout, 30, "bring_to_front", serde_json::json!({
            "pid": pid as i64, "window_id": wid
        }));
        std::thread::sleep(Duration::from_millis(300));

        let resp = tools_call(stdin, stdout, 31, "type_text", serde_json::json!({
            "pid": pid as i64, "text": "harness-typed"
        }));
        println!("type_text: {}", resp["result"]["content"][0]["text"]);
        std::thread::sleep(Duration::from_millis(700));

        let post = snapshot_elements(stdin, stdout, pid, wid);
        let text = snapshot_text(&post);
        let mirror_lines: Vec<&str> = text.lines()
            .filter(|l| l.contains("mirror=") || l.contains("txt-input"))
            .collect();
        assert!(text.contains("mirror=harness-typed"),
            "TextBox mirror did not reflect typed text. Mirror/input lines: {:?}",
            mirror_lines);
        println!("✅ harness_wpf_type_text: TextBox mirror advanced to 'harness-typed'");
    });
}

#[test]
#[ignore]
fn harness_wpf_set_value() {
    // Companion to harness_wpf_type_text: exercises the UIA ValuePattern
    // write path via the `set_value` tool. No focus needed — purely UIA.
    with_session(|pid, wid, stdin, stdout| {
        let snap = snapshot_elements(stdin, stdout, pid, wid);
        let idx = find_element_index_by_aid(&snap, "txt-input")
            .expect("txt-input not in snapshot");
        let _ = tools_call(stdin, stdout, 30, "set_value", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "element_index": idx,
            "value": "via-uia-setvalue"
        }));
        std::thread::sleep(Duration::from_millis(400));

        let post = snapshot_elements(stdin, stdout, pid, wid);
        let text = snapshot_text(&post);
        assert!(text.contains("mirror=via-uia-setvalue"),
            "set_value did not update TextBox. Excerpt: {}",
            text.chars().take(500).collect::<String>());
        println!("✅ harness_wpf_set_value: ValuePattern.SetValue wrote to TextBox");
    });
}

// In test-batch mode (many harnesses launched/killed in sequence) the WPF
// window's input pump occasionally misses background PostMessage events —
// reproducibly passes in isolation, intermittently fails in batch.
// `bring_to_front` pays the foreground swap once so the click test
// exercises the click-event-handling path itself, not the
// background-delivery path (which the counter_invoke test already
// covers via UIA Invoke).
fn focus_harness(stdin: &mut ChildStdin, stdout: &mut BufReader<&mut ChildStdout>,
                 pid: u32, wid: u64) {
    let _ = tools_call(stdin, stdout, 28, "bring_to_front", serde_json::json!({
        "pid": pid as i64, "window_id": wid
    }));
    std::thread::sleep(Duration::from_millis(300));
}

#[test]
#[ignore]
fn harness_wpf_right_click() {
    with_session(|pid, wid, stdin, stdout| {
        focus_harness(stdin, stdout, pid, wid);
        let snap = snapshot_elements(stdin, stdout, pid, wid);
        let idx = find_element_index_by_aid(&snap, "border-click-target")
            .expect("border-click-target not in snapshot");
        // Same dispatch:foreground rationale as type_text — PostMessage
        // WM_RBUTTONDOWN doesn't always reach WPF's MouseRightButtonDown
        // routed-event chain (intermittent in batch runs).
        let resp = tools_call(stdin, stdout, 30, "right_click", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "element_index": idx,
            "dispatch": "foreground"
        }));
        println!("right_click: {}", resp["result"]["content"][0]["text"]);
        std::thread::sleep(Duration::from_millis(400));

        let post = snapshot_elements(stdin, stdout, pid, wid);
        let text = snapshot_text(&post);
        let action_lines: Vec<&str> = text.lines()
            .filter(|l| l.contains("last_action=") || l.contains("clicks="))
            .collect();
        assert!(text.contains("last_action=right_click"),
            "right_click handler did not fire. Action/click lines: {:?}", action_lines);
        println!("✅ harness_wpf_right_click: last_action=right_click");
    });
}

#[test]
#[ignore]
fn harness_wpf_double_click() {
    with_session(|pid, wid, stdin, stdout| {
        focus_harness(stdin, stdout, pid, wid);
        let snap = snapshot_elements(stdin, stdout, pid, wid);
        let idx = find_element_index_by_aid(&snap, "border-click-target")
            .expect("border-click-target not in snapshot");
        // dispatch:foreground for the same reason as right_click —
        // PostMessage WM_LBUTTONDOWN ×2 doesn't always reach WPF's
        // MouseDoubleClick / ClickCount=2 path under test-batch load.
        let resp = tools_call(stdin, stdout, 30, "double_click", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "element_index": idx,
            "dispatch": "foreground"
        }));
        println!("double_click: {}", resp["result"]["content"][0]["text"]);
        std::thread::sleep(Duration::from_millis(400));

        let post = snapshot_elements(stdin, stdout, pid, wid);
        let text = snapshot_text(&post);
        let action_lines: Vec<&str> = text.lines()
            .filter(|l| l.contains("last_action=") || l.contains("clicks="))
            .collect();
        assert!(text.contains("last_action=double_click"),
            "double_click handler did not register a 2nd click. \
             Action/click lines: {:?}", action_lines);
        println!("✅ harness_wpf_double_click: last_action=double_click");
    });
}

#[test]
#[ignore]
fn harness_wpf_press_key_accelerator() {
    // F5 binding rather than the Ctrl+Shift+H one: cua-driver's hotkey
    // PostMessage path doesn't update OS modifier-key state (GetKeyState
    // returns "not pressed" for VK_CONTROL), so WPF's KeyBinding with
    // Modifiers=Control+Shift never matches. The UIA-worker SendInput
    // path would handle modifiers but requires the cua-driver-uia.exe
    // helper that isn't in our test config. F5 has no modifier and works
    // on the PostMessage path.
    with_session(|pid, wid, stdin, stdout| {
        let resp = tools_call(stdin, stdout, 30, "press_key", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "key": "f5"
        }));
        println!("press_key f5: {}", resp["result"]["content"][0]["text"]);
        std::thread::sleep(Duration::from_millis(400));

        let post = snapshot_elements(stdin, stdout, pid, wid);
        let text = snapshot_text(&post);
        assert!(text.contains("accel_fired=1"),
            "F5 KeyBinding did not fire. Snapshot excerpt: {}",
            text.chars().take(500).collect::<String>());
        println!("✅ harness_wpf_press_key_accelerator: accel_fired=1 (F5 via PostMessage)");
    });
}

#[test]
#[ignore]
fn harness_wpf_scroll() {
    with_session(|pid, wid, stdin, stdout| {
        // Pre-snapshot to populate the cache + read initial offset.
        let pre = snapshot_elements(stdin, stdout, pid, wid);
        let pre_text = snapshot_text(&pre);
        assert!(pre_text.contains("scroll_offset=0"),
            "expected initial scroll_offset=0, got: {}",
            pre_text.lines().filter(|l| l.contains("scroll_offset")).collect::<Vec<_>>().join(" / "));

        // Click into the ScrollViewer so it gets focus / its descendants
        // become the WM_VSCROLL target.
        let idx = find_element_index_by_aid(&pre, "scroll-tall")
            .expect("scroll-tall not in snapshot");
        let _ = tools_call(stdin, stdout, 30, "click", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "element_index": idx
        }));
        std::thread::sleep(Duration::from_millis(200));

        // Scroll down 5 lines.
        let resp = tools_call(stdin, stdout, 31, "scroll", serde_json::json!({
            "pid": pid as i64, "window_id": wid,
            "direction": "down", "by": "line", "amount": 5
        }));
        println!("scroll down: {}", resp["result"]["content"][0]["text"]);
        std::thread::sleep(Duration::from_millis(400));

        let post = snapshot_elements(stdin, stdout, pid, wid);
        let text = snapshot_text(&post);
        let advanced = text.lines().any(|l|
            l.contains("scroll_offset=") && !l.contains("scroll_offset=0\""));
        assert!(advanced, "scroll offset did not advance after WM_VSCROLL. Lines: {}",
            text.lines().filter(|l| l.contains("scroll_offset"))
                .collect::<Vec<_>>().join(" / "));
        println!("✅ harness_wpf_scroll: scroll_offset advanced past 0");
    });
}

#[test]
#[ignore]
fn harness_wpf_modal_messagebox() {
    with_session(|pid, wid, stdin, stdout| {
        let snap = snapshot_elements(stdin, stdout, pid, wid);
        let idx = find_element_index_by_aid(&snap, "btn-open-msgbox")
            .expect("btn-open-msgbox not in snapshot");
        let _ = tools_call(stdin, stdout, 30, "click", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "element_index": idx
        }));
        std::thread::sleep(Duration::from_millis(600));

        // List windows — the modal MessageBox should be a new top-level window
        // owned by the same pid.
        let resp = tools_call(stdin, stdout, 31, "list_windows", serde_json::json!({
            "pid": pid as i64
        }));
        let windows = resp["result"]["structuredContent"]["windows"].as_array()
            .expect("windows array");
        let modal = windows.iter()
            .find(|w| w["title"].as_str().map(|t| t.contains("Harness MessageBox")).unwrap_or(false))
            .expect("Harness MessageBox modal window not found");
        let modal_wid = modal["window_id"].as_u64().unwrap();
        println!("modal window_id={}", modal_wid);

        // Walk the modal's UIA tree — expect OK and Cancel buttons.
        let modal_snap = tools_call(stdin, stdout, 32, "get_window_state", serde_json::json!({
            "pid": pid as i64, "window_id": modal_wid, "capture_mode": "tree"
        }));
        let modal_text = snapshot_text(&modal_snap);
        assert!(modal_text.contains("\"OK\""),
            "MessageBox UIA tree missing OK button. Tree: {}",
            modal_text.chars().take(800).collect::<String>());
        assert!(modal_text.contains("\"Cancel\""),
            "MessageBox UIA tree missing Cancel button");

        // Dismiss by clicking Cancel in the modal.
        let cancel_idx = modal_text.lines()
            .find(|l| l.contains("\"Cancel\"") && l.contains('['))
            .and_then(|l| {
                let s = l.find('[')? + 1;
                let e = l[s..].find(']')? + s;
                l[s..e].trim().parse::<u64>().ok()
            })
            .expect("Cancel button element_index not parseable");
        let _ = tools_call(stdin, stdout, 33, "click", serde_json::json!({
            "pid": pid as i64, "window_id": modal_wid, "element_index": cancel_idx
        }));
        std::thread::sleep(Duration::from_millis(400));
        println!("✅ harness_wpf_modal_messagebox: opened + parsed + dismissed");
    });
}

#[test]
#[ignore]
fn harness_wpf_owned_popup() {
    with_session(|pid, wid, stdin, stdout| {
        let snap = snapshot_elements(stdin, stdout, pid, wid);
        let idx = find_element_index_by_aid(&snap, "btn-open-owned")
            .expect("btn-open-owned not in snapshot");
        let _ = tools_call(stdin, stdout, 30, "click", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "element_index": idx
        }));
        std::thread::sleep(Duration::from_millis(500));

        let resp = tools_call(stdin, stdout, 31, "list_windows", serde_json::json!({
            "pid": pid as i64
        }));
        let windows = resp["result"]["structuredContent"]["windows"].as_array().unwrap();
        let owned = windows.iter()
            .find(|w| w["title"].as_str().map(|t| t.contains("Harness Owned Popup")).unwrap_or(false))
            .expect("Harness Owned Popup window not found in list_windows");
        let owned_wid = owned["window_id"].as_u64().unwrap();

        let owned_snap = tools_call(stdin, stdout, 32, "get_window_state", serde_json::json!({
            "pid": pid as i64, "window_id": owned_wid, "capture_mode": "tree"
        }));
        let owned_text = snapshot_text(&owned_snap);
        assert!(owned_text.contains("OWNED_POPUP_MARKER_v1"),
            "owned popup body marker missing. Tree: {}",
            owned_text.chars().take(600).collect::<String>());
        println!("✅ harness_wpf_owned_popup: opened + parsed");
    });
}

#[test]
#[ignore]
fn harness_wpf_layered_popup_capture() {
    with_session(|pid, wid, stdin, stdout| {
        let snap = snapshot_elements(stdin, stdout, pid, wid);
        let idx = find_element_index_by_aid(&snap, "btn-open-layered")
            .expect("btn-open-layered not in snapshot");
        let _ = tools_call(stdin, stdout, 30, "click", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "element_index": idx
        }));
        std::thread::sleep(Duration::from_millis(600));

        let resp = tools_call(stdin, stdout, 31, "list_windows", serde_json::json!({
            "pid": pid as i64
        }));
        let windows = resp["result"]["structuredContent"]["windows"].as_array().unwrap();
        let layered = windows.iter()
            .find(|w| w["title"].as_str().map(|t| t.contains("Harness Layered Popup")).unwrap_or(false))
            .expect("Harness Layered Popup window not found");
        let layered_wid = layered["window_id"].as_u64().unwrap();

        // Capture-only path — assert the screenshot is not all-black, which
        // is the failure mode for PrintWindow against layered windows
        // without the WGC fallback.
        let cap = tools_call(stdin, stdout, 32, "get_window_state", serde_json::json!({
            "pid": pid as i64, "window_id": layered_wid, "capture_mode": "vision"
        }));
        let img_b64 = cap["result"]["content"].as_array()
            .and_then(|arr| arr.iter().find_map(|c| {
                if c["type"] == "image" { c["data"].as_str() } else { None }
            }))
            .expect("layered window capture returned no image");
        // Decode the PNG and look for any non-black pixel.
        let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, img_b64)
            .expect("base64");
        let img = image::load_from_memory(&bytes).expect("png decode");
        let rgb = img.to_rgb8();
        let any_color = rgb.pixels().any(|p| p.0[0] > 12 || p.0[1] > 12 || p.0[2] > 12);
        assert!(any_color,
            "layered window capture is all-black ({}x{}). PrintWindow likely needs WGC fallback.",
            rgb.width(), rgb.height());
        println!("✅ harness_wpf_layered_popup_capture: capture has non-black pixels ({}x{})",
                 rgb.width(), rgb.height());
    });
}

// ── slider / checkable / combo / list / menu coverage ────────────────────────

#[test]
#[ignore]
fn harness_wpf_slider_drag_tool_returns() {
    // Coverage note for the drag tool against a WPF Slider thumb.
    //
    // PostMessage WM_LBUTTONDOWN / WM_MOUSEMOVE / WM_LBUTTONUP doesn't
    // update the OS keyboard/mouse state visible to GetKeyState. WPF's
    // Slider Thumb relies on Mouse.LeftButton (which polls GetKeyState)
    // to recognise an in-progress drag — so a PostMessage drag never
    // moves a WPF thumb, even when from/to are correctly on the thumb
    // in client coords. The companion `harness_wpf_slider_increase_large`
    // test covers the slider via UIA Invoke on its internal IncreaseLarge
    // sub-button, which is what an agent SHOULD use for slider
    // manipulation on a backgrounded window.
    //
    // We still exercise the drag tool against the slider so its codepath
    // (coord translation, dispatch policy, message synthesis) is on the
    // critical-path test list — we just don't assert on the value moving.
    // TODO: add a SendInput-based drag path so dispatch:"foreground" can
    // drive the thumb, then enable a behavioral assertion here.
    with_session(|pid, wid, stdin, stdout| {
        focus_harness(stdin, stdout, pid, wid);
        let resp = tools_call(stdin, stdout, 30, "drag", serde_json::json!({
            "pid": pid as i64, "window_id": wid,
            "from_x": 50.0, "from_y": 275.0,
            "to_x": 330.0,  "to_y": 275.0,
            "duration_ms": 600, "steps": 30
        }));
        let msg = resp["result"]["content"][0]["text"].as_str().unwrap_or("");
        println!("drag slider: {msg}");
        assert!(msg.starts_with("✅"),
            "drag tool returned non-success: {msg}");
        println!("✅ harness_wpf_slider_drag_tool_returns: PostMessage drag emitted (known no-op vs WPF Thumb)");
    });
}

#[test]
#[ignore]
fn harness_wpf_slider_increase_large() {
    // Companion to slider_drag — exercises UIA Invoke on the Slider's
    // internal IncreaseLarge "page-up" button. Doesn't depend on screen
    // coords, so it's the more robust slider integration test.
    with_session(|pid, wid, stdin, stdout| {
        focus_harness(stdin, stdout, pid, wid);
        let snap = snapshot_elements(stdin, stdout, pid, wid);
        let idx = find_element_index_by_aid(&snap, "IncreaseLarge")
            .expect("slider IncreaseLarge button not in snapshot");
        for i in 0..3 {
            let resp = tools_call(stdin, stdout, 30 + i, "click", serde_json::json!({
                "pid": pid as i64, "window_id": wid, "element_index": idx
            }));
            println!("invoke IncreaseLarge #{i}: {}", resp["result"]["content"][0]["text"]);
            std::thread::sleep(Duration::from_millis(150));
        }
        std::thread::sleep(Duration::from_millis(300));
        let post = snapshot_elements(stdin, stdout, pid, wid);
        let text = snapshot_text(&post);
        let advanced = text.lines().any(|l|
            l.contains("slider_value=") && !l.contains("slider_value=0\""));
        assert!(advanced, "slider IncreaseLarge invokes did not advance value. Lines: {}",
            text.lines().filter(|l| l.contains("slider_value")).collect::<Vec<_>>().join(" / "));
        println!("✅ harness_wpf_slider_increase_large: advanced via UIA Invoke");
    });
}

#[test]
#[ignore]
fn harness_wpf_checkbox_toggle() {
    with_session(|pid, wid, stdin, stdout| {
        focus_harness(stdin, stdout, pid, wid);
        let snap = snapshot_elements(stdin, stdout, pid, wid);
        let idx = find_element_index_by_aid(&snap, "chk-agreed")
            .expect("chk-agreed missing");
        // CheckBox exposes UIA TogglePattern (actions=[toggle]), not Invoke.
        // cua-driver's click tool tries UIA Invoke first; for elements that
        // don't support it the PostMessage fallback path runs. Use
        // dispatch:"foreground" to land a SendInput click that WPF
        // recognises as a real user click and processes through Toggle.
        let resp = tools_call(stdin, stdout, 30, "click", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "element_index": idx,
            "dispatch": "foreground"
        }));
        println!("click chk-agreed: {}", resp["result"]["content"][0]["text"]);
        std::thread::sleep(Duration::from_millis(400));
        let post = snapshot_elements(stdin, stdout, pid, wid);
        assert!(snapshot_text(&post).contains("agreed=True"),
            "checkbox didn't toggle: {}",
            snapshot_text(&post).lines().filter(|l| l.contains("agreed=")).collect::<Vec<_>>().join(" / "));
        println!("✅ harness_wpf_checkbox_toggle: agreed=True");
    });
}

#[test]
#[ignore]
fn harness_wpf_radio_select() {
    with_session(|pid, wid, stdin, stdout| {
        focus_harness(stdin, stdout, pid, wid);
        let snap = snapshot_elements(stdin, stdout, pid, wid);
        let idx = find_element_index_by_aid(&snap, "rdo-high")
            .expect("rdo-high missing");
        // RadioButton exposes SelectionItem pattern (actions=[select]).
        // Same dispatch:foreground rationale as the checkbox test.
        let _ = tools_call(stdin, stdout, 30, "click", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "element_index": idx,
            "dispatch": "foreground"
        }));
        std::thread::sleep(Duration::from_millis(400));
        let post = snapshot_elements(stdin, stdout, pid, wid);
        assert!(snapshot_text(&post).contains("prio=High"),
            "radio didn't switch to High");
        println!("✅ harness_wpf_radio_select: prio=High");
    });
}

#[test]
#[ignore]
fn harness_wpf_combo_select() {
    with_session(|pid, wid, stdin, stdout| {
        focus_harness(stdin, stdout, pid, wid);
        let snap = snapshot_elements(stdin, stdout, pid, wid);
        let combo_idx = find_element_index_by_aid(&snap, "cbo-color")
            .expect("cbo-color missing");
        // WPF ComboBox UIA peer surfaces ExpandCollapsePattern (actions=[expand])
        // but not ValuePattern — set_value at the parent is a no-op. Standard
        // recipe: invoke the combo to expand the dropdown, re-snapshot so the
        // item AIDs land in the element cache, then click the target item.
        let _ = tools_call(stdin, stdout, 30, "click", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "element_index": combo_idx,
            "dispatch": "foreground"
        }));
        std::thread::sleep(Duration::from_millis(500));

        let snap2 = snapshot_elements(stdin, stdout, pid, wid);
        let item_idx = find_element_index_by_aid(&snap2, "cbo-item-orange")
            .expect("cbo-item-orange missing after expand");
        let _ = tools_call(stdin, stdout, 31, "click", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "element_index": item_idx,
            "dispatch": "foreground"
        }));
        std::thread::sleep(Duration::from_millis(500));

        let post = snapshot_elements(stdin, stdout, pid, wid);
        assert!(snapshot_text(&post).contains("color=orange"),
            "combo didn't switch to orange: {}",
            snapshot_text(&post).lines().filter(|l| l.contains("color=")).collect::<Vec<_>>().join(" / "));
        println!("✅ harness_wpf_combo_select: color=orange");
    });
}

#[test]
#[ignore]
fn harness_wpf_listbox_select() {
    with_session(|pid, wid, stdin, stdout| {
        focus_harness(stdin, stdout, pid, wid);
        let snap = snapshot_elements(stdin, stdout, pid, wid);
        let idx = find_element_index_by_aid(&snap, "lst-cherry")
            .expect("lst-cherry missing");
        let _ = tools_call(stdin, stdout, 30, "click", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "element_index": idx,
            "dispatch": "foreground"
        }));
        std::thread::sleep(Duration::from_millis(400));
        let post = snapshot_elements(stdin, stdout, pid, wid);
        assert!(snapshot_text(&post).contains("selected=cherry"),
            "list didn't select cherry: {}",
            snapshot_text(&post).lines().filter(|l| l.contains("selected=")).collect::<Vec<_>>().join(" / "));
        println!("✅ harness_wpf_listbox_select: selected=cherry");
    });
}

#[test]
#[ignore]
fn harness_wpf_menu_invoke() {
    with_session(|pid, wid, stdin, stdout| {
        focus_harness(stdin, stdout, pid, wid);
        // Expand File menu first (UIA expand pattern on MenuItem)
        let snap = snapshot_elements(stdin, stdout, pid, wid);
        let file_idx = find_element_index_by_aid(&snap, "menu-file")
            .expect("menu-file missing");
        let _ = tools_call(stdin, stdout, 30, "click", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "element_index": file_idx,
            "dispatch": "foreground"
        }));
        std::thread::sleep(Duration::from_millis(400));

        // Re-snapshot so menu-file-new is in the cache (it materialized
        // when the menu expanded).
        let snap2 = snapshot_elements(stdin, stdout, pid, wid);
        let new_idx = find_element_index_by_aid(&snap2, "menu-file-new")
            .expect("menu-file-new missing after expand");
        let _ = tools_call(stdin, stdout, 31, "click", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "element_index": new_idx,
            "dispatch": "foreground"
        }));
        std::thread::sleep(Duration::from_millis(500));

        let post = snapshot_elements(stdin, stdout, pid, wid);
        assert!(snapshot_text(&post).contains("menu_action=file_new"),
            "File>New didn't invoke: {}",
            snapshot_text(&post).lines().filter(|l| l.contains("menu_action=")).collect::<Vec<_>>().join(" / "));
        println!("✅ harness_wpf_menu_invoke: menu_action=file_new");
    });
}
