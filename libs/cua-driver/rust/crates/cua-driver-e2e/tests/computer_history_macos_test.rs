//! Packaged macOS lifecycle gate for the encrypted Computer History preview.
//!
//! The canonical Lume runner executes these two ignored tests around a real
//! daemon restart. The first test records one verified Cua-mediated action and
//! writes only opaque continuity evidence. The second proves that the fresh
//! daemon can hydrate from the encrypted store, then proves disable/preserve
//! and cryptographic deletion semantics.

#![cfg(target_os = "macos")]

use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::Command,
    thread::sleep,
    time::{Duration, Instant},
};

use cua_driver_testkit::{Driver, McpDriver};
use serde_json::{json, Value};

const CHESS_BUNDLE: &str = "com.apple.Chess";
const RAW_SESSION: &str = "centennial-lume-continuity";
const HISTORY_KEYCHAIN_SERVICE: &str = "com.trycua.cua-driver-local.computer-history.v1";
const HISTORY_KEYCHAIN_ACCOUNT: &str = "namespace-root-key-v1";

fn installed_driver() -> PathBuf {
    std::env::var_os("CUA_E2E_INSTALLED_DRIVER_BIN")
        .map(PathBuf::from)
        .expect("CUA_E2E_INSTALLED_DRIVER_BIN must identify the packaged local driver")
}

fn daemon_socket() -> String {
    std::env::var("CUA_E2E_MACOS_DAEMON_SOCKET")
        .expect("CUA_E2E_MACOS_DAEMON_SOCKET must identify the packaged daemon")
}

fn marker_path() -> PathBuf {
    std::env::var_os("CUA_E2E_HISTORY_MARKER")
        .map(PathBuf::from)
        .expect("CUA_E2E_HISTORY_MARKER must identify the run-owned continuity marker")
}

fn history_root() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").expect("HOME must be set"))
        .join("Library/Application Support/cua-driver-local/computer-history")
}

