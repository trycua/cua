//! Cross-platform behavioral scenarios derived from the external CUA matrix.
//!
//! These are source-owned Rust tests, not a copy of a partner test runner. The
//! shared web fixture is loaded by Electron and Tauri on every supported OS;
//! Windows also has WebView2 coverage in `harness_web_test.rs`. Assertions read
//! the fixture's mutated application state from a fresh accessibility snapshot.
//! A successful driver response alone is never sufficient.
//!
//! Run after building the shared fixtures:
//!
//! ```text
//! cargo test -p cua-driver --test cross_platform_behavior_test -- --ignored \
//!   --nocapture --test-threads=1
//! ```

#![cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]

use std::any::Any;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use cua_driver_testkit::ax::{element_index_by_id, element_index_containing};
use cua_driver_testkit::e2e::{
    recording_evidence, shared_web_route, write_declaration_from_env, write_result_from_env,
    CaseResult, CaseSpec, Delivery, Evidence, Observation, OracleKind, RefusalCode, Scope,
    Targeting,
};
use cua_driver_testkit::observer::TargetWindow;
use cua_driver_testkit::sentinel::ForegroundSentinel;
use cua_driver_testkit::{harness_app, spawn_in_job, Driver, McpDriver, ToolResponse};

struct HostSpec {
    name: &'static str,
    path: PathBuf,
    args: Vec<&'static str>,
    title: &'static str,
}

fn host_specs() -> Vec<HostSpec> {
    let mut hosts = Vec::new();

    #[cfg(target_os = "windows")]
    {
        hosts.push(HostSpec {
            name: "electron",
            path: harness_app("harness-electron", "CuaTestHarness.Electron.exe"),
            args: vec![
                "--no-sandbox",
                "--disable-gpu",
                "--force-renderer-accessibility",
            ],
            title: "CuaTestHarness Electron",
        });
        hosts.push(HostSpec {
            name: "tauri",
            path: harness_app("harness-tauri", "CuaTestHarness.Tauri.exe"),
            args: Vec::new(),
            title: "CuaTestHarness Tauri",
        });
    }

    #[cfg(target_os = "macos")]
    {
        hosts.push(HostSpec {
            name: "electron",
            path: harness_app(
                "harness-electron",
                "CuaTestHarness.Electron.app/Contents/MacOS/Electron",
            ),
            args: vec!["--force-renderer-accessibility"],
            title: "CuaTestHarness Electron",
        });
        hosts.push(HostSpec {
            name: "tauri",
            path: harness_app(
                "harness-tauri",
                "CuaTestHarness.Tauri.app/Contents/MacOS/CuaTestHarness.Tauri",
            ),
            args: Vec::new(),
            title: "CuaTestHarness Tauri",
        });
    }

    #[cfg(target_os = "linux")]
    {
        hosts.push(HostSpec {
            name: "electron",
            path: harness_app("harness-electron", "CuaTestHarness.Electron"),
            args: vec![
                "--no-sandbox",
                "--disable-gpu",
                "--force-renderer-accessibility",
            ],
            title: "CuaTestHarness Electron",
        });
        // WebKitGTK's AT-SPI tree is exposed through a separate WebProcess;
        // the Rust walker handles that reference shape, but headless Xvfb
        // still does not provide a reliable input-delivery contract for the
        // Tauri renderer. Keep this strict lane opt-in until that renderer
        // path is fixed, while the Electron matrix remains deterministic.
        if std::env::var_os("CUA_INCLUDE_TAURI_LINUX").is_some() {
            hosts.push(HostSpec {
                name: "tauri",
                path: harness_app("harness-tauri", "CuaTestHarness.Tauri"),
                args: Vec::new(),
                title: "CuaTestHarness Tauri",
            });
        }
    }

    hosts
}

fn panic_message(payload: &Box<dyn Any + Send>) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| {
            payload
                .downcast_ref::<&str>()
                .map(|message| (*message).to_owned())
        })
        .unwrap_or_else(|| "test panicked without a string payload".to_owned())
}

