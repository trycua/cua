//! Harness integration test for the **desktop-scope** modality (#1968 / #2019).
//!
//! Desktop-scope is cua-driver's *foreground*, vision-only, **screen-absolute**
//! loop (the "Computer-Use 1.0" mode), the complement to the default per-window
//! background model that `harness_bg_modality_test` / `e2e_windows_bg_input_test`
//! cover. This test exercises the Windows Phase-1 actuator end-to-end against a
//! real harness app:
//!
//!   1. `set_config capture_scope=desktop` → `get_desktop_state` returns a
//!      full-display capture with true `screen_width/height` (no downscale).
//!   2. A **window-less** screen-absolute `click` / `scroll` (no pid/window_id)
//!      lands via `WindowFromPoint` while in desktop scope.
//!   3. Negative gate: the same window-less `click` under `capture_scope=window`
//!      is rejected with the structured `desktop_scope_disabled` error.
//!
//! Note: `set_config` is a *session* override — it persists for the lifetime of
//! the one MCP server we spawn here (not across separate `cua-driver call`
//! processes), which is exactly why this test drives a single long-lived server.
//!
//! All tests are `#[ignore]` (need a real desktop session). Run explicitly:
//!   cargo test -p cua-driver --test harness_desktop_scope_test -- --ignored --nocapture --test-threads=1

#![cfg(target_os = "windows")]

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cua_driver_testkit::{harness_app, Driver, McpDriver};

/// WPF harness app (built by `tests/fixtures/build/windows.ps1`). Path mirrors
/// `shared/scenarios.json`'s `wpf.exe_relative_path`.
fn harness_wpf_exe() -> std::path::PathBuf {
    harness_app("harness-wpf", "CuaTestHarness.Wpf.exe")
}

/// Launch the WPF harness app and return (pid, window center in screen px).
/// Skips (returns None) if the harness app isn't built.
fn launch_wpf_and_center(driver: &mut McpDriver) -> Option<(u32, i32, i32)> {
    let exe = harness_wpf_exe();
    if !exe.exists() {
        eprintln!("[desktop-scope] WPF harness not built ({exe:?}) — skipping window-target tests");
        return None;
    }
    driver
        .reaper()
        .spawn(Command::new(&exe).stdout(Stdio::null()).stderr(Stdio::null()))
        .ok()?;

    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        let r = driver.call("list_windows", serde_json::json!({}));
        if let Some(arr) = r.structured()["windows"].as_array() {
            for w in arr {
                let title = w["title"].as_str().unwrap_or("");
                if !title.contains("CuaTestHarness") {
                    continue;
                }
                let pid = w["pid"].as_u64().unwrap_or(0) as u32;
                // #2018: bounds is nested {x,y,width,height} on Windows.
                let b = &w["bounds"];
                let (x, y, ww, h) = (
                    b["x"].as_i64().unwrap_or(0) as i32,
                    b["y"].as_i64().unwrap_or(0) as i32,
                    b["width"].as_i64().unwrap_or(0) as i32,
                    b["height"].as_i64().unwrap_or(0) as i32,
                );
                if pid != 0 && ww > 0 && h > 0 {
                    driver.reaper().track_pid(pid);
                    return Some((pid, x + ww / 2, y + h / 2));
                }
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    eprintln!("[desktop-scope] WPF harness window never appeared — skipping");
    None
}

fn set_scope(driver: &mut McpDriver, scope: &str) {
    let r = driver.call(
        "set_config",
        serde_json::json!({ "key": "capture_scope", "value": scope }),
    );
    assert!(!r.is_error(), "set_config capture_scope={scope} failed: {}", r.text());
    assert_eq!(
        r.structured()["capture_scope"].as_str(),
        Some(scope),
        "set_config did not report capture_scope={scope}: {}",
        r.text()
    );
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// `get_desktop_state` in desktop scope returns a full-display capture with
/// real screen dimensions (the Session-0 `handle is invalid` case is only the
/// service-session wall; this needs a real interactive desktop).
#[test]
#[ignore]
fn desktop_scope_capture_returns_screen_dims() {
    let Some(mut driver) = McpDriver::spawn() else { return };
    set_scope(&mut driver, "desktop");
    let r = driver.call("get_desktop_state", serde_json::json!({}));
    assert!(!r.is_error(), "get_desktop_state errored: {}", r.text());
    let sw = r.structured()["screen_width"].as_u64().unwrap_or(0);
    let sh = r.structured()["screen_height"].as_u64().unwrap_or(0);
    assert!(sw > 0 && sh > 0, "get_desktop_state returned no/zero screen size: {}", r.text());
    eprintln!("[desktop-scope] get_desktop_state OK — screen {sw}x{sh}");
}

/// In desktop scope, a window-less screen-absolute click + scroll succeed and
/// resolve a real window via WindowFromPoint (no pid/window_id supplied).
#[test]
#[ignore]
fn desktop_scope_windowless_click_and_scroll_land() {
    let Some(mut driver) = McpDriver::spawn() else { return };
    set_scope(&mut driver, "desktop");
    let Some((_pid, cx, cy)) = launch_wpf_and_center(&mut driver) else { return };

    let clicked = driver.call("click", serde_json::json!({ "x": cx, "y": cy }));
    assert!(!clicked.is_error(), "desktop-scope click errored: {}", clicked.text());
    let ct = clicked.text().to_lowercase();
    assert!(ct.contains("desktop scope"), "click not reported as desktop-scope: {}", clicked.text());
    assert!(ct.contains("hwnd"), "click did not resolve a window via WindowFromPoint: {}", clicked.text());

    let scrolled = driver.call(
        "scroll",
        serde_json::json!({ "x": cx, "y": cy, "direction": "down" }),
    );
    assert!(!scrolled.is_error(), "desktop-scope scroll errored: {}", scrolled.text());
    assert!(
        scrolled.text().to_lowercase().contains("desktop scope"),
        "scroll not reported as desktop-scope: {}",
        scrolled.text()
    );
}

/// Negative gate: a window-less screen-absolute click under `capture_scope=window`
/// must be rejected (the `desktop_scope_disabled` contract), not silently retargeted.
#[test]
#[ignore]
fn window_scope_rejects_windowless_click() {
    let Some(mut driver) = McpDriver::spawn() else { return };
    set_scope(&mut driver, "window");
    let r = driver.call("click", serde_json::json!({ "x": 100, "y": 100 }));
    let txt = r.text().to_lowercase();
    assert!(
        r.is_error() || txt.contains("desktop scope") || txt.contains("desktop_scope_disabled"),
        "window-scope window-less click was NOT rejected: {}",
        r.text()
    );
}
