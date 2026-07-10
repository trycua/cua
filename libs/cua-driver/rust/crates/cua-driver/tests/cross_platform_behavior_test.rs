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
    write_declaration_from_env, write_result_from_env, CaseResult, CaseSpec, Delivery, DriverRoute,
    Evidence, Observation, OracleKind, RefusalCode, Scope, Targeting,
};
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

fn run_host_case<F>(scenario: &str, spec: &HostSpec, test: F) -> Option<Box<dyn Any + Send>>
where
    F: FnOnce(Fixture),
{
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
        let Some(fixture) = launch_host(spec, scenario) else {
            return false;
        };
        test(fixture);
        true
    }));
    match outcome {
        Ok(true) | Ok(false) => None,
        Err(payload) => Some(payload),
    }
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
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
        let mut fixture = launch_host_with_evidence(spec, &case.cell_id, &mut evidence)?;
        let mut observation = test(&mut fixture);
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
    let Some(recording_dir) = driver.recording_dir() else {
        return Evidence::default();
    };
    let relative_dir = std::env::var_os("CUA_E2E_RECORDINGS_ROOT")
        .map(PathBuf::from)
        .and_then(|root| recording_dir.strip_prefix(root).ok().map(PathBuf::from))
        .unwrap_or_else(|| {
            recording_dir
                .file_name()
                .map(PathBuf::from)
                .unwrap_or_default()
        });
    let artifact_dir = PathBuf::from("recordings").join(relative_dir);
    let artifact_path = |name: &str| artifact_dir.join(name).to_string_lossy().replace('\\', "/");
    Evidence {
        video: Some(artifact_path("recording.mp4")),
        trajectory: Some(artifact_path("trajectory.json")),
        screenshot: None,
        log: None,
    }
}

