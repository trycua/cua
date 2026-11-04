//! `cua-driver permissions` reports usage errors with the documented exit code.

use std::process::{Command, Stdio};

#[test]
fn permissions_unknown_subcommand_is_a_usage_error() {
    let home = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_cua-driver"))
        .args(["permissions", "bogus"])
        .stdin(Stdio::null())
        .env("HOME", home.path())
        .env("CUA_DRIVER_RS_HOME", home.path())
        .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "0")
        .output()
        .expect("run cua-driver permissions with an unknown subcommand");
    assert_eq!(output.status.code(), Some(64));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown permissions subcommand 'bogus'"),
        "stderr={stderr}"
    );
    assert!(stderr.contains("Valid: status, grant."), "stderr={stderr}");
}
