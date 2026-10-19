// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Gives the `cua-spaces-cli` binary's main thread an 8 MiB stack on
//! Windows, as cua-cli's build script does for `cua`. Windows defaults to
//! 1 MiB, and the main thread's command future overflowed it
//! (STATUS_STACK_OVERFLOW) in unoptimized builds.

fn main() {
    const STACK: u32 = 8 * 1024 * 1024;
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if os == "windows" {
        if env == "msvc" {
            println!("cargo:rustc-link-arg-bins=/STACK:{STACK}");
        } else {
            println!("cargo:rustc-link-arg-bins=-Wl,--stack,{STACK}");
        }
    }
    println!("cargo:rerun-if-changed=build.rs");
}