fn run_host_case_with_outcome<F>(
    case: CaseSpec,
    spec: &HostSpec,
    test: F,
) -> Option<Box<dyn Any + Send>>
where
    F: FnOnce(&mut Fixture) -> Observation,
{
    write_declaration_from_env(&case).expect("write E2E case declaration");
    let started = Instant::now();
    let mut evidence = Evidence::default();
    let delivery = case.delivery;
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
        let mut fixture = launch_host_with_evidence(spec, &case.cell_id, &mut evidence)?;
        let sentinel = if delivery == Delivery::Background {
            Some(ForegroundSentinel::launch(&mut fixture.driver))
        } else {
            None
        };
        let mut observation = if delivery == Delivery::Background {
            let sentinel = sentinel.as_ref().expect("background sentinel");
            let (mut observation, passed) = sentinel
                .observe_background(
                    TargetWindow {
                        pid: fixture.pid,
                        native_id: fixture.wid,
                    },
                    || test(&mut fixture),
                )
                .expect("observe background desktop side effects");
            observation.passed_oracles.extend(passed);
            observation
        } else {
            test(&mut fixture)
        };
        drop(sentinel);
        observation.evidence = evidence.clone();
        Some(observation)
    }));
    match outcome {
        Ok(Some(observation)) => {
            let result = CaseResult::evaluate(case, observation, started.elapsed());
            write_result_from_env(&result).expect("write E2E case result");
            if result.test_status == cua_driver_testkit::e2e::TestStatus::Fail {
                return Some(Box::new(result.message));
            }
            None
        }
        Ok(None) => None,
        Err(payload) => {
            let message = panic_message(&payload);
            let result = CaseResult::evaluate(
                case,
                Observation::error(&message, evidence),
                started.elapsed(),
            );
            write_result_from_env(&result).expect("write failed E2E case result");
            Some(payload)
        }
    }
}

fn resume_first_failure(failure: Option<Box<dyn Any + Send>>) {
    if let Some(payload) = failure {
        panic::resume_unwind(payload);
    }
}

fn spawn_driver(recording_label: &str) -> Option<McpDriver> {
    #[cfg(target_os = "macos")]
    {
        McpDriver::spawn_macos_daemon_proxy_named(recording_label)
    }
    #[cfg(not(target_os = "macos"))]
    {
        McpDriver::spawn_named(recording_label)
    }
}

struct Fixture {
    driver: McpDriver,
    pid: u32,
    wid: u64,
    window_x: f64,
    window_y: f64,
    name: &'static str,
}

fn evidence_for_driver(driver: &McpDriver) -> Evidence {
    recording_evidence(driver.recording_dir())
}

fn launch_host_with_evidence(
    spec: &HostSpec,
    scenario: &str,
    evidence: &mut Evidence,
) -> Option<Fixture> {
    if !spec.path.exists() {
        if std::env::var_os("CUA_TEST_REQUIRE_FIXTURES").is_some() {
            panic!(
                "{} fixture is required but was not staged at {:?}",
                spec.name, spec.path
            );
        }
        eprintln!(
            "[{}] fixture not staged at {:?}; skipping",
            spec.name, spec.path
        );
        return None;
    }

    let recording_label = scenario.to_owned();
    let Some(mut driver) = spawn_driver(&recording_label) else {
        if std::env::var_os("CUA_TEST_REQUIRE_FIXTURES").is_some() {
            panic!(
                "cua-driver could not be started for the required {} fixture",
                spec.name
            );
        }
        return None;
    };
    *evidence = evidence_for_driver(&driver);
    let mut command = Command::new(&spec.path);
    command
        .args(&spec.args)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let before_windows = driver
        .call("list_windows", serde_json::json!({}))
        .structured()["windows"]
        .as_array()
        .map(|windows| {
            windows
                .iter()
                .filter_map(|window| window["window_id"].as_u64())
                .collect::<std::collections::HashSet<_>>()
        })
        .unwrap_or_default();
    let child = spawn_in_job(&mut command).ok()?;
    let pid = child.id();
    driver.reaper().push(child);

    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        let windows = driver.call("list_windows", serde_json::json!({}));
        if let Some(items) = windows.structured()["windows"].as_array() {
            if let Some(window) = items.iter().find(|window| {
                window["window_id"]
                    .as_u64()
                    .map(|id| !before_windows.contains(&id))
                    .unwrap_or(false)
                    && window["title"].as_str().unwrap_or("").contains(spec.title)
            }) {
                if let Some(wid) = window["window_id"].as_u64() {
                    let actual_pid = window["pid"].as_u64().unwrap_or(pid as u64) as u32;
                    let bounds = &window["bounds"];
                    driver.reaper().track_pid(actual_pid);
                    let mut fixture = Fixture {
                        driver,
                        pid: actual_pid,
                        wid,
                        window_x: bounds["x"].as_f64().unwrap_or(0.0),
                        window_y: bounds["y"].as_f64().unwrap_or(0.0),
                        name: spec.name,
                    };
                    let ax_deadline = Instant::now() + Duration::from_secs(10);
                    while Instant::now() < ax_deadline {
                        if snapshot(&mut fixture)
                            .tree_text()
                            .contains("WEB_HARNESS_MARKER_v1")
                        {
                            return Some(fixture);
                        }
                        thread::sleep(Duration::from_millis(250));
                    }
                    panic!(
                        "{} fixture accessibility tree did not become ready",
                        spec.name
                    );
                }
            }
        }
        thread::sleep(Duration::from_millis(250));
    }
    panic!("{} fixture window did not appear", spec.name);
}

