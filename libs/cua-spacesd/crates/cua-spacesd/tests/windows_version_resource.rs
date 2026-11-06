// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The shipped `cua-spacesd.exe` carries its version resource: the Windows
//! Firewall prompt names the app from `FileDescription` and its publisher
//! from `CompanyName` (not from the Authenticode signature), so without
//! them it said Publisher "Unknown" (CUA-3515).
#![cfg(target_os = "windows")]

use std::process::Command;

#[test]
fn the_exe_names_cua_ai_as_its_publisher() {
    let exe = env!("CARGO_BIN_EXE_cua-spacesd");
    let script = format!(
        "$v = (Get-Item -LiteralPath '{}').VersionInfo; \
         \"$($v.CompanyName)|$($v.FileDescription)|$($v.ProductName)|$($v.ProductVersion)\"",
        exe.replace('\'', "''")
    );
    let out = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .expect("powershell");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let fields: Vec<&str> = text.split('|').collect();
    assert_eq!(
        fields,
        [
            "Cua AI, Inc.",
            "Cua Spaces background service",
            "Cua Spaces",
            env!("CARGO_PKG_VERSION")
        ],
        "{text}"
    );
}
