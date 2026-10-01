// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! On macOS with the `sck-audio` feature, the screencapturekit crate's Swift
//! bridge needs the Swift compatibility static libraries. Its build script
//! only searches the Xcode.app toolchain layout; on Command Line Tools-only
//! hosts they live under `<developer dir>/usr/lib/swift/macosx`. Add that
//! directory when it exists so binaries and tests link on both layouts.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "macos" || std::env::var_os("CARGO_FEATURE_SCK_AUDIO").is_none() {
        return;
    }
    let Ok(out) = std::process::Command::new("xcode-select")
        .arg("-p")
        .output()
    else {
        return;
    };
    let dev = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    for candidate in [
        format!("{dev}/usr/lib/swift/macosx"),
        format!("{dev}/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift/macosx"),
    ] {
        if std::path::Path::new(&candidate)
            .join("libswiftCompatibility56.a")
            .exists()
        {
            println!("cargo:rustc-link-search=native={candidate}");
            break;
        }
    }
}
