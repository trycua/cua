//! Integration test against the CuaTestHarness.AppKit Swift app.
//!
//! Mirror of `harness_wpf_test.rs` for the macOS AppKit hosting pattern.
//! The harness app lives at `libs/cua-driver/test-harness/CuaTestHarness.AppKit`
//! and is published into `libs/cua-driver/rust/test-apps/harness-appkit/`
//! by `libs/cua-driver/test-harness/build.sh`.
//!
//! Scenarios (see `libs/cua-driver/test-harness/scenarios/scenarios.json`
//! `appkit` section):
//!   - counter        : NSButton AXPress invocation increments counter
//!   - text_body      : get_window_state extracts known marker text
//!   - text_input     : type_text into NSTextField updates mirror label
//!   - click_target   : right_click / double_click recognised by NSView
//!   - scroll_target  : scroll updates VerticalOffset label
//!   - ns_menubar     : main menubar item enumerable (Mac-specific)
//!
//! Run locally (after `libs/cua-driver/test-harness/build.sh`):
//!   cargo test --test harness_appkit_test -- --ignored --nocapture
//!
//! Tests are `#[ignore]` so they don't run in plain `cargo test`.
//!
//! **TCC caveat:** on a fresh Mac, the cua-driver process needs
//! Accessibility permission for AX queries to return non-empty trees.
//! These tests print a TCC hint and exit cleanly (PASS-by-skip) when the
//! AX tree is empty rather than misreporting as a test failure.

#![cfg(target_os = "macos")]

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Duration;

// ── paths ────────────────────────────────────────────────────────────────────

fn workspace_root() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    // crates/cua-driver -> rust
    PathBuf::from(manifest).parent().unwrap().parent().unwrap().to_owned()
}

fn driver_binary() -> PathBuf {
    let root = workspace_root();
    let release = root.join("target/release/cua-driver");
    if release.exists() { return release; }
    root.join("target/debug/cua-driver")
}

fn harness_app() -> PathBuf {
    if let Ok(p) = std::env::var("HARNESS_APPKIT_APP") {
        let pb = PathBuf::from(p);
        if pb.exists() { return pb; }
    }
    workspace_root().join("test-apps/harness-appkit/CuaTestHarness.AppKit.app")
}

fn harness_exe() -> PathBuf {
    harness_app().join("Contents/MacOS/CuaTestHarness.AppKit")
}

// ── JSON-RPC ────────────────────────────────────────────────────────────────

fn send(stdin: &mut ChildStdin, req: serde_json::Value) {
    writeln!(stdin, "{}", serde_json::to_string(&req).unwrap()).unwrap();
}

fn recv(stdout: &mut BufReader<&mut ChildStdout>) -> serde_json::Value {
    let mut line = String::new();
    stdout.read_line(&mut line).expect("read response");
    serde_json::from_str(line.trim()).expect("parse json")
}

fn init(stdin: &mut ChildStdin, stdout: &mut BufReader<&mut ChildStdout>) {
    send(stdin, serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}
    }));
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
            eprintln!("harness exe not found at {exe:?} \
                — run libs/cua-driver/test-harness/build.sh first");
            return None;
        }
        // Launch the binary directly (not via `open`) so we control the pid
        // and can kill it cleanly on Drop. The app still installs an AppKit
        // window via NSApp.run().
        let app = Command::new(&exe)
            .stdout(Stdio::null()).stderr(Stdio::null())
            .spawn().ok()?;
        let pid = app.id();
        // Settle for window creation + activation.
        std::thread::sleep(Duration::from_millis(800));
        Some(Self { _app: app, pid })
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self._app.kill();
        let _ = self._app.wait();
        std::thread::sleep(Duration::from_millis(200));
    }
}

// ── window / element helpers ─────────────────────────────────────────────────

