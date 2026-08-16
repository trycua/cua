//! Packaged Windows/Linux lifecycle gate for encrypted Computer History.
//!
//! This test intentionally uses the installed local product, its authenticated
//! daemon transport, and the platform credential store. It records one real
//! window action, restarts the daemon, hydrates the action from ciphertext, and
//! finally proves disable/preserve plus cryptographic deletion.

#![cfg(any(target_os = "windows", target_os = "linux"))]

use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread::sleep,
    time::{Duration, Instant},
};

use cua_driver_core::history::KeyProvider;
use cua_driver_testkit::ax::{element_index_by_id, element_index_containing};
use cua_driver_testkit::e2e::{
    recording_evidence, write_declaration_from_env, write_result_from_env, CaseResult, CaseSpec,
    Delivery, DriverRoute, Observation, OracleKind, Scope, Targeting, TestStatus,
};
use cua_driver_testkit::{harness_app, spawn_in_job, Driver, McpDriver};
use serde_json::{json, Value};

const RAW_SESSION: &str = "centennial-cross-platform-continuity";

fn installed_driver() -> PathBuf {
    std::env::var_os("CUA_E2E_INSTALLED_DRIVER_BIN")
        .map(PathBuf::from)
        .expect("CUA_E2E_INSTALLED_DRIVER_BIN must identify the packaged local driver")
}

fn daemon_socket() -> String {
    std::env::var("CUA_E2E_HISTORY_DAEMON_SOCKET")
        .expect("CUA_E2E_HISTORY_DAEMON_SOCKET must identify the packaged daemon")
}

fn history_root() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        return PathBuf::from(std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA must be set"))
            .join("cua-driver-local/computer-history");
    }
    #[cfg(target_os = "linux")]
    {
        let state = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("HOME").expect("HOME must be set"))
                    .join(".local/state")
            });
        state.join("cua-driver-local/computer-history")
    }
}

fn history_cli(subcommand: &str, extra: &[&str]) -> Value {
    let mut command = Command::new(installed_driver());
    command.args(["history", subcommand]);
    command.args(extra);
    command.args(["--json", "--socket", &daemon_socket()]);
    command.env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "false");
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("could not run history {subcommand}: {error}"));
    assert!(
        output.status.success(),
        "history {subcommand} failed (status {:?}): stdout={} stderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "history {subcommand} did not emit JSON: {error}; stdout={}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

fn assert_daemon_remains_unrestricted() {
    let output = Command::new(installed_driver())
        .args(["status", "--socket", &daemon_socket()])
        .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "false")
        .output()
        .expect("inspect packaged daemon authorization mode");
    assert!(
        output.status.success(),
        "daemon status failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("permission mode: unrestricted"),
        "history enable changed daemon authorization mode: {stdout}"
    );
}

fn assert_ready(status: &Value) {
    assert_eq!(status["supported"], true, "history unsupported: {status}");
    assert_eq!(status["admitted"], true, "history not admitted: {status}");
    assert_eq!(status["enabled"], true, "history not enabled: {status}");
    assert_eq!(
        status["paused"], false,
        "history unexpectedly paused: {status}"
    );
    assert_eq!(
        status["encrypted"], true,
        "history is not encrypted: {status}"
    );
    assert_eq!(status["health"], "ready", "history is unhealthy: {status}");
    assert_eq!(
        status["dropped_events"], 0,
        "history dropped events: {status}"
    );
}

fn query(driver: &mut McpDriver, since_sequence: Option<u64>) -> Value {
    let mut arguments = json!({"limit": 200});
    if let Some(sequence) = since_sequence {
        arguments["since_sequence"] = json!(sequence);
    }
    let response = driver.call("history_query", arguments);
    assert!(
        !response.is_error(),
        "history_query failed: {} / {}",
        response.text(),
        response.raw
    );
    assert_eq!(response.structured()["metadata_only"], true);
    assert_eq!(response.structured()["model_context_disclosure"], true);
    response.structured().clone()
}

fn max_sequence(query: &Value) -> u64 {
    query["events"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|event| event["data"]["sequence"].as_u64())
        .max()
        .unwrap_or(0)
}

