//! Cross-platform behavioral scenarios derived from the external CUA matrix.
//!
//! These are source-owned Rust tests, not a copy of a partner test runner. The
//! shared web fixture is loaded by Electron and Tauri on every supported OS;
//! Windows also has WebView2 coverage. The embedded-browser rows require an
//! exact CDP round trip for Electron and a structured, side-effect-free refusal
//! for hosts whose engine-to-native-window relationship cannot be proven.
//! Assertions read mutated application state from a fixture-owned loopback
//! journal, independently of the driver's accessibility snapshot. A successful
//! response alone is never sufficient.
//!
//! Run after building the shared fixtures:
//!
//! ```text
//! cargo test -p cua-driver --test cross_platform_behavior_test -- --ignored \
//!   --nocapture --test-threads=1
//! ```

#![cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]

use std::any::Any;
#[cfg(target_os = "macos")]
use std::io::Write as _;
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
use cua_driver_testkit::{
    harness_app, spawn_in_job, Driver, FixtureJournal, McpDriver, ToolResponse,
};

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
        hosts.push(HostSpec {
            name: "wkwebview",
            path: harness_app(
                "harness-wkwebview",
                "CuaTestHarness.WKWebView.app/Contents/MacOS/CuaTestHarness.WKWebView",
            ),
            args: Vec::new(),
            title: "CuaTestHarness WKWebView",
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
        hosts.push(HostSpec {
            name: "tauri",
            path: harness_app("harness-tauri", "CuaTestHarness.Tauri"),
            args: Vec::new(),
            title: "CuaTestHarness Tauri",
        });
    }

    retain_selected_hosts(&mut hosts);
    hosts
}

fn retain_selected_hosts(hosts: &mut Vec<HostSpec>) {
    if let Ok(filter) = std::env::var("CUA_E2E_HARNESS_FILTER") {
        let selected = filter
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .collect::<std::collections::HashSet<_>>();
        if !selected.is_empty() {
            hosts.retain(|host| selected.contains(host.name));
        }
    }
}

fn embedded_browser_specs() -> Vec<HostSpec> {
    let mut hosts = host_specs();
    #[cfg(target_os = "windows")]
    hosts.push(HostSpec {
        name: "webview2",
        path: harness_app("harness-webview", "CuaTestHarness.WebView.exe"),
        args: Vec::new(),
        title: "CuaTestHarness WebView",
    });
    retain_selected_hosts(&mut hosts);
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
        let mut fixture = launch_host_with_evidence(spec, &case.cell_id, &mut evidence);
        let sentinel = if delivery == Delivery::Background {
            Some(ForegroundSentinel::launch(&mut fixture.driver))
        } else {
            None
        };
        if let Some(sentinel) = &sentinel {
            sentinel
                .assert_background_posture(TargetWindow {
                    pid: fixture.pid,
                    native_id: fixture.wid,
                })
                .expect("establish background posture before recording");
        }
        fixture.driver.start_behavior_recording();
        if let Some(sentinel) = &sentinel {
            sentinel
                .prepare_background_observation(
                    &mut fixture.driver,
                    TargetWindow {
                        pid: fixture.pid,
                        native_id: fixture.wid,
                    },
                )
                .expect("re-establish background posture after recording setup");
        }
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
        observation
    }));
    match outcome {
        Ok(observation) => {
            let result = CaseResult::evaluate(case, observation, started.elapsed());
            write_result_from_env(&result).expect("write E2E case result");
            if result.test_status == cua_driver_testkit::e2e::TestStatus::Fail {
                return Some(Box::new(result.message));
            }
            None
        }
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

fn spawn_driver(recording_label: &str, cdp_port: Option<u16>) -> Option<McpDriver> {
    #[cfg(target_os = "macos")]
    {
        let _ = cdp_port;
        McpDriver::spawn_macos_daemon_proxy_named(recording_label)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let port = cdp_port.map(|value| value.to_string());
        match port.as_deref() {
            Some(port) => {
                McpDriver::spawn_named_with_env(recording_label, &[("CUA_DRIVER_CDP_PORT", port)])
            }
            None => McpDriver::spawn_named(recording_label),
        }
    }
}

struct Fixture {
    driver: McpDriver,
    pid: u32,
    wid: u64,
    window_x: f64,
    window_y: f64,
    name: &'static str,
    journal: FixtureJournal,
}

fn evidence_for_driver(driver: &McpDriver) -> Evidence {
    recording_evidence(driver.recording_dir())
}

fn allocate_loopback_port() -> u16 {
    let listener =
        std::net::TcpListener::bind(("127.0.0.1", 0)).expect("allocate an ephemeral fixture port");
    listener.local_addr().expect("read fixture port").port()
}

fn launch_host_with_evidence(spec: &HostSpec, scenario: &str, evidence: &mut Evidence) -> Fixture {
    if !spec.path.exists() {
        panic!(
            "{} fixture is required but was not staged at {:?}",
            spec.name, spec.path
        );
    }

    let recording_label = scenario.to_owned();
    let cdp_port = matches!(spec.name, "electron" | "webview2").then(allocate_loopback_port);
    let mut driver = spawn_driver(&recording_label, cdp_port).unwrap_or_else(|| {
        panic!(
            "cua-driver could not be started for the required {} fixture",
            spec.name
        )
    });
    *evidence = evidence_for_driver(&driver);
    let journal = FixtureJournal::start();
    let mut command = Command::new(&spec.path);
    command
        .args(&spec.args)
        .env("CUA_E2E_FIXTURE_JOURNAL_URL", journal.url())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    match spec.name {
        "electron" => {
            command.env(
                "CUA_ELECTRON_CDP_PORT",
                cdp_port.expect("Electron CDP port").to_string(),
            );
        }
        "webview2" => {
            command.env(
                "CUA_WEBVIEW_CDP_PORT",
                cdp_port.expect("WebView2 CDP port").to_string(),
            );
        }
        _ => {}
    }
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
    let child = spawn_in_job(&mut command)
        .unwrap_or_else(|error| panic!("failed to launch {} fixture: {error}", spec.name));
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
                        journal,
                    };
                    let ax_deadline = Instant::now() + Duration::from_secs(10);
                    let mut last_tree = String::new();
                    while Instant::now() < ax_deadline {
                        let state = snapshot(&mut fixture);
                        last_tree.clear();
                        last_tree.push_str(state.tree_text());
                        if last_tree.contains("WEB_HARNESS_MARKER_v1")
                            && fixture.journal.contains("WEB_HARNESS_MARKER_v1")
                        {
                            return fixture;
                        }
                        thread::sleep(Duration::from_millis(250));
                    }
                    panic!(
                        "{} fixture accessibility tree did not become ready; last tree:\n{}",
                        spec.name, last_tree
                    );
                }
            }
        }
        thread::sleep(Duration::from_millis(250));
    }
    panic!("{} fixture window did not appear", spec.name);
}

