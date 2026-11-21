// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

use std::process::{Command, Output};

fn run(home: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cua-driver"))
        .args(args)
        .env("CUA_DRIVER_RS_HOME", home)
        .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "0")
        .output()
        .expect("run cua-driver")
}

fn help(home: &std::path::Path, args: &[&str]) -> String {
    let output = run(home, args);
    assert!(
        output.status.success(),
        "{args:?} exited {:?}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("UTF-8 help")
}

/// The top-level help must state the same cache lifetime that
/// `CACHE_REFRESH_SECONDS` implements.
///
/// The help text used to say "20h" while the constant had been one hour
/// since #4757, and the generated CLI reference already said "1-hour", so the
/// two descriptions of the same cache disagreed.
#[test]
fn top_level_help_states_the_implemented_cache_lifetime() {
    let home = tempfile::tempdir().expect("temp home");

    let top = help(home.path(), &["--help"]);

    assert!(
        top.contains("Skip the 1-hour on-disk cache"),
        "the top-level help must state the one-hour cache lifetime:\n{top}"
    );
    assert!(
        !top.contains("20h on-disk cache"),
        "the top-level help must not state the removed 20-hour cache lifetime:\n{top}"
    );
}

/// The top-level help and the generated command reference must agree on the
/// cache lifetime.
#[test]
fn top_level_help_agrees_with_the_check_update_reference() {
    let home = tempfile::tempdir().expect("temp home");

    let top = help(home.path(), &["--help"]);
    let reference = help(home.path(), &["check-update", "--help"]);

    assert!(
        reference.contains("1-hour"),
        "the check-update reference must state the one-hour cache lifetime:\n{reference}"
    );
    assert!(
        !reference.contains("20h"),
        "the check-update reference must not state the removed 20-hour cache lifetime:\n{reference}"
    );
    assert!(
        !top.contains("20h"),
        "the top-level help must not state the removed 20-hour cache lifetime:\n{top}"
    );
}