fn fixture_path() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        return harness_app("harness-electron", "CuaTestHarness.Electron.exe");
    }
    #[cfg(target_os = "linux")]
    {
        harness_app("harness-electron", "CuaTestHarness.Electron")
    }
}

fn launch_fixture(driver: &mut McpDriver) -> (u32, u64, Value) {
    let path = fixture_path();
    assert!(
        path.exists(),
        "Electron fixture is missing at {}",
        path.display()
    );
    let mut command = Command::new(path);
    command
        .args([
            "--no-sandbox",
            "--disable-gpu",
            "--force-renderer-accessibility",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let child = spawn_in_job(&mut command).expect("launch Electron fixture");
    let launch_pid = child.id();
    driver.reaper().push(child);

    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let windows = driver.call("list_windows", json!({"pid": launch_pid}));
        assert!(
            !windows.is_error(),
            "list_windows failed: {}",
            windows.text()
        );
        if let Some(window) = windows.structured()["windows"]
            .as_array()
            .and_then(|items| {
                items.iter().find(|window| {
                    window["title"]
                        .as_str()
                        .unwrap_or("")
                        .contains("CuaTestHarness Electron")
                })
            })
        {
            let pid = window["pid"].as_u64().unwrap_or(u64::from(launch_pid)) as u32;
            let window_id = window["window_id"].as_u64().expect("fixture window id");
            driver.reaper().track_pid(pid);
            return (pid, window_id, window["bounds"].clone());
        }
        assert!(
            Instant::now() < deadline,
            "Electron fixture opened no usable window"
        );
        sleep(Duration::from_millis(200));
    }
}

fn wait_for_window_position(
    driver: &mut McpDriver,
    pid: u32,
    window_id: u64,
    requested_x: f64,
    requested_y: f64,
) -> Value {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut last_bounds = None;
    loop {
        let readback = driver.call("list_windows", json!({"pid": pid}));
        assert!(
            !readback.is_error(),
            "independent list_windows readback failed: {}",
            readback.text()
        );
        if let Some(bounds) = readback.structured()["windows"]
            .as_array()
            .and_then(|windows| {
                windows
                    .iter()
                    .find(|window| window["window_id"].as_u64() == Some(window_id))
            })
            .map(|window| window["bounds"].clone())
        {
            let observed_x = bounds["x"].as_f64().expect("readback x");
            let observed_y = bounds["y"].as_f64().expect("readback y");
            if (observed_x - requested_x).abs() <= 2.0 && (observed_y - requested_y).abs() <= 2.0 {
                return bounds;
            }
            last_bounds = Some(bounds);
        }
        assert!(
            Instant::now() < deadline,
            "window frame did not settle within 2s: requested=({requested_x}, {requested_y}) observed={last_bounds:?}"
        );
        sleep(Duration::from_millis(50));
    }
}

fn wait_for_window_text(
    driver: &mut McpDriver,
    pid: u32,
    window_id: u64,
    expected: &str,
) -> cua_driver_testkit::ToolResponse {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let state = driver.call(
            "get_window_state",
            json!({
                "pid": pid,
                "window_id": window_id,
                "capture_mode": "ax"
            }),
        );
        assert!(
            !state.is_error(),
            "independent accessibility readback failed: {}",
            state.text()
        );
        if state.tree_text().contains(expected) {
            return state;
        }
        assert!(
            Instant::now() < deadline,
            "window state did not contain {expected:?}: {}",
            state.tree_text()
        );
        sleep(Duration::from_millis(50));
    }
}

fn assert_no_private_fields(value: &Value) {
    const FORBIDDEN_KEYS: &[&str] = &[
        "screenshot",
        "screenshot_png_b64",
        "typed_text",
        "clipboard",
        "raw_arguments",
        "raw_results",
        "accessibility_tree",
        "path",
        "title",
        "url",
        "diagnostic",
    ];
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                assert!(
                    !FORBIDDEN_KEYS.contains(&key.as_str()),
                    "history exposed forbidden field {key}"
                );
                assert_no_private_fields(child);
            }
        }
        Value::Array(items) => items.iter().for_each(assert_no_private_fields),
        _ => {}
    }
}

