//! Installed-product identity.
//!
//! Release and source builds deliberately use different executable/app names.
//! Deriving the channel from the canonical executable path keeps every runtime
//! entry point (MCP proxy, daemon, status, autostart) in the same namespace
//! without a mutable "active version" switch.

use std::path::Path;

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

pub fn app_bundle_path() -> String {
    format!("/Applications/{}.app", app_name())
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
