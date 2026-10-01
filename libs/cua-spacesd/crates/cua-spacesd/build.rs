// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Embeds the source revision as `CUA_SPACESD_GIT_SHA`: the build
//! environment's `CUA_SPACESD_GIT_SHA` wins (image builds in containers
//! without `.git`), else `git rev-parse HEAD`, else empty. `main` hands it to
//! `cua_driver_core::build_info` so `health_report` (served in-process) and
//! `build-info` report the running build. Reruns when HEAD or its ref moves.

use std::path::PathBuf;
use std::process::Command;

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
    println!("cargo:rerun-if-env-changed=CUA_SPACESD_GIT_SHA");
    let from_env = std::env::var("CUA_SPACESD_GIT_SHA").unwrap_or_default();
    let sha = if !from_env.trim().is_empty() {
        from_env.trim().to_owned()
    } else {
        git(&["rev-parse", "HEAD"]).unwrap_or_default()
    };
    if let Some(git_dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        let git_dir = PathBuf::from(git_dir);
        println!("cargo:rerun-if-changed={}", git_dir.join("HEAD").display());
        if let Some(common) = git(&["rev-parse", "--git-common-dir"]) {
            let common = PathBuf::from(common);
            let common = if common.is_absolute() {
                common
            } else {
                git_dir.join(common)
            };
            println!(
                "cargo:rerun-if-changed={}",
                common.join("packed-refs").display()
            );
            if let Some(reference) = git(&["symbolic-ref", "-q", "HEAD"]) {
                println!(
                    "cargo:rerun-if-changed={}",
                    common.join(reference).display()
                );
            }
        }
    }
    println!("cargo:rustc-env=CUA_SPACESD_GIT_SHA={sha}");
}
