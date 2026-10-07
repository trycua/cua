// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Host safety: the app keeps its webview data (and the page's saved state)
//! under its data directory, which follows `HOME`, so a run under a temp
//! `HOME` writes nothing under the real user's `~/Library` for this app.
//!
//! WKWebView ignores `HOME` and would keep web storage under the real
//! `~/Library/WebKit/<app>`; the app runs it with a non-persistent store
//! (see `isolate_webview` in src/lib.rs). The page saves on mount, so a
//! short run exercises every storage write.
//!
//! Opt-in (it opens the app's window for a few seconds):
//!
//!   (cd .. && pnpm build)   # ui/dist, served from the binary
//!   OKB_WEBVIEW_ISOLATION=1 cargo test -p openkoalabot-example-tauri \
//!     --features custom-protocol --test webview_isolation -- --nocapture
//!
//! The real `~/Library` paths are only stat'ed (names, sizes, modification
//! times); no file there is opened or read.
#![cfg(target_os = "macos")]

use std::collections::BTreeMap;
use std::ffi::CStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

const BIN: &str = env!("CARGO_BIN_EXE_openkoalabot-example-tauri");
const IDENTIFIER: &str = "ai.cua.openkoalabot.example";
const PROCESS_NAME: &str = "openkoalabot-example-tauri";

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

/// Every place macOS or WebKit keeps per-app data, by bundle identifier and
/// by process name (the binary runs unbundled).
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
        if meta.is_dir()
            && let Ok(entries) = std::fs::read_dir(&p)
        {
            stack.extend(entries.flatten().map(|e| e.path()));
        }
    }
    out
}

#[test]
fn a_temp_home_run_writes_nothing_under_the_real_library() {
    if std::env::var("OKB_WEBVIEW_ISOLATION").as_deref() != Ok("1") {
        eprintln!("skipped: set OKB_WEBVIEW_ISOLATION=1 (opens the app window briefly)");
        return;
    }
    if !cfg!(feature = "custom-protocol") {
        panic!(
            "run with --features custom-protocol: a dev build loads the dev server, not ui/dist"
        );
    }
    let library = real_home().join("Library");
    let roots = watched(&library);
    let before = snapshot(&roots);

    let home = tempfile::tempdir().unwrap();
    let mut child = Command::new(BIN)
        .env_clear()
        .env("HOME", home.path())
        .env("CUA_HOME", home.path().join(".cua"))
        .env("PATH", "/usr/bin:/bin")
        .env("TMPDIR", home.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start the app");
    let data_dir = home
        .path()
        .join("Library/Application Support")
        .join(IDENTIFIER);
    let state = data_dir.join("ui-state.json");
    // The page saves its state on mount: wait (bounded) for that write,
    // then a little longer for any web storage flush.
    let deadline = Instant::now() + Duration::from_secs(60);
    while !state.exists() && Instant::now() < deadline {
        if let Ok(Some(status)) = child.try_wait() {
            panic!("the app exited early: {status}");
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    std::thread::sleep(Duration::from_secs(5));
    let _ = child.kill();
    let _ = child.wait();
    std::thread::sleep(Duration::from_secs(1));

    assert!(
        state.exists(),
        "the page never saved its state into the temp data directory"
    );
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
    // Everything the webview keeps is in the temp home.
    assert!(
        !home
            .path()
            .join("Library/WebKit")
            .join(IDENTIFIER)
            .join("WebsiteData/LocalStorage")
            .exists()
    );
}
