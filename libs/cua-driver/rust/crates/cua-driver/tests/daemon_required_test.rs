//! Public tool transports must fail closed when no daemon is reachable.

#[cfg(not(target_os = "macos"))]
use std::io::Write as _;
use std::process::Command;

use cua_driver_testkit::{CliDriver, Driver};

fn missing_socket() -> (String, Option<tempfile::TempDir>) {
    #[cfg(unix)]
    {
        let directory = tempfile::Builder::new()
            .prefix("cua-missing-")
            .tempdir_in("/tmp")
            .expect("temporary socket directory");
        let socket = directory.path().join("missing.sock").display().to_string();
        (socket, Some(directory))
    }
    #[cfg(target_os = "windows")]
    {
        (
            format!(r"\\.\pipe\cua-driver-missing-{}", std::process::id()),
            None,
        )
    }
    #[cfg(not(any(unix, target_os = "windows")))]
    {
        (format!("cua-driver-missing-{}", std::process::id()), None)
    }
}

#[test]
fn cli_call_does_not_execute_without_daemon() {
    let (socket, _directory) = missing_socket();
    let output = Command::new(env!("CARGO_BIN_EXE_cua-driver"))
        .args(["call", "list_apps", "--socket", &socket])
        .output()
        .expect("run cua-driver call");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("daemon is not running"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn embedded_mcp_does_not_fall_back_without_daemon() {
    let (socket, _directory) = missing_socket();
    let output = Command::new(env!("CARGO_BIN_EXE_cua-driver"))
        .args(["mcp", "--embedded", "--socket", &socket])
        .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "false")
        .output()
        .expect("run cua-driver mcp");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no Cua Driver daemon listening"),
        "unexpected stderr: {stderr}"
    );
}

#[cfg(not(target_os = "macos"))]
#[test]
fn default_mcp_owns_a_runtime_without_a_daemon() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_cua-driver"))
        .arg("mcp")
        .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "false")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn direct MCP");
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "clientInfo": {"name": "direct-runtime-test", "version": "0"}
        }
    });
    writeln!(
        child.stdin.as_mut().expect("direct MCP stdin"),
        "{}",
        request
    )
    .expect("write initialize");
    drop(child.stdin.take());
    let output = child.wait_with_output().expect("wait for direct MCP EOF");
    assert!(
        output.status.success(),
        "direct MCP failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("initialize response JSON");
    assert_eq!(response["result"]["serverInfo"]["name"], "cua-driver");
}

#[cfg(not(target_os = "macos"))]
#[test]
fn direct_mcp_rejects_admin_disabled_unrestricted_mode_before_requests() {
    let output = Command::new(env!("CARGO_BIN_EXE_cua-driver"))
        .arg("mcp")
        .env("CUA_DRIVER_PERMISSION_MODE", "unrestricted")
        .env("CUA_DRIVER_DANGEROUSLY_BYPASS_APPROVALS", "1")
        .env("CUA_DRIVER_DISABLE_UNRESTRICTED", "1")
        .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "false")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run direct MCP with managed lock");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unrestricted is disabled"),
        "unexpected stderr: {stderr}"
    );
    assert!(
        output.stdout.is_empty(),
        "authorization rejection must precede protocol output"
    );
}