fn snapshot(fixture: &mut Fixture) -> ToolResponse {
    fixture.driver.call(
        "get_window_state",
        serde_json::json!({
            "pid": fixture.pid as i64,
            "window_id": fixture.wid,
            "capture_mode": "ax"
        }),
    )
}

fn window_origin(fixture: &Fixture, _state: &ToolResponse) -> (f64, f64) {
    #[cfg(target_os = "macos")]
    {
        // macOS list_windows can report zero bounds for a daemon-proxied app;
        // the AX window frame remains the authoritative screen origin.
        if let Some(frame) = _state.structured()["elements"]
            .as_array()
            .and_then(|elements| {
                elements
                    .iter()
                    .find(|element| element["role"].as_str() == Some("AXWindow"))
            })
            .and_then(|window| window["frame"].as_object())
        {
            return (
                frame["x"].as_f64().unwrap_or(0.0),
                frame["y"].as_f64().unwrap_or(0.0),
            );
        }
    }
    (fixture.window_x, fixture.window_y)
}

fn screenshot_scale(state: &ToolResponse) -> f64 {
    let Some(width) = state.structured()["screenshot_width"].as_f64() else {
        return 1.0;
    };
    let window_width = state.structured()["elements"]
        .as_array()
        .and_then(|elements| {
            elements
                .iter()
                .find(|element| element["role"].as_str() == Some("AXWindow"))
        })
        .and_then(|window| window["frame"]["w"].as_f64())
        .unwrap_or(0.0);
    if window_width > 0.0 && width > window_width {
        width / window_width
    } else {
        1.0
    }
}

fn require_element(snapshot: &ToolResponse, id: &str) -> u64 {
    // Some WebKit/Chromium AX adapters preserve the DOM id as an annotation
    // instead of emitting the common `id=...` form. Searching the visible
    // label fallback handles adapters that omit both forms.
    let visible_label = match id {
        "editor-document" | "editor-save" | "scroll-tall" => id,
        "border-click-target" => "Click target (left / right / double)",
        "txt-input" => "type here",
        "keyboard-input" => "keyboard-input",
        "drag-source" => "Drag source",
        "drop-target" => "Drop target",
        "btn-open-child-window" => "Open child window",
        _ => id,
    };
    let platform_id = match id {
        // Chromium exposes the DOM id through AX, while data-cua-id is the
        // canonical identifier shared with the native harnesses.
        "border-click-target" => "click-target",
        _ => id,
    };
    element_index_by_id(snapshot.tree_text(), id)
        .or_else(|| element_index_by_id(snapshot.tree_text(), platform_id))
        .or_else(|| element_index_containing(snapshot.tree_text(), visible_label))
        .unwrap_or_else(|| {
            panic!(
                "{}: missing AX element {id:?}: {}",
                "shared-web",
                snapshot.tree_text()
            )
        })
}

