// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The driver Cua Spaces ships in `Contents/Helpers` updates with the app.
//! Its update and channel commands must refuse instead of running the
//! standalone installer, which would put a second driver on the Mac.
#![cfg(target_os = "macos")]

use std::process::{Command, Output};

const GUIDANCE: &str = "Cua Driver is part of Cua Spaces and updates with it";

fn bundled_driver(root: &std::path::Path) -> std::path::PathBuf {
    let helpers = root.join("Cua Spaces.app/Contents/Helpers");
    std::fs::create_dir_all(&helpers).unwrap();
    let driver = helpers.join("cua-driver");
    std::fs::copy(env!("CARGO_BIN_EXE_cua-driver"), &driver).unwrap();
    driver
}

fn run(driver: &std::path::Path, home: &std::path::Path, args: &[&str]) -> Output {
    Command::new(driver)
        .args(args)
        .env("HOME", home)
        .env("CUA_DRIVER_RS_HOME", home)
        .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "0")
        .output()
        .unwrap()
}

#[test]
fn bundled_driver_refuses_update_and_channel_with_spaces_guidance() {
    let root = tempfile::tempdir().unwrap();
    let driver = bundled_driver(root.path());
    // Checks come first: on a build without the guard they fail here, before
    // `--apply` could start the real installer.
    for args in [
        &["check-update", "--json"][..],
        &["update", "--json"],
        &["update", "--apply", "--json"],
    ] {
        let output = run(&driver, root.path(), args);
        assert!(!output.status.success(), "{args:?}");
        let state: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(state["update_available"], false, "{args:?}: {state}");
        assert!(state["latest_version"].is_null(), "{args:?}: {state}");
        assert!(state["install_command"].is_null(), "{args:?}: {state}");
        assert!(
            state["error"].as_str().unwrap().contains(GUIDANCE),
            "{args:?}: {state}"
        );
    }
    let apply = run(&driver, root.path(), &["update", "--apply"]);
    assert!(!apply.status.success());
    let stdout = String::from_utf8_lossy(&apply.stdout);
    assert!(stdout.contains(GUIDANCE), "{stdout}");
    assert!(!stdout.contains("Downloading"), "{stdout}");

    let channel = run(&driver, root.path(), &["channel", "set", "nightly"]);
    assert!(!channel.status.success());
    assert!(String::from_utf8_lossy(&channel.stderr).contains(GUIDANCE));
    assert!(!root.path().join("release-channel").exists());
}

#[test]
fn a_driver_inside_another_app_names_that_app_and_one_outside_any_app_is_unmanaged() {
    let root = tempfile::tempdir().unwrap();
    let helpers = root.path().join("Other.app/Contents/Helpers");
    std::fs::create_dir_all(&helpers).unwrap();
    let hosted = helpers.join("cua-driver");
    std::fs::copy(env!("CARGO_BIN_EXE_cua-driver"), &hosted).unwrap();
    let output = run(&hosted, root.path(), &["channel", "set", "nightly"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Cua Driver is part of Other and updates with it"),
        "{stderr}"
    );

    let plain = root.path().join("cua-driver");
    std::fs::copy(env!("CARGO_BIN_EXE_cua-driver"), &plain).unwrap();
    let output = run(&plain, root.path(), &["channel", "set", "stable"]);
    assert!(output.status.success(), "{output:?}");
    assert!(root.path().join("release-channel").exists());
}
