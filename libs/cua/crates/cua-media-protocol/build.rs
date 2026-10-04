// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    if let Ok(head) = std::fs::read_to_string("../../.git/HEAD") {
        if let Some(reference) = head.trim().strip_prefix("ref: ") {
            println!("cargo:rerun-if-changed=../../.git/{reference}");
        }
    }
    println!("cargo:rerun-if-env-changed=CUA_ENV_BUILD_REVISION");
    if std::env::var_os("CUA_ENV_BUILD_REVISION").is_some() {
        return;
    }
    let revision = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|revision| revision.trim().to_owned())
        .filter(|revision| !revision.is_empty());
    if let Some(revision) = revision {
        println!("cargo:rustc-env=CUA_ENV_BUILD_REVISION={revision}");
    }
}
