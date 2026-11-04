// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Host safety: the app keeps its webview data under its data directory,
//! which follows `HOME`, so a run under a temp `HOME` writes nothing under
//! the real user's `~/Library` for this app (WKWebView would otherwise use
//! the real `~/Library/WebKit/<app>`; see src/webview_data.rs). The page's
//! settings come from the injected `ui-storage.json`, seeded here.
//!
//! Opt-in (it shows the app's windows and tray icon for a few seconds):
//!
//!   (cd .. && pnpm build)   # ../dist, served from the binary
//!   CUA_SPACES_WEBVIEW_ISOLATION=1 cargo test --features custom-protocol \
//!     --test webview_isolation -- --nocapture
//!
//! The real `~/Library` paths are only stat'ed (names, sizes, modification
//! times); no file there is opened or read. `PATH` has no `cua`, so no
//! daemon starts, and telemetry is off.
#![cfg(target_os = "macos")]

use std::collections::BTreeMap;
use std::ffi::CStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

const BIN: &str = env!("CARGO_BIN_EXE_cua-spaces");
const IDENTIFIER: &str = "com.trycua.spaces.prototype";
const PROCESS_NAME: &str = "cua-spaces";

/// The login user's home from the password database, whatever `HOME` says.
fn real_home() -> PathBuf {
    // SAFETY: getpwuid returns a pointer into static storage or null; the
    // directory string is copied before any other passwd call.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        assert!(!pw.is_null(), "no passwd entry");
        PathBuf::from(CStr::from_ptr((*pw).pw_dir).to_string_lossy().into_owned())
    }
}

fn watched(library: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for id in [IDENTIFIER, PROCESS_NAME] {
        for dir in [
            "WebKit",
            "Caches",
            "HTTPStorages",
            "Application Support",
            "Saved Application State",
            "Containers",
        ] {
            paths.push(library.join(dir).join(id));
        }
        paths.push(
            library
                .join("HTTPStorages")
                .join(format!("{id}.binarycookies")),
        );
        paths.push(
            library
                .join("Saved Application State")
                .join(format!("{id}.savedState")),
        );
        paths.push(library.join("Preferences").join(format!("{id}.plist")));
    }
    paths
}

/// Path -> (size, mtime) for everything under `roots`, from metadata only.
fn snapshot(roots: &[PathBuf]) -> BTreeMap<PathBuf, (u64, Option<SystemTime>)> {
    let mut out = BTreeMap::new();
    let mut stack: Vec<PathBuf> = roots.to_vec();
    let mut budget = 200_000usize;
    while let Some(p) = stack.pop() {
        budget = budget
            .checked_sub(1)
            .expect("too many entries under the watched paths");
        let Ok(meta) = std::fs::symlink_metadata(&p) else {
            continue;
        };
        out.insert(p.clone(), (meta.len(), meta.modified().ok()));
        if meta.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&p) {
                stack.extend(entries.flatten().map(|e| e.path()));
            }
        }
    }
    out
}

#[test]
fn a_temp_home_run_writes_nothing_under_the_real_library() {
    if std::env::var("CUA_SPACES_WEBVIEW_ISOLATION").as_deref() != Ok("1") {
        eprintln!("skipped: set CUA_SPACES_WEBVIEW_ISOLATION=1 (shows the app briefly)");
        return;
    }
    if !cfg!(feature = "custom-protocol") {
        panic!(
            "run with --features custom-protocol: a dev build loads the dev server, not ../dist"
        );
    }
    let library = real_home().join("Library");
    let roots = watched(&library);
    let before = snapshot(&roots);

    let home = tempfile::tempdir().unwrap();
    let data_dir = home
        .path()
        .join("Library/Application Support")
        .join(IDENTIFIER);
    std::fs::create_dir_all(&data_dir).unwrap();
    std::fs::write(
        data_dir.join("ui-storage.json"),
        r#"{"cua.settings.menuBar":"false"}"#,
    )
    .unwrap();
    let mut child = Command::new(BIN)
        .env_clear()
        .env("HOME", home.path())
        .env("CUA_HOME", home.path().join(".cua"))
        .env("PATH", "/usr/bin:/bin")
        .env("TMPDIR", home.path())
        .env("DO_NOT_TRACK", "1")
        .env("CUA_TELEMETRY", "0")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start the app");
    for _ in 0..40 {
        if let Ok(Some(status)) = child.try_wait() {
            panic!("the app exited early: {status}");
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    let _ = child.kill();
    let _ = child.wait();
    std::thread::sleep(Duration::from_secs(1));

    let after = snapshot(&roots);
    let changed: Vec<_> = after
        .iter()
        .filter(|(p, v)| before.get(*p) != Some(*v))
        .map(|(p, _)| p.display().to_string())
        .collect();
    assert!(
        changed.is_empty(),
        "the run wrote under the real ~/Library: {changed:#?}"
    );
}
