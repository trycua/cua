//! Integration test against the CuaTestHarness.AppKit Swift app.
//!
//! Mirror of `harness_wpf_test.rs` for the macOS AppKit hosting pattern.
//! The harness app lives at `libs/cua-driver/test-harness/apps/macos/appkit`
//! and is published into `libs/cua-driver/rust/test-apps/harness-appkit/`
//! by `libs/cua-driver/test-harness/build/macos.sh`.
//!
//! Scenarios (see `libs/cua-driver/test-harness/shared/scenarios.json`
//! `appkit` section):
//!   - counter        : NSButton AXPress invocation increments counter
//!   - text_body      : get_window_state extracts known marker text
//!   - text_input     : type_text into NSTextField updates mirror label
//!   - click_target   : right_click / double_click recognised by NSView
//!   - scroll_target  : scroll updates VerticalOffset label
//!   - ns_menubar     : main menubar item enumerable (Mac-specific)
//!
//! Run locally (after `libs/cua-driver/test-harness/build/macos.sh`):
//!   cargo test --test harness_appkit_test -- --ignored --nocapture
//!
//! Tests are `#[ignore]` so they don't run in plain `cargo test`.
//!
//! **TCC caveat:** on a fresh Mac, the cua-driver process needs
//! Accessibility permission for AX queries to return non-empty trees.
//! These tests print a TCC hint and exit cleanly (PASS-by-skip) when the
//! AX tree is empty rather than misreporting as a test failure.

#![cfg(target_os = "macos")]

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use cua_driver_testkit::ax::{element_index_by_id, has_id, looks_empty};
use cua_driver_testkit::{Driver, McpDriver, ToolResponse};

// ── paths ────────────────────────────────────────────────────────────────────

fn harness_app() -> PathBuf {
    if let Ok(p) = std::env::var("HARNESS_APPKIT_APP") {
        let pb = PathBuf::from(p);
        if pb.exists() { return pb; }
    }
    cua_driver_testkit::harness_app("harness-appkit", "CuaTestHarness.AppKit.app")
}

fn harness_exe() -> PathBuf {
    harness_app().join("Contents/MacOS/CuaTestHarness.AppKit")
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
                — run libs/cua-driver/test-harness/build/macos.sh first");
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

fn snapshot_elements(driver: &mut McpDriver, pid: u32, window_id: u64) -> ToolResponse {
    driver.call("get_window_state",
        serde_json::json!({
            "pid": pid as i64,
            "window_id": window_id,
            "capture_mode": "tree"
        }))
}

// ── tests ────────────────────────────────────────────────────────────────────

#[test]
#[ignore]
fn harness_appkit_smoke() {
    let Some(mut driver) = McpDriver::spawn() else { return };
    let harness = match Harness::launch() {
        Some(h) => h,
        None => { eprintln!("harness not built — skipping"); return; }
    };
    println!("harness pid={}", harness.pid);

    let (wid, title) = driver.find_window(harness.pid as i64, "CuaTestHarness AppKit")
        .expect("main window not found via list_windows");
    println!("main window: id={wid} title={title:?}");

    let snap = snapshot_elements(&mut driver, harness.pid, wid);

    if looks_empty(snap.text()) {
        eprintln!("AX tree empty — likely TCC Accessibility not granted to the test runner. \
                   Skipping element-assertion phase. To enable: System Settings → Privacy & \
                   Security → Accessibility → add the binary running `cargo test`.");
        return;
    }

    let text = snap.text();
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
        assert!(has_id(snap.text(), aid),
                "missing AX identifier {aid} in AppKit snapshot");
    }

    // text_body marker carried by the visible string of the NSTextField
    assert!(text.contains("HARNESS_TEXT_MARKER_v1"),
            "text_body marker not in AppKit snapshot");
    // The two label-mode NSTextFields under click_target render as
    // AXStaticText nodes — assert on their starting text instead of ids.
    assert!(text.contains("L=0 R=0 D=0"), "click_count label missing");
    assert!(text.contains("(none)"),       "last_action label missing");
}

/// text_input: type_text into the NSTextField, verify the mirror label
/// shows the typed string. Exercises the AX type_text path
/// (AXSetAttribute on AXValue, or CGEvent fallback).
#[test]
#[ignore]
fn harness_appkit_text_input() {
    let Some(mut driver) = McpDriver::spawn() else { return };
    let harness = match Harness::launch() {
        Some(h) => h,
        None => { eprintln!("harness not built — skipping"); return; }
    };

    let (wid, _) = driver.find_window(harness.pid as i64, "CuaTestHarness AppKit")
        .expect("main window not found");
    let snap_pre = snapshot_elements(&mut driver, harness.pid, wid);
    if looks_empty(snap_pre.text()) {
        eprintln!("AX empty — TCC not granted; skipping"); return;
    }
    let idx = element_index_by_id(snap_pre.text(), "txt-input")
        .expect("txt-input element_index not found");

    // set_value via AX is the deterministic background path; type_text would
    // also work but races with cursor focus on cold-launched windows.
    let resp = driver.call("set_value",
        serde_json::json!({
            "pid": harness.pid as i64,
            "window_id": wid,
            "element_index": idx,
            "value": "hello-cua"
        }));
    println!("set_value resp: {}", resp.text());

    std::thread::sleep(Duration::from_millis(250));
    let snap_post = snapshot_elements(&mut driver, harness.pid, wid);
    let post_text = snap_post.text().to_owned();
    assert!(post_text.contains("hello-cua"),
            "text_input value did not propagate to mirror; snapshot:\n{post_text}");
}

