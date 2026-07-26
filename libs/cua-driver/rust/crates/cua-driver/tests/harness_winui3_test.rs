//! Integration test against the CuaTestHarness.WinUI3 .NET 8 app.
//!
//! Pair-companion to harness_wpf_test.rs but exercises WinUI3 + DComp
//! rendering paths. The two tests share scenario IDs (counter, text_body,
//! exit) so any AutomationId regression that affects both UI frameworks
//! shows up in both suites.
//!
//! WinUI3-specific surfaces under test:
//! - CommandBarFlyout: popup hosted in same HWND, rendered via XAML Islands /
//!   DComp. Tests UIA descent into the flyout subtree.
//! - XAML Popup: Popup primitive (NOT a separate HWND). Regression guard that
//!   the agent doesn't lose track of in-window flyouts.
//!
//! Run via:
//!   ..\tests\runners\windows-sandbox\run-tests-in-sandbox.ps1 harness_winui3
//! or locally:
//!   cargo test --test harness_winui3_test -- --ignored --nocapture

#![cfg(target_os = "windows")]

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cua_driver_testkit::ax::element_index_by_id;
use cua_driver_testkit::e2e::{
    execute_case, native_background_case, native_readonly_case, recording_evidence, DriverRoute,
    Evidence, Observation, OracleKind, Targeting,
};
use cua_driver_testkit::observer::TargetWindow;
use cua_driver_testkit::sentinel::run_with_background_oracles;
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
        eprintln!("WinUI3 harness exe not found at {exe:?} — run tests/fixtures/build/windows.ps1");
        return None;
    }
    let child = spawn_in_job(
        Command::new(&exe)
            .stdout(Stdio::null())
            .stderr(Stdio::null()),
    )
    .ok()?;
    let pid = child.id();
    driver.reaper().push(child);
    Some(pid)
}

fn wait_for_winui3_ready(driver: &mut McpDriver, pid: u32, window_id: u64) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let state = driver.call(
            "get_window_state",
            serde_json::json!({"pid": pid as i64, "window_id": window_id}),
        );
        let last_state = state.text();
        if !state.is_error()
            && last_state.contains("HARNESS_TEXT_MARKER_v1")
            && last_state.contains("id=chk-agreed")
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "WinUI3 UIA tree did not become ready: {last_state}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn run_case(
    case: cua_driver_testkit::e2e::CaseSpec,
    test: impl FnOnce(u32, u64, &mut McpDriver) -> Observation,
) {
    let cell_id = case.cell_id.clone();
    let delivery = case.delivery;
    execute_case(case, |evidence| {
        let mut driver = McpDriver::spawn_named(&cell_id)
            .expect("required source-built Windows driver did not start");
        *evidence = recording_evidence(driver.recording_dir());
        let pid = launch_winui3(&mut driver).expect("required WinUI3 harness did not launch");
        let (wid, _) = driver
            .find_window(pid as i64, "CuaTestHarness WinUI3")
            .expect("WinUI3 main window not found");
        wait_for_winui3_ready(&mut driver, pid, wid);
        if delivery != cua_driver_testkit::e2e::Delivery::Background {
            driver.start_behavior_recording();
        }
        test(pid, wid, &mut driver)
    });
}

fn run_background_case(
    action: &str,
    route: DriverRoute,
    test: impl FnOnce(u32, u64, &mut McpDriver),
) {
    run_case(
        native_background_case("winui3", action, Targeting::Ax, route),
        |pid, wid, driver| {
            let (_, passed) = run_with_background_oracles(
                driver,
                TargetWindow {
                    pid,
                    native_id: wid,
                },
                |driver| test(pid, wid, driver),
            )
            .unwrap_or_else(|error| panic!("background desktop contract failed: {error}"));
            Observation::delivered_with_fixture_state(passed)
        },
    );
}