fn ciphertext_paths(root: &Path) -> Vec<PathBuf> {
    let chunks = root.join("chunks");
    if !chunks.exists() {
        return Vec::new();
    }
    fs::read_dir(chunks)
        .expect("read history chunk directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("cborseq"))
        .collect()
}

#[cfg(unix)]
fn forged_control_response() -> Value {
    use std::os::unix::net::UnixStream;

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match UnixStream::connect(daemon_socket()) {
            Ok(mut stream) => return write_forged_control(&mut stream),
            Err(error) if Instant::now() < deadline => {
                let _ = error;
                sleep(Duration::from_millis(50));
            }
            Err(error) => panic!("connect test process directly to packaged daemon: {error}"),
        }
    }
}

#[cfg(windows)]
fn forged_control_response() -> Value {
    use std::os::windows::fs::OpenOptionsExt;

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match fs::OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0x0000_0001 | 0x0000_0002)
            .open(daemon_socket())
        {
            Ok(mut pipe) => return write_forged_control(&mut pipe),
            Err(error) if Instant::now() < deadline => {
                let _ = error;
                sleep(Duration::from_millis(50));
            }
            Err(error) => panic!("connect test process directly to packaged daemon: {error}"),
        }
    }
}

fn write_forged_control(stream: &mut (impl std::io::Read + Write)) -> Value {
    let forged = json!({
        "method": "history_control",
        "args": {"operation": "status"},
        "observation_origin": "direct",
        "client_kind": "cli"
    });
    writeln!(stream, "{forged}").expect("write forged CLI control request");
    stream.flush().expect("flush forged CLI control request");
    let mut response = String::new();
    BufReader::new(stream)
        .read_line(&mut response)
        .expect("read forged CLI control response");
    serde_json::from_str(&response).expect("forged CLI response must be JSON")
}

fn assert_forged_cli_control_is_rejected() {
    let response = forged_control_response();
    assert_eq!(response["ok"], false, "forged CLI control was accepted");
    assert_eq!(response["exit_code"], 77);
    assert_eq!(response["error"], "history_control_requires_local_cli");
}

fn stop_daemon() {
    let output = Command::new(installed_driver())
        .args(["stop", "--socket", &daemon_socket()])
        .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "false")
        .output()
        .expect("stop packaged daemon");
    assert!(
        output.status.success(),
        "daemon stop failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

struct DaemonChild(Child);

impl Drop for DaemonChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start_daemon() -> DaemonChild {
    let child = Command::new(installed_driver())
        .args([
            "serve",
            "--socket",
            &daemon_socket(),
            "--permission-mode",
            "unrestricted",
            "--dangerously-bypass-approvals",
            "--experimental-history",
        ])
        .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "false")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("restart packaged daemon");
    let child = DaemonChild(child);
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if daemon_is_reachable() {
            return child;
        }
        assert!(
            Instant::now() < deadline,
            "restarted daemon never became reachable"
        );
        sleep(Duration::from_millis(100));
    }
}

fn daemon_is_reachable() -> bool {
    cua_driver_core::daemon::is_daemon_listening(&daemon_socket())
}

fn key_references() -> Vec<String> {
    #[cfg(target_os = "windows")]
    {
        platform_windows::history::WindowsCredentialKeyProvider
            .references("cua-driver-local")
            .expect("inspect Windows Credential Manager history key")
    }
    #[cfg(target_os = "linux")]
    {
        platform_linux::history::LinuxSecretServiceKeyProvider
            .references("cua-driver-local")
            .expect("inspect Secret Service history key")
    }
}