fn snapshot(fixture: &mut Fixture) -> ToolResponse {
    let state = fixture.driver.call(
        "get_window_state",
        serde_json::json!({
            "pid": fixture.pid as i64,
            "window_id": fixture.wid,
            "capture_mode": "ax"
        }),
    );
    refresh_wayland_window_geometry(fixture);
    state
}

#[cfg(target_os = "linux")]
fn refresh_wayland_window_geometry(fixture: &mut Fixture) {
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return;
    }
    // A compositor may place a newly mapped toplevel after its first
    // list_windows appearance. Re-read the current compositor-owned geometry
    // for every snapshot, matching the native GTK harness, instead of turning
    // that launch-time placement into a stale PX coordinate origin.
    let windows = fixture.driver.call(
        "list_windows",
        serde_json::json!({ "pid": fixture.pid as i64 }),
    );
    let Some(window) = windows.structured()["windows"]
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .find(|window| window["pid"].as_u64() == Some(fixture.pid as u64))
        })
    else {
        return;
    };
    if let Some(window_id) = window["window_id"].as_u64() {
        fixture.wid = window_id;
    }
    let bounds = &window["bounds"];
    fixture.window_x = bounds["x"].as_f64().unwrap_or(fixture.window_x);
    fixture.window_y = bounds["y"].as_f64().unwrap_or(fixture.window_y);
}

