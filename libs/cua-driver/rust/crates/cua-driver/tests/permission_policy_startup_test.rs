//! Explicit permission-policy configuration must fail before a daemon exposes
//! an action endpoint.

use std::process::Command;

fn test_socket(directory: &tempfile::TempDir, suffix: &str) -> String {
    #[cfg(unix)]
    {
        directory
            .path()
            .join(format!("{suffix}.sock"))
            .display()
            .to_string()
    }
    #[cfg(target_os = "windows")]
    {
        format!(r"\\.\pipe\cua-driver-{suffix}-{}", std::process::id())
    }
}

fn rejected_serve(socket: &str, extra_args: &[&str]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cua-driver"));
    command.args([
        "serve",
        "--socket",
        socket,
        "--no-overlay",
        "--no-permissions-gate",
    ]);
    command.args(extra_args);
    command
        .env("CUA_DRIVER_RS_PERMISSIONS_GATE", "0")
        .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "false")
        .output()
        .expect("run rejected cua-driver serve configuration")
}

#[test]
fn missing_configured_policy_prevents_daemon_startup() {
    let directory = tempfile::tempdir().expect("temporary policy test directory");
    let missing_policy = directory.path().join("missing-policy.yaml");

    let socket = test_socket(&directory, "missing-policy");
    let output = Command::new(env!("CARGO_BIN_EXE_cua-driver"))
        .args([
            "serve",
            "--socket",
            &socket,
            "--no-overlay",
            "--no-permissions-gate",
        ])
        .env("CUA_DRIVER_POLICY_FILE", &missing_policy)
        .env("CUA_DRIVER_RS_PERMISSIONS_GATE", "0")
        .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "false")
        .output()
        .expect("run cua-driver serve with a missing configured policy");

    assert!(
        !output.status.success(),
        "daemon unexpectedly started with a missing configured policy"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("configured permission policy path does not exist"),
        "unexpected stderr: {stderr}"
    );

    #[cfg(unix)]
    assert!(
        !std::path::Path::new(&socket).exists(),
        "daemon bound its socket before rejecting the configured policy"
    );
}

#[test]
fn missing_managed_policy_prevents_daemon_startup() {
    let directory = tempfile::tempdir().expect("temporary managed policy test directory");
    let missing_policy = directory.path().join("missing-managed-policy.yaml");
    let socket = test_socket(&directory, "missing-managed-policy");
    let output = Command::new(env!("CARGO_BIN_EXE_cua-driver"))
        .args([
            "serve",
            "--socket",
            &socket,
            "--no-overlay",
            "--no-permissions-gate",
        ])
        .env("CUA_DRIVER_MANAGED_POLICY_FILE", &missing_policy)
        .env("CUA_DRIVER_RS_PERMISSIONS_GATE", "0")
        .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "false")
        .output()
        .expect("run cua-driver serve with a missing managed policy");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("managed policy")
            && stderr.contains("configured permission policy path does not exist"),
        "unexpected stderr: {stderr}"
    );
    #[cfg(unix)]
    assert!(!std::path::Path::new(&socket).exists());
}

#[test]
fn unrestricted_mode_requires_the_danger_acknowledgement_before_bind() {
    let directory = tempfile::tempdir().expect("temporary mode test directory");
    let socket = test_socket(&directory, "unrestricted-no-ack");
    let output = rejected_serve(&socket, &["--permission-mode", "unrestricted"]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("requires --dangerously-bypass-approvals"),
        "unexpected stderr: {stderr}"
    );
    #[cfg(unix)]
    assert!(!std::path::Path::new(&socket).exists());
}

#[test]
fn danger_acknowledgement_cannot_weaken_standard_mode() {
    let directory = tempfile::tempdir().expect("temporary mode test directory");
    let socket = test_socket(&directory, "standard-danger-ack");
    let output = rejected_serve(
        &socket,
        &[
            "--permission-mode",
            "standard",
            "--dangerously-bypass-approvals",
        ],
    );

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("valid only with --permission-mode unrestricted"),
        "unexpected stderr: {stderr}"
    );
    #[cfg(unix)]
    assert!(!std::path::Path::new(&socket).exists());
}

