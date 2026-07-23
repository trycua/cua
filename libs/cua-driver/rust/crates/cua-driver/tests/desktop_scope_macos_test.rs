//! macOS **desktop-scope** (vision/foreground) modality, exercised through the
//! SAME cua-driver interface as the Windows `desktop_scope_windows_test`:
//! a window-less screen-absolute `click` with `scope=desktop`
//! (no `pid`, no `window_id`, no `list_windows`). The macOS actuator resolves
//! the frontmost on-screen window under the point (the `WindowFromPoint` peer,
//! via `CGWindowList`) and clicks it through the proven window-local pixel path,
//! falling back to a cursor-warp + HID post when no window owns the pixel — the
//! deliberate foreground complement to the background no-foreground contract.
//!
//! The test grounds the click on a real control: it reads the increment
//! button's screen-absolute `frame` from a window-scope AX snapshot (the way an
//! agent would locate it by vision in `get_desktop_state`), clicks those screen
//! pixels window-less, and asserts the harness counter advanced. A second test
//! asserts the `window`-scope gate rejects a window-less click
//! (`desktop_scope_disabled`), matching the Windows contract.
//!
//! #[ignore] (needs a real desktop session + TCC Accessibility + the AppKit
//! harness). Run:
//!   cargo test -p cua-driver --test desktop_scope_macos_test -- --ignored --nocapture --test-threads=1

