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
