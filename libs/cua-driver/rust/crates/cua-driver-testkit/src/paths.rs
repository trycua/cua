//! Filesystem paths, resolved at test runtime from `CARGO_MANIFEST_DIR`.
//!
//! When an integration test runs, Cargo sets `CARGO_MANIFEST_DIR` to the crate
//! under test (`crates/cua-driver`), so `workspace_root()` resolves the same
//! whether called from the test or from here.

use std::path::PathBuf;

/// The Rust workspace root (`libs/cua-driver/rust`).
pub fn workspace_root() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest)
        .parent()
        .unwrap() // crates/
        .parent()
        .unwrap() // workspace root
        .to_owned()
}

/// The built `cua-driver` binary (`.exe` on Windows). One impl replaces the
/// four divergent spellings that were copy-pasted across the test files.
pub fn driver_binary() -> PathBuf {
    let name = if cfg!(target_os = "windows") {
        "cua-driver.exe"
    } else {
        "cua-driver"
    };
    workspace_root().join("target/debug").join(name)
}

/// A built harness app under `test-apps/<dir>/<exe>` (produced by
/// `test-harness/build/{windows.ps1,macos.sh}`). Example:
/// `harness_app("harness-wpf", "CuaTestHarness.Wpf.exe")`.
pub fn harness_app(dir: &str, exe: &str) -> PathBuf {
    workspace_root().join("test-apps").join(dir).join(exe)
}