#[test]
fn embedded_mcp_without_private_endpoint_fails_closed() {
    let output = Command::new(env!("CARGO_BIN_EXE_cua-driver"))
        .args(["mcp", "--embedded"])
        .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "false")
        .output()
        .expect("run embedded MCP without endpoint");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("must provide their private service endpoint"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn cli_call_succeeds_through_test_owned_daemon() {
    let mut driver = CliDriver::new();
    assert!(driver.available(), "test daemon failed to start");

    let response = driver.call("get_config", serde_json::json!({}));
    assert!(!response.is_error(), "CLI call failed: {}", response.text());
    assert!(response.structured().is_object());
}

#[test]
fn named_session_survives_across_one_shot_cli_calls() {
    let mut driver = CliDriver::new();
    assert!(driver.available(), "test daemon failed to start");
    let session = format!("synthetic-cli-lifecycle-{}", std::process::id());

    let started = driver.call(
        "start_session",
        serde_json::json!({"session": session, "capture_scope": "window"}),
    );
    assert!(!started.is_error(), "start failed: {}", started.text());

    let state = driver.call("get_session_state", serde_json::json!({"session": session}));
    assert!(
        !state.is_error(),
        "named session did not survive the next CLI process: {}",
        state.text()
    );
    assert_eq!(state.structured()["session"], session);

    let sessions = Command::new(env!("CARGO_BIN_EXE_cua-driver"))
        .args([
            "sessions",
            "list",
            "--json",
            "--socket",
            driver.daemon_socket().expect("test daemon socket"),
        ])
        .output()
        .expect("list live sessions");
    assert!(
        sessions.status.success(),
        "session list failed: {}",
        String::from_utf8_lossy(&sessions.stderr)
    );
    let sessions: serde_json::Value =
        serde_json::from_slice(&sessions.stdout).expect("session list JSON");
    assert_eq!(sessions["count"], 1);

    let ended = driver.call("end_session", serde_json::json!({"session": session}));
    assert!(!ended.is_error(), "end failed: {}", ended.text());

    let sessions = Command::new(env!("CARGO_BIN_EXE_cua-driver"))
        .args([
            "sessions",
            "list",
            "--json",
            "--socket",
            driver.daemon_socket().expect("test daemon socket"),
        ])
        .output()
        .expect("list ended sessions");
    assert!(
        sessions.status.success(),
        "session list failed: {}",
        String::from_utf8_lossy(&sessions.stderr)
    );
    let sessions: serde_json::Value =
        serde_json::from_slice(&sessions.stdout).expect("session list JSON");
    assert_eq!(sessions["count"], 0);
}

#[test]
fn implicitly_started_named_session_survives_across_one_shot_cli_calls() {
    let mut driver = CliDriver::new();
    assert!(driver.available(), "test daemon failed to start");
    let session = format!("synthetic-cli-implicit-{}", std::process::id());

    let first_action = driver.call("get_config", serde_json::json!({"session": session}));
    assert!(
        !first_action.is_error(),
        "implicit first action failed: {}",
        first_action.text()
    );

    let state = driver.call("get_session_state", serde_json::json!({"session": session}));
    assert!(
        !state.is_error(),
        "implicitly started session did not survive the next CLI process: {}",
        state.text()
    );
    assert_eq!(state.structured()["session"], session);

    let ended = driver.call("end_session", serde_json::json!({"session": session}));
    assert!(!ended.is_error(), "end failed: {}", ended.text());
}

#[test]
fn named_cli_session_cleanup_is_isolated() {
    let mut driver = CliDriver::new();
    assert!(driver.available(), "test daemon failed to start");
    let first = format!("synthetic-cli-isolation-a-{}", std::process::id());
    let second = format!("synthetic-cli-isolation-b-{}", std::process::id());

    for session in [&first, &second] {
        let started = driver.call(
            "start_session",
            serde_json::json!({"session": session, "capture_scope": "window"}),
        );
        assert!(!started.is_error(), "start failed: {}", started.text());
    }

    let ended = driver.call("end_session", serde_json::json!({"session": first}));
    assert!(!ended.is_error(), "first end failed: {}", ended.text());

    let anonymous = driver.call("get_config", serde_json::json!({}));
    assert!(
        !anonymous.is_error(),
        "anonymous one-shot call failed: {}",
        anonymous.text()
    );

    let state = driver.call("get_session_state", serde_json::json!({"session": second}));
    assert!(
        !state.is_error(),
        "ending another named session or cleaning an anonymous call ended the survivor: {}",
        state.text()
    );
    assert_eq!(state.structured()["session"], second);

    let ended = driver.call("end_session", serde_json::json!({"session": second}));
    assert!(!ended.is_error(), "second end failed: {}", ended.text());
}

#[test]
fn revoke_cli_ends_the_exact_live_session() {
    let mut driver = CliDriver::new();
    assert!(driver.available(), "test daemon failed to start");
    let session = format!("revoke-cli-{}", std::process::id());
    let started = driver.call(
        "start_session",
        serde_json::json!({"session": session, "capture_scope": "window"}),
    );
    assert!(!started.is_error(), "start failed: {}", started.text());

    let output = Command::new(env!("CARGO_BIN_EXE_cua-driver"))
        .args([
            "revoke",
            "--session",
            &session,
            "--socket",
            driver.daemon_socket().expect("test daemon socket"),
        ])
        .output()
        .expect("run authorization revoke command");
    assert!(
        output.status.success(),
        "revoke failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let state = driver.call("get_session_state", serde_json::json!({"session": session}));
    assert!(state.is_error());
}

#[test]
fn standard_mode_refuses_existing_profile_without_a_launch_grant() {
    let mut driver = CliDriver::new();
    assert!(driver.available(), "test daemon failed to start");

    let response = driver.call(
        "browser_prepare",
        serde_json::json!({
            "pid": 42,
            "window_id": 7,
            "session": "forged-standard-attach",
            "strategy": { "kind": "existing_profile" }
        }),
    );

    assert_eq!(response.structured()["status"], "refused");
    assert_eq!(
        response.structured()["refusal"]["code"],
        "browser_consent_required"
    );
    assert!(
        response.structured()["refusal"]["message"]
            .as_str()
            .is_some_and(|message| {
                message.contains("--grant existing-profile")
                    && message.contains("embedding authorization host")
            }),
        "unexpected refusal: {}",
        response.structured()
    );
}

#[test]
fn unrestricted_mode_skips_runtime_existing_profile_consent() {
    let mut driver = CliDriver::with_daemon_env(&[
        ("CUA_DRIVER_PERMISSION_MODE", "unrestricted"),
        ("CUA_DRIVER_DANGEROUSLY_BYPASS_APPROVALS", "1"),
    ]);
    assert!(
        driver.available(),
        "unrestricted test daemon failed to start"
    );

    let response = driver.call(
        "browser_prepare",
        serde_json::json!({
            "pid": i64::MAX,
            "window_id": 7,
            "session": "unrestricted-attach",
            "strategy": { "kind": "existing_profile" }
        }),
    );

    assert_eq!(response.structured()["status"], "refused");
    assert_ne!(
        response.structured()["refusal"]["code"],
        "browser_consent_required",
        "unrestricted attach unexpectedly requested runtime consent: {}",
        response.structured()
    );
}

#[test]
fn bounded_manifest_is_an_immutable_deny_by_default_layer() {
    let directory = tempfile::tempdir().expect("temporary bounded policy directory");
    let policy_path = directory.path().join("bounded.yaml");
    std::fs::write(
        &policy_path,
        r#"
version: 1
mode: bounded
expires_after: 1h
idle_timeout: 10m
resources: {}
allow:
  tools: [get_config]
deny:
  tools: [list_apps]
"#,
    )
    .expect("write bounded policy");
    let policy = policy_path.display().to_string();
    let mut driver = CliDriver::with_daemon_env(&[
        ("CUA_DRIVER_PERMISSION_MODE", "bounded"),
        ("CUA_DRIVER_SESSION_POLICY_FILE", &policy),
        ("CUA_DRIVER_SESSION_POLICY_APPROVED", "1"),
    ]);
    assert!(driver.available(), "bounded test daemon failed to start");

    let allowed = driver.call("get_config", serde_json::json!({}));
    assert!(
        !allowed.is_error(),
        "allowed call failed: {}",
        allowed.text()
    );

    let denied = driver.call("list_apps", serde_json::json!({}));
    assert!(denied.is_error());
    assert!(denied
        .text()
        .contains("capability manifest denies tool 'list_apps'"));

    let undeclared = driver.call("get_screen_size", serde_json::json!({}));
    assert!(undeclared.is_error());
    assert!(undeclared
        .text()
        .contains("outside the capability manifest"));
}

#[test]
fn capability_manifest_narrows_standard_mode() {
    let directory = tempfile::tempdir().expect("temporary capability manifest directory");
    let manifest_path = directory.path().join("standard-v3.yaml");
    std::fs::write(
        &manifest_path,
        "version: 3\nallow:\n  tools: [get_config]\ndeny:\n  tools: [list_apps]\n",
    )
    .expect("write standard capability manifest");
    let manifest = manifest_path.display().to_string();
    let mut driver = CliDriver::with_daemon_env(&[
        ("CUA_DRIVER_PERMISSION_MODE", "standard"),
        ("CUA_DRIVER_CAPABILITY_MANIFEST_FILE", &manifest),
        ("CUA_DRIVER_CAPABILITY_MANIFEST_APPROVED", "1"),
    ]);
    assert!(
        driver.available(),
        "standard manifest daemon failed to start"
    );
    assert!(!driver.call("get_config", serde_json::json!({})).is_error());
    assert!(driver.call("list_apps", serde_json::json!({})).is_error());
    assert!(driver
        .call("get_screen_size", serde_json::json!({}))
        .is_error());
}

#[test]
fn capability_manifest_narrows_unrestricted_mode() {
    let directory = tempfile::tempdir().expect("temporary capability manifest directory");
    let manifest_path = directory.path().join("unrestricted-v3.yaml");
    std::fs::write(
        &manifest_path,
        "version: 3\nallow:\n  tools: [get_config]\ndeny:\n  tools: [list_apps]\n",
    )
    .expect("write unrestricted capability manifest");
    let manifest = manifest_path.display().to_string();
    let mut driver = CliDriver::with_daemon_env(&[
        ("CUA_DRIVER_PERMISSION_MODE", "unrestricted"),
        ("CUA_DRIVER_DANGEROUSLY_BYPASS_APPROVALS", "1"),
        ("CUA_DRIVER_CAPABILITY_MANIFEST_FILE", &manifest),
        ("CUA_DRIVER_CAPABILITY_MANIFEST_APPROVED", "1"),
    ]);
    assert!(
        driver.available(),
        "unrestricted manifest daemon failed to start"
    );
    assert!(!driver.call("get_config", serde_json::json!({})).is_error());
    assert!(driver.call("list_apps", serde_json::json!({})).is_error());
    assert!(driver
        .call("get_screen_size", serde_json::json!({}))
        .is_error());
}
