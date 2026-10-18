// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

#![cfg(any(unix, target_os = "windows"))]

use cua_driver_testkit::{spawn_in_job, ChildReaper};
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const BOUND: Duration = Duration::from_secs(10);
const PROBE_BOUND: Duration = Duration::from_secs(2);

fn driver(home: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cua-driver"));
    command
        .args(args)
        .stdin(Stdio::null())
        .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "false")
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("LOCALAPPDATA", home)
        .env("XDG_STATE_HOME", home)
        .env("CUA_DRIVER_HOME", home)
        .env("CUA_DRIVER_LOCAL_HOME", home)
        .env("CUA_DRIVER_RS_HOME", home);
    command
}

fn run_status(home: &Path, args: &[&str], limit: Duration) -> Option<Output> {
    let mut command = driver(home, args);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = spawn_in_job(&mut command).expect("spawn cua-driver status");
    let deadline = Instant::now() + limit;
    loop {
        if child.try_wait().expect("poll cua-driver status").is_some() {
            return Some(child.wait_with_output().expect("collect status output"));
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn status_pid(home: &Path, args: &[&str]) -> String {
    let output = run_status(home, args, BOUND)
        .unwrap_or_else(|| panic!("status {args:?} did not finish within {BOUND:?}"));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "status {args:?} failed: {stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    stdout
        .lines()
        .find_map(|line| line.trim().strip_prefix("pid: "))
        .unwrap_or_else(|| panic!("status {args:?} printed no pid line: {stdout}"))
        .to_owned()
}

fn wait_until_ready(
    daemon: &mut Child,
    home: &Path,
    socket: &str,
    pid_file: &str,
) -> Result<String, String> {
    let deadline = Instant::now() + BOUND;
    loop {
        if let Ok(Some(status)) = daemon.try_wait() {
            return Err(format!(
                "daemon exited before becoming ready: {status}. \
                 Set CUA_TEST_DRIVER_STDERR=1 to inherit daemon stderr."
            ));
        }
        let written = std::fs::read_to_string(pid_file).unwrap_or_default();
        let reachable = run_status(home, &["status", "--socket", socket], PROBE_BOUND)
            .is_some_and(|output| output.status.success());
        if reachable && !written.trim().is_empty() {
            return Ok(written.trim().to_owned());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "daemon did not become ready within {BOUND:?} (process still running). \
                 Set CUA_TEST_DRIVER_STDERR=1 to inherit daemon stderr."
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn status_reports_the_pid_stored_in_the_file_named_by_pid_file() {
    let home = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    let endpoint_dir = tempfile::Builder::new()
        .prefix("cua-status-")
        .tempdir_in("/tmp")
        .unwrap();
    #[cfg(unix)]
    let socket = endpoint_dir.path().join("d.sock").display().to_string();
    #[cfg(target_os = "windows")]
    let socket = format!(r"\\.\pipe\cua-status-pid-file-{}", std::process::id());

    let daemon_pid_file = home.path().join("daemon.pid").display().to_string();
    let other_pid_file = home.path().join("other.pid").display().to_string();

    let mut serve = driver(
        home.path(),
        &[
            "serve",
            "--socket",
            &socket,
            "--pid-file",
            &daemon_pid_file,
            "--no-overlay",
            "--no-permissions-gate",
        ],
    );
    serve
        .stdout(Stdio::null())
        .stderr(if std::env::var_os("CUA_TEST_DRIVER_STDERR").is_some() {
            Stdio::inherit()
        } else {
            Stdio::null()
        });
    let mut reaper = ChildReaper::new();
    let mut daemon = spawn_in_job(&mut serve).expect("spawn source-built daemon");
    let ready = wait_until_ready(&mut daemon, home.path(), &socket, &daemon_pid_file);
    reaper.push(daemon);
    let daemon_pid = ready.unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(
        status_pid(
            home.path(),
            &[
                "status",
                "--socket",
                &socket,
                "--pid-file",
                &daemon_pid_file
            ],
        ),
        daemon_pid,
    );
    assert_eq!(
        status_pid(
            home.path(),
            &[
                &format!("--pid-file={daemon_pid_file}"),
                "status",
                "--socket",
                &socket,
            ],
        ),
        daemon_pid,
    );

    let this_process = std::process::id().to_string();
    assert_ne!(this_process, daemon_pid);
    std::fs::write(&other_pid_file, &this_process).unwrap();
    assert_eq!(
        status_pid(
            home.path(),
            &["status", "--socket", &socket, "--pid-file", &other_pid_file],
        ),
        this_process,
    );

    assert_eq!(
        status_pid(home.path(), &["status", "--socket", &socket]),
        "unknown (no pid file)",
    );
}