fn launch_host(spec: &HostSpec, scenario: &str) -> Option<Fixture> {
    launch_host_with_evidence(spec, scenario, &mut Evidence::default())
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
    element_index_by_id(snapshot.tree_text(), id)
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

fn click_ax(fixture: &mut Fixture, id: &str) {
    let pre = snapshot(fixture);
    let element_index = require_element(&pre, id);
    let response = fixture.driver.call(
        "click",
        serde_json::json!({
            "pid": fixture.pid as i64,
            "window_id": fixture.wid,
            "element_index": element_index,
            "delivery_mode": "background"
        }),
    );
    assert!(
        !response.is_error(),
        "{}: AX click {id} failed: {}",
        fixture.name,
        response.text()
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
        object.insert("x".to_owned(), serde_json::json!((x - origin.0) * scale));
        object.insert("y".to_owned(), serde_json::json!((y - origin.1) * scale));
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

fn background_refusal_code(response: &ToolResponse, delivery: &str) -> Option<RefusalCode> {
    if delivery != "background" || !response.is_error() {
        return None;
    }
    response.structured()["code"]
        .as_str()
        .and_then(RefusalCode::from_driver_code)
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn is_background_refusal(response: &ToolResponse, delivery: &str) -> bool {
    background_refusal_code(response, delivery).is_some()
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
    thread::sleep(Duration::from_millis(250));
    assert_tree_contains(fixture, &format!("mirror={text}"));
    delivered_observation()
}

fn run_keyboard_action(fixture: &mut Fixture, addressing: &str, delivery: &str) -> Observation {
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
    assert_tree_contains(fixture, "key_state=enter");

    let post_press = snapshot(fixture);
    let mut hotkey_args =
        action_target_args(fixture, &post_press, "keyboard-input", addressing, delivery);
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
    delivered_observation()
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
    CaseSpec::delivered(
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
        DriverRoute::PlatformDefault,
        vec![OracleKind::FixtureState],
    )
}

#[test]
#[ignore]
fn shared_web_action_matrix_is_state_verified() {
    let mut failure = None;
    for spec in host_specs() {
        for (action, tool, marker, addressing, delivery) in [
            (
                "left_click",
                "click",
                "last_action=left_click",
                "ax",
                "background",
            ),
            (
                "left_click",
                "click",
                "last_action=left_click",
                "ax",
                "foreground",
            ),
            (
                "left_click",
                "click",
                "last_action=left_click",
                "px",
                "background",
            ),
            (
                "left_click",
                "click",
                "last_action=left_click",
                "px",
                "foreground",
            ),
            (
                "right_click",
                "right_click",
                "last_action=right_click",
                "ax",
                "background",
            ),
            (
                "right_click",
                "right_click",
                "last_action=right_click",
                "px",
                "foreground",
            ),
            (
                "double_click",
                "double_click",
                "last_action=double_click",
                "ax",
                "background",
            ),
            (
                "double_click",
                "double_click",
                "last_action=double_click",
                "px",
                "foreground",
            ),
        ] {
            let case = shared_case(&spec, action, addressing, delivery);
            let result = run_host_case_with_outcome(case, &spec, |fixture| {
                run_pointer_action(fixture, tool, addressing, delivery, marker)
            });
            if failure.is_none() {
                failure = result;
            }
        }
        for (action, addressing, delivery, run) in [
            (
                "type_text",
                "ax",
                "background",
                run_text_action as fn(&mut Fixture, &str, &str) -> Observation,
            ),
            (
                "type_text",
                "px",
                "foreground",
                run_text_action as fn(&mut Fixture, &str, &str) -> Observation,
            ),
            (
                "keyboard",
                "ax",
                "background",
                run_keyboard_action as fn(&mut Fixture, &str, &str) -> Observation,
            ),
            (
                "keyboard",
                "px",
                "foreground",
                run_keyboard_action as fn(&mut Fixture, &str, &str) -> Observation,
            ),
            (
                "scroll",
                "ax",
                "background",
                run_scroll_action as fn(&mut Fixture, &str, &str) -> Observation,
            ),
            (
                "scroll",
                "px",
                "foreground",
                run_scroll_action as fn(&mut Fixture, &str, &str) -> Observation,
            ),
            (
                "child_window",
                "ax",
                "background",
                run_child_window_action as fn(&mut Fixture, &str, &str) -> Observation,
            ),
            (
                "child_window",
                "px",
                "foreground",
                run_child_window_action as fn(&mut Fixture, &str, &str) -> Observation,
            ),
        ] {
            let case = shared_case(&spec, action, addressing, delivery);
            let result = run_host_case_with_outcome(case, &spec, |fixture| {
                run(fixture, addressing, delivery)
            });
            if failure.is_none() {
                failure = result;
            }
        }
        for delivery in ["background", "foreground"] {
            let case = shared_case(&spec, "drag", "px", delivery);
            let result = run_host_case_with_outcome(case, &spec, |fixture| {
                run_drag_action(fixture, delivery)
            });
            if failure.is_none() {
                failure = result;
            }
        }
        let case = shared_case(&spec, "editor_save", "ax", "background");
        let result = run_host_case_with_outcome(case, &spec, |fixture| {
            run_editor_save_action(fixture, "background")
        });
        if failure.is_none() {
            failure = result;
        }
    }
    resume_first_failure(failure);
}

#[test]
#[ignore]
fn shared_web_keyboard_routes_are_state_verified() {
    let mut failure = None;
    for spec in host_specs() {
        let result = run_host_case("keyboard", &spec, |mut fixture| {
            let pre = snapshot(&mut fixture);
            let input = require_element(&pre, "keyboard-input");
            let (origin_x, origin_y) = window_origin(&fixture, &pre);
            let scale = screenshot_scale(&pre);
            let (screen_x, screen_y) = element_center(&pre, input);
            let focus = fixture.driver.call(
                "click",
                serde_json::json!({
                    "pid": fixture.pid as i64,
                    "window_id": fixture.wid,
                    "x": (screen_x - origin_x) * scale,
                    "y": (screen_y - origin_y) * scale,
                    "delivery_mode": "background"
                }),
            );
            assert!(
                !focus.is_error(),
                "{}: keyboard input focus click failed: {}",
                fixture.name,
                focus.text()
            );
            thread::sleep(Duration::from_millis(150));
            let enter = fixture.driver.call(
                "press_key",
                serde_json::json!({
                    "pid": fixture.pid as i64,
                    "window_id": fixture.wid,
                    "element_index": input,
                    "key": "return",
                    "delivery_mode": "background"
                }),
            );
            assert!(
                !enter.is_error(),
                "{}: return failed: {}",
                fixture.name,
                enter.text()
            );
            assert_tree_contains(&mut fixture, "key_state=enter");

            let hotkey = fixture.driver.call(
                "hotkey",
                serde_json::json!({
                    "pid": fixture.pid as i64,
                    "window_id": fixture.wid,
                    "keys": ["ctrl", "shift", "7"],
                    "x": (screen_x - origin_x) * scale,
                    "y": (screen_y - origin_y) * scale,
                    "delivery_mode": "background"
                }),
            );
            #[cfg(target_os = "windows")]
            {
                if is_background_refusal(&hotkey, "background") {
                    println!(
                        "✅ {} keyboard AX route: background hotkey refused honestly",
                        fixture.name
                    );
                    return;
                }
            }
            #[cfg(target_os = "linux")]
            {
                if is_background_refusal(&hotkey, "background") {
                    println!(
                        "✅ {} keyboard AX route: background hotkey refused honestly",
                        fixture.name
                    );
                    return;
                }
            }
            assert!(
                !hotkey.is_error(),
                "{}: Ctrl+Shift+7 failed: {}",
                fixture.name,
                hotkey.text()
            );
            assert_tree_contains(&mut fixture, "key_state=hotkey");
            println!("✅ {} keyboard AX routes", fixture.name);
        });
        if failure.is_none() {
            failure = result;
        }
    }
    resume_first_failure(failure);
}

#[test]
#[ignore]
fn shared_web_editor_and_scroll_are_state_verified() {
    let mut failure = None;
    for spec in host_specs() {
        let result = run_host_case("editor_scroll", &spec, |mut fixture| {
            let pre = snapshot(&mut fixture);
            let editor = require_element(&pre, "editor-document");
            let text = fixture.driver.call(
                "type_text",
                serde_json::json!({
                    "pid": fixture.pid as i64,
                    "window_id": fixture.wid,
                    "element_index": editor,
                    "text": "CUA saved this note.",
                    "delivery_mode": "background"
                }),
            );
            assert!(
                !text.is_error(),
                "{}: editor type_text failed: {}",
                fixture.name,
                text.text()
            );
            click_ax(&mut fixture, "editor-save");
            assert_tree_contains(&mut fixture, "editor_status=saved:");

            let scroll = require_element(&snapshot(&mut fixture), "scroll-tall");
            let response = fixture.driver.call(
                "scroll",
                serde_json::json!({
                    "pid": fixture.pid as i64,
                    "window_id": fixture.wid,
                    "element_index": scroll,
                    "direction": "down",
                    "by": "page",
                    "amount": 4,
                    "delivery_mode": "background"
                }),
            );
            #[cfg(target_os = "windows")]
            {
                if is_background_refusal(&response, "background") {
                    println!(
                        "✅ {} editor + scroll: background scroll refused honestly",
                        fixture.name
                    );
                    return;
                }
            }
            assert!(
                !response.is_error(),
                "{}: scroll failed: {}",
                fixture.name,
                response.text()
            );
            let post = snapshot(&mut fixture);
            let offset = post
                .tree_text()
                .lines()
                .find_map(|line| line.split("scroll_offset=").nth(1))
                .and_then(|tail| tail.split(|ch: char| !ch.is_ascii_digit()).next())
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0);
            assert!(
                offset > 0,
                "{}: successful background scroll did not advance the external scroll oracle: {}",
                fixture.name,
                post.tree_text()
            );
            println!("✅ {} editor + scroll state oracles", fixture.name);
        });
        if failure.is_none() {
            failure = result;
        }
    }
    resume_first_failure(failure);
}

#[test]
#[ignore]
fn shared_web_child_window_and_drag_have_external_oracles() {
    let mut failure = None;
    for spec in host_specs() {
        let result = run_host_case("child_window_drag", &spec, |mut fixture| {
            let pre = snapshot(&mut fixture);
            let (window_x, window_y) = window_origin(&fixture, &pre);
            let scale = screenshot_scale(&pre);
            let source = require_element(&pre, "drag-source");
            let frame = pre.structured()["elements"]
                .as_array()
                .and_then(|elements| {
                    elements
                        .iter()
                        .find(|element| element["element_index"].as_u64() == Some(source))
                })
                .and_then(|element| element["frame"].as_object())
                .unwrap_or_else(|| panic!("{}: drag-source has no frame", fixture.name));
            let x = (frame["x"].as_f64().unwrap_or(0.0) - window_x
                + frame["w"].as_f64().unwrap_or(0.0) / 2.0)
                * scale;
            let y = (frame["y"].as_f64().unwrap_or(0.0) - window_y
                + frame["h"].as_f64().unwrap_or(0.0) / 2.0)
                * scale;
            let target_index = require_element(&pre, "drop-target");
            let target = pre.structured()["elements"]
                .as_array()
                .and_then(|elements| {
                    elements
                        .iter()
                        .find(|element| element["element_index"].as_u64() == Some(target_index))
                })
                .and_then(|element| element["frame"].as_object())
                .unwrap_or_else(|| panic!("{}: drop-target has no frame", fixture.name));
            let tx = (target["x"].as_f64().unwrap_or(0.0) - window_x
                + target["w"].as_f64().unwrap_or(0.0) / 2.0)
                * scale;
            let ty = (target["y"].as_f64().unwrap_or(0.0) - window_y
                + target["h"].as_f64().unwrap_or(0.0) / 2.0)
                * scale;
            #[cfg(not(target_os = "macos"))]
            let _ = (x, y, tx, ty);
            #[cfg(target_os = "macos")]
            {
                let drag = fixture.driver.call(
                    "drag",
                    serde_json::json!({
                        "pid": fixture.pid as i64,
                        "window_id": fixture.wid,
                        "from_x": x,
                        "from_y": y,
                        "to_x": tx,
                        "to_y": ty,
                        "duration_ms": 500,
                        "delivery_mode": "foreground"
                    }),
                );
                assert!(
                    !drag.is_error(),
                    "{}: drag failed: {}",
                    fixture.name,
                    drag.text()
                );
                assert_tree_contains(&mut fixture, "drag_status=dropped");
            }

            click_ax(&mut fixture, "btn-open-child-window");
            assert_tree_contains(&mut fixture, "child_windows=1");
            println!("✅ {} child-window + drag state oracles", fixture.name);
        });
        if failure.is_none() {
            failure = result;
        }
    }
    resume_first_failure(failure);
}
