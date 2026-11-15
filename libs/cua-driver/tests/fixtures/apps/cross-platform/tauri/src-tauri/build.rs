// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

fn main() {
    println!("cargo:rerun-if-changed=../web");
    println!("cargo:rerun-if-changed=tauri.conf.json");
    tauri_build::build()
}