#[test]
#[ignore]
fn harness_winui3_smoke() {
    run_case(
        native_readonly_case(
            "winui3",
            "ax_tree",
            Targeting::Ax,
            DriverRoute::AxRead,
            vec![OracleKind::AxState],
        ),
        |pid, wid, driver| {
            println!("WinUI3 harness pid={pid}");
            let snap = driver.call(
                "get_window_state",
                serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode":"ax"}),
            );
            assert!(
                !snap.is_error(),
                "WinUI3 AX snapshot failed: {}",
                snap.text()
            );
            let text = snap.text();
            for aid in [
                "btn-increment",
                "btn-reset",
                "btn-open-flyout",
                "btn-open-popup",
                "btn-exit",
            ] {
                assert!(
                    text.contains(&format!("id={aid}")),
                    "missing AutomationId {aid} in WinUI3 UIA snapshot"
                );
            }
            assert!(
                text.contains("HARNESS_TEXT_MARKER_v1"),
                "WinUI3 text_body marker not in snapshot"
            );
            assert!(
                text.contains("counter=0"),
                "WinUI3 initial counter label not in snapshot"
            );
            Observation::delivered(vec![OracleKind::AxState], Evidence::default())
        },
    );
}

#[test]
#[ignore]
fn harness_winui3_type_text() {
    run_background_case("type_text", DriverRoute::UiaValue, |pid, wid, driver| {
        let snap = driver.call(
            "get_window_state",
            serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode":"ax"}),
        );
        let idx = element_index_by_id(snap.text(), "txt-input")
            .expect("txt-input not in WinUI3 snapshot");
        let resp = driver.call(
            "type_text",
            serde_json::json!({
                "pid": pid as i64, "window_id": wid, "element_index": idx,
                "text": "winui3-typed", "delivery_mode": "background"
            }),
        );
        assert!(!resp.is_error(), "WinUI3 type_text failed: {}", resp.text());
        std::thread::sleep(Duration::from_millis(500));
        let post = driver.call(
            "get_window_state",
            serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode":"ax"}),
        );
        assert!(
            post.text().contains("mirror=winui3-typed"),
            "WinUI3 TextBox mirror did not advance. Snapshot excerpt: {}",
            post.text().chars().take(600).collect::<String>()
        );
    });
}

#[test]
#[ignore]
fn harness_winui3_xaml_popup_open() {
    run_background_case(
        "xaml_popup_open",
        DriverRoute::UiaExpandCollapse,
        |pid, wid, driver| {
            let snap = driver.call(
                "get_window_state",
                serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode":"ax"}),
            );
            let idx = element_index_by_id(snap.text(), "btn-open-popup").expect("btn-open-popup");
            let response = driver.call(
                "click",
                serde_json::json!({
                    "pid": pid as i64, "window_id": wid, "element_index": idx,
                    "delivery_mode": "background"
                }),
            );
            assert!(
                !response.is_error(),
                "open popup failed: {}",
                response.text()
            );
            std::thread::sleep(Duration::from_millis(500));
            let post = driver.call(
                "get_window_state",
                serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode":"ax"}),
            );
            assert!(
                post.text().contains("XAML_POPUP_MARKER_v1"),
                "XAML popup body did not appear in tree after click. Excerpt: {}",
                post.text().chars().take(600).collect::<String>()
            );
        },
    );
}

/// Regression guard for the click → TogglePattern dispatch fix.
/// cua-driver `click` now tries Invoke → Toggle → SelectionItem →
/// ExpandCollapse before falling through to PostMessage, so WinUI3
/// CheckBox toggles correctly via UIA without needing dispatch:foreground.
#[test]
#[ignore]
fn harness_winui3_checkbox_toggle() {
    run_background_case(
        "checkbox_toggle",
        DriverRoute::UiaToggle,
        |pid, wid, driver| {
            let snap = driver.call(
                "get_window_state",
                serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode": "ax"}),
            );
            let idx = element_index_by_id(snap.text(), "chk-agreed").expect("chk-agreed");
            let response = driver.call(
                "click",
                serde_json::json!({
                    "pid": pid as i64, "window_id": wid, "element_index": idx,
                    "delivery_mode": "background"
                }),
            );
            assert!(
                !response.is_error(),
                "checkbox toggle failed: {}",
                response.text()
            );
            std::thread::sleep(Duration::from_millis(400));
            let post = driver.call(
                "get_window_state",
                serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode": "ax"}),
            );
            assert!(
                post.text().contains("agreed=True"),
                "WinUI3 CheckBox didn't toggle: TogglePattern dispatch may have regressed."
            );
            println!("✅ harness_winui3_checkbox_toggle: agreed=True via UIA Toggle");
        },
    );
}

