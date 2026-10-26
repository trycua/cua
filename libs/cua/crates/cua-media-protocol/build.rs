// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    // The repository root is four levels up from this crate, not two: the old
    // `../../.git/HEAD` path never existed, and Cargo treats a missing
    // `rerun-if-changed` path as always changed, so this crate and the five
    // that depend on it rebuilt on every cargo invocation. Ask git for the
    // real directories and watch only paths that exist.
    if let Some(git_dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        let git_dir = PathBuf::from(git_dir);
        rerun_if_exists(&git_dir.join("HEAD"));
        rerun_if_exists(&git_dir.join("logs").join("HEAD"));
        if let Some(common) = git(&["rev-parse", "--git-common-dir"]) {
            let common = PathBuf::from(common);
            let common = if common.is_absolute() {
                common
            } else {
                PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default()).join(common)
            };
            rerun_if_exists(&common.join("packed-refs"));
            if let Some(reference) = git(&["symbolic-ref", "-q", "HEAD"]) {
                rerun_if_ref_changes(&common, &reference);
            }
        }
    }
    println!("cargo:rerun-if-env-changed=CUA_ENV_BUILD_REVISION");
    if std::env::var_os("CUA_ENV_BUILD_REVISION").is_some() {
        return;
    }
    let revision = git(&["rev-parse", "--short=12", "HEAD"]);
    if let Some(revision) = revision {
        println!("cargo:rustc-env=CUA_ENV_BUILD_REVISION={revision}");
    }
}

fn rerun_if_exists(path: &Path) {
    if path.exists() {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

/// Watches the branch ref `reference` (for example `refs/heads/main`). While
/// it is a loose file, watch that file. When it is packed, git writes a new
/// loose file at the next commit without touching HEAD, the reflog (it may be
/// disabled) or packed-refs, so watch the nearest existing directory above it
/// instead: creating the file changes that directory. Paths that do not exist
/// are never watched (Cargo would treat them as always changed).
fn rerun_if_ref_changes(common: &Path, reference: &str) {
    let refs = common.join("refs");
    let mut path = common.join(reference);
    loop {
        if path.exists() {
            println!("cargo:rerun-if-changed={}", path.display());
            return;
        }
        match path.parent() {
            Some(parent) if parent.starts_with(&refs) => path = parent.to_path_buf(),
            _ => return,
        }
    }
}

/// Runs git in the crate directory; `None` when it is missing or fails.
fn git(args: &[&str]) -> Option<String> {
    let dir = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    let text = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (!text.is_empty()).then_some(text)
}
