//! macOS **desktop-scope** (vision/foreground) modality, exercised through the
//! SAME cua-driver interface as the Windows `modality_desktop_scope_test`:
//! `set_config capture_scope=desktop` + a window-less screen-absolute `click`
//! (no `pid`, no `window_id`, no `list_windows`). This is the macOS peer of the
//! Windows `WindowFromPoint` desktop click — a global-HID `CGEvent` posted at
//! true screen pixels that lands on whatever is visually frontmost there, the
//! deliberate complement to the background no-foreground contract.
//!
//! The test grounds the click on a real control: it reads the increment
//! button's screen-absolute `frame` from a window-scope AX snapshot (the way an
//! agent would locate it by vision in `get_desktop_state`), clicks those screen
//! pixels window-less, and asserts the harness counter advanced. A second test
//! asserts the `window`-scope gate rejects a window-less click
//! (`desktop_scope_disabled`), matching the Windows contract.
//!
//! `set_config` is made session-scoped (a `session` arg → `_session_id`), so it
//! is in-memory only and never writes the developer's `~/.cua-driver/config.json`.
//!
//! #[ignore] (needs a real desktop session + TCC Accessibility + the AppKit
//! harness). Run:
//!   cargo test -p cua-driver --test modality_desktop_scope_macos_test -- --ignored --nocapture --test-threads=1

#![cfg(target_os = "macos")]

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cua_driver_testkit::{harness_app, Driver, McpDriver};

/// Session id so `set_config capture_scope=desktop` is session-scoped (no disk
/// write) and the `click` resolves the same scope override.
const SESSION: &str = "vf-desktop";

fn harness_exe() -> std::path::PathBuf {
    std::env::var("HARNESS_APPKIT_APP")
        .map(std::path::PathBuf::from)
        .ok()
        .filter(|p| p.exists())
        .unwrap_or_else(|| harness_app("harness-appkit", "CuaTestHarness.AppKit.app"))
        .join("Contents/MacOS/CuaTestHarness.AppKit")
}

