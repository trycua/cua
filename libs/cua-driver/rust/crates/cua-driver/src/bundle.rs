//! Installed-product identity.
//!
//! Release and source builds deliberately use different executable/app names.
//! Deriving the channel from the canonical executable path keeps every runtime
//! entry point (MCP proxy, daemon, status, autostart) in the same namespace
//! without a mutable "active version" switch.

use std::path::{Path, PathBuf};

pub const RELEASE_CLI_NAME: &str = "cua-driver";
pub const LOCAL_CLI_NAME: &str = "cua-driver-local";

pub const RELEASE_APP_NAME: &str = "CuaDriver";
pub const LOCAL_APP_NAME: &str = "CuaDriverLocal";
pub const RELEASE_BUNDLE_ID: &str = "com.trycua.driver";
pub const LOCAL_BUNDLE_ID: &str = "com.trycua.driver.local";

pub(crate) fn path_is_local(path: &Path) -> bool {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    file_name == LOCAL_CLI_NAME
        || file_name == format!("{LOCAL_CLI_NAME}.exe")
        || path
            .components()
            .any(|component| component.as_os_str().to_str() == Some("CuaDriverLocal.app"))
}

/// Whether this process is the explicitly-installed source-build product.
pub fn is_local_installation() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|path| std::fs::canonicalize(path).ok())
        .is_some_and(|path| path_is_local(&path))
}

pub fn cli_name() -> &'static str {
    if is_local_installation() {
        LOCAL_CLI_NAME
    } else {
        RELEASE_CLI_NAME
    }
}

pub fn state_namespace() -> &'static str {
    if is_local_installation() {
        "cua-driver-local"
    } else {
        "cua-driver"
    }
}

pub fn user_home_subdirectory() -> &'static str {
    if is_local_installation() {
        ".cua-driver-local"
    } else {
        ".cua-driver"
    }
}

#[cfg(target_os = "windows")]
pub fn uia_executable_name() -> &'static str {
    if is_local_installation() {
        "cua-driver-uia-local.exe"
    } else {
        "cua-driver-uia.exe"
    }
}

#[cfg(target_os = "windows")]
pub fn autostart_task_name() -> &'static str {
    if is_local_installation() {
        "cua-driver-local-serve"
    } else {
        "cua-driver-serve"
    }
}

pub fn app_name() -> &'static str {
    if is_local_installation() {
        LOCAL_APP_NAME
    } else {
        RELEASE_APP_NAME
    }
}

/// The stable installed product path.
///
/// Trust decisions (notably Computer History admission) deliberately use this
/// path rather than whichever app bundle happened to launch the current
/// process.
pub fn app_bundle_path() -> String {
    format!("/Applications/{}.app", app_name())
}

fn exact_enclosing_app_bundle(executable: &Path) -> Option<PathBuf> {
    let macos = executable.parent()?;
    let contents = macos.parent()?;
    let app = contents.parent()?;
    let (expected_app, expected_cli) = match (
        app.file_name().and_then(|name| name.to_str()),
        executable.file_name().and_then(|name| name.to_str()),
    ) {
        (Some("CuaDriver.app"), Some(RELEASE_CLI_NAME)) => ("CuaDriver.app", RELEASE_CLI_NAME),
        (Some("CuaDriverLocal.app"), Some(LOCAL_CLI_NAME)) => {
            ("CuaDriverLocal.app", LOCAL_CLI_NAME)
        }
        _ => return None,
    };
    if macos.file_name().and_then(|name| name.to_str()) != Some("MacOS")
        || contents.file_name().and_then(|name| name.to_str()) != Some("Contents")
        || app.file_name().and_then(|name| name.to_str()) != Some(expected_app)
        || executable.file_name().and_then(|name| name.to_str()) != Some(expected_cli)
    {
        return None;
    }
    Some(app.to_path_buf())
}

/// Select the app bundle LaunchServices should open for this executable.
///
/// A CLI running from an exact packaged layout must relaunch that same bundle,
/// including a uniquely staged test bundle. Standalone binaries retain the
/// established `/Applications` fallback for their release/local channel.
pub(crate) fn launch_app_bundle_path_for_executable(executable: &Path) -> PathBuf {
    exact_enclosing_app_bundle(executable).unwrap_or_else(|| {
        let name = if path_is_local(executable) {
            LOCAL_APP_NAME
        } else {
            RELEASE_APP_NAME
        };
        PathBuf::from(format!("/Applications/{name}.app"))
    })
}

