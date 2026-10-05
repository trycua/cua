//! Regression coverage for #4094: testkit-owned daemons must not inherit the
//! developer's installed-product state.
//!
//! A maintainer machine with Computer History admitted keeps
//! `{"history_preview_admitted":true}` in the per-user history root. A
//! source-built daemon that reads it requests admission, fails installed-app
//! verification, and exits before binding its socket, so every testkit
//! transport silently becomes unavailable.
//!
//! This binary contains exactly one test so it can safely point its own
//! `HOME` (and the platform state variables) at a temporary directory before
//! any daemon starts. The real user's files are never touched.
//!
//! Windows keeps its OS profile folders unchanged in the testkit. The driver's
//! own `CUA_DRIVER_RS_HOME` must isolate History there too (#4137), and must not
//! leak an inherited override on any platform.

use std::path::{Path, PathBuf};
use std::process::Command;

use cua_driver_testkit::{
    driver_binary, ensure_driver_binary, CliDriver, McpDriver, RawDriver, SHARE_HOST_STATE,
};

/// Every per-user history root the driver may resolve for `home`.
fn history_roots(home: &Path) -> Vec<PathBuf> {
    let base = if cfg!(target_os = "windows") {
        home.join("AppData").join("Local")
    } else if cfg!(target_os = "macos") {
        home.join("Library").join("Application Support")
    } else {
        home.join(".local").join("state")
    };
    let mut roots: Vec<_> = ["cua-driver", "cua-driver-local"]
        .into_iter()
        .map(|namespace| base.join(namespace).join("computer-history"))
        .collect();
    roots.push(home.join(".cua-driver").join("computer-history"));
    roots
}

fn seed_admitted_host_home() -> tempfile::TempDir {
    let home = tempfile::Builder::new()
        .prefix("cua host home-")
        .tempdir()
        .expect("temporary host home");
    for root in history_roots(home.path()) {
        std::fs::create_dir_all(&root).expect("history root");
        std::fs::write(
            root.join("admission.json"),
            br#"{"history_preview_admitted":true}"#,
        )
        .expect("seed admission preference");
    }
    // Only this single-test binary's process environment changes; it runs
    // before any thread or child exists.
    std::env::set_var("HOME", home.path());
    std::env::set_var("CUA_DRIVER_RS_HOME", home.path().join(".cua-driver"));
    if cfg!(target_os = "windows") {
        // Simulate a host entirely inside a disposable profile. Testkit must
        // leave these values alone while isolating the driver-owned root.
        std::env::set_var("USERPROFILE", home.path());
        std::env::set_var("APPDATA", home.path().join("AppData").join("Roaming"));
        std::env::set_var("LOCALAPPDATA", home.path().join("AppData").join("Local"));
    } else if !cfg!(target_os = "macos") {
        std::env::set_var("XDG_STATE_HOME", home.path().join(".local").join("state"));
    }
    home
}

/// With no daemon on this private endpoint, the status command reports whether
/// the selected admission preference requires an experimental-history launch.
/// This observes the real CLI's path resolution without admitting History or
/// accessing an OS credential store.
fn assert_admission_hint(driver_home: Option<&Path>, admitted: bool) {
    #[cfg(windows)]
    let socket = format!(r"\\.\pipe\cua-history-missing-{}", uuid::Uuid::new_v4());
    #[cfg(unix)]
    let socket = format!("/tmp/cua-history-missing-{}.sock", uuid::Uuid::new_v4());

    let mut command = Command::new(driver_binary());
    command
        .args(["history", "status", "--socket", &socket])
        .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "false");
    match driver_home {
        Some(path) => command.env("CUA_DRIVER_RS_HOME", path),
        None => command.env_remove("CUA_DRIVER_RS_HOME"),
    };
    let output = command.output().expect("history status command");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("Cua Driver daemon is not running."),
        "{stderr}"
    );
    assert_eq!(
        stderr.contains(" --experimental-history"),
        admitted,
        "{stderr}"
    );
}

#[test]
fn testkit_daemons_ignore_host_history_admission() {
    if !ensure_driver_binary(&driver_binary()) {
        return;
    }
    let host_home = seed_admitted_host_home();

    // Default locations and an empty override retain the existing behavior.
    // A nonempty override must ignore the admitted OS-profile state, and use
    // the preference under the selected driver home when one is present.
    assert_admission_hint(None, true);
    assert_admission_hint(Some(Path::new("")), true);
    let empty_home = tempfile::tempdir().expect("empty driver home");
    assert_admission_hint(Some(empty_home.path()), false);
    assert_admission_hint(Some(&host_home.path().join(".cua-driver")), true);

    // Control: sharing the seeded host state reproduces the original failure,
    // proving the seed is the state that used to leak into test daemons.
    assert!(
        McpDriver::spawn_with_env(&[SHARE_HOST_STATE]).is_none(),
        "a daemon sharing the admitted host state is expected to refuse to start; \
         if product admission behavior changed, update this control"
    );

    let mcp =
        McpDriver::spawn().expect("MCP testkit daemon must start despite host history admission");
    let mcp_root = mcp.state_root().expect("MCP daemon uses an isolated root");
    assert!(!mcp_root.starts_with(host_home.path()));

    let raw =
        RawDriver::spawn().expect("raw testkit daemon must start despite host history admission");
    let raw_root = raw.state_root().expect("raw daemon uses an isolated root");
    assert_ne!(raw_root, mcp_root, "each daemon owns its own state root");

    let cli = CliDriver::new();
    assert!(
        cli.available(),
        "CLI testkit daemon must start despite host history admission"
    );
    assert!(cli.state_root().is_some());

    // The seeded host preference is still intact: isolation is read-side only.
    for root in history_roots(host_home.path()) {
        assert_eq!(
            std::fs::read(root.join("admission.json")).unwrap(),
            br#"{"history_preview_admitted":true}"#
        );
    }
}