fn element_center(snapshot: &ToolResponse, element_index: u64) -> (f64, f64) {
    let frame = snapshot.structured()["elements"]
        .as_array()
        .and_then(|elements| {
            elements
                .iter()
                .find(|element| element["element_index"].as_u64() == Some(element_index))
        })
        .and_then(|element| element["frame"].as_object())
        .unwrap_or_else(|| panic!("element [{element_index}] has no frame"));
    (
        frame["x"].as_f64().unwrap_or(0.0) + frame["w"].as_f64().unwrap_or(0.0) / 2.0,
        frame["y"].as_f64().unwrap_or(0.0) + frame["h"].as_f64().unwrap_or(0.0) / 2.0,
    )
}

fn assert_tree_contains(fixture: &mut Fixture, marker: &str) {
    let post = snapshot(fixture);
    assert!(
        post.tree_text().contains(marker),
        "{}: application state did not reach {marker:?}: {}",
        fixture.name,
        post.tree_text()
    );
}

fn action_target_args(
    fixture: &Fixture,
    state: &ToolResponse,
    id: &str,
    addressing: &str,
    delivery: &str,
) -> serde_json::Value {
    let index = require_element(state, id);
    let mut args = serde_json::json!({
        "pid": fixture.pid as i64,
        "window_id": fixture.wid,
        "delivery_mode": delivery,
    });
    let object = args.as_object_mut().expect("action arguments object");
    if addressing == "ax" {
        object.insert("element_index".to_owned(), serde_json::json!(index));
    } else {
        let origin = window_origin(fixture, state);
        let scale = screenshot_scale(state);
        let (x, y) = element_center(state, index);
        let local_x = (x - origin.0) * scale;
        let local_y = (y - origin.1) * scale;
        let width = state.structured()["screenshot_width"]
            .as_f64()
            .expect("PX action requires screenshot_width");
        let height = state.structured()["screenshot_height"]
            .as_f64()
            .expect("PX action requires screenshot_height");
        assert!(
            local_x >= 0.0 && local_x < width && local_y >= 0.0 && local_y < height,
            "{}: PX target {id:?} center ({local_x:.1}, {local_y:.1}) is outside the captured window ({width:.1}x{height:.1}); fix the harness layout",
            fixture.name
        );
        object.insert("x".to_owned(), serde_json::json!(local_x));
        object.insert("y".to_owned(), serde_json::json!(local_y));
    }
    args
}

fn run_pointer_action(
    fixture: &mut Fixture,
    tool: &str,
    addressing: &str,
    delivery: &str,
    expected_marker: &str,
) -> Observation {
    let pre = snapshot(fixture);
    let args = action_target_args(fixture, &pre, "border-click-target", addressing, delivery);
    let response = fixture.driver.call(tool, args);
    if let Some(code) = background_refusal_code(&response, delivery) {
        return Observation::refused(code, Vec::new(), response.text(), Evidence::default());
    }
    assert!(
        !response.is_error(),
        "{}: {tool} {addressing}/{delivery} failed: {}",
        fixture.name,
        response.text()
    );
    assert_tree_contains(fixture, expected_marker);
    delivered_observation()
}

fn delivered_observation() -> Observation {
    Observation::delivered(vec![OracleKind::FixtureState], Evidence::default())
}

fn unverified_background_protocol_oracle(
    response: &ToolResponse,
    delivery: &str,
) -> Vec<OracleKind> {
    if !cfg!(target_os = "windows") || delivery != "background" {
        return Vec::new();
    }
    assert_eq!(
        response.verified(),
        Some(false),
        "background dispatch without independent read-back must remain unverified: {}",
        response.text()
    );
    assert_ne!(
        response.structured()["verify"].as_str(),
        Some("confirmed"),
        "background dispatch overclaimed a confirmed read-back: {}",
        response.text()
    );
    vec![OracleKind::Protocol]
}

fn background_refusal_code(response: &ToolResponse, delivery: &str) -> Option<RefusalCode> {
    if delivery != "background" || !response.is_error() {
        return None;
    }
    response.structured()["code"]
        .as_str()
        .and_then(RefusalCode::from_driver_code)
}

