//! Explicit permission-policy configuration must fail before a daemon exposes
//! an action endpoint.

use std::process::{Command, Stdio};

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
fn mcp_rejects_serve_only_authorization_flags() {
    let cases: &[(&[&str], &str)] = &[
        (
            &["mcp", "--direct", "--permission-mode", "bounded"],
            "--permission-mode",
        ),
        (
            &["mcp", "--capability-manifest", "unused.yaml"],
            "--capability-manifest",
        ),
        (
            &["mcp", "--approve-capability-manifest"],
            "--approve-capability-manifest",
        ),
        (
            &["mcp", "--dangerously-bypass-approvals"],
            "--dangerously-bypass-approvals",
        ),
        (
            &["mcp", "--session-policy", "unused.yaml"],
            "--session-policy",
        ),
        (
            &["mcp", "--approve-session-policy"],
            "--approve-session-policy",
        ),
        (&["--permission-mode=bounded"], "--permission-mode"),
    ];

    for (args, rejected_flag) in cases {
        let output = Command::new(env!("CARGO_BIN_EXE_cua-driver"))
            .args(*args)
            .stdin(Stdio::null())
            .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "false")
            .output()
            .expect("run cua-driver mcp with a serve-only authorization flag");
        assert_eq!(output.status.code(), Some(64), "args={args:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(&format!("does not accept {rejected_flag}")),
            "args={args:?}, stderr={stderr}"
        );
        assert!(
            stderr.contains("CUA_DRIVER_PERMISSION_MODE"),
            "args={args:?}, stderr={stderr}"
        );
        assert!(
            stderr.contains("mcp --socket"),
            "args={args:?}, stderr={stderr}"
        );
    }
}

#[test]
fn direct_mcp_still_reads_permission_profile_from_environment() {
    let output = Command::new(env!("CARGO_BIN_EXE_cua-driver"))
        .args(["mcp", "--direct"])
        .stdin(Stdio::null())
        .env("CUA_DRIVER_PERMISSION_MODE", "bounded")
        .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "false")
        .output()
        .expect("run direct MCP with bounded profile from the environment");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("requires --capability-manifest"));
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
        stderr.contains("cannot be combined with a non-unrestricted --permission-mode"),
        "unexpected stderr: {stderr}"
    );
    #[cfg(unix)]
    assert!(!std::path::Path::new(&socket).exists());
}

#[test]
fn danger_flag_alone_selects_unrestricted_and_managed_config_can_disable_it() {
    let directory = tempfile::tempdir().expect("temporary mode test directory");
    let socket = test_socket(&directory, "unrestricted-admin-disabled");
    let output = Command::new(env!("CARGO_BIN_EXE_cua-driver"))
        .args([
            "serve",
            "--socket",
            &socket,
            "--no-overlay",
            "--no-permissions-gate",
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

fn write_valid_capability_manifest(directory: &tempfile::TempDir) -> String {
    let path = directory.path().join("capability-manifest.yaml");
    std::fs::write(
        &path,
        r#"
version: 3
expires_after: 1h
idle_timeout: 10m
resources: {}
allow:
  tools: [start_session, get_session_state, end_session]
deny:
  tools: [page]
"#,
    )
    .unwrap();
    path.display().to_string()
}

#[test]
fn bounded_mode_requires_a_capability_manifest_before_bind() {
    let directory = tempfile::tempdir().expect("temporary bounded test directory");
    let socket = test_socket(&directory, "bounded-no-policy");
    let output = rejected_serve(&socket, &["--permission-mode", "bounded"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("requires --capability-manifest"));
    #[cfg(unix)]
    assert!(!std::path::Path::new(&socket).exists());
}

#[test]
fn bounded_mode_requires_launch_time_manifest_approval_before_bind() {
    let directory = tempfile::tempdir().expect("temporary bounded test directory");
    let policy = write_valid_capability_manifest(&directory);
    let socket = test_socket(&directory, "bounded-no-approval");
    let output = rejected_serve(
        &socket,
        &[
            "--permission-mode",
            "bounded",
            "--capability-manifest",
            &policy,
        ],
    );
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("requires --approve-capability-manifest")
    );
    #[cfg(unix)]
    assert!(!std::path::Path::new(&socket).exists());
}

#[test]
fn deprecated_session_policy_flags_remain_startup_aliases() {
    let directory = tempfile::tempdir().expect("temporary bounded alias test directory");
    let policy = directory.path().join("legacy-session-policy.yaml");
    std::fs::write(
        &policy,
        r#"
version: 3
resources: {}
allow:
  tools: [start_session]
"#,
    )
    .expect("write v3 manifest without bounded lifetime fields");
    let socket = test_socket(&directory, "bounded-deprecated-aliases");
    let output = rejected_serve(
        &socket,
        &[
            "--permission-mode",
            "bounded",
            "--session-policy",
            &policy.display().to_string(),
            "--approve-session-policy",
        ],
    );

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("requires capability manifest expires_after and idle_timeout"),
        "deprecated aliases did not reach bounded manifest validation: {stderr}"
    );
    #[cfg(unix)]
    assert!(!std::path::Path::new(&socket).exists());
}

#[test]
fn capability_manifest_without_acknowledgement_fails_in_standard_mode() {
    let directory = tempfile::tempdir().expect("temporary standard test directory");
    let policy = write_valid_capability_manifest(&directory);
    let socket = test_socket(&directory, "standard-session-policy");
    let output = rejected_serve(
        &socket,
        &[
            "--permission-mode",
            "standard",
            "--capability-manifest",
            &policy,
        ],
    );
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("requires --approve-capability-manifest")
    );
    #[cfg(unix)]
    assert!(!std::path::Path::new(&socket).exists());
}

#[test]
fn capability_manifest_acknowledgement_without_manifest_fails_closed() {
    let directory = tempfile::tempdir().expect("temporary standard test directory");
    let socket = test_socket(&directory, "standard-manifest-ack-only");
    let output = rejected_serve(
        &socket,
        &[
            "--permission-mode",
            "standard",
            "--approve-capability-manifest",
        ],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("requires --capability-manifest"));
    #[cfg(unix)]
    assert!(!std::path::Path::new(&socket).exists());
}

#[test]
fn autonomous_mode_name_remains_a_bounded_compatibility_alias() {
    let directory = tempfile::tempdir().expect("temporary mode alias test directory");
    let socket = test_socket(&directory, "autonomous-alias");
    let output = rejected_serve(&socket, &["--permission-mode", "autonomous"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("permission mode bounded requires --capability-manifest"));
    #[cfg(unix)]
    assert!(!std::path::Path::new(&socket).exists());
}
