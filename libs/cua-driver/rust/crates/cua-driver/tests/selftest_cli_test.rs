// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `cua-driver test` is registered in the CLI help and reference.

use std::process::Command;

fn run(args: &[&str]) -> String {
    let home = tempfile::tempdir().expect("temp home");
    let output = Command::new(env!("CARGO_BIN_EXE_cua-driver"))
        .args(args)
        .env("CUA_DRIVER_RS_HOME", home.path())
        .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "0")
        .output()
        .expect("run cua-driver");
    assert!(output.status.success(), "{args:?}: {:?}", output.status);
    String::from_utf8(output.stdout).expect("UTF-8 output")
}

#[test]
fn test_command_is_listed_in_help() {
    let help = run(&["--help"]);
    assert!(help.contains("test"), "{help}");
    assert!(help.contains("cua-driver test [--json]"), "{help}");
}

#[test]
fn test_command_has_its_own_reference_page() {
    let page = run(&["test", "--help"]);
    assert!(page.contains("--json"), "{page}");
    assert!(page.contains("Exit code is 0"), "{page}");
}

#[test]
fn test_command_is_in_the_manifest() {
    let manifest: serde_json::Value =
        serde_json::from_str(&run(&["manifest"])).expect("manifest JSON");
    let names: Vec<&str> = manifest["subcommands"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["name"].as_str())
        .collect();
    assert!(names.contains(&"test"), "{names:?}");
}