fn run_text_action(fixture: &mut Fixture, addressing: &str, delivery: &str) -> Observation {
    let pre = snapshot(fixture);
    let text = format!("cua-{addressing}-{delivery}");
    let mut args = action_target_args(fixture, &pre, "txt-input", addressing, delivery);
    args.as_object_mut()
        .expect("type_text arguments object")
        .insert("text".to_owned(), serde_json::json!(text));
    let response = fixture.driver.call("type_text", args);
    if let Some(code) = background_refusal_code(&response, delivery) {
        return Observation::refused(code, Vec::new(), response.text(), Evidence::default());
    }
    assert!(
        !response.is_error(),
        "{}: type_text {addressing}/{delivery} failed: {}",
        fixture.name,
        response.text()
    );
    let mut passed = vec![OracleKind::FixtureState];
    if addressing == "px" {
        passed.extend(unverified_background_protocol_oracle(&response, delivery));
    }
    thread::sleep(Duration::from_millis(250));
    assert_tree_contains(fixture, &format!("mirror={text}"));
    Observation::delivered(passed, Evidence::default())
}

fn run_press_key_action(fixture: &mut Fixture, addressing: &str, delivery: &str) -> Observation {
    let pre = snapshot(fixture);
    let mut press_args = action_target_args(fixture, &pre, "keyboard-input", addressing, delivery);
    press_args
        .as_object_mut()
        .expect("press_key arguments object")
        .insert("key".to_owned(), serde_json::json!("return"));
    let response = fixture.driver.call("press_key", press_args);
    if let Some(code) = background_refusal_code(&response, delivery) {
        return Observation::refused(code, Vec::new(), response.text(), Evidence::default());
    }
    assert!(
        !response.is_error(),
        "{}: press_key {addressing}/{delivery} failed: {}",
        fixture.name,
        response.text()
    );
    let mut passed = vec![OracleKind::FixtureState];
    passed.extend(unverified_background_protocol_oracle(&response, delivery));
    assert_tree_contains(fixture, "key_state=enter");

    Observation::delivered(passed, Evidence::default())
}

fn run_hotkey_action(fixture: &mut Fixture, addressing: &str, delivery: &str) -> Observation {
    let pre = snapshot(fixture);
    let mut hotkey_args = action_target_args(fixture, &pre, "keyboard-input", addressing, delivery);
    hotkey_args
        .as_object_mut()
        .expect("hotkey arguments object")
        .insert("keys".to_owned(), serde_json::json!(["ctrl", "shift", "7"]));
    let response = fixture.driver.call("hotkey", hotkey_args);
    if let Some(code) = background_refusal_code(&response, delivery) {
        return Observation::refused(code, Vec::new(), response.text(), Evidence::default());
    }
    assert!(
        !response.is_error(),
        "{}: hotkey {addressing}/{delivery} failed: {}",
        fixture.name,
        response.text()
    );
    assert_tree_contains(fixture, "key_state=hotkey");
    let passed = unverified_background_protocol_oracle(&response, delivery);
    Observation::delivered_with_fixture_state(passed)
}

fn run_scroll_action(fixture: &mut Fixture, addressing: &str, delivery: &str) -> Observation {
    let pre = snapshot(fixture);
    let mut args = action_target_args(fixture, &pre, "scroll-tall", addressing, delivery);
    let object = args.as_object_mut().expect("scroll arguments object");
    object.insert("direction".to_owned(), serde_json::json!("down"));
    object.insert("by".to_owned(), serde_json::json!("page"));
    object.insert("amount".to_owned(), serde_json::json!(2));
    let response = fixture.driver.call("scroll", args);
    if let Some(code) = background_refusal_code(&response, delivery) {
        return Observation::refused(code, Vec::new(), response.text(), Evidence::default());
    }
    assert!(
        !response.is_error(),
        "{}: scroll {addressing}/{delivery} failed: {}",
        fixture.name,
        response.text()
    );
    thread::sleep(Duration::from_millis(250));
    let post = snapshot(fixture);
    let offset = post
        .tree_text()
        .lines()
        .find_map(|line| line.split("scroll_offset=").nth(1))
        .and_then(|tail| tail.split(|ch: char| !ch.is_ascii_digit()).next())
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    assert!(
        offset > 0,
        "{}: scroll did not advance the external oracle: {}",
        fixture.name,
        post.tree_text()
    );
    delivered_observation()
}

