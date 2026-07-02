//! Integration test against the CuaTestHarness.WinUI3 .NET 8 app.
//!
//! Pair-companion to harness_wpf_test.rs but exercises WinUI3 + DComp
//! rendering paths. The two tests share scenario IDs (counter, text_body,
//! exit) so any AutomationId regression that affects both UI frameworks
//! shows up in both suites.
//!
//! WinUI3-specific surfaces under test:
//!   - CommandBarFlyout : popup hosted in same HWND, rendered via XAML
//!                        Islands / DComp. Tests UIA descent into the
//!                        flyout subtree.
//!   - XAML Popup       : Popup primitive (NOT a separate HWND).
//!                        Regression guard that the agent doesn't lose
//!                        track of in-window flyouts.
//!
//! Run via:
//!   .\sandbox\run-tests-in-sandbox.ps1 harness_winui3
//! or locally:
//!   cargo test --test harness_winui3_test -- --ignored --nocapture

#![cfg(target_os = "windows")]

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use cua_driver_testkit::ax::element_index_by_id;
use cua_driver_testkit::{harness_app, spawn_in_job, Driver, McpDriver};

/// Resolve the WinUI3 harness exe — the `HARNESS_WINUI3_EXE` override wins (if it
/// points at an existing file), else the built path under `test-apps/`.
fn harness_winui3_exe() -> PathBuf {
    if let Ok(p) = std::env::var("HARNESS_WINUI3_EXE") {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return pb;
        }
    }
    harness_app("harness-winui3", "CuaTestHarness.WinUI3.exe")
}

/// Launch the WinUI3 harness through the driver's reaper (kill-on-close Job
/// Object) and return its pid. Returns `None` (skip) if the harness isn't built.
fn launch_winui3(driver: &mut McpDriver) -> Option<u32> {
    let exe = harness_winui3_exe();
    if !exe.exists() {
        eprintln!("WinUI3 harness exe not found at {exe:?} — run test-harness/build/windows.ps1");
        return None;
    }
    let child = spawn_in_job(Command::new(&exe).stdout(Stdio::null()).stderr(Stdio::null())).ok()?;
    let pid = child.id();
    driver.reaper().push(child);
    // Short fixed cold-start settle (window creation + foreground
    // establishment after spawn). `find_window`'s polling handles the
    // variable tail (WinUI3 first-run cold-start under sandbox load).
    std::thread::sleep(Duration::from_millis(1500));
    Some(pid)
}

#[test]
#[ignore]
fn harness_winui3_smoke() {
    let Some(mut driver) = McpDriver::spawn() else { return };
    let Some(pid) = launch_winui3(&mut driver) else { return };
    println!("WinUI3 harness pid={pid}");

    let (wid, _) = driver
        .find_window(pid as i64, "CuaTestHarness WinUI3")
        .expect("WinUI3 main window not found");

    let snap = driver.call(
        "get_window_state",
        serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode":"ax"}),
    );
    let text = snap.text();

    // Button-class controls surface AutomationIds in the UIA tree.
    for aid in [
        "btn-increment", "btn-reset",
        "btn-open-flyout",
        "btn-open-popup",
        "btn-exit",
    ] {
        assert!(text.contains(&format!("id={aid}")),
            "missing AutomationId {aid} in WinUI3 UIA snapshot");
    }

    // TextBlock content (no AutomationId surfaces) — assert markers.
    assert!(text.contains("HARNESS_TEXT_MARKER_v1"), "WinUI3 text_body marker not in snapshot");
    assert!(text.contains("counter=0"),             "WinUI3 initial counter label not in snapshot");

    println!("✅ harness_winui3_smoke: all expected scenarios present in UIA tree");
}