fn history_cli(subcommand: &str, extra: &[&str]) -> Value {
    let mut command = Command::new(installed_driver());
    command.args(["history", subcommand]);
    command.args(extra);
    command.args(["--json", "--socket", &daemon_socket()]);
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

fn assert_forged_cli_control_is_rejected() {
    let mut stream = UnixStream::connect(daemon_socket())
        .expect("connect test process directly to packaged daemon");
    let forged = json!({
        "method": "history_control",
        "args": {"operation": "status"},
        "observation_origin": "direct",
        "client_kind": "cli"
    });
    writeln!(stream, "{forged}").expect("write forged CLI control request");
    let mut response = String::new();
    BufReader::new(stream)
        .read_line(&mut response)
        .expect("read forged CLI control response");
    let response: Value =
        serde_json::from_str(&response).expect("forged CLI response must be JSON");
    assert_eq!(response["ok"], false, "forged CLI control was accepted");
    assert_eq!(response["exit_code"], 77);
    assert_eq!(response["error"], "history_control_requires_local_cli");
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
        "history dropped events before the action: {status}"
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

fn chess_window(driver: &mut McpDriver) -> (u64, u64, Value) {
    let launch = driver.call("launch_app", json!({"bundle_id": CHESS_BUNDLE}));
    assert!(
        !launch.is_error(),
        "could not launch Chess: {}",
        launch.text()
    );
    let pid = launch.structured()["pid"]
        .as_u64()
        .expect("Chess launch returned no pid");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let listed = driver.call("list_windows", json!({"pid": pid}));
        assert!(!listed.is_error(), "list_windows failed: {}", listed.text());
        if let Some(window) = listed.structured()["windows"]
            .as_array()
            .and_then(|windows| {
                windows.iter().find(|window| {
                    window["is_on_screen"].as_bool().unwrap_or(false)
                        && window["bounds"]["width"].as_f64().unwrap_or(0.0) > 100.0
                        && window["bounds"]["height"].as_f64().unwrap_or(0.0) > 100.0
                })
            })
        {
            return (
                pid,
                window["window_id"]
                    .as_u64()
                    .expect("Chess window has no id"),
                window["bounds"].clone(),
            );
        }
        assert!(
            Instant::now() < deadline,
            "Chess opened no usable on-screen window"
        );
        sleep(Duration::from_millis(200));
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

#[test]
#[ignore = "requires the signed, TCC-authorized packaged daemon in the canonical Lume runner"]
fn history_records_agent_action_before_restart() {
    assert_forged_cli_control_is_rejected();

    let deleted = history_cli("delete", &["--yes"]);
    assert_eq!(deleted["enabled"], false, "initial purge failed: {deleted}");
    assert_eq!(
        deleted["bytes_used"], 0,
        "initial purge retained ciphertext"
    );

    let enabled = history_cli("enable", &[]);
    assert_ready(&enabled);

    let mut driver = McpDriver::spawn_macos_daemon_proxy_named("macos-history-before-restart")
        .expect("start installed macOS daemon proxy");
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

    let (pid, window_id, bounds) = chess_window(&mut driver);
    let x = bounds["x"].as_f64().expect("Chess window x");
    let y = bounds["y"].as_f64().expect("Chess window y");
    let width = bounds["width"].as_f64().expect("Chess window width");
    let height = bounds["height"].as_f64().expect("Chess window height");
    let requested_x = x + 18.0;
    let requested_y = y + 12.0;
    let moved = driver.call(
        "set_window_frame",
        json!({
            "pid": pid,
            "window_id": window_id,
            "x": requested_x,
            "y": requested_y,
            "width": width,
            "height": height,
            "session": RAW_SESSION
        }),
    );
    assert!(
        !moved.is_error(),
        "set_window_frame failed: {} / {}",
        moved.text(),
        moved.raw
    );
    assert_eq!(moved.action_effect(), Some("confirmed"));
    assert_eq!(moved.action_route(), Some("accessibility"));

    let readback = driver.call("list_windows", json!({"pid": pid}));
    let observed = readback.structured()["windows"]
        .as_array()
        .and_then(|windows| {
            windows
                .iter()
                .find(|window| window["window_id"].as_u64() == Some(window_id))
        })
        .expect("moved Chess window disappeared");
    assert!((observed["bounds"]["x"].as_f64().unwrap() - requested_x).abs() <= 2.0);
    assert!((observed["bounds"]["y"].as_f64().unwrap() - requested_y).abs() <= 2.0);

    let ended = driver.call("end_session", json!({"session": RAW_SESSION}));
    assert!(!ended.is_error(), "end_session failed: {}", ended.text());
    let flushed = history_cli("flush", &[]);
    assert_ready(&flushed);

    let hydrated = query(&mut driver, Some(since_sequence));
    assert_no_private_fields(&hydrated);
    let events = hydrated["events"].as_array().expect("history events array");
    let completion = events
        .iter()
        .find(|event| {
            event["data"]["capability"] == "window.frame.set"
                && event["data"]["payload"]["kind"] == "action_completed"
                && event["data"]["payload"]["effect"] == "confirmed"
                && event["data"]["payload"]["route"] == "accessibility"
        })
        .expect("history did not contain the confirmed window-frame action");
    let action_id = completion["data"]["action_id"]
        .as_str()
        .expect("history action has no opaque action id");
    let opaque_session_id = completion["data"]["session_id"]
        .as_str()
        .expect("history action has no opaque session id");
    assert_ne!(
        opaque_session_id, RAW_SESSION,
        "raw session id entered history"
    );
    assert_eq!(completion["data"]["application"]["bundle_id"], CHESS_BUNDLE);
    assert_eq!(completion["data"]["application"]["display_name"], "Chess");

    let marker = json!({
        "schema": "cua-driver/history-continuity-evidence@v1",
        "action_id": action_id,
        "session_id": opaque_session_id,
        "capability": "window.frame.set",
        "last_sequence": max_sequence(&hydrated)
    });
    fs::write(
        marker_path(),
        serde_json::to_vec_pretty(&marker).expect("serialize continuity marker"),
    )
    .expect("write continuity marker");

    let ciphertext = ciphertext_paths(&history_root());
    assert!(
        !ciphertext.is_empty(),
        "history produced no encrypted chunks"
    );
    for path in ciphertext {
        let bytes = fs::read(&path).expect("read encrypted history chunk");
        for forbidden in [
            RAW_SESSION,
            "Chess",
            CHESS_BUNDLE,
            "window.frame.set",
            "action_completed",
        ] {
            assert!(
                !bytes
                    .windows(forbidden.len())
                    .any(|window| window == forbidden.as_bytes()),
                "plaintext marker {forbidden:?} appeared in {}",
                path.display()
            );
        }
    }

    let final_status = driver.call("history_status", json!({}));
    assert_ready(final_status.structured());
}

#[test]
#[ignore = "requires the daemon restart performed by the canonical Lume runner"]
fn history_reopens_after_restart_and_cryptographically_purges() {
    let marker: Value = serde_json::from_slice(
        &fs::read(marker_path()).expect("continuity marker from pre-restart test is missing"),
    )
    .expect("continuity marker is invalid JSON");
    let action_id = marker["action_id"].as_str().expect("marker action id");

    let mut driver = McpDriver::spawn_macos_daemon_proxy_named("macos-history-after-restart")
        .expect("start restarted installed macOS daemon proxy");
    let status = driver.call("history_status", json!({}));
    assert!(
        !status.is_error(),
        "history_status failed: {}",
        status.text()
    );
    assert_ready(status.structured());

    let hydrated = query(&mut driver, None);
    assert_no_private_fields(&hydrated);
    assert!(
        hydrated["events"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|event| {
                event["data"]["action_id"].as_str() == Some(action_id)
                    && event["data"]["payload"]["kind"] == "action_completed"
                    && event["data"]["capability"] == "window.frame.set"
            }),
        "restarted daemon could not hydrate the recorded action"
    );

    let disabled = history_cli("disable", &[]);
    assert_eq!(disabled["enabled"], false, "disable did not stop capture");
    assert_eq!(disabled["health"], "disabled");
    assert!(disabled["bytes_used"].as_u64().unwrap_or(0) > 0);

    let preserved = history_cli("list", &["200"]);
    assert!(
        preserved["events"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|event| event["data"]["action_id"].as_str() == Some(action_id)),
        "disable unexpectedly removed stored history"
    );

    let deleted = history_cli("delete", &["--yes"]);
    assert_eq!(deleted["enabled"], false);
    assert_eq!(deleted["bytes_used"], 0, "delete retained encrypted chunks");
    assert!(
        ciphertext_paths(&history_root()).is_empty(),
        "delete retained encrypted history files"
    );

    let key_lookup = Command::new("/usr/bin/security")
        .args([
            "find-generic-password",
            "-s",
            HISTORY_KEYCHAIN_SERVICE,
            "-a",
            HISTORY_KEYCHAIN_ACCOUNT,
        ])
        .output()
        .expect("could not inspect the history Keychain item");
    assert!(
        !key_lookup.status.success(),
        "history encryption key remained after delete"
    );
    let key_error = String::from_utf8_lossy(&key_lookup.stderr);
    assert!(
        key_error.contains("could not be found") || key_error.contains("-25300"),
        "unexpected Keychain lookup result after delete: {key_error}"
    );
}