fn run_drag_action(fixture: &mut Fixture, delivery: &str) -> Observation {
    let pre = snapshot(fixture);
    let source = require_element(&pre, "drag-source");
    let target = require_element(&pre, "drop-target");
    let origin = window_origin(fixture, &pre);
    let scale = screenshot_scale(&pre);
    let point = |index: u64| {
        let (x, y) = element_center(&pre, index);
        ((x - origin.0) * scale, (y - origin.1) * scale)
    };
    let (from_x, from_y) = point(source);
    let (to_x, to_y) = point(target);
    let response = fixture.driver.call(
        "drag",
        serde_json::json!({
            "pid": fixture.pid as i64,
            "window_id": fixture.wid,
            "from_x": from_x,
            "from_y": from_y,
            "to_x": to_x,
            "to_y": to_y,
            "duration_ms": 400,
            "steps": 20,
            "delivery_mode": delivery,
        }),
    );
    if let Some(code) = background_refusal_code(&response, delivery) {
        return Observation::refused(code, Vec::new(), response.text(), Evidence::default());
    }
    assert!(
        !response.is_error(),
        "{}: drag PX/{delivery} failed: {}",
        fixture.name,
        response.text()
    );
    assert_tree_contains(fixture, "drag_status=dropped");
    delivered_observation()
}

fn run_child_window_action(fixture: &mut Fixture, addressing: &str, delivery: &str) -> Observation {
    let pre = snapshot(fixture);
    let args = action_target_args(fixture, &pre, "btn-open-child-window", addressing, delivery);
    let response = fixture.driver.call("click", args);
    if let Some(code) = background_refusal_code(&response, delivery) {
        return Observation::refused(code, Vec::new(), response.text(), Evidence::default());
    }
    assert!(
        !response.is_error(),
        "{}: child-window click {addressing}/{delivery} failed: {}",
        fixture.name,
        response.text()
    );
    assert_tree_contains(fixture, "child_windows=1");
    delivered_observation()
}

fn run_editor_save_action(fixture: &mut Fixture, delivery: &str) -> Observation {
    let pre = snapshot(fixture);
    let text = format!("cua-editor-{delivery}");
    let mut text_args = action_target_args(fixture, &pre, "editor-document", "ax", delivery);
    text_args
        .as_object_mut()
        .expect("editor type_text arguments object")
        .insert("text".to_owned(), serde_json::json!(text));
    let response = fixture.driver.call("type_text", text_args);
    if let Some(code) = background_refusal_code(&response, delivery) {
        return Observation::refused(code, Vec::new(), response.text(), Evidence::default());
    }
    assert!(
        !response.is_error(),
        "{}: editor type_text failed: {}",
        fixture.name,
        response.text()
    );

    let post_text = snapshot(fixture);
    let save_args = action_target_args(fixture, &post_text, "editor-save", "ax", delivery);
    let response = fixture.driver.call("click", save_args);
    if let Some(code) = background_refusal_code(&response, delivery) {
        return Observation::refused(code, Vec::new(), response.text(), Evidence::default());
    }
    assert!(
        !response.is_error(),
        "{}: editor save failed: {}",
        fixture.name,
        response.text()
    );
    assert_tree_contains(fixture, "editor_status=saved:");
    delivered_observation()
}