#[test]
#[ignore = "requires an installed local product, native credential store, GUI fixture, and daemon"]
fn encrypted_history_survives_restart_and_cryptographically_purges() {
    let started_at = Instant::now();
    let pure_wayland = cfg!(target_os = "linux")
        && std::env::var_os("WAYLAND_DISPLAY").is_some()
        && std::env::var_os("DISPLAY").is_none();
    let (case_action, case_route) = if pure_wayland {
        ("left_click", DriverRoute::LinuxAtSpiAction)
    } else {
        ("computer_history_continuity", DriverRoute::WindowState)
    };
    let case_oracles = if pure_wayland {
        vec![OracleKind::AxState, OracleKind::Protocol]
    } else {
        vec![OracleKind::Protocol]
    };
    let case = CaseSpec::delivered(
        format!("{}-computer-history-continuity", std::env::consts::OS),
        "electron",
        "electron",
        case_action,
        Targeting::Ax,
        Delivery::Foreground,
        Scope::Window,
        case_route,
        case_oracles.clone(),
    );
    write_declaration_from_env(&case).expect("write history E2E declaration");

    assert_forged_cli_control_is_rejected();
    eprintln!("[history-e2e] forged control denied");
    assert!(
        key_references().is_empty(),
        "runner preflight retained the history key"
    );
    eprintln!("[history-e2e] native key store starts empty");
    assert!(
        ciphertext_paths(&history_root()).is_empty(),
        "runner preflight retained history ciphertext"
    );

    eprintln!("[history-e2e] enabling preview");
    let enabled = history_cli("enable", &[]);
    assert_ready(&enabled);
    eprintln!("[history-e2e] preview enabled");
    assert_daemon_remains_unrestricted();
    eprintln!("[history-e2e] authorization mode preserved");
    assert_eq!(
        key_references().len(),
        1,
        "enable did not create one native key"
    );
    eprintln!("[history-e2e] native key created");

    let mut driver = McpDriver::spawn_daemon_proxy_named(
        &daemon_socket(),
        &format!("{}-computer-history-continuity", std::env::consts::OS),
    )
    .expect("start installed daemon proxy");
    eprintln!("[history-e2e] MCP proxy attached");
    let evidence = recording_evidence(driver.recording_dir());
    let status = driver.call("history_status", json!({}));
    assert!(
        !status.is_error(),
        "history_status failed: {}",
        status.text()
    );
    assert_ready(status.structured());
    let baseline = query(&mut driver, None);
    let since_sequence = max_sequence(&baseline).saturating_add(1).max(1);

    let started = driver.call("start_session", json!({"session": RAW_SESSION}));
    assert!(
        !started.is_error(),
        "start_session failed: {}",
        started.text()
    );
    let (pid, window_id, bounds) = launch_fixture(&mut driver);
    driver.start_behavior_recording();
    let (capability, expected_effect, expected_route) = if pure_wayland {
        let snapshot = driver.call(
            "get_window_state",
            json!({
                "pid": pid,
                "window_id": window_id,
                "capture_mode": "ax"
            }),
        );
        assert!(
            !snapshot.is_error(),
            "Wayland history snapshot failed: {}",
            snapshot.text()
        );
        assert!(
            snapshot.tree_text().contains("counter=0"),
            "Wayland fixture did not expose its initial counter state: {}",
            snapshot.tree_text()
        );
        let element_index = element_index_by_id(snapshot.tree_text(), "btn-increment")
            .or_else(|| element_index_containing(snapshot.tree_text(), "btn-increment"))
            .or_else(|| element_index_containing(snapshot.tree_text(), "Increment"))
            .expect("Wayland fixture exposed no accessible increment button");
        let clicked = driver.call(
            "click",
            json!({
                "pid": pid,
                "window_id": window_id,
                "element_index": element_index,
                "snapshot_id": snapshot.snapshot_id(),
                "delivery_mode": "foreground",
                "session": RAW_SESSION
            }),
        );
        assert!(
            !clicked.is_error(),
            "Wayland accessibility click failed: {}",
            clicked.text()
        );
        let effect = clicked
            .action_effect()
            .expect("Wayland click emitted no action effect");
        assert!(
            matches!(effect, "confirmed" | "unverifiable"),
            "Wayland click did not produce a delivered effect: {}",
            clicked.raw
        );
        let _settled_state = wait_for_window_text(&mut driver, pid, window_id, "counter=1");
        let capability = cua_driver_core::tool::default_capabilities_for("click")
            .into_iter()
            .next()
            .expect("click has no primary capability");
        assert_eq!(
            capability, "input.pointer.click",
            "history must use click's primary closed-contract capability"
        );
        (capability, effect.to_owned(), "accessibility")
    } else {
        let requested_x = bounds["x"].as_f64().expect("fixture x") + 18.0;
        let requested_y = bounds["y"].as_f64().expect("fixture y") + 12.0;
        let moved = driver.call(
            "set_window_frame",
            json!({
                "pid": pid,
                "window_id": window_id,
                "x": requested_x,
                "y": requested_y,
                "width": bounds["width"],
                "height": bounds["height"],
                "session": RAW_SESSION
            }),
        );
        assert!(
            !moved.is_error(),
            "set_window_frame failed: {}",
            moved.text()
        );
        assert_eq!(moved.action_effect(), Some("confirmed"));
        let _settled_bounds =
            wait_for_window_position(&mut driver, pid, window_id, requested_x, requested_y);
        (
            "window.frame.set".to_owned(),
            "confirmed".to_owned(),
            "system_api",
        )
    };
    let ended = driver.call("end_session", json!({"session": RAW_SESSION}));
    assert!(!ended.is_error(), "end_session failed: {}", ended.text());
    assert_ready(&history_cli("flush", &[]));

    let hydrated = query(&mut driver, Some(since_sequence));
    assert_no_private_fields(&hydrated);
    let completion = hydrated["events"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|event| {
            event["data"]["capability"].as_str() == Some(capability.as_str())
                && event["data"]["payload"]["kind"] == "action_completed"
                && event["data"]["payload"]["effect"].as_str() == Some(expected_effect.as_str())
                && event["data"]["payload"]["route"] == expected_route
        })
        .expect("history did not contain the delivered platform action");
    let action_id = completion["data"]["action_id"]
        .as_str()
        .expect("history action has no opaque id")
        .to_owned();
    assert_ne!(completion["data"]["session_id"], RAW_SESSION);
    let application = &completion["data"]["application"];
    assert!(
        application["bundle_id"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
            || application["display_name"]
                .as_str()
                .is_some_and(|value| !value.is_empty()),
        "history action has no fixed-field application identity: {completion}"
    );

    let ciphertext = ciphertext_paths(&history_root());
    assert!(
        !ciphertext.is_empty(),
        "history produced no encrypted chunks"
    );
    for path in ciphertext {
        let bytes = fs::read(&path).expect("read encrypted history chunk");
        for forbidden in [RAW_SESSION, capability.as_str(), "action_completed"] {
            assert!(
                !bytes
                    .windows(forbidden.len())
                    .any(|window| window == forbidden.as_bytes()),
                "plaintext marker {forbidden:?} appeared in {}",
                path.display()
            );
        }
    }
    drop(driver);

    stop_daemon();
    let restarted = start_daemon();
    let mut driver = McpDriver::spawn_daemon_proxy_unrecorded(&daemon_socket())
        .expect("start restarted daemon proxy");
    let status = driver.call("history_status", json!({}));
    assert_ready(status.structured());
    let reopened = query(&mut driver, None);
    assert!(
        reopened["events"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|event| {
                event["data"]["action_id"].as_str() == Some(&action_id)
                    && event["data"]["capability"].as_str() == Some(capability.as_str())
                    && event["data"]["payload"]["kind"] == "action_completed"
                    && event["data"]["payload"]["effect"].as_str() == Some(expected_effect.as_str())
                    && event["data"]["payload"]["route"] == expected_route
            }),
        "restarted daemon could not hydrate the recorded action"
    );
    drop(driver);

    let disabled = history_cli("disable", &[]);
    assert_eq!(disabled["enabled"], false);
    assert!(disabled["bytes_used"].as_u64().unwrap_or(0) > 0);
    let preserved = history_cli("list", &["200"]);
    assert!(
        preserved["events"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|event| { event["data"]["action_id"].as_str() == Some(&action_id) }),
        "disable unexpectedly removed stored history"
    );
    let deleted = history_cli("delete", &["--yes"]);
    assert_eq!(deleted["bytes_used"], 0);
    assert!(
        ciphertext_paths(&history_root()).is_empty(),
        "delete retained ciphertext"
    );
    assert!(
        key_references().is_empty(),
        "delete retained the native encryption key"
    );
    drop(restarted);

    let observation = Observation::delivered(case_oracles, evidence);
    let result = CaseResult::evaluate(case, observation, started_at.elapsed());
    write_result_from_env(&result).expect("write history E2E result");
    assert_eq!(result.test_status, TestStatus::Pass, "{}", result.message);
}
