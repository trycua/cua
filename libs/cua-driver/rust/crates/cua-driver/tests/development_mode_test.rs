//! Regression coverage for #4693: a source-built `cua-driver` must be able to
//! run `serve` without the installed CuaDriver.app when `--development` is
//! passed.
//!
//! A persisted Computer History admission (`history enable` wrote
//! `{"history_preview_admitted":true}` under the per-user history root) makes
//! a source-built binary request admission, fail installed-product
//! verification, and exit before binding. `--development` vetoes that request
//! and serves unadmitted instead. Verified on macOS, where #4693 was filed.

#![cfg(target_os = "macos")]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn seeded_admitted_home(root: &Path) -> PathBuf {
    let home = root.join("rs-home");
    let history = home.join("computer-history");
    std::fs::create_dir_all(&history).expect("create history root");
    std::fs::write(
        history.join("admission.json"),
        br#"{"history_preview_admitted":true}"#,
    )
    .expect("seed admission preference");
    home
}

fn serve_command(home: &Path, state_root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cua-driver"));
    command
        .arg("serve")
        .env("HOME", state_root)
        .env("CUA_DRIVER_RS_HOME", home)
        .env("CUA_DRIVER_RS_PERMISSIONS_GATE", "0")
        .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "false")
        .env("CUA_DRIVER_RS_UPDATE_CHECK", "false")
        // Skipping the unbundled re-exec keeps the daemon single-process so
        // the test's `kill` reaches the socket holder.
        .env("CUA_DRIVER_RS_RESPONSIBILITY_DISCLAIMED", "1");
    command
}

#[test]
fn development_mode_serves_despite_persisted_history_admission() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let home = seeded_admitted_home(directory.path());
    let socket = directory.path().join("dev.sock");
    let socket = socket.to_str().expect("socket path");

    // Control: without --development the seeded admission demands the
    // installed product, so a source-built binary exits before binding.
    let output = serve_command(&home, directory.path())
        .args(["--socket", socket, "--no-overlay"])
        .output()
        .expect("serve without --development");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("Computer History admission error"),
        "{stderr}"
    );

    // With --development the same persisted admission is ignored (with one
    // stderr notice) and the daemon binds its socket. The pid file goes under
    // the tempdir too so the real daemon state is untouched.
    let pid_file = directory.path().join("dev.pid");
    let mut child = serve_command(&home, directory.path())
        .args([
            "--development",
            "--socket",
            socket,
            "--pid-file",
            pid_file.to_str().expect("pid path"),
            "--no-overlay",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn serve --development");
    let deadline = Instant::now() + Duration::from_secs(15);
    let bound = loop {
        if std::os::unix::net::UnixStream::connect(socket).is_ok() {
            break true;
        }
        if child.try_wait().expect("poll serve").is_some() || Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let _ = child.kill();
    let output = child.wait_with_output().expect("collect serve output");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(bound, "serve --development did not bind {socket}: {stderr}");
    assert!(
        stderr.contains("ignoring the persisted Computer History admission"),
        "{stderr}"
    );
}

#[test]
fn development_mode_rejects_an_explicit_history_request() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let home = directory.path().join("empty-home");
    let socket = directory.path().join("dev.sock");
    let output = serve_command(&home, directory.path())
        .args([
            "--development",
            "--experimental-history",
            "--socket",
            socket.to_str().expect("socket path"),
            "--no-overlay",
        ])
        .output()
        .expect("serve with conflicting flags");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(64), "{stderr}");
    assert!(stderr.contains("--experimental-history"), "{stderr}");
}
