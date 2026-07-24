use std::process::Command;

use cua_driver_sdk::{
    ConfiguredDriverOptions, CuaDriver, RuntimeAuthorizationOptions, SessionPermissionMode,
};

const PROBE_ENV: &str = "CUA_DRIVER_SDK_CONFIGURATION_PROBE";

fn options(allowed_modes: Vec<SessionPermissionMode>) -> ConfiguredDriverOptions {
    ConfiguredDriverOptions {
        claude_code_compatibility: false,
        authorization: RuntimeAuthorizationOptions {
            allowed_modes,
            compatibility_mode: SessionPermissionMode::Standard,
            compatibility_bounded_manifest_path: None,
            unrestricted_acknowledged: true,
            max_session_ttl_seconds: 60,
            max_idle_ttl_seconds: 30,
        },
    }
}

#[test]
fn child_configuration_probe() {
    let Ok(case) = std::env::var(PROBE_ENV) else {
        return;
    };
    let result = match case.as_str() {
        "matching" | "conflicting_mode" => {
            CuaDriver::create_configured(options(vec![SessionPermissionMode::Standard]))
        }
        "managed_disable" => CuaDriver::create_configured(options(vec![
            SessionPermissionMode::Standard,
            SessionPermissionMode::Unrestricted,
        ])),
        other => panic!("unknown probe case {other}"),
    };

    match case.as_str() {
        "matching" => {
            if let Err(error) = result {
                panic!("matching environment was rejected: {error}");
            }
        }
        "conflicting_mode" | "managed_disable" => {
            assert!(result.is_err(), "contradictory environment was accepted")
        }
        _ => unreachable!(),
    }
}

fn run_probe(case: &str, environment: &[(&str, &str)]) {
    let mut command = Command::new(std::env::current_exe().expect("current test executable"));
    command
        .args(["--exact", "child_configuration_probe", "--nocapture"])
        .env(PROBE_ENV, case)
        .env_remove("CUA_DRIVER_PERMISSION_MODE")
        .env_remove("CUA_DRIVER_DANGEROUSLY_BYPASS_APPROVALS")
        .env_remove("CUA_DRIVER_DISABLE_UNRESTRICTED")
        .env_remove("CUA_DRIVER_SESSION_POLICY_FILE")
        .env_remove("CUA_DRIVER_SESSION_POLICY_APPROVED");
    for (name, value) in environment {
        command.env(name, value);
    }
    let output = command.output().expect("run configuration probe");
    assert!(
        output.status.success(),
        "configuration probe {case} failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn explicit_runtime_configuration_rejects_contradictory_environment() {
    run_probe("matching", &[("CUA_DRIVER_PERMISSION_MODE", "standard")]);
    run_probe(
        "conflicting_mode",
        &[
            ("CUA_DRIVER_PERMISSION_MODE", "unrestricted"),
            ("CUA_DRIVER_DANGEROUSLY_BYPASS_APPROVALS", "1"),
        ],
    );
    run_probe(
        "managed_disable",
        &[("CUA_DRIVER_DISABLE_UNRESTRICTED", "1")],
    );
}
