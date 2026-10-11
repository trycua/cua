// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[path = "macos.rs"]
mod native;
#[cfg(any(target_os = "macos", target_os = "windows"))]
mod telemetry;
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
mod v2;

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    if print_version_if_requested() {
        return Ok(());
    }
    let diagnostics =
        cua_logging::init("client").map_err(|error| -> Box<dyn std::error::Error> { error })?;
    let result = native::run();
    if let Err(error) = &result {
        tracing::error!(%error, log_path = %diagnostics.path().display(), "RCDP client stopped with an error");
    }
    result
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn main() {
    if print_version_if_requested() {
        return;
    }
    eprintln!("cua-viewer currently provides its native window surface on macOS and Windows");
    std::process::exit(2);
}

fn print_version_if_requested() -> bool {
    if std::env::args()
        .skip(1)
        .any(|argument| matches!(argument.as_str(), "--version" | "-V"))
    {
        println!("cua-viewer {}", env!("CARGO_PKG_VERSION"));
        true
    } else {
        false
    }
}
