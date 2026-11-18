// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! An option the command does not take is a usage error (exit 64) that never
//! reaches a daemon, and the documented options keep working (#4698).

#![cfg(any(unix, target_os = "windows"))]

use cua_driver_testkit::{spawn_in_job, ChildReaper};
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

const BOUND: Duration = Duration::from_secs(15);

fn driver(home: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cua-driver"));
    command
        .args(args)
        .stdin(Stdio::null())
        .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "false")
        .env("CUA_DRIVER_RS_PERMISSIONS_GATE", "0")
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("LOCALAPPDATA", home)
        .env("XDG_STATE_HOME", home)
        .env("CUA_DRIVER_HOME", home)
        .env("CUA_DRIVER_LOCAL_HOME", home)
        .env("CUA_DRIVER_RS_HOME", home);
    command
}

/// Run a finite command, or `None` when it is still running after `BOUND`.
fn run(home: &Path, args: &[&str]) -> Option<Output> {
    let mut command = driver(home, args);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = spawn_in_job(&mut command).expect("spawn cua-driver");
    let deadline = Instant::now() + BOUND;
    while child.try_wait().expect("poll cua-driver").is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Some(child.wait_with_output().expect("collect cua-driver output"))
}

fn exit_code(home: &Path, args: &[&str]) -> Option<i32> {
    run(home, args).and_then(|output| output.status.code())
}

fn assert_usage_error(home: &Path, args: &[&str], option: &str) {
    let output = run(home, args).unwrap_or_else(|| panic!("{args:?} did not exit"));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(64), "{args:?}: {stderr}");
    assert!(
        stderr.contains(&format!("unknown option '{option}'")),
        "{args:?}: {stderr}"
    );
}

fn endpoint(directory: &tempfile::TempDir, name: &str) -> String {
    #[cfg(target_os = "windows")]
    let _ = directory;
    #[cfg(unix)]
    return directory.path().join(name).display().to_string();
    #[cfg(target_os = "windows")]
    return format!(r"\\.\pipe\cua-unknown-option-{name}-{}", std::process::id());
}

#[test]
fn misspelled_options_exit_64_without_touching_the_daemon() {
    let home = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    let endpoints = tempfile::Builder::new()
        .prefix("cua-opt-")
        .tempdir_in("/tmp")
        .unwrap();
    #[cfg(not(unix))]
    let endpoints = tempfile::tempdir().unwrap();
    let socket = endpoint(&endpoints, "d.sock");
    let other_socket = endpoint(&endpoints, "e.sock");

    let mut serve = driver(
        home.path(),
        &[
            "serve",
            "--socket",
            &socket,
            "--no-overlay",
            "--no-permissions-gate",
        ],
    );
    serve.stdout(Stdio::null()).stderr(Stdio::null());
    let mut reaper = ChildReaper::new();
    reaper.push(spawn_in_job(&mut serve).expect("spawn source-built daemon"));
    let deadline = Instant::now() + BOUND;
    while exit_code(home.path(), &["status", "--socket", &socket]) != Some(0) {
        assert!(Instant::now() < deadline, "daemon never became ready");
        std::thread::sleep(Duration::from_millis(100));
    }

    // Documented options still work.
    assert_eq!(
        exit_code(home.path(), &["status", "--socket", &socket]),
        Some(0)
    );
    assert_eq!(
        exit_code(home.path(), &["sessions", "--socket", &socket, "--json"]),
        Some(0)
    );
    assert_eq!(
        exit_code(home.path(), &["channel", "status", "--json"]),
        Some(0)
    );

    assert_usage_error(
        home.path(),
        &["status", "--socket", &socket, "--bogus-flag", "x"],
        "--bogus-flag",
    );
    assert_usage_error(
        home.path(),
        &["status", "--socket", &socket, "--json"],
        "--json",
    );
    assert_usage_error(home.path(), &["status", "--sokcet", &socket], "--sokcet");
    assert_usage_error(
        home.path(),
        &["sessions", "--socket", &socket, "--bogus-flag"],
        "--bogus-flag",
    );
    assert_usage_error(
        home.path(),
        &["channel", "status", "--bogus-flag"],
        "--bogus-flag",
    );
    assert_usage_error(
        home.path(),
        &["skills", "path", "--bogus-flag"],
        "--bogus-flag",
    );

    // A misspelled guard must not stop the daemon without the PID check.
    assert_usage_error(
        home.path(),
        &["stop", "--socket", &socket, "--expected-pidd", "1"],
        "--expected-pidd",
    );
    assert_eq!(
        exit_code(home.path(), &["status", "--socket", &socket]),
        Some(0),
        "stop with a misspelled option stopped the daemon"
    );

    // A misspelled authorization mode must not start a daemon in another mode.
    assert_usage_error(
        home.path(),
        &[
            "serve",
            "--socket",
            &other_socket,
            "--no-overlay",
            "--no-permissions-gate",
            "--permision-mode",
            "bounded",
        ],
        "--permision-mode",
    );

    assert_eq!(
        exit_code(home.path(), &["stop", "--socket", &socket]),
        Some(0)
    );
}