#[test]
#[ignore]
fn harness_winui3_type_text() {
    let Some(mut driver) = McpDriver::spawn() else { return };
    let Some(pid) = launch_winui3(&mut driver) else { return };

    let (wid, _) = driver
        .find_window(pid as i64, "CuaTestHarness WinUI3")
        .expect("WinUI3 main window");

    let snap = driver.call(
        "get_window_state",
        serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode":"ax"}),
    );
    let idx = element_index_by_id(snap.text(), "txt-input").expect("txt-input not in WinUI3 snapshot");

    // WinUI3 is a XAML host — type_text requires element_index + window_id
    // (routes through UIA ValuePattern.SetValue, see Windows backend docs).
    let resp = driver.call("type_text", serde_json::json!({
        "pid": pid as i64,
        "window_id": wid,
        "element_index": idx,
        "text": "winui3-typed"
    }));
    println!("type_text (WinUI3): {}", resp.text());
    std::thread::sleep(Duration::from_millis(500));

    let post = driver.call(
        "get_window_state",
        serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode":"ax"}),
    );
    assert!(post.text().contains("mirror=winui3-typed"),
        "WinUI3 TextBox mirror did not advance. Snapshot excerpt: {}",
        post.text().chars().take(600).collect::<String>());
    println!("✅ harness_winui3_type_text: WinUI3 TextBox mirror reflects 'winui3-typed'");
}

#[test]
#[ignore]
fn harness_winui3_xaml_popup_open() {
    let Some(mut driver) = McpDriver::spawn() else { return };
    let Some(pid) = launch_winui3(&mut driver) else { return };

    let (wid, _) = driver
        .find_window(pid as i64, "CuaTestHarness WinUI3")
        .expect("WinUI3 main window");

    let snap = driver.call(
        "get_window_state",
        serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode":"ax"}),
    );
    let idx = element_index_by_id(snap.text(), "btn-open-popup").expect("btn-open-popup");

    let _ = driver.call("click", serde_json::json!({
        "pid": pid as i64, "window_id": wid, "element_index": idx
    }));
    std::thread::sleep(Duration::from_millis(500));

    let post = driver.call(
        "get_window_state",
        serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode":"ax"}),
    );
    let text = post.text();
    assert!(text.contains("XAML_POPUP_MARKER_v1"),
        "XAML popup body did not appear in tree after click. Excerpt: {}",
        text.chars().take(600).collect::<String>());
    println!("✅ harness_winui3_xaml_popup_open: popup body visible in UIA tree");
}

// ── Session helper for the additional control tests ──────────────────────────

fn winui3_with_session<F>(f: F)
where
    F: FnOnce(u32, u64, &mut McpDriver),
{
    let Some(mut driver) = McpDriver::spawn() else { return };
    let Some(pid) = launch_winui3(&mut driver) else { return };
    let (wid, _) = driver
        .find_window(pid as i64, "CuaTestHarness WinUI3")
        .expect("WinUI3 main window");
    f(pid, wid, &mut driver);
}

/// Regression guard for the click → TogglePattern dispatch fix.
/// cua-driver `click` now tries Invoke → Toggle → SelectionItem →
/// ExpandCollapse before falling through to PostMessage, so WinUI3
/// CheckBox toggles correctly via UIA without needing dispatch:foreground.
#[test]
#[ignore]
fn harness_winui3_checkbox_toggle() {
    winui3_with_session(|pid, wid, driver| {
        let snap = driver.call("get_window_state",
            serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode": "ax"}));
        let idx = element_index_by_id(snap.text(), "chk-agreed").expect("chk-agreed");
        let _ = driver.call("click", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "element_index": idx
        }));
        std::thread::sleep(Duration::from_millis(400));
        let post = driver.call("get_window_state",
            serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode": "ax"}));
        assert!(post.text().contains("agreed=True"),
            "WinUI3 CheckBox didn't toggle: TogglePattern dispatch may have regressed.");
        println!("✅ harness_winui3_checkbox_toggle: agreed=True via UIA Toggle");
    });
}