#[test]
fn managed_configuration_can_disable_unrestricted_before_bind() {
    let directory = tempfile::tempdir().expect("temporary mode test directory");
    let socket = test_socket(&directory, "unrestricted-admin-disabled");
    let output = Command::new(env!("CARGO_BIN_EXE_cua-driver"))
        .args([
            "serve",
            "--socket",
            &socket,
            "--no-overlay",
            "--no-permissions-gate",
            "--permission-mode",
            "unrestricted",
            "--dangerously-bypass-approvals",
        ])
        .env("CUA_DRIVER_DISABLE_UNRESTRICTED", "1")
        .env("CUA_DRIVER_RS_PERMISSIONS_GATE", "0")
        .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "false")
        .output()
        .expect("run administratively disabled unrestricted mode");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("unrestricted is disabled by managed startup configuration"));
    #[cfg(unix)]
    assert!(!std::path::Path::new(&socket).exists());
}

fn write_valid_session_policy(directory: &tempfile::TempDir) -> String {
    let path = directory.path().join("session-policy.yaml");
    std::fs::write(
        &path,
        r#"
version: 1
mode: autonomous
expires_after: 1h
idle_timeout: 10m
resources: {}
allow:
  tools: [start_session, get_session_state, end_session]
deny:
  tools: [page]
ask:
  tools: [browser_download]
"#,
    )
    .unwrap();
    path.display().to_string()
}

#[test]
fn autonomous_mode_requires_a_session_policy_before_bind() {
    let directory = tempfile::tempdir().expect("temporary autonomous test directory");
    let socket = test_socket(&directory, "autonomous-no-policy");
    let output = rejected_serve(&socket, &["--permission-mode", "autonomous"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("requires --session-policy"));
    #[cfg(unix)]
    assert!(!std::path::Path::new(&socket).exists());
}

#[test]
fn autonomous_mode_requires_launch_time_manifest_approval_before_bind() {
    let directory = tempfile::tempdir().expect("temporary autonomous test directory");
    let policy = write_valid_session_policy(&directory);
    let socket = test_socket(&directory, "autonomous-no-approval");
    let output = rejected_serve(
        &socket,
        &[
            "--permission-mode",
            "autonomous",
            "--session-policy",
            &policy,
        ],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("requires --approve-session-policy"));
    #[cfg(unix)]
    assert!(!std::path::Path::new(&socket).exists());
}

#[test]
fn session_policy_cannot_be_smuggled_into_standard_mode() {
    let directory = tempfile::tempdir().expect("temporary standard test directory");
    let policy = write_valid_session_policy(&directory);
    let socket = test_socket(&directory, "standard-session-policy");
    let output = rejected_serve(
        &socket,
        &[
            "--permission-mode",
            "standard",
            "--session-policy",
            &policy,
            "--approve-session-policy",
        ],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("valid only with"));
    #[cfg(unix)]
    assert!(!std::path::Path::new(&socket).exists());
}

#[test]
fn legacy_existing_profile_approval_cannot_bypass_autonomous_indicator() {
    let directory = tempfile::tempdir().expect("temporary autonomous test directory");
    let policy = write_valid_session_policy(&directory);
    let socket = test_socket(&directory, "autonomous-legacy-approval");
    let output = rejected_serve(
        &socket,
        &[
            "--permission-mode",
            "autonomous",
            "--session-policy",
            &policy,
            "--approve-session-policy",
            "--allow-legacy-existing-profile-approval",
        ],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("valid only with --permission-mode standard"));
    #[cfg(unix)]
    assert!(!std::path::Path::new(&socket).exists());
}
