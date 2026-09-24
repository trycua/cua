//! Filesystem paths, resolved at test runtime from environment overrides or
//! `CARGO_MANIFEST_DIR`.
//!
//! When an integration test runs, Cargo sets `CARGO_MANIFEST_DIR` to the crate
//! under test (`crates/cua-driver`), so `workspace_root()` resolves the same
//! whether called from the test or from here.

use std::path::{Path, PathBuf};

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
    if let Ok(test_exe) = std::env::current_exe() {
        if let Some(profile_dir) = test_exe
            .parent()
            .filter(|dir| dir.file_name().is_some_and(|name| name == "deps"))
            .and_then(std::path::Path::parent)
        {
            let sibling = profile_dir.join(name);
            if sibling.exists() {
                return sibling;
            }
        }
    }
    let root = workspace_root();
    let release = root.join("target/release").join(name);
    if release.exists() {
        return release;
    }
    root.join("target/debug").join(name)
}

/// Environment switch that turns a missing driver binary into a test failure.
///
/// Local runs keep the historical behavior: a test whose driver binary is not
/// built logs a skip note and returns early. CI jobs that gate on these tests
/// set `CUA_TEST_REQUIRE_DRIVER_BIN=1` so a missing or misrouted binary fails
/// loudly instead of passing without exercising the driver.
pub const REQUIRE_DRIVER_BIN_ENV: &str = "CUA_TEST_REQUIRE_DRIVER_BIN";

/// Whether `CUA_TEST_REQUIRE_DRIVER_BIN` requests strict binary resolution.
pub fn driver_binary_required() -> bool {
    std::env::var(REQUIRE_DRIVER_BIN_ENV).is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes"
        )
    })
}

/// Returns whether `bin` exists. A missing binary panics when
/// [`REQUIRE_DRIVER_BIN_ENV`] is set, and otherwise logs a skip note so the
/// caller can return early.
pub fn ensure_driver_binary(bin: &Path) -> bool {
    check_driver_binary(bin, driver_binary_required())
}

fn check_driver_binary(bin: &Path, required: bool) -> bool {
    if bin.exists() {
        return true;
    }
    if required {
        panic!(
            "[testkit] driver binary not built at {bin:?}; {REQUIRE_DRIVER_BIN_ENV}=1 \
             forbids skipping. Build cua-driver or point CUA_TEST_DRIVER_BIN at it."
        );
    }
    eprintln!("[testkit] driver binary not built at {bin:?} — skipping");
    false
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
    use super::{check_driver_binary, driver_binary, harness_app, workspace_root};
    use std::path::{Path, PathBuf};

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

    #[test]
    fn missing_driver_binary_skips_by_default() {
        assert!(!check_driver_binary(
            Path::new("/nonexistent/cua-driver-testkit-missing"),
            false
        ));
    }

    #[test]
    #[should_panic(expected = "CUA_TEST_REQUIRE_DRIVER_BIN=1 forbids skipping")]
    fn missing_driver_binary_panics_when_required() {
        check_driver_binary(Path::new("/nonexistent/cua-driver-testkit-missing"), true);
    }

    #[test]
    fn present_driver_binary_is_accepted_when_required() {
        let exe = std::env::current_exe().expect("current test executable");
        assert!(check_driver_binary(&exe, true));
    }
}