/// Regression guard for SelectionItem.Select dispatch on RadioButton.
#[test]
#[ignore]
fn harness_winui3_radio_select() {
    winui3_with_session(|pid, wid, driver| {
        let snap = driver.call("get_window_state",
            serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode": "ax"}));
        let idx = element_index_by_id(snap.text(), "rdo-high").expect("rdo-high");
        let _ = driver.call("click", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "element_index": idx
        }));
        std::thread::sleep(Duration::from_millis(400));
        let post = driver.call("get_window_state",
            serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode": "ax"}));
        assert!(post.text().contains("prio=High"),
            "WinUI3 radio didn't select High via SelectionItem.Select.");
        println!("✅ harness_winui3_radio_select: prio=High via UIA SelectionItem");
    });
}

/// Documents cua-driver gap: WinUI3 Slider implements
/// `RangeValuePattern`, not `ValuePattern`. cua-driver's `set_value` tool
/// queries `ValuePatternId` specifically (impl_.rs:2640), so it silently
/// fails on RangeValuePattern-only elements. Real fix: cua-driver should
/// try RangeValuePattern.SetValue (coercing the string to a double) when
/// ValuePattern isn't found.
/// Regression guard for Slider element enumeration + RangeValuePattern
/// set_value. cua-driver's UIA cache now pre-fetches RangeValuePattern,
/// and `detect_cached_actions` reports `set_value` when present — so
/// Slider parents get an `[N]` flat-tree index. The `set_value` tool
/// already falls back to RangeValuePattern, so writing the value works
/// end-to-end against a WinUI3 Slider.
#[test]
#[ignore]
fn harness_winui3_slider_set_value() {
    winui3_with_session(|pid, wid, driver| {
        let snap = driver.call("get_window_state",
            serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode": "ax"}));
        let idx = element_index_by_id(snap.text(), "sld-value")
            .expect("sld-value should now be in the UIA flat tree after RangeValuePattern detection fix");
        let resp = driver.call("set_value", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "element_index": idx,
            "value": "42"
        }));
        println!("set_value sld-value=42: {}", resp.text());
        std::thread::sleep(Duration::from_millis(400));
        let post = driver.call("get_window_state",
            serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode": "ax"}));
        let text = post.text();
        let advanced = text.lines().any(|l|
            l.contains("slider_value=") && !l.contains("slider_value=0\""));
        assert!(advanced,
            "WinUI3 Slider didn't move via RangeValuePattern.SetValue. Lines: {}",
            text.lines().filter(|l| l.contains("slider_value"))
                .collect::<Vec<_>>().join(" / "));
        println!("✅ harness_winui3_slider_set_value: value moved via UIA RangeValuePattern.SetValue");
    });
}

/// Regression guard for ExpandCollapse.Expand + SelectionItem.Select on
/// WinUI3 ComboBox.
#[test]
#[ignore]
fn harness_winui3_combo_select() {
    winui3_with_session(|pid, wid, driver| {
        let snap = driver.call("get_window_state",
            serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode": "ax"}));
        let combo_idx = element_index_by_id(snap.text(), "cbo-color").expect("cbo-color");
        // Expand the dropdown via ExpandCollapse.
        let _ = driver.call("click", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "element_index": combo_idx
        }));
        std::thread::sleep(Duration::from_millis(400));
        // Re-snapshot — items materialize after expand.
        let snap2 = driver.call("get_window_state",
            serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode": "ax"}));
        let item_idx = element_index_by_id(snap2.text(), "cbo-item-orange")
            .expect("cbo-item-orange after expand");
        // Select the item via SelectionItem.Select.
        let _ = driver.call("click", serde_json::json!({
            "pid": pid as i64, "window_id": wid, "element_index": item_idx
        }));
        std::thread::sleep(Duration::from_millis(400));
        let post = driver.call("get_window_state",
            serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode": "ax"}));
        assert!(post.text().contains("color=orange"),
            "WinUI3 combo didn't switch to orange via ExpandCollapse + SelectionItem.Select.");
        println!("✅ harness_winui3_combo_select: color=orange via UIA Expand + Select");
    });
}