fn shared_case(spec: &HostSpec, action: &str, addressing: &str, delivery: &str) -> CaseSpec {
    let targeting = match addressing {
        "ax" => Targeting::Ax,
        "px" => Targeting::Px,
        _ => Targeting::NotApplicable,
    };
    let delivery_kind = match delivery {
        "background" => Delivery::Background,
        "foreground" => Delivery::Foreground,
        _ => Delivery::NotApplicable,
    };
    let scenario = format!("{action}_{addressing}_{delivery}");
    let cell_id = format!("{}-{}-{scenario}", std::env::consts::OS, spec.name).replace('_', "-");
    let expected_background_refusal = cfg!(target_os = "windows")
        && spec.name == "electron"
        && action == "left_click"
        && targeting == Targeting::Ax
        && delivery_kind == Delivery::Background;
    let mut oracles = if expected_background_refusal {
        Vec::new()
    } else {
        vec![OracleKind::FixtureState]
    };
    if delivery_kind == Delivery::Background {
        oracles.extend([
            OracleKind::Focus,
            OracleKind::ZOrder,
            OracleKind::Cursor,
            OracleKind::NoLeakedInput,
        ]);
        if cfg!(target_os = "windows")
            && (matches!(action, "press_key" | "hotkey")
                || (action == "type_text" && targeting == Targeting::Px))
        {
            oracles.push(OracleKind::Protocol);
        }
    }
    let route = shared_web_route(
        cua_driver_testkit::e2e::Platform::current(),
        cua_driver_testkit::e2e::DisplayServer::current(),
        action,
        targeting,
        delivery_kind,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let case = CaseSpec::delivered(
        cell_id,
        spec.name,
        if spec.name == "electron" {
            "chromium"
        } else {
            "platform-webview"
        },
        action,
        targeting,
        delivery_kind,
        Scope::Window,
        route,
        oracles,
    );
    if expected_background_refusal {
        case.expecting_refusal(vec![RefusalCode::BackgroundOccluded])
    } else {
        case
    }
}

fn cell_selected(case: &CaseSpec) -> bool {
    let Ok(filter) = std::env::var("CUA_E2E_CELL_FILTER") else {
        return true;
    };
    filter
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .any(|part| case.cell_id == part || case.cell_id.contains(part))
}

#[test]
#[ignore]
fn shared_web_action_matrix_is_state_verified() {
    let mut failure = None;
    let mut selected = 0usize;
    for spec in host_specs() {
        for (action, tool, marker) in [
            ("left_click", "click", "last_action=left_click"),
            ("right_click", "right_click", "last_action=right_click"),
            ("double_click", "double_click", "last_action=double_click"),
        ] {
            for addressing in ["ax", "px"] {
                for delivery in ["background", "foreground"] {
                    let case = shared_case(&spec, action, addressing, delivery);
                    if !cell_selected(&case) {
                        continue;
                    }
                    selected += 1;
                    let result = run_host_case_with_outcome(case, &spec, |fixture| {
                        run_pointer_action(fixture, tool, addressing, delivery, marker)
                    });
                    if failure.is_none() {
                        failure = result;
                    }
                }
            }
        }
        for (action, run) in [
            (
                "type_text",
                run_text_action as fn(&mut Fixture, &str, &str) -> Observation,
            ),
            (
                "press_key",
                run_press_key_action as fn(&mut Fixture, &str, &str) -> Observation,
            ),
            (
                "hotkey",
                run_hotkey_action as fn(&mut Fixture, &str, &str) -> Observation,
            ),
            (
                "scroll",
                run_scroll_action as fn(&mut Fixture, &str, &str) -> Observation,
            ),
            (
                "child_window",
                run_child_window_action as fn(&mut Fixture, &str, &str) -> Observation,
            ),
        ] {
            for addressing in ["ax", "px"] {
                for delivery in ["background", "foreground"] {
                    let case = shared_case(&spec, action, addressing, delivery);
                    if !cell_selected(&case) {
                        continue;
                    }
                    selected += 1;
                    let result = run_host_case_with_outcome(case, &spec, |fixture| {
                        run(fixture, addressing, delivery)
                    });
                    if failure.is_none() {
                        failure = result;
                    }
                }
            }
        }
        for delivery in ["background", "foreground"] {
            let case = shared_case(&spec, "drag", "px", delivery);
            if !cell_selected(&case) {
                continue;
            }
            selected += 1;
            let result = run_host_case_with_outcome(case, &spec, |fixture| {
                run_drag_action(fixture, delivery)
            });
            if failure.is_none() {
                failure = result;
            }
        }
        for delivery in ["background", "foreground"] {
            let case = shared_case(&spec, "editor_save", "ax", delivery);
            if cell_selected(&case) {
                selected += 1;
                let result = run_host_case_with_outcome(case, &spec, |fixture| {
                    run_editor_save_action(fixture, delivery)
                });
                if failure.is_none() {
                    failure = result;
                }
            }
        }
    }
    assert!(
        std::env::var_os("CUA_E2E_CELL_FILTER").is_none() || selected > 0,
        "CUA_E2E_CELL_FILTER matched no shared E2E cells"
    );
    resume_first_failure(failure);
}