/// App bundle to pass to LaunchServices. This is intentionally not used for
/// installed-product trust decisions; see [`app_bundle_path`].
pub fn launch_app_bundle_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|path| std::fs::canonicalize(path).ok())
        .map(|path| launch_app_bundle_path_for_executable(&path))
        .unwrap_or_else(|| PathBuf::from(app_bundle_path()))
}

pub fn bundle_id() -> &'static str {
    if is_local_installation() {
        LOCAL_BUNDLE_ID
    } else {
        RELEASE_BUNDLE_ID
    }
}

/// A bundled daemon already has its stable TCC responsibility identity and
/// must not disclaim it during startup.
#[cfg(target_os = "macos")]
pub fn is_executable_inside_cuadriver_app() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|path| std::fs::canonicalize(path).ok())
        .is_some_and(|path| {
            path.to_str().is_some_and(|path| {
                path.contains("/CuaDriver.app/Contents/MacOS/")
                    || path.contains("/CuaDriverLocal.app/Contents/MacOS/")
            })
        })
}

/// Returns `true` when the env var is one of `1|true|yes|on`
/// (case-insensitive). Anything else, including unset, is falsy.
#[cfg(target_os = "windows")]
pub fn is_env_truthy(name: &str) -> bool {
    match std::env::var(name) {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_identity_requires_the_explicit_local_product_name() {
        assert!(path_is_local(Path::new(
            "/Applications/CuaDriverLocal.app/Contents/MacOS/cua-driver-local"
        )));
        assert!(path_is_local(Path::new(
            "/dev/.cua-driver-local/cua-driver-local.exe"
        )));
        assert!(!path_is_local(Path::new(
            "/Applications/CuaDriver.app/Contents/MacOS/cua-driver"
        )));
        assert!(!path_is_local(Path::new("/tmp/cua-driver-local-test")));
    }

    #[test]
    fn launch_path_keeps_the_exact_enclosing_release_or_local_bundle() {
        assert_eq!(
            launch_app_bundle_path_for_executable(Path::new(
                "/private/tmp/staged-release/CuaDriver.app/Contents/MacOS/cua-driver"
            )),
            PathBuf::from("/private/tmp/staged-release/CuaDriver.app")
        );
        assert_eq!(
            launch_app_bundle_path_for_executable(Path::new(
                "/private/tmp/staged-local/CuaDriverLocal.app/Contents/MacOS/cua-driver-local"
            )),
            PathBuf::from("/private/tmp/staged-local/CuaDriverLocal.app")
        );
    }

    #[test]
    fn launch_path_rejects_lookalike_or_mismatched_bundle_layouts() {
        assert_eq!(
            launch_app_bundle_path_for_executable(Path::new(
                "/private/tmp/CuaDriver.app.backup/Contents/MacOS/cua-driver"
            )),
            PathBuf::from("/Applications/CuaDriver.app")
        );
        assert_eq!(
            launch_app_bundle_path_for_executable(Path::new(
                "/private/tmp/CuaDriver.app/Contents/MacOS/helpers/cua-driver"
            )),
            PathBuf::from("/Applications/CuaDriver.app")
        );
        assert_eq!(
            launch_app_bundle_path_for_executable(Path::new(
                "/private/tmp/CuaDriverLocal.app/Contents/MacOS/cua-driver"
            )),
            PathBuf::from("/Applications/CuaDriverLocal.app")
        );
        assert_eq!(
            launch_app_bundle_path_for_executable(Path::new("/opt/bin/cua-driver-local")),
            PathBuf::from("/Applications/CuaDriverLocal.app")
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn env_truthiness_is_strict() {
        let name = "CUA_DRIVER_RS_TEST_TRUTHY";
        for value in ["1", "true", "TRUE", "Yes", "on", " 1 "] {
            std::env::set_var(name, value);
            assert!(is_env_truthy(name), "expected truthy for {value:?}");
        }
        for value in ["0", "false", "no", "off", ""] {
            std::env::set_var(name, value);
            assert!(!is_env_truthy(name), "expected falsy for {value:?}");
        }
        std::env::remove_var(name);
    }
}
