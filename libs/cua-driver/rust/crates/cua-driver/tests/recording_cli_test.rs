// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `cua-driver recording` argument errors describe only invocations the
//! subcommands accept.

use std::process::Command;

use cua_driver_testkit::CliDriver;

#[test]
fn unknown_recording_subcommand_lists_render_as_render_accepts_it() {
    let driver = CliDriver::with_binary(env!("CARGO_BIN_EXE_cua-driver"));
    assert!(driver.available(), "test daemon failed to start");
    let socket = driver.daemon_socket().expect("test daemon socket");

    let output = Command::new(env!("CARGO_BIN_EXE_cua-driver"))
        .args(["recording", "bogus", "--socket", socket])
        .output()
        .expect("run cua-driver recording bogus");

    assert_eq!(output.status.code(), Some(64));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Unknown recording subcommand 'bogus'"),
        "{stderr}"
    );
    // `recording render` takes the output path as its second positional
    // argument and defines no `--output` option.
    assert!(stderr.contains("render <input-dir> <out.mp4>"), "{stderr}");
    assert!(!stderr.contains("--output"), "{stderr}");
}
