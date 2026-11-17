//! A `cua-local` symlink is Local even where `current_exe()` resolves
//! symlinks (Linux), so the link's name is lost (CUA-1776).
//!
//! The test re-runs its own binary as a child, through a real symlink named
//! `cua-local`, and has the child report the channel it resolves. Nothing is
//! set but the executable path: `CUA_CHANNEL` is removed.

#![cfg(unix)]

use std::path::Path;
use std::process::Command;

use cua_home::channel::{CUA_CHANNEL_VAR, CuaChannel};

const PROBE_VAR: &str = "CUA_CHANNEL_PROBE";

/// The child: prints the channel this process resolves. A no-op when the
/// test suite runs it as an ordinary test.
#[test]
fn probe() {
    if std::env::var_os(PROBE_VAR).is_some() {
        println!("resolved channel={}", CuaChannel::current().name());
    }
}

/// Runs this test binary as `program` (a path, or a bare name looked up in
/// `path`) and returns the channel it printed.
fn channel_of(program: impl AsRef<std::ffi::OsStr>, path: Option<&Path>) -> String {
    let mut cmd = Command::new(program);
    cmd.args(["--exact", "probe", "--nocapture", "--test-threads=1"])
        .env(PROBE_VAR, "1")
        .env_remove(CUA_CHANNEL_VAR);
    if let Some(path) = path {
        cmd.env("PATH", path);
    }
    let out = cmd.output().expect("run the probe");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "probe failed: {stdout} {}",
        String::from_utf8_lossy(&out.stderr)
    );
    stdout
        .lines()
        // libtest prints `test probe ... ` before the test's own output.
        .find_map(|l| l.split("resolved channel=").nth(1))
        .unwrap_or_else(|| panic!("the probe printed no channel: {stdout}"))
        .trim()
        .to_string()
}

#[test]
fn a_symlink_named_cua_local_resolves_local() {
    if CuaChannel::build_default().is_some() {
        // A build with CUA_DEFAULT_CHANNEL decides before the path does.
        return;
    }
    let exe = std::env::current_exe().expect("test binary");
    // The control: the binary under its own name is Stable.
    assert_eq!(channel_of(&exe, None), "stable");

    let dir = tempfile::Builder::new()
        .prefix("cua-local-link-")
        .tempdir()
        .expect("temp dir");
    let link = dir.path().join("cua-local");
    std::os::unix::fs::symlink(&exe, &link).expect("symlink");

    // Started by path through the link.
    assert_eq!(channel_of(&link, None), "local");
    // Started by bare name, found on PATH (argv[0] is just `cua-local`).
    assert_eq!(channel_of("cua-local", Some(dir.path())), "local");

    // A link under another name is Stable.
    let other = dir.path().join("cua");
    std::os::unix::fs::symlink(&exe, &other).expect("symlink");
    assert_eq!(channel_of(&other, None), "stable");
}