/// Launch the AppKit harness under the driver's reaper; resolve its (pid,
/// window_id) by title. Returns None (skip) if the harness isn't built.
fn launch(driver: &mut McpDriver) -> Option<(u32, u64)> {
    let exe = harness_exe();
    if !exe.exists() {
        eprintln!("[desktop-mac] AppKit harness not built ({exe:?}) — run test-harness/build/macos.sh; skipping");
        return None;
    }
    driver
        .reaper()
        .spawn(Command::new(&exe).stdout(Stdio::null()).stderr(Stdio::null()))
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(14);
    while Instant::now() < deadline {
        let r = driver.call("list_windows", serde_json::json!({}));
        if let Some(wins) = r.structured()["windows"].as_array() {
            for w in wins {
                if w["title"].as_str().unwrap_or("").contains("CuaTestHarness AppKit") {
                    let pid = w["pid"].as_u64().unwrap_or(0) as u32;
                    let wid = w["window_id"].as_u64().unwrap_or(0);
                    if pid != 0 && wid != 0 {
                        driver.reaper().track_pid(pid);
                        return Some((pid, wid));
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(400));
    }
    eprintln!("[desktop-mac] harness window never appeared — graphical session available? skipping");
    None
}

fn ax_snapshot(driver: &mut McpDriver, pid: u32, wid: u64) -> serde_json::Value {
    driver
        .call(
            "get_window_state",
            serde_json::json!({ "pid": pid as i64, "window_id": wid, "capture_mode": "ax" }),
        )
        .structured()
        .clone()
}

/// Screen-absolute center (points) of the increment button, read from the AX
/// snapshot's `elements[].frame`. The frame is screen-absolute on macOS (its x
/// exceeds the window width), matching CGEvent click coordinates.
fn increment_center(snap: &serde_json::Value) -> Option<(i64, i64)> {
    let els = snap["elements"].as_array()?;
    let btn = els.iter().find(|e| {
        e["role"].as_str() == Some("AXButton")
            && e["label"].as_str().map(|l| l.trim().eq_ignore_ascii_case("increment")).unwrap_or(false)
    })?;
    let f = &btn["frame"];
    let (x, y, w, h) = (f["x"].as_f64()?, f["y"].as_f64()?, f["w"].as_f64()?, f["h"].as_f64()?);
    Some(((x + w / 2.0) as i64, (y + h / 2.0) as i64))
}

/// The current counter value parsed from the snapshot's `tree_markdown`
/// (`counter=N`). The AppKit AXStaticText label doesn't propagate to
/// `elements[].label`, so the value lives in the rendered tree text.
fn counter(snap: &serde_json::Value) -> Option<u64> {
    let tree = snap["tree_markdown"].as_str()?;
    let idx = tree.find("counter=")? + "counter=".len();
    let digits: String = tree[idx..].chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

fn set_scope(driver: &mut McpDriver, scope: &str) {
    // macOS set_config reads the direct `capture_scope` field (not {key,value}).
    // `session` makes the override session-scoped (in-memory, no disk write).
    let r = driver.call(
        "set_config",
        serde_json::json!({ "capture_scope": scope, "session": SESSION }),
    );
    assert!(!r.is_error(), "set_config capture_scope={scope} failed: {}", r.text());
}

// ── tests ───────────────────────────────────────────────────────────────────────

/// In desktop scope, a window-less screen-absolute click (no pid/window_id)
/// lands on a real control: the increment button's counter advances.
#[test]
#[ignore]
fn desktop_scope_windowless_click_lands_on_control() {
    let Some(mut driver) = McpDriver::spawn() else { return };
    let Some((pid, wid)) = launch(&mut driver) else { return };

    // Settle for the AppKit AX tree to register the button + its frame.
    let mut snap = ax_snapshot(&mut driver, pid, wid);
    let mut center = increment_center(&snap);
    let deadline = Instant::now() + Duration::from_secs(8);
    while center.is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(400));
        snap = ax_snapshot(&mut driver, pid, wid);
        center = increment_center(&snap);
    }
    let Some((cx, cy)) = center else {
        eprintln!("[desktop-mac] increment button frame not found (TCC Accessibility missing?) — skipping");
        return;
    };
    let pre = counter(&snap).unwrap_or(0);
    println!("[desktop-mac] increment button screen-center=({cx},{cy}) pre-counter={pre}");

    set_scope(&mut driver, "desktop");

    // Window-less screen-absolute click — no pid, no window_id.
    let clicked = driver.call("click", serde_json::json!({ "x": cx, "y": cy, "session": SESSION }));
    assert!(!clicked.is_error(), "desktop-scope click errored: {}", clicked.text());
    assert!(
        clicked.text().to_lowercase().contains("desktop scope"),
        "click not reported as desktop-scope: {}",
        clicked.text()
    );

    std::thread::sleep(Duration::from_millis(600));
    let post = counter(&ax_snapshot(&mut driver, pid, wid)).unwrap_or(pre);
    assert!(
        post > pre,
        "counter did not advance after window-less desktop click: pre={pre} post={post} \
         (click did not land on the control)"
    );
    println!("✅ desktop_scope_windowless_click_lands_on_control: counter {pre} → {post}");
}

/// Negative gate: a window-less screen-absolute click while `capture_scope=window`
/// must be rejected (`desktop_scope_disabled`), not silently treated as
/// window-local pixels. Mirrors the Windows contract.
#[test]
#[ignore]
fn window_scope_rejects_windowless_click() {
    let Some(mut driver) = McpDriver::spawn() else { return };
    set_scope(&mut driver, "window");
    let r = driver.call("click", serde_json::json!({ "x": 100, "y": 100, "session": SESSION }));
    let txt = r.text().to_lowercase();
    assert!(
        r.is_error() || txt.contains("desktop scope") || txt.contains("desktop_scope_disabled"),
        "window-scope window-less click was NOT rejected: {}",
        r.text()
    );
    println!("✅ window_scope_rejects_windowless_click: window-less click correctly gated");
}
