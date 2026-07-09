//! Integration test against the CuaTestHarness.SwiftUI Swift app.
//!
//! macOS equivalent of `harness_winui3_test.rs` — SwiftUI plays the same
//! role on macOS that WinUI3 plays on Windows: the modern declarative
//! UI hosting pattern with retained-mode AX exposed via a different
//! backend than the older AppKit one.
//!
//! Scenarios (see scenarios.json `swiftui` section):
//!   - counter   : SwiftUI Button increments State<Int>
//!   - text_body : Text with HARNESS_TEXT_MARKER_v1
//!   - text_input: TextField with mirror Text
//!   - popover   : .popover() — SwiftUI analogue of WinUI3 CommandBarFlyout
//!
//! Run locally (after `libs/cua-driver/test-harness/build/macos.sh`):
//!   cargo test --test harness_swiftui_test -- --ignored --nocapture

#![cfg(target_os = "macos")]

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use cua_driver_testkit::ax::{element_index_by_id, has_id, looks_empty};
use cua_driver_testkit::{harness_app, Driver, McpDriver, ToolResponse};

fn harness_exe() -> PathBuf {
    if let Ok(p) = std::env::var("HARNESS_SWIFTUI_APP") {
        let pb = PathBuf::from(p).join("Contents/MacOS/CuaTestHarness.SwiftUI");
        if pb.exists() {
            return pb;
        }
    }
    harness_app(
        "harness-swiftui",
        "CuaTestHarness.SwiftUI.app/Contents/MacOS/CuaTestHarness.SwiftUI",
    )
}

struct Harness {
    _app: Child,
    pid: u32,
}

impl Harness {
    fn launch() -> Option<Self> {
        let exe = harness_exe();
        if !exe.exists() {
            eprintln!("harness exe not found at {exe:?}");
            return None;
        }
        let app = Command::new(&exe)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let pid = app.id();
        std::thread::sleep(Duration::from_millis(900));
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

fn snapshot_elements(driver: &mut McpDriver, pid: u32, window_id: u64) -> ToolResponse {
    driver.call(
        "get_window_state",
        serde_json::json!({
            "pid": pid as i64,
            "window_id": window_id,
            "capture_mode": "ax"
        }),
    )
}

#[test]
#[ignore]
fn harness_swiftui_smoke() {
    let harness = match Harness::launch() {
        Some(h) => h,
        None => {
            eprintln!("harness not built — skipping");
            return;
        }
    };
    println!("harness pid={}", harness.pid);

    let Some(mut driver) = McpDriver::spawn_macos_daemon_proxy() else {
        return;
    };

    let (wid, title) = driver
        .find_window(harness.pid as i64, "CuaTestHarness SwiftUI")
        .expect("main window not found");
    println!("main window: id={wid} title={title:?}");

    let snap = snapshot_elements(&mut driver, harness.pid, wid);
    if looks_empty(snap.tree_text()) {
        eprintln!("AX empty — TCC not granted; skipping element-assertions");
        return;
    }
    let text = snap.tree_text();
    println!("snapshot:\n{text}");

    // SwiftUI Text views render as AXStaticText leaves and don't propagate
    // accessibilityIdentifier into the AX tree's identifier slot (same
    // quirk as AppKit's NSTextField label mode + WPF's TextBlock). Assert
    // on text content for labels, AX-id only for actionable controls.
    for aid in [
        "btn-increment",
        "btn-reset",
        "txt-input",
        "btn-open-popover",
        "btn-exit",
    ] {
        assert!(
            has_id(snap.tree_text(), aid),
            "missing AX identifier {aid} in SwiftUI snapshot"
        );
    }

    assert!(
        text.contains("HARNESS_TEXT_MARKER_v1"),
        "text_body marker not in SwiftUI snapshot"
    );
}

/// popover: click the popover trigger, verify the popover body text appears
/// in the AX tree after the open. SwiftUI's analogue of WinUI3 CommandBarFlyout.
#[test]
#[ignore]
fn harness_swiftui_popover() {
    let harness = match Harness::launch() {
        Some(h) => h,
        None => {
            eprintln!("harness not built — skipping");
            return;
        }
    };

    let Some(mut driver) = McpDriver::spawn_macos_daemon_proxy() else {
        return;
    };

    let (wid, _) = driver
        .find_window(harness.pid as i64, "CuaTestHarness SwiftUI")
        .expect("main window not found");
    let snap_pre = snapshot_elements(&mut driver, harness.pid, wid);
    if looks_empty(snap_pre.tree_text()) {
        eprintln!("AX empty — TCC not granted; skipping");
        return;
    }
    // Verify popover body is NOT yet in the tree.
    let pre_text = snap_pre.tree_text().to_owned();
    assert!(
        !pre_text.contains("POPOVER_MARKER_v1"),
        "popover body unexpectedly present BEFORE open"
    );

    let trigger_idx: u64 =
        if let Some(i) = element_index_by_id(snap_pre.tree_text(), "btn-open-popover") {
            i
        } else {
            eprintln!("popover trigger not found, skipping");
            return;
        };
    let click = driver.call(
        "click",
        serde_json::json!({
            "pid": harness.pid as i64,
            "window_id": wid,
            "element_index": trigger_idx,
            "action": "press"
        }),
    );
    println!("popover trigger click: {}", click.text());

    std::thread::sleep(Duration::from_millis(300));

    // Popovers may live in a separate AXWindow on macOS — try the main
    // window first, then list_windows for additional candidates.
    let snap_post = snapshot_elements(&mut driver, harness.pid, wid);
    let mut found_marker = snap_post.tree_text().contains("POPOVER_MARKER_v1");
    if !found_marker {
        // Walk any new windows for the same pid.
        let resp = driver.call(
            "list_windows",
            serde_json::json!({ "pid": harness.pid as i64 }),
        );
        if let Some(wins) = resp.structured()["windows"].as_array() {
            for w in wins {
                if let Some(other_wid) = w["window_id"].as_u64() {
                    if other_wid == wid {
                        continue;
                    }
                    let s = snapshot_elements(&mut driver, harness.pid, other_wid);
                    if s.tree_text().contains("POPOVER_MARKER_v1") {
                        found_marker = true;
                        break;
                    }
                }
            }
        }
    }
    assert!(found_marker, "popover body marker not found after open");
}
