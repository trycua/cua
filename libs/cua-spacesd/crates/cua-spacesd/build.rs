// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Embeds the source revision as `CUA_SPACESD_GIT_SHA`: the build
//! environment's `CUA_SPACESD_GIT_SHA` wins (image builds in containers
//! without `.git`), else `git rev-parse HEAD`, else empty. `main` hands it to
//! `cua_driver_core::build_info` so `health_report` (served in-process) and
//! `build-info` report the running build. Reruns when HEAD or its ref moves.

use std::path::PathBuf;
use std::process::Command;

fn rerun_if_exists(path: &std::path::Path) {
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
fn rerun_if_ref_changes(common: &std::path::Path, reference: &str) {
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

fn git(args: &[&str]) -> Option<String> {
    let dir = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?.trim().to_owned();
    (!text.is_empty()).then_some(text)
}

fn main() {
    // A musl Linux build is the static headless guest daemon that Cua injects
    // into any OCI image: exec, files, processes, ports, no desktop (no
    // cua-driver platform, capture, presence or doctor).
    println!("cargo:rustc-check-cfg=cfg(cua_headless)");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("musl") {
        println!("cargo:rustc-cfg=cua_headless");
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // The linked Windows driver expects physical pixels. Declare the
        // process context before any async, native, or overlay thread starts.
        println!("cargo:rerun-if-changed=cua-spacesd.rc");
        println!("cargo:rerun-if-changed=cua-spacesd.manifest");
        embed_resource::compile_for("cua-spacesd.rc", ["cua-spacesd"], embed_resource::NONE);
    }

    println!("cargo:rerun-if-env-changed=CUA_SPACESD_GIT_SHA");
    let from_env = std::env::var("CUA_SPACESD_GIT_SHA").unwrap_or_default();
    let sha = if !from_env.trim().is_empty() {
        from_env.trim().to_owned()
    } else {
        git(&["rev-parse", "HEAD"]).unwrap_or_default()
    };
    // Only paths that exist are watched: Cargo treats a missing
    // `rerun-if-changed` path as always changed, which rebuilt this crate on
    // every cargo invocation of a CI checkout (it has no packed-refs file).
    // The HEAD reflog is appended by every commit, checkout and reset, so it
    // follows the checkout even when the branch ref is packed.
    if let Some(git_dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        let git_dir = PathBuf::from(git_dir);
        rerun_if_exists(&git_dir.join("HEAD"));
        rerun_if_exists(&git_dir.join("logs").join("HEAD"));
        if let Some(common) = git(&["rev-parse", "--git-common-dir"]) {
            // git prints a relative path relative to the directory it ran in.
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
    println!("cargo:rustc-env=CUA_SPACESD_GIT_SHA={sha}");
}
