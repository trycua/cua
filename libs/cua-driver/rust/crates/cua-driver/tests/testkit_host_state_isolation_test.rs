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

use std::path::{Path, PathBuf};

use cua_driver_testkit::{driver_binary, CliDriver, McpDriver, RawDriver, SHARE_HOST_STATE};

/// Every per-user history root the driver may resolve for `home`.
fn history_roots(home: &Path) -> Vec<PathBuf> {
    let base = if cfg!(target_os = "macos") {
        home.join("Library").join("Application Support")
    } else if cfg!(target_os = "windows") {
        home.join("AppData").join("Local")
    } else {
        home.join(".local").join("state")
    };
    ["cua-driver", "cua-driver-local"]
        .into_iter()
        .map(|namespace| base.join(namespace).join("computer-history"))
        .collect()
}

fn seed_admitted_host_home() -> tempfile::TempDir {
    let home = tempfile::Builder::new()
        .prefix("cua-host-home-")
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
    if cfg!(target_os = "windows") {
        std::env::set_var("LOCALAPPDATA", home.path().join("AppData").join("Local"));
    } else if !cfg!(target_os = "macos") {
        std::env::set_var("XDG_STATE_HOME", home.path().join(".local").join("state"));
    }
    home
}

#[test]
fn testkit_daemons_ignore_host_history_admission() {
    if !driver_binary().exists() {
        eprintln!("[skip] driver binary not built at {:?}", driver_binary());
        return;
    }
    let host_home = seed_admitted_host_home();

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
        assert!(root.join("admission.json").is_file());
    }
}