#[cfg(not(target_os = "linux"))]
fn refresh_wayland_window_geometry(_fixture: &mut Fixture) {}

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
    if window_width > 0.0 && width > 0.0 {
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
        .or_else(|| element_index_containing(snapshot.tree_text(), id))
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

fn assert_fixture_contains(fixture: &Fixture, marker: &str) {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if fixture.journal.contains(marker) {
            return;
        }
        if Instant::now() >= deadline {
            panic!(
                "{}: fixture journal did not reach {marker:?}: {}",
                fixture.name,
                fixture.journal.snapshot()
            );
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn assert_fixture_value(fixture: &Fixture, id: &str, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let state = fixture.journal.snapshot();
        if state[id]["value"].as_str() == Some(expected) {
            return;
        }
        if Instant::now() >= deadline {
            panic!(
                "{}: fixture value {id:?} did not reach {expected:?}: {state}",
                fixture.name
            );
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn assert_fixture_single_insertion(
    fixture: &Fixture,
    id: &str,
    initial: &str,
    inserted: &str,
) -> String {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let state = fixture.journal.snapshot();
        if let Some(value) = state[id]["value"].as_str() {
            if value.len() == initial.len() + inserted.len() {
                for (offset, _) in value.match_indices(inserted) {
                    let mut without_insertion = value.to_owned();
                    without_insertion.replace_range(offset..offset + inserted.len(), "");
                    if without_insertion == initial {
                        return value.to_owned();
                    }
                }
            }
        }
        if Instant::now() >= deadline {
            panic!(
                "{}: fixture value {id:?} did not insert {inserted:?} exactly once into {initial:?}: {state}",
                fixture.name
            );
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn assert_fixture_text(fixture: &Fixture, id: &str, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let state = fixture.journal.snapshot();
        if state[id]["text"].as_str() == Some(expected) {
            return;
        }
        if Instant::now() >= deadline {
            panic!(
                "{}: fixture text {id:?} did not reach {expected:?}: {state}",
                fixture.name
            );
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn fixture_marker_number(fixture: &Fixture, id: &str, prefix: &str) -> Option<u64> {
    fixture
        .journal
        .text(id)
        .and_then(|text| text.strip_prefix(prefix).map(str::to_owned))
        .and_then(|value| value.parse().ok())
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
        eprintln!(
            "[shared-px] {} target={id} screen=({x:.1},{y:.1}) origin=({:.1},{:.1}) scale={scale:.3} local=({local_x:.1},{local_y:.1}) capture=({width:.1}x{height:.1})",
            fixture.name, origin.0, origin.1
        );
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
    let journal_before = fixture.journal.snapshot();
    let args = action_target_args(fixture, &pre, "border-click-target", addressing, delivery);
    let response = fixture.driver.call(tool, args);
    if let Some(code) = background_refusal_code(&response, delivery) {
        return refused_without_fixture_mutation(fixture, &journal_before, code, &response);
    }
    assert!(
        !response.is_error(),
        "{}: {tool} {addressing}/{delivery} failed: {}",
        fixture.name,
        response.text()
    );
    assert_fixture_contains(fixture, expected_marker);
    delivered_observation()
}

fn delivered_observation() -> Observation {
    Observation::delivered(vec![OracleKind::FixtureState], Evidence::default())
}

fn browser_ref_by_label(snapshot: &ToolResponse, label_fragment: &str) -> String {
    snapshot.structured()["refs"]
        .as_array()
        .and_then(|refs| {
            refs.iter().find(|entry| {
                entry["label"]
                    .as_str()
                    .is_some_and(|label| label.contains(label_fragment))
            })
        })
        .and_then(|entry| entry["ref"].as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            panic!(
                "browser snapshot is missing ref label {label_fragment:?}: {}",
                snapshot.raw
            )
        })
}

fn refused_without_fixture_mutation(
    fixture: &Fixture,
    before: &serde_json::Value,
    code: RefusalCode,
    response: &ToolResponse,
) -> Observation {
    thread::sleep(Duration::from_millis(150));
    let after = fixture.journal.snapshot();
    assert_eq!(
        &after, before,
        "{}: refused action mutated fixture state: before={before}, after={after}",
        fixture.name
    );
    Observation::refused(
        code,
        vec![OracleKind::FixtureState],
        response.text(),
        Evidence::default(),
    )
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
    let journal_before = fixture.journal.snapshot();
    let text = format!("cua-{addressing}-{delivery}");
    let mut args = action_target_args(fixture, &pre, "txt-input", addressing, delivery);
    args.as_object_mut()
        .expect("type_text arguments object")
        .insert("text".to_owned(), serde_json::json!(text));
    let response = fixture.driver.call("type_text", args);
    if let Some(code) = background_refusal_code(&response, delivery) {
        return refused_without_fixture_mutation(fixture, &journal_before, code, &response);
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
    assert_fixture_contains(fixture, &format!("mirror={text}"));
    Observation::delivered(passed, Evidence::default())
}

fn run_type_submit_action(fixture: &mut Fixture, addressing: &str, delivery: &str) -> Observation {
    let pre = snapshot(fixture);
    let journal_before = fixture.journal.snapshot();
    let text = format!("cua-submit-{addressing}-{delivery}");
    let mut type_args = action_target_args(fixture, &pre, "keyboard-input", addressing, delivery);
    type_args
        .as_object_mut()
        .expect("type-submit type_text arguments object")
        .insert("text".to_owned(), serde_json::json!(text));
    let type_response = fixture.driver.call("type_text", type_args);
    if let Some(code) = background_refusal_code(&type_response, delivery) {
        return refused_without_fixture_mutation(fixture, &journal_before, code, &type_response);
    }
    assert!(
        !type_response.is_error(),
        "{}: type-submit type_text {addressing}/{delivery} failed: {}",
        fixture.name,
        type_response.text()
    );
    assert_fixture_value(fixture, "keyboard-input", &text);

    let post_type = snapshot(fixture);
    let mut enter_args =
        action_target_args(fixture, &post_type, "keyboard-input", addressing, delivery);
    enter_args
        .as_object_mut()
        .expect("type-submit press_key arguments object")
        .insert("key".to_owned(), serde_json::json!("return"));
    let enter_response = fixture.driver.call("press_key", enter_args);
    if let Some(code) = background_refusal_code(&enter_response, delivery) {
        return refused_without_fixture_mutation(fixture, &journal_before, code, &enter_response);
    }
    assert!(
        !enter_response.is_error(),
        "{}: type-submit Enter {addressing}/{delivery} failed: {}",
        fixture.name,
        enter_response.text()
    );
    assert_fixture_contains(fixture, &format!("key_state=submitted:{text}"));

    let mut passed = vec![OracleKind::FixtureState];
    if addressing == "px" {
        passed.extend(unverified_background_protocol_oracle(
            &type_response,
            delivery,
        ));
    }
    passed.extend(unverified_background_protocol_oracle(
        &enter_response,
        delivery,
    ));
    Observation::delivered(passed, Evidence::default())
}

fn run_press_key_action(fixture: &mut Fixture, addressing: &str, delivery: &str) -> Observation {
    let pre = snapshot(fixture);
    let journal_before = fixture.journal.snapshot();
    let mut press_args = action_target_args(fixture, &pre, "keyboard-input", addressing, delivery);
    press_args
        .as_object_mut()
        .expect("press_key arguments object")
        .insert("key".to_owned(), serde_json::json!("return"));
    let response = fixture.driver.call("press_key", press_args);
    if let Some(code) = background_refusal_code(&response, delivery) {
        return refused_without_fixture_mutation(fixture, &journal_before, code, &response);
    }
    assert!(
        !response.is_error(),
        "{}: press_key {addressing}/{delivery} failed: {}",
        fixture.name,
        response.text()
    );
    let mut passed = vec![OracleKind::FixtureState];
    passed.extend(unverified_background_protocol_oracle(&response, delivery));
    assert_fixture_contains(fixture, "key_state=enter");

    Observation::delivered(passed, Evidence::default())
}

fn run_hotkey_action(fixture: &mut Fixture, addressing: &str, delivery: &str) -> Observation {
    let pre = snapshot(fixture);
    let journal_before = fixture.journal.snapshot();
    let mut hotkey_args = action_target_args(fixture, &pre, "keyboard-input", addressing, delivery);
    hotkey_args
        .as_object_mut()
        .expect("hotkey arguments object")
        .insert("keys".to_owned(), serde_json::json!(["ctrl", "shift", "7"]));
    let response = fixture.driver.call("hotkey", hotkey_args);
    if let Some(code) = background_refusal_code(&response, delivery) {
        return refused_without_fixture_mutation(fixture, &journal_before, code, &response);
    }
    assert!(
        !response.is_error(),
        "{}: hotkey {addressing}/{delivery} failed: {}",
        fixture.name,
        response.text()
    );
    assert_fixture_contains(fixture, "key_state=hotkey");
    #[cfg(target_os = "macos")]
    if fixture.name == "electron" && addressing == "px" && delivery == "foreground" {
        run_macos_selection_hotkeys(fixture);
    }
    let passed = unverified_background_protocol_oracle(&response, delivery);
    Observation::delivered_with_fixture_state(passed)
}

#[cfg(target_os = "macos")]
struct TextClipboardGuard {
    previous: Vec<u8>,
}

#[cfg(target_os = "macos")]
impl TextClipboardGuard {
    fn replace(text: &str) -> Self {
        let previous = Command::new("/usr/bin/pbpaste")
            .output()
            .expect("read macOS text pasteboard")
            .stdout;
        write_text_clipboard(text.as_bytes());
        Self { previous }
    }
}

#[cfg(target_os = "macos")]
impl Drop for TextClipboardGuard {
    fn drop(&mut self) {
        write_text_clipboard(&self.previous);
    }
}

#[cfg(target_os = "macos")]
fn write_text_clipboard(contents: &[u8]) {
    let mut child = Command::new("/usr/bin/pbcopy")
        .stdin(Stdio::piped())
        .spawn()
        .expect("start macOS text pasteboard writer");
    child
        .stdin
        .as_mut()
        .expect("pbcopy stdin")
        .write_all(contents)
        .expect("write macOS text pasteboard");
    assert!(
        child.wait().expect("wait for pbcopy").success(),
        "pbcopy failed"
    );
}

#[cfg(target_os = "macos")]
fn run_macos_selection_hotkeys(fixture: &mut Fixture) {
    const INITIAL: &str = "cua-hotkey-initial";
    const REPLACEMENT: &str = "cua-hotkey-replacement";
    const PASTED: &str = "cua-hotkey-pasted";

    let state = snapshot(fixture);
    let mut type_args = action_target_args(fixture, &state, "keyboard-input", "px", "foreground");
    type_args["text"] = serde_json::json!(INITIAL);
    let typed = fixture.driver.call("type_text", type_args);
    assert!(!typed.is_error(), "seed selection fixture: {}", typed.raw);
    assert_fixture_value(fixture, "keyboard-input", INITIAL);

    let select_all = |fixture: &mut Fixture| {
        let state = snapshot(fixture);
        let mut args = action_target_args(fixture, &state, "keyboard-input", "px", "foreground");
        args["keys"] = serde_json::json!(["cmd", "a"]);
        let response = fixture.driver.call("hotkey", args);
        assert!(!response.is_error(), "Cmd+A failed: {}", response.raw);
        assert_eq!(response.verified(), Some(false), "{}", response.raw);
        assert_eq!(
            response.structured()["effect"],
            "unverifiable",
            "generic hotkeys must retain the honest unverifiable contract: {}",
            response.raw
        );
    };

    select_all(fixture);
    let state = snapshot(fixture);
    let mut replace_args =
        action_target_args(fixture, &state, "keyboard-input", "px", "foreground");
    replace_args["text"] = serde_json::json!(REPLACEMENT);
    let replaced = fixture.driver.call("type_text", replace_args);
    assert!(!replaced.is_error(), "replace selection: {}", replaced.raw);
    assert_fixture_value(fixture, "keyboard-input", REPLACEMENT);

    select_all(fixture);
    let _clipboard = TextClipboardGuard::replace(PASTED);
    let state = snapshot(fixture);
    let mut paste_args = action_target_args(fixture, &state, "keyboard-input", "px", "foreground");
    paste_args["keys"] = serde_json::json!(["cmd", "v"]);
    let pasted = fixture.driver.call("hotkey", paste_args);
    assert!(!pasted.is_error(), "Cmd+V failed: {}", pasted.raw);
    assert_eq!(pasted.verified(), Some(false), "{}", pasted.raw);
    assert_eq!(pasted.structured()["effect"], "unverifiable");
    assert_fixture_value(fixture, "keyboard-input", PASTED);
}

fn run_scroll_action(fixture: &mut Fixture, addressing: &str, delivery: &str) -> Observation {
    let pre = snapshot(fixture);
    let journal_before = fixture.journal.snapshot();
    let mut args = action_target_args(fixture, &pre, "scroll-tall", addressing, delivery);
    let object = args.as_object_mut().expect("scroll arguments object");
    object.insert("direction".to_owned(), serde_json::json!("down"));
    object.insert("by".to_owned(), serde_json::json!("page"));
    object.insert("amount".to_owned(), serde_json::json!(2));
    let response = fixture.driver.call("scroll", args);
    if let Some(code) = background_refusal_code(&response, delivery) {
        return refused_without_fixture_mutation(fixture, &journal_before, code, &response);
    }
    assert!(
        !response.is_error(),
        "{}: scroll {addressing}/{delivery} failed: {}; raw={}",
        fixture.name,
        response.text(),
        response.raw
    );
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let offset =
            fixture_marker_number(fixture, "lbl-scroll-offset", "scroll_offset=").unwrap_or(0);
        let page =
            fixture_marker_number(fixture, "lbl-scroll-client-height", "scroll_client_height=")
                .unwrap_or(0);
        if page > 0 && offset >= page {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "{}: two-page scroll did not advance by at least one viewport: {}; response={}; raw={}",
            fixture.name,
            fixture.journal.snapshot(),
            response.text(),
            response.raw
        );
        thread::sleep(Duration::from_millis(100));
    }
    delivered_observation()
}

fn run_drag_action(fixture: &mut Fixture, delivery: &str) -> Observation {
    let pre = snapshot(fixture);
    let journal_before = fixture.journal.snapshot();
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
        return refused_without_fixture_mutation(fixture, &journal_before, code, &response);
    }
    assert!(
        !response.is_error(),
        "{}: drag PX/{delivery} failed: {}; raw={}",
        fixture.name,
        response.text(),
        response.raw
    );
    assert_fixture_contains(fixture, "drag_status=dropped");
    delivered_observation()
}

fn run_child_window_action(fixture: &mut Fixture, addressing: &str, delivery: &str) -> Observation {
    let pre = snapshot(fixture);
    let journal_before = fixture.journal.snapshot();
    let args = action_target_args(fixture, &pre, "btn-open-child-window", addressing, delivery);
    let response = fixture.driver.call("click", args);
    if let Some(code) = background_refusal_code(&response, delivery) {
        return refused_without_fixture_mutation(fixture, &journal_before, code, &response);
    }
    assert!(
        !response.is_error(),
        "{}: child-window click {addressing}/{delivery} failed: {}",
        fixture.name,
        response.text()
    );
    assert_fixture_contains(fixture, "child_windows=1");
    delivered_observation()
}

fn run_editor_save_action(fixture: &mut Fixture, delivery: &str) -> Observation {
    let pre = snapshot(fixture);
    let journal_before = fixture.journal.snapshot();
    let text = format!("cua-editor-{delivery}");
    let initial_value = journal_before["editor-document"]["value"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "{}: editor fixture did not publish its initial value: {journal_before}",
                fixture.name
            )
        });
    let mut text_args = action_target_args(fixture, &pre, "editor-document", "ax", delivery);
    text_args
        .as_object_mut()
        .expect("editor type_text arguments object")
        .insert("text".to_owned(), serde_json::json!(text));
    let response = fixture.driver.call("type_text", text_args);
    if let Some(code) = background_refusal_code(&response, delivery) {
        return refused_without_fixture_mutation(fixture, &journal_before, code, &response);
    }
    assert!(
        !response.is_error(),
        "{}: editor type_text failed: {}",
        fixture.name,
        response.text()
    );
    let expected_value =
        assert_fixture_single_insertion(fixture, "editor-document", initial_value, &text);

    let post_text = snapshot(fixture);
    let journal_before_save = fixture.journal.snapshot();
    let save_args = action_target_args(fixture, &post_text, "editor-save", "ax", delivery);
    let response = fixture.driver.call("click", save_args);
    if let Some(code) = background_refusal_code(&response, delivery) {
        return refused_without_fixture_mutation(fixture, &journal_before_save, code, &response);
    }
    assert!(
        !response.is_error(),
        "{}: editor save failed: {}",
        fixture.name,
        response.text()
    );
    assert_fixture_text(
        fixture,
        "editor-status",
        &format!("editor_status=saved:{expected_value}"),
    );
    delivered_observation()
}

#[cfg(target_os = "linux")]
fn linux_real_pointer_input_available() -> bool {
    platform_linux::input::real_pointer_input_available()
}

#[cfg(not(target_os = "linux"))]
fn linux_real_pointer_input_available() -> bool {
    false
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
    let expected_refusals = if cfg!(target_os = "windows") && delivery_kind == Delivery::Background
    {
        match (spec.name, action, targeting) {
            ("electron", "right_click" | "double_click" | "drag", _) => {
                vec![RefusalCode::BackgroundOccluded]
            }
            (
                "electron",
                "type_text" | "type_submit" | "press_key" | "hotkey" | "scroll" | "editor_save",
                _,
            ) => {
                vec![RefusalCode::BackgroundUnavailable]
            }
            ("tauri", "hotkey", _) | ("tauri", "scroll", Targeting::Px) => {
                vec![RefusalCode::BackgroundUnavailable]
            }
            ("tauri", "drag", Targeting::Px) => vec![RefusalCode::BackgroundOccluded],
            _ => Vec::new(),
        }
    } else if cfg!(target_os = "macos") && delivery_kind == Delivery::Background {
        match (spec.name, action, targeting) {
            ("electron", "scroll", _) | (_, "drag", Targeting::Px) => {
                vec![RefusalCode::BackgroundUnavailable]
            }
            _ => Vec::new(),
        }
    } else if cfg!(target_os = "linux")
        && spec.name == "electron"
        && delivery_kind == Delivery::Background
    {
        // Chromium's X11 renderer drops synthetic input addressed to a fully
        // occluded, unfocused toplevel. Focus-free AT-SPI button actions are
        // the exception: they are externally verified by the fixture and the
        // focus/z-order/leak sentinels.
        if std::env::var_os("CUA_INJECT_SOCKET").is_some() {
            Vec::new()
        } else {
            match (action, targeting) {
                ("left_click" | "child_window", Targeting::Ax) => Vec::new(),
                _ => vec![RefusalCode::BackgroundUnavailable],
            }
        }
    } else if cfg!(target_os = "linux")
        && spec.name == "tauri"
        && delivery_kind == Delivery::Background
    {
        // WebKitGTK accepts AT-SPI text writes without emitting the renderer's
        // user-input event, and its keyboard channel is focus-bound. The driver
        // must refuse those background keyboard composites instead of reporting
        // a successful write that the page never observes.
        match (action, targeting) {
            ("type_text" | "type_submit", _)
            | ("editor_save", Targeting::Ax)
            | ("press_key" | "hotkey", _) => {
                vec![RefusalCode::BackgroundUnavailable]
            }
            ("right_click" | "double_click" | "scroll", _) | ("drag", Targeting::Px)
                if !linux_real_pointer_input_available() =>
            {
                vec![RefusalCode::BackgroundUnavailable]
            }
            _ => Vec::new(),
        }
    } else {
        Vec::new()
    };
    let expected_background_refusal = !expected_refusals.is_empty();
    let mut oracles = vec![OracleKind::FixtureState];
    if delivery_kind == Delivery::Background {
        oracles.extend([
            OracleKind::Focus,
            OracleKind::ZOrder,
            OracleKind::NoLeakedInput,
        ]);
        if cua_driver_testkit::e2e::DisplayServer::current()
            != cua_driver_testkit::e2e::DisplayServer::Wayland
        {
            oracles.push(OracleKind::Cursor);
        }
        if !expected_background_refusal
            && cfg!(target_os = "windows")
            && (matches!(action, "press_key" | "hotkey" | "type_submit")
                || (action == "type_text" && targeting == Targeting::Px))
        {
            oracles.push(OracleKind::Protocol);
        }
    }
    let mut route = shared_web_route(
        cua_driver_testkit::e2e::Platform::current(),
        cua_driver_testkit::e2e::DisplayServer::current(),
        action,
        targeting,
        delivery_kind,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    if cfg!(target_os = "windows")
        && spec.name == "electron"
        && action == "left_click"
        && delivery_kind == Delivery::Background
    {
        route = cua_driver_testkit::e2e::DriverRoute::PostMessage;
    } else if cfg!(target_os = "macos")
        && targeting == Targeting::Px
        && delivery_kind == Delivery::Background
        && matches!(action, "left_click" | "child_window")
    {
        route = cua_driver_testkit::e2e::DriverRoute::MacosAxAction;
    }
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
        case.expecting_refusal(expected_refusals)
    } else {
        case
    }
}

fn cell_selected(case: &CaseSpec) -> bool {
    let Ok(filter) = std::env::var("CUA_E2E_CELL_FILTER") else {
        return true;
    };
    let mut parts = filter
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .peekable();
    parts.peek().is_none() || parts.any(|part| case.cell_id == part || case.cell_id.contains(part))
}

fn run_browser_tool_roundtrip(fixture: &mut Fixture) -> Observation {
    let session = format!("browser-v1-e2e-{}", fixture.pid);
    let started = fixture
        .driver
        .call("start_session", serde_json::json!({ "session": session }));
    assert!(
        !started.is_error(),
        "browser E2E session did not start: {}",
        started.raw
    );
    let prepared = fixture.driver.call(
        "browser_prepare",
        serde_json::json!({
            "pid": fixture.pid as i64,
            "session": session,
        }),
    );
    assert_eq!(
        prepared.structured()["prepared"].as_bool(),
        Some(true),
        "browser_prepare did not recognize the fixture's owned endpoint: {}",
        prepared.raw
    );
    let bind = fixture.driver.call(
        "get_browser_state",
        serde_json::json!({
            "pid": fixture.pid as i64,
            "window_id": fixture.wid,
            "session": session,
        }),
    );
    assert_eq!(
        bind.structured()["status"].as_str(),
        Some("ok"),
        "Electron browser binding failed: {}; raw={}",
        bind.text(),
        bind.raw
    );
    assert_eq!(
        bind.structured()["binding_quality"].as_str(),
        Some("exact"),
        "Electron browser binding must be exact: {}",
        bind.raw
    );
    assert_eq!(
        bind.structured()["binding_route"].as_str(),
        Some("embedded_single_page"),
        "Electron must exercise the bounded embedded-browser route: {}",
        bind.raw
    );
    let target_id = bind.structured()["target_id"]
        .as_str()
        .expect("bind target_id")
        .to_owned();
    let tab_id = bind.structured()["tabs"]
        .as_array()
        .and_then(|tabs| tabs.iter().find(|tab| tab["active"] == true))
        .and_then(|tab| tab["tab_id"].as_str())
        .expect("active tab_id")
        .to_owned();

    let first_snapshot = fixture.driver.call(
        "get_browser_state",
        serde_json::json!({
            "target_id": target_id,
            "tab_id": tab_id,
            "session": session,
        }),
    );
    assert_eq!(
        first_snapshot.structured()["status"].as_str(),
        Some("ok"),
        "Electron browser snapshot failed: {}",
        first_snapshot.raw
    );
    let increment_ref = browser_ref_by_label(&first_snapshot, "id=btn-increment");
    let click = fixture.driver.call(
        "browser_click",
        serde_json::json!({
            "target_id": target_id,
            "tab_id": tab_id,
            "ref": increment_ref,
            "session": session,
        }),
    );
    assert_eq!(
        click.structured()["status"].as_str(),
        Some("ok"),
        "trusted browser click failed: {}",
        click.raw
    );
    assert_eq!(click.structured()["route"].as_str(), Some("trusted"));
    assert_fixture_text(fixture, "lbl-counter", "counter=1");

    let second_snapshot = fixture.driver.call(
        "get_browser_state",
        serde_json::json!({
            "target_id": target_id,
            "tab_id": tab_id,
            "session": session,
        }),
    );
    let input_ref = browser_ref_by_label(&second_snapshot, "id=txt-input");
    let typed = fixture.driver.call(
        "browser_type",
        serde_json::json!({
            "target_id": target_id,
            "tab_id": tab_id,
            "ref": input_ref,
            "text": "browser-v1",
            "session": session,
        }),
    );
    assert_eq!(
        typed.structured()["status"].as_str(),
        Some("ok"),
        "browser type failed: {}",
        typed.raw
    );
    assert_fixture_value(fixture, "txt-input", "browser-v1");
    assert_fixture_text(fixture, "lbl-input-mirror", "mirror=browser-v1");

    let stale = fixture.driver.call(
        "browser_click",
        serde_json::json!({
            "target_id": target_id,
            "tab_id": tab_id,
            "ref": increment_ref,
            "session": session,
        }),
    );
    assert_eq!(
        stale.structured()["refusal"]["code"].as_str(),
        Some("browser_ref_stale"),
        "older snapshot ref should fail closed: {}",
        stale.raw
    );
    assert_fixture_text(fixture, "lbl-counter", "counter=1");

    let navigated = fixture.driver.call(
        "browser_navigate",
        serde_json::json!({
            "target_id": target_id,
            "tab_id": tab_id,
            "url": "about:blank",
            "session": session,
        }),
    );
    assert_eq!(
        navigated.structured()["status"].as_str(),
        Some("ok"),
        "browser navigation failed: {}",
        navigated.raw
    );
    assert_eq!(
        navigated.structured()["refs_invalidated"].as_bool(),
        Some(true),
        "browser navigation must invalidate page refs: {}",
        navigated.raw
    );
    let blank_snapshot = fixture.driver.call(
        "get_browser_state",
        serde_json::json!({
            "target_id": target_id,
            "tab_id": tab_id,
            "session": session,
        }),
    );
    assert_eq!(
        blank_snapshot.structured()["url"].as_str(),
        Some("about:blank"),
        "browser did not reach the requested page: {}",
        blank_snapshot.raw
    );
    let stale_after_navigation = fixture.driver.call(
        "browser_click",
        serde_json::json!({
            "target_id": target_id,
            "tab_id": tab_id,
            "ref": input_ref,
            "session": session,
        }),
    );
    assert_eq!(
        stale_after_navigation.structured()["refusal"]["code"].as_str(),
        Some("browser_ref_stale"),
        "navigation-invalidated ref should fail closed: {}",
        stale_after_navigation.raw
    );
    let ended = fixture
        .driver
        .call("end_session", serde_json::json!({ "session": session }));
    assert!(
        !ended.is_error(),
        "browser E2E session did not end: {}",
        ended.raw
    );
    delivered_observation()
}

fn run_browser_tool_refusal(fixture: &mut Fixture) -> Observation {
    let session = format!("browser-embedded-refusal-{}", fixture.pid);
    let started = fixture
        .driver
        .call("start_session", serde_json::json!({ "session": session }));
    assert!(
        !started.is_error(),
        "browser refusal session did not start: {}",
        started.raw
    );
    let journal_before = fixture.journal.snapshot();
    let response = fixture.driver.call(
        "get_browser_state",
        serde_json::json!({
            "pid": fixture.pid as i64,
            "window_id": fixture.wid,
            "session": session,
        }),
    );
    let code = response.structured()["refusal"]["code"]
        .as_str()
        .and_then(RefusalCode::from_driver_code)
        .unwrap_or_else(|| {
            panic!(
                "embedded browser did not refuse structurally: {}",
                response.raw
            )
        });
    assert_eq!(
        code,
        RefusalCode::BrowserRouteUnavailable,
        "embedded browser returned the wrong refusal: {}",
        response.raw
    );
    assert_eq!(
        fixture.journal.snapshot(),
        journal_before,
        "browser refusal mutated the fixture journal"
    );
    let ended = fixture
        .driver
        .call("end_session", serde_json::json!({ "session": session }));
    assert!(
        !ended.is_error(),
        "browser refusal session did not end: {}",
        ended.raw
    );
    Observation::refused(
        code,
        vec![OracleKind::FixtureState],
        response.text(),
        Evidence::default(),
    )
}

fn embedded_browser_case(spec: &HostSpec) -> CaseSpec {
    let mut oracles = vec![
        OracleKind::FixtureState,
        OracleKind::Focus,
        OracleKind::ZOrder,
        OracleKind::NoLeakedInput,
    ];
    if cua_driver_testkit::e2e::DisplayServer::current()
        != cua_driver_testkit::e2e::DisplayServer::Wayland
    {
        oracles.push(OracleKind::Cursor);
    }
    let case = CaseSpec::delivered(
        format!(
            "{}-{}-browser-tool-roundtrip",
            std::env::consts::OS,
            spec.name
        ),
        spec.name,
        if spec.name == "electron" {
            "chromium"
        } else {
            "platform-webview"
        },
        "browser_tool_roundtrip",
        Targeting::Page,
        Delivery::Background,
        Scope::Window,
        cua_driver_testkit::e2e::DriverRoute::Cdp,
        oracles,
    );
    if spec.name == "electron" {
        case
    } else {
        case.expecting_refusal(vec![RefusalCode::BrowserRouteUnavailable])
    }
}

#[test]
#[ignore]
fn embedded_browser_routes_are_exact_or_refused() {
    let mut failure = None;
    let mut selected = 0usize;
    for spec in embedded_browser_specs() {
        let case = embedded_browser_case(&spec);
        if !cell_selected(&case) {
            continue;
        }
        selected += 1;
        let result = if spec.name == "electron" {
            run_host_case_with_outcome(case, &spec, run_browser_tool_roundtrip)
        } else {
            run_host_case_with_outcome(case, &spec, run_browser_tool_refusal)
        };
        if failure.is_none() {
            failure = result;
        }
    }
    if selected == 0
        && std::env::var("CUA_E2E_CELL_FILTER").is_ok_and(|filter| !filter.trim().is_empty())
    {
        eprintln!("no embedded-browser rows matched CUA_E2E_CELL_FILTER");
        return;
    }
    assert!(
        selected > 0,
        "no embedded-browser cells were selected; check CUA_E2E_HARNESS_FILTER"
    );
    resume_first_failure(failure);
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
                "type_submit",
                run_type_submit_action as fn(&mut Fixture, &str, &str) -> Observation,
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
        selected > 0,
        "no shared E2E cells were selected; check CUA_E2E_HARNESS_FILTER and CUA_E2E_CELL_FILTER"
    );
    resume_first_failure(failure);
}
