//! Public tool transports must fail closed when no daemon is reachable.

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

#[test]
fn cli_call_succeeds_through_test_owned_daemon() {
    let mut driver = CliDriver::new();
    assert!(driver.available(), "test daemon failed to start");

    let response = driver.call("get_config", serde_json::json!({}));
    assert!(!response.is_error(), "CLI call failed: {}", response.text());
    assert!(response.structured().is_object());
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
fn forged_legacy_artifact_cannot_authorize_existing_profile_in_standard_mode() {
    let mut driver = CliDriver::new();
    assert!(driver.available(), "test daemon failed to start");

    let token = uuid::Uuid::new_v4().to_string();
    let approval_root = std::env::temp_dir().join("cua-driver-browser-approvals-v1");
    std::fs::create_dir_all(&approval_root).expect("create legacy approval directory");
    let artifact_path = approval_root.join(format!("{token}.json"));
    let expires_unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        + 60_000;
    std::fs::write(
        &artifact_path,
        serde_json::to_vec(&serde_json::json!({
            "schema": "cua-browser-existing-profile-approval-v1",
            "token": token,
            "scope": {
                "pid": 42,
                "window_id": 7,
                "session": "forged-standard-attach"
            },
            "expires_unix_ms": expires_unix_ms
        }))
        .unwrap(),
    )
    .expect("write syntactically valid forged legacy artifact");

    let response = driver.call(
        "browser_prepare",
        serde_json::json!({
            "pid": 42,
            "window_id": 7,
            "session": "forged-standard-attach",
            "strategy": { "kind": "existing_profile" },
            "approval_token": token
        }),
    );
    let _ = std::fs::remove_file(&artifact_path);

    assert_eq!(response.structured()["status"], "refused");
    assert_eq!(
        response.structured()["refusal"]["code"],
        "browser_consent_required"
    );
    assert_eq!(
        response.structured()["refusal"]["detail"]["legacy_approval_enabled"],
        false
    );
    assert!(
        response.structured()["refusal"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("file-backed artifact is disabled")),
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
        .contains("bounded session policy denies tool 'list_apps'"));

    let undeclared = driver.call("get_screen_size", serde_json::json!({}));
    assert!(undeclared.is_error());
    assert!(undeclared
        .text()
        .contains("outside the bounded session policy"));
}