fn find_harness_window(stdin: &mut ChildStdin, stdout: &mut BufReader<&mut ChildStdout>,
                       pid: u32, title_substr: &str) -> Option<(u64, String)> {
    let deadline = std::time::Instant::now() + Duration::from_secs(12);
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

fn snapshot_elements(stdin: &mut ChildStdin, stdout: &mut BufReader<&mut ChildStdout>,
                     pid: u32, window_id: u64) -> serde_json::Value {
    tools_call(stdin, stdout, 20, "get_window_state",
        serde_json::json!({
            "pid": pid as i64,
            "window_id": window_id,
            "capture_mode": "tree"
        }))
}

fn snapshot_text(snapshot: &serde_json::Value) -> &str {
    snapshot["result"]["content"][0]["text"].as_str().unwrap_or("")
}

fn elements_have_aid(snapshot: &serde_json::Value, aid: &str) -> bool {
    snapshot_text(snapshot).contains(&format!("id={aid}"))
}

fn find_element_index_by_aid(snapshot: &serde_json::Value, aid: &str) -> Option<u64> {
    let needle = format!("id={aid}");
    for line in snapshot_text(snapshot).lines() {
        if !line.contains(&needle) { continue; }
        let start = line.find('[')? + 1;
        let end = line[start..].find(']')? + start;
        return line[start..end].trim().parse().ok();
    }
    None
}

fn ax_tree_looks_empty(snapshot: &serde_json::Value) -> bool {
    let txt = snapshot_text(snapshot);
    let line_count = txt.lines().count();
    txt.is_empty() || line_count <= 2 ||
        txt.contains("Accessibility permission required") ||
        txt.contains("TCC permission")
}

// ── tests ────────────────────────────────────────────────────────────────────

#[test]
#[ignore]
fn harness_appkit_smoke() {
    let driver = driver_binary();
    if !driver.exists() {
        eprintln!("cua-driver not built — run `cargo build` first");
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

    let (wid, title) = find_harness_window(&mut stdin, &mut stdout, harness.pid,
                                           "CuaTestHarness AppKit")
        .expect("main window not found via list_windows");
    println!("main window: id={wid} title={title:?}");

    let snap = snapshot_elements(&mut stdin, &mut stdout, harness.pid, wid);

    if ax_tree_looks_empty(&snap) {
        eprintln!("AX tree empty — likely TCC Accessibility not granted to the test runner. \
                   Skipping element-assertion phase. To enable: System Settings → Privacy & \
                   Security → Accessibility → add the binary running `cargo test`.");
        let _ = child.kill();
        return;
    }

    let text = snapshot_text(&snap);
    println!("snapshot:\n{text}");

    // AppKit AX quirk (mirrors the WPF behavior documented in
    // harness_wpf_test.rs::harness_wpf_smoke): NSTextField in label mode
    // and other AXStaticText leaves do NOT propagate
    // setAccessibilityIdentifier into the AX tree's identifier slot, so
    // we don't assert on ids for labels. We assert on text-presence for
    // those, and on AX ids only for actionable controls (Buttons,
    // TextFields).
    for aid in [
        "wnd-main",                    // NSWindow
        "btn-increment", "btn-reset",  // NSButton
        "txt-input",                   // editable NSTextField
        "menu-test-item",              // NSMenuItem (Mac-specific)
        "btn-exit",
    ] {
        assert!(elements_have_aid(&snap, aid),
                "missing AX identifier {aid} in AppKit snapshot");
    }

    // text_body marker carried by the visible string of the NSTextField
    assert!(text.contains("HARNESS_TEXT_MARKER_v1"),
            "text_body marker not in AppKit snapshot");
    // The two label-mode NSTextFields under click_target render as
    // AXStaticText nodes — assert on their starting text instead of ids.
    assert!(text.contains("L=0 R=0 D=0"), "click_count label missing");
    assert!(text.contains("(none)"),       "last_action label missing");

    let _ = child.kill();
}

/// text_input: type_text into the NSTextField, verify the mirror label
/// shows the typed string. Exercises the AX type_text path
/// (AXSetAttribute on AXValue, or CGEvent fallback).
#[test]
#[ignore]
fn harness_appkit_text_input() {
    let driver = driver_binary();
    if !driver.exists() { eprintln!("cua-driver not built"); return; }
    let harness = match Harness::launch() {
        Some(h) => h,
        None => { eprintln!("harness not built — skipping"); return; }
    };

    let mut child = Command::new(&driver)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn cua-driver");
    let mut stdin = child.stdin.take().unwrap();
    let mut raw_stdout = child.stdout.take().unwrap();
    let mut stdout = BufReader::new(&mut raw_stdout);
    init(&mut stdin, &mut stdout);

    let (wid, _) = find_harness_window(&mut stdin, &mut stdout, harness.pid,
                                       "CuaTestHarness AppKit")
        .expect("main window not found");
    let snap_pre = snapshot_elements(&mut stdin, &mut stdout, harness.pid, wid);
    if ax_tree_looks_empty(&snap_pre) {
        eprintln!("AX empty — TCC not granted; skipping"); let _ = child.kill(); return;
    }
    let idx = find_element_index_by_aid(&snap_pre, "txt-input")
        .expect("txt-input element_index not found");

    // set_value via AX is the deterministic background path; type_text would
    // also work but races with cursor focus on cold-launched windows.
    let resp = tools_call(&mut stdin, &mut stdout, 40, "set_value",
        serde_json::json!({
            "pid": harness.pid as i64,
            "window_id": wid,
            "element_index": idx,
            "value": "hello-cua"
        }));
    println!("set_value resp: {resp}");

    std::thread::sleep(Duration::from_millis(250));
    let snap_post = snapshot_elements(&mut stdin, &mut stdout, harness.pid, wid);
    let post_text = snapshot_text(&snap_post).to_owned();
    assert!(post_text.contains("hello-cua"),
            "text_input value did not propagate to mirror; snapshot:\n{post_text}");

    let _ = child.kill();
}

/// type_text: synthesize a keystroke into the NSTextField (CGEvent
/// path, distinct from set_value's AX path). Verifies the keyboard
/// dispatch chain reaches a backgrounded Cocoa text input.
#[test]
#[ignore]
fn harness_appkit_type_text_keystroke() {
    let driver = driver_binary();
    if !driver.exists() { eprintln!("cua-driver not built"); return; }
    let harness = match Harness::launch() {
        Some(h) => h,
        None => { eprintln!("harness not built — skipping"); return; }
    };

    let mut child = Command::new(&driver)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn cua-driver");
    let mut stdin = child.stdin.take().unwrap();
    let mut raw_stdout = child.stdout.take().unwrap();
    let mut stdout = BufReader::new(&mut raw_stdout);
    init(&mut stdin, &mut stdout);

    let (wid, _) = find_harness_window(&mut stdin, &mut stdout, harness.pid,
                                       "CuaTestHarness AppKit")
        .expect("main window not found");
    let snap_pre = snapshot_elements(&mut stdin, &mut stdout, harness.pid, wid);
    if ax_tree_looks_empty(&snap_pre) {
        eprintln!("AX empty — TCC not granted; skipping"); let _ = child.kill(); return;
    }
    let idx: u64 = if let Some(i) = find_element_index_by_aid(&snap_pre, "txt-input") {
        i
    } else {
        eprintln!("txt-input not found; skipping"); let _ = child.kill(); return;
    };

    // Focus the field first so the keystrokes land in it. AX press on
    // a text field has the side effect of giving it keyboard focus.
    let _ = tools_call(&mut stdin, &mut stdout, 60, "click",
        serde_json::json!({
            "pid": harness.pid as i64, "window_id": wid,
            "element_index": idx, "action": "press"
        }));
    std::thread::sleep(Duration::from_millis(150));

    // CGEvent-based type_text against the focused field (does NOT use
    // set_value — exercises the keystroke synthesis chain).
    let resp = tools_call(&mut stdin, &mut stdout, 61, "type_text",
        serde_json::json!({
            "pid": harness.pid as i64, "window_id": wid,
            "text": "kbd-cua"
        }));
    println!("type_text resp: {resp}");
    std::thread::sleep(Duration::from_millis(250));

    let snap_post = snapshot_elements(&mut stdin, &mut stdout, harness.pid, wid);
    let post = snapshot_text(&snap_post).to_owned();
    assert!(post.contains("kbd-cua"),
            "type_text keystroke did not land in the text field; snapshot:\n{post}");

    let _ = child.kill();
}

/// scroll: scroll the NSScrollView downward, verify the offset label
/// changes (it mirrors the clip view's documentVisibleRect.origin.y).
///
/// **Status:** EXPECTED-FAIL today (see notes below). The `scroll` tool
/// itself works at the API level — `scripts/mac-smoke.sh` confirms it
/// PASSes against the same harness window. What this test would
/// verify additionally is that the scroll event actually moved the
/// scroll view's bounds (state-change observation, not just API
/// success).
///
/// Why it's expected-fail: on macOS, `CGEvent.scroll` requires the
/// cursor position to lie inside the target NSScrollView for the event
/// to be routed to it (Cocoa scroll-routing is cursor-anchored). The
/// `move_cursor` tool we expose is overlay-only — it doesn't move the
/// OS hardware cursor on macOS. So state-change tests for scroll need
/// either (a) an OS-cursor warp (intentionally not exposed) or (b) a
/// different scroll dispatch primitive (NSEvent.otherEvent
/// keyDown(.swipeUp) on focused window, or an AXScrollAreaScrollTo
/// action). Both are open implementation work; tracking in the journal's
/// "Open items" section.
#[test]
#[ignore]
#[should_panic(expected = "scroll offset label did not advance from 0")]
fn harness_appkit_scroll_expected_fail() {
    let driver = driver_binary();
    if !driver.exists() { eprintln!("cua-driver not built"); return; }
    let harness = match Harness::launch() {
        Some(h) => h,
        None => { eprintln!("harness not built — skipping"); return; }
    };

    let mut child = Command::new(&driver)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn cua-driver");
    let mut stdin = child.stdin.take().unwrap();
    let mut raw_stdout = child.stdout.take().unwrap();
    let mut stdout = BufReader::new(&mut raw_stdout);
    init(&mut stdin, &mut stdout);

    let (wid, _) = find_harness_window(&mut stdin, &mut stdout, harness.pid,
                                       "CuaTestHarness AppKit")
        .expect("main window not found");
    let snap_pre = snapshot_elements(&mut stdin, &mut stdout, harness.pid, wid);
    if ax_tree_looks_empty(&snap_pre) {
        eprintln!("AX empty — TCC not granted; skipping"); let _ = child.kill(); return;
    }
    // Pre-condition: offset label should be at "0" (the controller
    // initial state). The label text appears as an AXStaticText leaf
    // immediately after the AXTextArea body in the rendered tree.
    let pre = snapshot_text(&snap_pre).to_owned();
    let pre_has_zero_offset = pre.lines()
        .any(|l| l.trim() == "- AXStaticText = \"0\"");
    assert!(pre_has_zero_offset, "scroll offset label not at 0 pre-scroll");

    // Scroll the scroll view down a few ticks. The scroll tool takes
    // window-local pixel coords; pick a point inside the scroller
    // (the scroll target sits roughly mid-window).
    let resp = tools_call(&mut stdin, &mut stdout, 70, "scroll",
        serde_json::json!({
            "pid": harness.pid as i64, "window_id": wid,
            "x": 180, "y": 450,            // inside the scroll view
            "direction": "down",
            "amount": 5
        }));
    println!("scroll resp: {resp}");
    std::thread::sleep(Duration::from_millis(250));

    let snap_post = snapshot_elements(&mut stdin, &mut stdout, harness.pid, wid);
    let post = snapshot_text(&snap_post).to_owned();
    // After scroll, the offset label should no longer be "0" — any
    // positive integer indicates the bounds-change notification fired
    // and the label updated. We don't pin a specific value (scroll
    // wheel pixel delta varies by macOS version + accessibility setting).
    let still_zero = post.lines().any(|l| l.trim() == "- AXStaticText = \"0\"");
    let unchanged_count = post.matches("- AXStaticText = \"0\"").count();
    let pre_count = pre.matches("- AXStaticText = \"0\"").count();
    // Counter label is also "0" so the bare presence isn't a signal —
    // instead check the COUNT decreased by 1 (only the offset label
    // moved off zero, not the counter).
    assert!(
        unchanged_count < pre_count || !still_zero,
        "scroll offset label did not advance from 0; pre: {} \"0\" leaves; post: {} \"0\" leaves",
        pre_count, unchanged_count
    );

    let _ = child.kill();
}

/// counter: click the increment button via element_index, verify the
/// counter label flips from 0 to 1.
#[test]
#[ignore]
fn harness_appkit_counter() {
    let driver = driver_binary();
    if !driver.exists() {
        eprintln!("cua-driver not built — skipping"); return;
    }
    let harness = match Harness::launch() {
        Some(h) => h,
        None => { eprintln!("harness not built — skipping"); return; }
    };

    let mut child = Command::new(&driver)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn cua-driver");
    let mut stdin = child.stdin.take().unwrap();
    let mut raw_stdout = child.stdout.take().unwrap();
    let mut stdout = BufReader::new(&mut raw_stdout);
    init(&mut stdin, &mut stdout);

    let (wid, _) = find_harness_window(&mut stdin, &mut stdout, harness.pid,
                                       "CuaTestHarness AppKit")
        .expect("main window not found");
    let snap_pre = snapshot_elements(&mut stdin, &mut stdout, harness.pid, wid);
    if ax_tree_looks_empty(&snap_pre) {
        eprintln!("AX empty — TCC not granted; skipping"); let _ = child.kill(); return;
    }
    let pre_text = snapshot_text(&snap_pre).to_owned();
    assert!(pre_text.contains("\"0\""), "counter not 0 pre-click; snapshot:\n{pre_text}");

    let idx = find_element_index_by_aid(&snap_pre, "btn-increment")
        .expect("btn-increment element_index not found");

    let click_resp = tools_call(&mut stdin, &mut stdout, 30, "click",
        serde_json::json!({
            "pid": harness.pid as i64,
            "window_id": wid,
            "element_index": idx,
            "action": "press"
        }));
    println!("click resp: {}", click_resp);

    // Let the AppKit run-loop process the press and refresh the label.
    std::thread::sleep(Duration::from_millis(200));

    let snap_post = snapshot_elements(&mut stdin, &mut stdout, harness.pid, wid);
    let post_text = snapshot_text(&snap_post).to_owned();
    assert!(post_text.contains("\"1\""),
            "counter did not advance to 1 after press; post snapshot:\n{post_text}");

    let _ = child.kill();
}