/// Regression guard for SelectionItem.Select dispatch on RadioButton.
#[test]
#[ignore]
fn harness_winui3_radio_select() {
    run_background_case(
        "radio_select",
        DriverRoute::UiaSelection,
        |pid, wid, driver| {
            let snap = driver.call(
                "get_window_state",
                serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode": "ax"}),
            );
            let idx = element_index_by_id(snap.text(), "rdo-high").expect("rdo-high");
            let response = driver.call(
                "click",
                serde_json::json!({
                    "pid": pid as i64, "window_id": wid, "element_index": idx,
                    "delivery_mode": "background"
                }),
            );
            assert!(
                !response.is_error(),
                "radio select failed: {}",
                response.text()
            );
            std::thread::sleep(Duration::from_millis(400));
            let post = driver.call(
                "get_window_state",
                serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode": "ax"}),
            );
            assert!(
                post.text().contains("prio=High"),
                "WinUI3 radio didn't select High via SelectionItem.Select."
            );
            println!("✅ harness_winui3_radio_select: prio=High via UIA SelectionItem");
        },
    );
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
    run_background_case(
        "slider_set_value",
        DriverRoute::UiaRangeValue,
        |pid, wid, driver| {
            let snap = driver.call(
                "get_window_state",
                serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode": "ax"}),
            );
            let idx = element_index_by_id(snap.text(), "sld-value")
            .expect("sld-value should now be in the UIA flat tree after RangeValuePattern detection fix");
            let resp = driver.call(
                "set_value",
                serde_json::json!({
                    "pid": pid as i64, "window_id": wid, "element_index": idx,
                    "value": "42"
                }),
            );
            println!("set_value sld-value=42: {}", resp.text());
            assert!(!resp.is_error(), "slider set_value failed: {}", resp.text());
            std::thread::sleep(Duration::from_millis(400));
            let post = driver.call(
                "get_window_state",
                serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode": "ax"}),
            );
            let text = post.text();
            let advanced = text
                .lines()
                .any(|l| l.contains("slider_value=") && !l.contains("slider_value=0\""));
            assert!(
                advanced,
                "WinUI3 Slider didn't move via RangeValuePattern.SetValue. Lines: {}",
                text.lines()
                    .filter(|l| l.contains("slider_value"))
                    .collect::<Vec<_>>()
                    .join(" / ")
            );
            println!("✅ harness_winui3_slider_set_value: value moved via UIA RangeValuePattern.SetValue");
        },
    );
}

/// Regression guard for ExpandCollapse.Expand + SelectionItem.Select on
/// WinUI3 ComboBox.
#[test]
#[ignore]
fn harness_winui3_combo_select() {
    run_background_case(
        "combo_select",
        DriverRoute::Composite,
        |pid, wid, driver| {
            let snap = driver.call(
                "get_window_state",
                serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode": "ax"}),
            );
            let combo_idx = element_index_by_id(snap.text(), "cbo-color").expect("cbo-color");
            // Expand the dropdown via ExpandCollapse.
            let expand = driver.call(
                "click",
                serde_json::json!({
                    "pid": pid as i64, "window_id": wid, "element_index": combo_idx,
                    "delivery_mode": "background"
                }),
            );
            assert!(!expand.is_error(), "combo expand failed: {}", expand.text());
            std::thread::sleep(Duration::from_millis(400));
            // Re-snapshot — items materialize after expand.
            let snap2 = driver.call(
                "get_window_state",
                serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode": "ax"}),
            );
            let item_idx = element_index_by_id(snap2.text(), "cbo-item-orange")
                .expect("cbo-item-orange after expand");
            // Select the item via SelectionItem.Select.
            let select = driver.call(
                "click",
                serde_json::json!({
                    "pid": pid as i64, "window_id": wid, "element_index": item_idx,
                    "delivery_mode": "background"
                }),
            );
            assert!(!select.is_error(), "combo select failed: {}", select.text());
            std::thread::sleep(Duration::from_millis(400));
            let post = driver.call(
                "get_window_state",
                serde_json::json!({"pid": pid as i64, "window_id": wid, "capture_mode": "ax"}),
            );
            assert!(
                post.text().contains("color=orange"),
                "WinUI3 combo didn't switch to orange via ExpandCollapse + SelectionItem.Select."
            );
            println!("✅ harness_winui3_combo_select: color=orange via UIA Expand + Select");
        },
    );
}
