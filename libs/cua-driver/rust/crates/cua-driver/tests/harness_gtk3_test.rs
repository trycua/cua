//! Integration test against the CuaTestHarness.Gtk3 (PyGObject / GTK3) app —
//! the Linux peer of `harness_appkit_test` / `harness_swiftui_test` (macOS) and
//! `harness_wpf_test` / `harness_winui3_test` (Windows). Brings Linux to parity
//! with the other platforms' controlled-harness coverage (alongside the real-app
//! Nix GUI scenarios).
//!
//! ## The Linux id contract: NAME, not id
//! cua-driver's Linux `get_window_state` renders each element as
//! `[idx] <role> "<name>"` from AT-SPI — there is NO `id=` field like Windows
//! (UIA AutomationId) or macOS (AX identifier). So the harness sets each
//! actionable control's AT-SPI **accessible name** to the scenario aid
//! (`btn-increment`, …), and this test matches on name via
//! `ax::element_index_containing` (substring), not `ax::has_id`.
//!
//! Requires a real Linux desktop with AT-SPI running + a display (Xwayland is
//! fine — the launcher forces `GDK_BACKEND=x11`), GTK3 + PyGObject, and the app
//! built via `tests/fixtures/build/linux.sh`. `#[ignore]`; run explicitly:
//!   cargo test -p cua-driver --test harness_gtk3_test -- --ignored --nocapture --test-threads=1

#![cfg(target_os = "linux")]

use std::process::{Command, Stdio};
use std::time::Duration;

use cua_driver_testkit::{ax, harness_app, Driver, McpDriver};

fn harness_exe() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("HARNESS_GTK3_EXE") {
        let pb = std::path::PathBuf::from(p);
        if pb.exists() {
            return pb;
        }
    }
    harness_app("harness-gtk3", "CuaTestHarness.Gtk3")
}

/// Launch the GTK3 harness, tying its lifetime to the driver's reaper, and
/// return (pid, main window_id). Returns None (skip) if the app isn't built.
fn launch(driver: &mut McpDriver) -> Option<(u32, u64)> {
    let exe = harness_exe();
    if !exe.exists() {
        eprintln!("GTK3 harness not built at {exe:?} — run tests/fixtures/build/linux.sh");
        return None;
    }
    driver
        .reaper()
        .spawn(Command::new(&exe).stdout(Stdio::null()).stderr(Stdio::null()))
        .ok()?;
    // GTK app cold-start + window map + AT-SPI registration.
    std::thread::sleep(Duration::from_millis(1500));

    // Resolve the harness window by title. find_window needs the pid; the
    // launcher execs python3 in-place so the spawned pid IS the GTK process.
    // We don't have that pid directly here (reaper owns the child), so discover
    // by title across list_windows.
    let deadline = std::time::Instant::now() + Duration::from_secs(12);
    while std::time::Instant::now() < deadline {
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
    eprintln!("GTK3 harness window never appeared — is a graphical session + AT-SPI available?");
    None
}

fn snapshot(driver: &mut McpDriver, pid: u32, wid: u64) -> String {
    driver
        .call(
            "get_window_state",
            serde_json::json!({ "pid": pid as i64, "window_id": wid }),
        )
        .text()
        .to_string()
}

fn ax_empty(text: &str) -> bool {
    ax::looks_empty(text) || text.contains("D-Bus") || text.contains("AT-SPI")
}

#[test]
#[ignore]
fn harness_gtk3_smoke() {
    let Some(mut driver) = McpDriver::spawn() else { return };
    let Some((pid, wid)) = launch(&mut driver) else { return };
    println!("gtk3 harness pid={pid} wid={wid}");

    let text = snapshot(&mut driver, pid, wid);
    if ax_empty(&text) {
        eprintln!("AT-SPI tree empty — accessibility bus unavailable; skipping element assertions");
        return;
    }
    println!("snapshot:\n{text}");

    // Actionable controls expose their aid as the AT-SPI accessible name.
    for name in ["btn-increment", "btn-reset", "txt-input", "btn-open-popover", "btn-exit"] {
        assert!(text.contains(name), "missing control named {name:?} in GTK3 AT-SPI tree");
    }
    // Marker label + counter label carry their text as the accessible name.
    assert!(text.contains("HARNESS_TEXT_MARKER_v1"), "text_body marker not in tree");
    assert!(text.contains("counter=0"), "initial counter label not in tree");

    println!("✅ harness_gtk3_smoke: all scenarios present in AT-SPI tree");
}

#[test]
#[ignore]
fn harness_gtk3_counter_click() {
    let Some(mut driver) = McpDriver::spawn() else { return };
    let Some((pid, wid)) = launch(&mut driver) else { return };

    let pre = snapshot(&mut driver, pid, wid);
    if ax_empty(&pre) {
        eprintln!("AT-SPI empty — skipping");
        return;
    }
    let idx = match ax::element_index_containing(&pre, "btn-increment") {
        Some(i) => i,
        None => {
            eprintln!("btn-increment not found in tree — skipping");
            return;
        }
    };
    let click = driver.call(
        "click",
        serde_json::json!({ "pid": pid as i64, "window_id": wid, "element_index": idx }),
    );
    println!("click [{idx}] btn-increment: {}", click.text());
    std::thread::sleep(Duration::from_millis(400));

    let post = snapshot(&mut driver, pid, wid);
    assert!(
        post.contains("counter=1"),
        "counter did not advance after clicking btn-increment. counter lines: {}",
        post.lines().filter(|l| l.contains("counter=")).collect::<Vec<_>>().join(" / ")
    );
    println!("✅ harness_gtk3_counter_click: counter advanced via AT-SPI click");
}

#[test]
#[ignore]
fn harness_gtk3_popover() {
    let Some(mut driver) = McpDriver::spawn() else { return };
    let Some((pid, wid)) = launch(&mut driver) else { return };

    let pre = snapshot(&mut driver, pid, wid);
    if ax_empty(&pre) {
        eprintln!("AT-SPI empty — skipping");
        return;
    }
    assert!(!pre.contains("POPOVER_MARKER_v1"), "popover body present BEFORE open");

    let idx = match ax::element_index_containing(&pre, "btn-open-popover") {
        Some(i) => i,
        None => {
            eprintln!("btn-open-popover not found — skipping");
            return;
        }
    };
    let _ = driver.call(
        "click",
        serde_json::json!({ "pid": pid as i64, "window_id": wid, "element_index": idx }),
    );
    std::thread::sleep(Duration::from_millis(500));

    // GtkPopover may surface as a separate top-level; check the main window
    // first, then any new window of this pid.
    let mut found = snapshot(&mut driver, pid, wid).contains("POPOVER_MARKER_v1");
    if !found {
        let r = driver.call("list_windows", serde_json::json!({}));
        if let Some(wins) = r.structured()["windows"].as_array() {
            for w in wins {
                if w["pid"].as_u64() == Some(pid as u64) {
                    if let Some(other) = w["window_id"].as_u64() {
                        if other != wid && snapshot(&mut driver, pid, other).contains("POPOVER_MARKER_v1") {
                            found = true;
                            break;
                        }
                    }
                }
            }
        }
    }
    assert!(found, "popover body marker POPOVER_MARKER_v1 not found after open");
    println!("✅ harness_gtk3_popover: popover body enumerated after open");
}