#![cfg(target_os = "macos")]

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cua_driver_testkit::e2e::{
    execute_case, recording_evidence, CaseSpec, Delivery, DriverRoute, Evidence, Observation,
    OracleKind, Scope, Targeting,
};
use cua_driver_testkit::{harness_app, Driver, McpDriver};

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
        if std::env::var_os("CUA_TEST_REQUIRE_FIXTURES").is_some() {
            panic!("required AppKit harness is missing at {exe:?}");
        }
        eprintln!("[desktop-mac] AppKit harness not built ({exe:?}) — run tests/fixtures/build/macos.sh; skipping");
        return None;
    }
    let child = match cua_driver_testkit::spawn_in_job(
        Command::new(&exe)
            .stdout(Stdio::null())
            .stderr(Stdio::null()),
    ) {
        Ok(child) => child,
        Err(error) => {
            if std::env::var_os("CUA_TEST_REQUIRE_FIXTURES").is_some() {
                panic!("failed to launch required AppKit harness {exe:?}: {error}");
            }
            eprintln!("[desktop-mac] AppKit harness launch failed: {error}; skipping");
            return None;
        }
    };
    let launched_pid = child.id();
    driver.reaper().push(child);
    let deadline = Instant::now() + Duration::from_secs(14);
    while Instant::now() < deadline {
        let r = driver.call("list_windows", serde_json::json!({}));
        if let Some(wins) = r.structured()["windows"].as_array() {
            for w in wins {
                if w["pid"].as_u64() != Some(launched_pid as u64) {
                    continue;
                }
                if w["title"]
                    .as_str()
                    .unwrap_or("")
                    .contains("CuaTestHarness AppKit")
                {
                    let pid = w["pid"].as_u64().unwrap_or(0) as u32;
                    let wid = w["window_id"].as_u64().unwrap_or(0);
                    if pid != 0 && wid != 0 {
                        return Some((pid, wid));
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(400));
    }
    if std::env::var_os("CUA_TEST_REQUIRE_FIXTURES").is_some() {
        panic!("required AppKit harness window never appeared");
    }
    eprintln!(
        "[desktop-mac] harness window never appeared — graphical session available? skipping"
    );
    None
}

fn ax_snapshot(driver: &mut McpDriver, session: &str, pid: u32, wid: u64) -> serde_json::Value {
    driver
        .call(
            "get_window_state",
            serde_json::json!({ "session": session, "pid": pid as i64, "window_id": wid, "capture_mode": "ax" }),
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
            && e["label"]
                .as_str()
                .map(|l| l.trim().eq_ignore_ascii_case("increment"))
                .unwrap_or(false)
    })?;
    let f = &btn["frame"];
    let (x, y, w, h) = (
        f["x"].as_f64()?,
        f["y"].as_f64()?,
        f["w"].as_f64()?,
        f["h"].as_f64()?,
    );
    Some(((x + w / 2.0) as i64, (y + h / 2.0) as i64))
}

fn element_center_containing(snap: &serde_json::Value, marker: &str) -> Option<(i64, i64)> {
    let element = snap["elements"].as_array()?.iter().find(|element| {
        serde_json::to_string(element)
            .map(|serialized| serialized.contains(marker))
            .unwrap_or(false)
            && element
                .get("frame")
                .map(|frame| frame.is_object())
                .unwrap_or(false)
    })?;
    let frame = &element["frame"];
    Some((
        (frame["x"].as_f64()? + frame["w"].as_f64()? / 2.0) as i64,
        (frame["y"].as_f64()? + frame["h"].as_f64()? / 2.0) as i64,
    ))
}

fn marker_value(snap: &serde_json::Value, marker: &str) -> Option<u64> {
    let tree = snap["tree_markdown"].as_str()?;
    let idx = tree.find(marker)? + marker.len();
    let digits: String = tree[idx..]
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

/// The current counter value parsed from the snapshot's `tree_markdown`
/// (`counter=N`). The AppKit AXStaticText label doesn't propagate to
/// `elements[].label`, so the value lives in the rendered tree text.
fn counter(snap: &serde_json::Value) -> Option<u64> {
    let tree = snap["tree_markdown"].as_str()?;
    let idx = tree.find("counter=")? + "counter=".len();
    let digits: String = tree[idx..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

fn start_scope(driver: &mut McpDriver, session: &str, capture_scope: &str) {
    let started = driver.call(
        "start_session",
        serde_json::json!({"session": session, "capture_scope": capture_scope}),
    );
    assert!(
        !started.is_error(),
        "start_session capture_scope={capture_scope} failed: {}",
        started.text()
    );
}

/// Bring the harness process to the foreground. Desktop scope is the
/// *foreground* modality — a screen-absolute click lands on whatever is visually
/// frontmost at the point — so the test puts the harness there first (it stands
/// in for "the app the user is looking at"). A binary launched directly from the
/// test process is not activated by LaunchServices, unlike Linux's GTK3 harness
/// which grabs focus on map.
fn activate_pid(pid: u32) {
    let _ = std::process::Command::new("osascript")
        .arg("-e")
        .arg(format!(
            "tell application \"System Events\" to set frontmost of \
             (first process whose unix id is {pid}) to true"
        ))
        .output();
    std::thread::sleep(Duration::from_millis(500));
}

// scope is a per-call param on `click` now (was a config setting) — passed
// directly on each click below, no set_config step.

// ── tests ───────────────────────────────────────────────────────────────────────

/// In desktop scope, a window-less screen-absolute click (no pid/window_id)
/// lands on a real control: the increment button's counter advances.
#[test]
#[ignore]
fn desktop_scope_windowless_click_lands_on_control() {
    let cell_id = "macos-appkit-desktop-left-click-px-foreground";
    let case = CaseSpec::delivered(
        cell_id,
        "appkit",
        "appkit",
        "left_click",
        Targeting::Px,
        Delivery::Foreground,
        Scope::Desktop,
        DriverRoute::MacosCgEventHid,
        vec![OracleKind::FixtureState],
    );
    execute_case(case, |evidence| {
        let mut driver = McpDriver::spawn_macos_daemon_proxy_named(cell_id)
            .expect("start installed macOS daemon proxy");
        *evidence = recording_evidence(driver.recording_dir());
        let window_session = format!("{cell_id}-window");
        let desktop_session = format!("{cell_id}-desktop");
        for (session, capture_scope) in [
            (window_session.as_str(), "window"),
            (desktop_session.as_str(), "desktop"),
        ] {
            let started = driver.call(
                "start_session",
                serde_json::json!({"session": session, "capture_scope": capture_scope}),
            );
            assert!(
                !started.is_error(),
                "start_session failed: {}",
                started.text()
            );
        }
        let (pid, wid) = launch(&mut driver).expect("required AppKit harness did not launch");

        // Settle for the AppKit AX tree to register the button + its frame.
        let mut snap = ax_snapshot(&mut driver, &window_session, pid, wid);
        let mut center = increment_center(&snap);
        let deadline = Instant::now() + Duration::from_secs(8);
        while center.is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(400));
            snap = ax_snapshot(&mut driver, &window_session, pid, wid);
            center = increment_center(&snap);
        }
        let Some((cx, cy)) = center else {
            panic!("increment button frame not found in required AppKit AX tree");
        };
        let pre = counter(&snap).unwrap_or(0);
        println!("[desktop-mac] increment button screen-center=({cx},{cy}) pre-counter={pre}");

        // Desktop scope clicks the frontmost window at the point — put the harness there.
        activate_pid(pid);
        driver.start_behavior_recording();

        // Window-less screen-absolute click — no pid, no window_id; scope per-call.
        let clicked = driver.call(
            "click",
            serde_json::json!({ "session": desktop_session, "x": cx, "y": cy, "scope": "desktop" }),
        );
        assert!(
            !clicked.is_error(),
            "desktop-scope click errored: {}",
            clicked.text()
        );
        assert!(
            clicked.text().to_lowercase().contains("desktop scope"),
            "click not reported as desktop-scope: {}",
            clicked.text()
        );
        println!("[desktop-mac] {}", clicked.text());

        std::thread::sleep(Duration::from_millis(600));
        let post = counter(&ax_snapshot(&mut driver, &window_session, pid, wid)).unwrap_or(pre);
        assert!(
            post > pre,
            "desktop click did not advance AppKit counter at ({cx},{cy}): {pre} -> {post}"
        );
        Observation::delivered_with_fixture_state(Vec::new())
    });
}

/// A desktop wheel must move the AppKit scroll view, not merely report that a
/// CGEvent was posted.
#[test]
#[ignore]
fn desktop_scope_windowless_scroll_lands_on_control() {
    let cell_id = "macos-appkit-desktop-scroll-px-foreground";
    let case = CaseSpec::delivered(
        cell_id,
        "appkit",
        "appkit",
        "scroll",
        Targeting::Px,
        Delivery::Foreground,
        Scope::Desktop,
        DriverRoute::MacosCgEventHid,
        vec![OracleKind::FixtureState],
    );
    execute_case(case, |evidence| {
        let mut driver = McpDriver::spawn_macos_daemon_proxy_named(cell_id)
            .expect("start installed macOS daemon proxy");
        *evidence = recording_evidence(driver.recording_dir());
        let window_session = format!("{cell_id}-window");
        let desktop_session = format!("{cell_id}-desktop");
        start_scope(&mut driver, &window_session, "window");
        start_scope(&mut driver, &desktop_session, "desktop");
        let (pid, wid) = launch(&mut driver).expect("required AppKit harness did not launch");

        let mut snap = ax_snapshot(&mut driver, &window_session, pid, wid);
        let deadline = Instant::now() + Duration::from_secs(8);
        let (x, y) = loop {
            if let Some(center) = element_center_containing(&snap, "SCROLL_TOP_MARKER_v1") {
                break center;
            }
            assert!(
                Instant::now() < deadline,
                "visible AppKit scroll target was not exposed in the AX tree"
            );
            std::thread::sleep(Duration::from_millis(300));
            snap = ax_snapshot(&mut driver, &window_session, pid, wid);
        };
        let before = marker_value(&snap, "scroll_offset=").unwrap_or(0);

        activate_pid(pid);
        driver.start_behavior_recording();
        let response = driver.call(
            "scroll",
            serde_json::json!({
                "session": desktop_session, "scope": "desktop", "x": x, "y": y,
                "direction": "down", "by": "line", "amount": 5
            }),
        );
        assert!(
            !response.is_error(),
            "desktop scroll failed: {}; raw={}",
            response.text(),
            response.raw
        );

        let deadline = Instant::now() + Duration::from_secs(3);
        let after = loop {
            let state = ax_snapshot(&mut driver, &window_session, pid, wid);
            let offset = marker_value(&state, "scroll_offset=").unwrap_or(before);
            if offset > before || Instant::now() >= deadline {
                break offset;
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        assert!(
            after > before,
            "desktop scroll reported success but AppKit scroll_offset did not change: \
             {before} -> {after}; response={}; raw={}",
            response.text(),
            response.raw
        );
        Observation::delivered_with_fixture_state(Vec::new())
    });
}

/// A desktop chord must reach the foreground app and release every modifier.
/// The immediate ordinary click catches macOS control-click leakage; the exact
/// unmodified F5 binding catches any remaining Ctrl/Shift state.
#[test]
#[ignore]
fn desktop_scope_hotkey_releases_modifiers() {
    let cell_id = "macos-appkit-desktop-hotkey-px-foreground";
    let case = CaseSpec::delivered(
        cell_id,
        "appkit",
        "appkit",
        "hotkey",
        Targeting::Px,
        Delivery::Foreground,
        Scope::Desktop,
        DriverRoute::MacosCgEventHid,
        vec![OracleKind::FixtureState],
    );
    execute_case(case, |evidence| {
        let mut driver = McpDriver::spawn_macos_daemon_proxy_named(cell_id)
            .expect("start installed macOS daemon proxy");
        *evidence = recording_evidence(driver.recording_dir());
        let window_session = format!("{cell_id}-window");
        let desktop_session = format!("{cell_id}-desktop");
        start_scope(&mut driver, &window_session, "window");
        start_scope(&mut driver, &desktop_session, "desktop");
        let (pid, wid) = launch(&mut driver).expect("required AppKit harness did not launch");

        let mut snap = ax_snapshot(&mut driver, &window_session, pid, wid);
        let deadline = Instant::now() + Duration::from_secs(8);
        let (click_x, click_y) = loop {
            if let Some(center) = element_center_containing(&snap, "btn-clicktarget") {
                break center;
            }
            assert!(
                Instant::now() < deadline,
                "AppKit click target was not exposed in the AX tree"
            );
            std::thread::sleep(Duration::from_millis(300));
            snap = ax_snapshot(&mut driver, &window_session, pid, wid);
        };

        activate_pid(pid);
        driver.start_behavior_recording();
        let hotkey = driver.call(
            "hotkey",
            serde_json::json!({
                "session": desktop_session, "scope": "desktop",
                "keys": ["ctrl", "shift", "k"]
            }),
        );
        assert!(
            !hotkey.is_error(),
            "desktop hotkey failed: {}",
            hotkey.text()
        );

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            snap = ax_snapshot(&mut driver, &window_session, pid, wid);
            if marker_value(&snap, "accel_fired=").unwrap_or(0) >= 1 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "AppKit did not observe ctrl+shift+k: {}",
                hotkey.text()
            );
            std::thread::sleep(Duration::from_millis(100));
        }

        let click = driver.call(
            "click",
            serde_json::json!({
                "session": desktop_session, "scope": "desktop",
                "x": click_x, "y": click_y
            }),
        );
        assert!(
            !click.is_error(),
            "post-hotkey click failed: {}",
            click.text()
        );
        std::thread::sleep(Duration::from_millis(300));
        snap = ax_snapshot(&mut driver, &window_session, pid, wid);
        let tree = snap["tree_markdown"].as_str().unwrap_or_default();
        assert!(
            tree.contains("last_action=click") && !tree.contains("last_action=right_click"),
            "desktop hotkey leaked modifiers into the next ordinary click: {tree}"
        );

        let plain_key = driver.call(
            "press_key",
            serde_json::json!({
                "session": desktop_session, "scope": "desktop", "key": "f5"
            }),
        );
        assert!(
            !plain_key.is_error(),
            "post-hotkey plain F5 failed: {}",
            plain_key.text()
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            snap = ax_snapshot(&mut driver, &window_session, pid, wid);
            if marker_value(&snap, "accel_fired=").unwrap_or(0) >= 2 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "plain F5 did not match after hotkey; modifier state may still be latched"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
        Observation::delivered_with_fixture_state(Vec::new())
    });
}

/// Negative gate: a window-less screen-absolute click while `capture_scope=window`
/// must be rejected (`desktop_scope_disabled`), not silently treated as
/// window-local pixels. Mirrors the Windows contract.
#[test]
#[ignore]
fn window_scope_rejects_windowless_click() {
    let cell_id = "macos-window-scope-gate-px-not-applicable";
    let case = CaseSpec::delivered(
        cell_id,
        "desktop",
        "quartz",
        "window_scope_gate",
        Targeting::Px,
        Delivery::NotApplicable,
        Scope::Window,
        DriverRoute::CaptureScopeGate,
        vec![OracleKind::Protocol],
    );
    execute_case(case, |evidence| {
        let mut driver = McpDriver::spawn_macos_daemon_proxy_named(cell_id)
            .expect("start installed macOS daemon proxy");
        *evidence = recording_evidence(driver.recording_dir());
        let started = driver.call(
            "start_session",
            serde_json::json!({"session": cell_id, "capture_scope": "window"}),
        );
        assert!(
            !started.is_error(),
            "start_session failed: {}",
            started.text()
        );
        driver.start_behavior_recording();
        let r = driver.call(
            "click",
            serde_json::json!({ "session": cell_id, "x": 100, "y": 100, "scope": "desktop" }),
        );
        assert!(
            r.is_error() && r.structured()["code"].as_str() == Some("desktop_scope_disabled"),
            "window-scope window-less click was NOT rejected: {}",
            r.text()
        );
        Observation::delivered(vec![OracleKind::Protocol], Evidence::default())
    });
}