/// type_text: synthesize a keystroke into the NSTextField (CGEvent
/// path, distinct from set_value's AX path). Verifies the keyboard
/// dispatch chain reaches a backgrounded Cocoa text input.
#[test]
#[ignore]
fn harness_appkit_type_text_keystroke() {
    let Some(mut driver) = McpDriver::spawn() else { return };
    let harness = match Harness::launch() {
        Some(h) => h,
        None => { eprintln!("harness not built — skipping"); return; }
    };

    let (wid, _) = driver.find_window(harness.pid as i64, "CuaTestHarness AppKit")
        .expect("main window not found");
    let snap_pre = snapshot_elements(&mut driver, harness.pid, wid);
    if looks_empty(snap_pre.text()) {
        eprintln!("AX empty — TCC not granted; skipping"); return;
    }
    let idx: u64 = if let Some(i) = element_index_by_id(snap_pre.text(), "txt-input") {
        i
    } else {
        eprintln!("txt-input not found; skipping"); return;
    };

    // Focus the field first so the keystrokes land in it. AX press on
    // a text field has the side effect of giving it keyboard focus.
    let _ = driver.call("click",
        serde_json::json!({
            "pid": harness.pid as i64, "window_id": wid,
            "element_index": idx, "action": "press"
        }));
    std::thread::sleep(Duration::from_millis(150));

    // CGEvent-based type_text against the focused field (does NOT use
    // set_value — exercises the keystroke synthesis chain).
    let resp = driver.call("type_text",
        serde_json::json!({
            "pid": harness.pid as i64, "window_id": wid,
            "text": "kbd-cua"
        }));
    println!("type_text resp: {}", resp.text());
    std::thread::sleep(Duration::from_millis(250));

    let snap_post = snapshot_elements(&mut driver, harness.pid, wid);
    let post = snap_post.text().to_owned();
    assert!(post.contains("kbd-cua"),
            "type_text keystroke did not land in the text field; snapshot:\n{post}");
}

/// scroll: scroll the NSScrollView downward, verify the offset label
/// changes (it mirrors the clip view's documentVisibleRect.origin.y).
///
/// **Status:** EXPECTED-FAIL today (see notes below). The `scroll` tool
/// itself works at the API level — `libs/cua-driver/test-harness/smoke/macos.sh` confirms it
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
    let Some(mut driver) = McpDriver::spawn() else { return };
    let harness = match Harness::launch() {
        Some(h) => h,
        None => { eprintln!("harness not built — skipping"); return; }
    };

    let (wid, _) = driver.find_window(harness.pid as i64, "CuaTestHarness AppKit")
        .expect("main window not found");
    let snap_pre = snapshot_elements(&mut driver, harness.pid, wid);
    if looks_empty(snap_pre.text()) {
        eprintln!("AX empty — TCC not granted; skipping"); return;
    }
    // Pre-condition: offset label should be at "0" (the controller
    // initial state). The label text appears as an AXStaticText leaf
    // immediately after the AXTextArea body in the rendered tree.
    let pre = snap_pre.text().to_owned();
    let pre_has_zero_offset = pre.lines()
        .any(|l| l.trim() == "- AXStaticText = \"0\"");
    assert!(pre_has_zero_offset, "scroll offset label not at 0 pre-scroll");

    // Scroll the scroll view down a few ticks. The scroll tool takes
    // window-local pixel coords; pick a point inside the scroller
    // (the scroll target sits roughly mid-window).
    let resp = driver.call("scroll",
        serde_json::json!({
            "pid": harness.pid as i64, "window_id": wid,
            "x": 180, "y": 450,            // inside the scroll view
            "direction": "down",
            "amount": 5
        }));
    println!("scroll resp: {}", resp.text());
    std::thread::sleep(Duration::from_millis(250));

    let snap_post = snapshot_elements(&mut driver, harness.pid, wid);
    let post = snap_post.text().to_owned();
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
}

/// counter: click the increment button via element_index, verify the
/// counter label flips from 0 to 1.
#[test]
#[ignore]
fn harness_appkit_counter() {
    let Some(mut driver) = McpDriver::spawn() else { return };
    let harness = match Harness::launch() {
        Some(h) => h,
        None => { eprintln!("harness not built — skipping"); return; }
    };

    let (wid, _) = driver.find_window(harness.pid as i64, "CuaTestHarness AppKit")
        .expect("main window not found");
    let snap_pre = snapshot_elements(&mut driver, harness.pid, wid);
    if looks_empty(snap_pre.text()) {
        eprintln!("AX empty — TCC not granted; skipping"); return;
    }
    let pre_text = snap_pre.text().to_owned();
    assert!(pre_text.contains("\"0\""), "counter not 0 pre-click; snapshot:\n{pre_text}");

    let idx = element_index_by_id(snap_pre.text(), "btn-increment")
        .expect("btn-increment element_index not found");

    let click_resp = driver.call("click",
        serde_json::json!({
            "pid": harness.pid as i64,
            "window_id": wid,
            "element_index": idx,
            "action": "press"
        }));
    println!("click resp: {}", click_resp.text());

    // Let the AppKit run-loop process the press and refresh the label.
    std::thread::sleep(Duration::from_millis(200));

    let snap_post = snapshot_elements(&mut driver, harness.pid, wid);
    let post_text = snap_post.text().to_owned();
    assert!(post_text.contains("\"1\""),
            "counter did not advance to 1 after press; post snapshot:\n{post_text}");
}
