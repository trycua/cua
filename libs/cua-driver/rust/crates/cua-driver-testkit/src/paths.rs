//! Filesystem paths, resolved at test runtime from environment overrides or
//! `CARGO_MANIFEST_DIR`.
//!
//! When an integration test runs, Cargo sets `CARGO_MANIFEST_DIR` to the crate
//! under test (`crates/cua-driver`), so `workspace_root()` resolves the same
//! whether called from the test or from here.

use std::path::PathBuf;

/// The Rust workspace root (`libs/cua-driver/rust`).
pub fn workspace_root() -> PathBuf {
    if let Some(root) = std::env::var_os("CUA_TEST_WORKSPACE_ROOT") {
        return PathBuf::from(root);
    }
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest)
        .parent()
        .unwrap() // crates/
        .parent()
        .unwrap() // workspace root
        .to_owned()
}

/// The built `cua-driver` binary (`.exe` on Windows), preferring a release
/// build when present and falling back to debug. One impl replaces the four
/// divergent spellings (and the macOS release-or-debug variant) that were
/// copy-pasted across the test files.
pub fn driver_binary() -> PathBuf {
    if let Some(path) = std::env::var_os("CUA_TEST_DRIVER_BIN") {
        return PathBuf::from(path);
    }
    let name = if cfg!(target_os = "windows") {
        "cua-driver.exe"
    } else {
        "cua-driver"
    };
    let root = workspace_root();
    let release = root.join("target/release").join(name);
    if release.exists() {
        return release;
    }
    root.join("target/debug").join(name)
}

/// A built harness app under `test-apps/<dir>/<exe>` (produced by
/// `tests/fixtures/build/{windows.ps1,macos.sh}`). Example:
/// `harness_app("harness-wpf", "CuaTestHarness.Wpf.exe")`.
pub fn harness_app(dir: &str, exe: &str) -> PathBuf {
    let root = std::env::var_os("CUA_TEST_APPS_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root().join("test-apps"));
    root.join(dir).join(exe)
}

#[cfg(test)]
mod tests {
    use super::{driver_binary, harness_app, workspace_root};
    use std::path::PathBuf;

    fn with_env<F>(name: &str, value: &str, test: F)
    where
        F: FnOnce(),
    {
        let previous = std::env::var_os(name);
        std::env::set_var(name, value);
        test();
        match previous {
            Some(value) => std::env::set_var(name, value),
            None => std::env::remove_var(name),
        }
    }

    #[test]
    fn relocated_runner_overrides_are_respected() {
        with_env("CUA_TEST_WORKSPACE_ROOT", "/tmp/cua-test-workspace", || {
            assert_eq!(workspace_root(), PathBuf::from("/tmp/cua-test-workspace"));
        });
        with_env("CUA_TEST_DRIVER_BIN", "/tmp/cua-driver", || {
            assert_eq!(driver_binary(), PathBuf::from("/tmp/cua-driver"));
        });
        with_env("CUA_TEST_APPS_ROOT", "/tmp/cua-test-apps", || {
            assert_eq!(
                harness_app("harness-electron", "CuaTestHarness.Electron"),
                PathBuf::from("/tmp/cua-test-apps/harness-electron/CuaTestHarness.Electron")
            );
        });
    }
}
