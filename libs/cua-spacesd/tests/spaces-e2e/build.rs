// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The in-process driver links cua-driver's macOS platform crate, which
//! references the Swift runtime through `@rpath`. Link arguments from a
//! dependency's build script do not reach this package's binaries and tests,
//! so emit the rpath here (the same one cua-driver's own binary bakes in).
//!
//! It also stamps the checkout's commit into `CUA_FIXTURES_SOURCE_COMMIT`:
//! `cua-test-fixtures` refuses to serve when its checkout has moved on since
//! it was built, so the binding suites never run against a stale fixture.
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(std::env::var("CARGO_MANIFEST_DIR").ok()?)
        .output()
        .ok()?;
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (out.status.success() && !s.is_empty()).then_some(s)
}

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    }
    println!("cargo:rerun-if-changed=build.rs");
    let commit = git(&["rev-parse", "HEAD"]).unwrap_or_default();
    println!("cargo:rustc-env=CUA_FIXTURES_SOURCE_COMMIT={commit}");
    // Re-stamp when HEAD moves: the HEAD file (a checkout or a worktree's
    // own), the branch it names, and packed refs.
    let mut watch = vec!["HEAD".to_string(), "packed-refs".to_string()];
    if let Some(branch) = git(&["symbolic-ref", "-q", "HEAD"]) {
        watch.push(branch);
    }
    for w in watch {
        if let Some(path) = git(&["rev-parse", "--path-format=absolute", "--git-path", &w]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
}
