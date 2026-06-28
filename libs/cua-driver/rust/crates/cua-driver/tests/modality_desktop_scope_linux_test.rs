//! Linux **desktop-scope** (vision/foreground) modality through the SAME
//! cua-driver interface as Windows/macOS: `set_config capture_scope=desktop` +
//! a window-less screen-absolute `click` (no `pid`/`window_id`/`list_windows`).
//! The Linux actuator warps the pointer and injects a real button press via the
//! XTest extension — the peer of the Windows `WindowFromPoint` + macOS
//! global-HID desktop click. (XTest delivering to the under-pointer window is
//! why the *background* paths use `XSendEvent`; for desktop scope that delivery
//! is exactly what we want.)
//!
//! Grounds the click on the GTK3 harness increment button's screen-absolute
//! `frame` (AT-SPI Component extents), asserts the counter advanced, plus the
//! window-scope rejection gate.
//!
//! Linux config is global-only (no per-session override), so `set_config`
//! capture_scope writes the on-disk default — the test resets it to `window`
//! before asserting so a failure can't leave the sandbox in desktop scope.
//!
//! #[ignore] (needs an X11/Xwayland display + AT-SPI + the GTK3 harness). Run:
//!   cargo test -p cua-driver --test modality_desktop_scope_linux_test -- --ignored --nocapture --test-threads=1

#![cfg(target_os = "linux")]

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cua_driver_testkit::{harness_app, Driver, McpDriver};

fn harness_exe() -> std::path::PathBuf {
    std::env::var("HARNESS_GTK3_EXE")
        .map(std::path::PathBuf::from)
        .ok()
        .filter(|p| p.exists())
        .unwrap_or_else(|| harness_app("harness-gtk3", "CuaTestHarness.Gtk3"))
}

fn launch(driver: &mut McpDriver) -> Option<(u32, u64)> {
    let exe = harness_exe();
    if !exe.exists() {
        eprintln!("[desktop-linux] GTK3 harness not built ({exe:?}) — run test-harness/build/linux.sh; skipping");
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
                if w["title"].as_str().unwrap_or("").contains("CuaTestHarness GTK3") {
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
    eprintln!("[desktop-linux] harness window never appeared — graphical session + AT-SPI available? skipping");
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

/// Screen-absolute center (px) of the increment button from `elements[].frame`
/// (AT-SPI Component extents are screen-absolute). The GTK3 harness sets the
/// button's accessible NAME to `btn-increment`; match any element whose blob
/// carries that aid and has a frame, so we're robust to the exact field name.
fn increment_center(snap: &serde_json::Value) -> Option<(i64, i64)> {
    let els = snap["elements"].as_array()?;
    let btn = els.iter().find(|e| {
        serde_json::to_string(e).map(|s| s.contains("btn-increment")).unwrap_or(false)
            && e.get("frame").map(|f| f.is_object()).unwrap_or(false)
    })?;
    let f = &btn["frame"];
    let (x, y, w, h) = (f["x"].as_f64()?, f["y"].as_f64()?, f["w"].as_f64()?, f["h"].as_f64()?);
    Some(((x + w / 2.0) as i64, (y + h / 2.0) as i64))
}

fn counter(snap: &serde_json::Value) -> Option<u64> {
    let tree = snap["tree_markdown"].as_str()?;
    let idx = tree.find("counter=")? + "counter=".len();
    let digits: String = tree[idx..].chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

fn set_scope(driver: &mut McpDriver, scope: &str) {
    let r = driver.call(
        "set_config",
        serde_json::json!({ "key": "capture_scope", "value": scope }),
    );
    assert!(!r.is_error(), "set_config capture_scope={scope} failed: {}", r.text());
}

// ── tests ───────────────────────────────────────────────────────────────────────

/// In desktop scope, a window-less screen-absolute click (no pid/window_id)
/// lands on the increment button — its counter advances.
#[test]
#[ignore]
fn desktop_scope_windowless_click_lands_on_control() {
    let Some(mut driver) = McpDriver::spawn() else { return };
    let Some((pid, wid)) = launch(&mut driver) else { return };

    // Settle for the AT-SPI tree to register the button + its extents.
    let mut snap = ax_snapshot(&mut driver, pid, wid);
    let mut center = increment_center(&snap);
    let deadline = Instant::now() + Duration::from_secs(8);
    while center.is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(400));
        snap = ax_snapshot(&mut driver, pid, wid);
        center = increment_center(&snap);
    }
    let Some((cx, cy)) = center else {
        eprintln!("[desktop-linux] increment button frame not found (AT-SPI extents missing?) — skipping");
        return;
    };
    let pre = counter(&snap).unwrap_or(0);
    println!("[desktop-linux] increment button screen-center=({cx},{cy}) pre-counter={pre}");

    set_scope(&mut driver, "desktop");

    // Retry the window-less desktop click until the counter advances. A
    // freshly-mapped harness window may not yet be raised under the pointer on
    // the first click (X11 window-raise timing differs across WMs — XFCE/Openbox
    // lag GNOME), so the screen-absolute XTest click can miss the first attempt.
    // Re-issuing the SAME click is safe: extra landed clicks only increment the
    // counter further, and `post > pre` still holds. We assert the click was
    // *dispatched as desktop scope* on the first attempt, and that it eventually
    // *lands* within the budget.
    let mut post = pre;
    let mut first_text = String::new();
    for attempt in 0..12 {
        let clicked = driver.call("click", serde_json::json!({ "x": cx, "y": cy }));
        if attempt == 0 {
            first_text = clicked.text().to_string();
            assert!(!clicked.is_error(), "desktop-scope click errored: {}", clicked.text());
        }
        std::thread::sleep(Duration::from_millis(500));
        post = counter(&ax_snapshot(&mut driver, pid, wid)).unwrap_or(pre);
        if post > pre {
            break;
        }
    }
    // Reset scope so a later failure can't leave the box in desktop scope.
    set_scope(&mut driver, "window");

    assert!(
        first_text.to_lowercase().contains("desktop scope"),
        "click not reported as desktop-scope: {first_text}"
    );
    assert!(
        post > pre,
        "counter did not advance after window-less desktop clicks: pre={pre} post={post} \
         (the harness window never became clickable at ({cx},{cy}) within the retry budget)"
    );
    println!("✅ desktop_scope_windowless_click_lands_on_control: counter {pre} → {post}");
}

/// Negative gate: a window-less click under `capture_scope=window` is rejected.
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
    println!("✅ window_scope_rejects_windowless_click: window-less click correctly gated");
}
