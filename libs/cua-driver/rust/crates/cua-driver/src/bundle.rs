// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Installed-product identity.
//!
//! Release and source builds deliberately use different executable/app names.
//! Deriving the channel from the canonical executable path keeps every runtime
//! entry point (MCP proxy, daemon, status, autostart) in the same namespace
//! without a mutable "active version" switch.

use std::path::Path;

use cua_driver_core::paths::{AppPaths, LOCAL_NAMESPACE, RELEASE_NAMESPACE};

pub const RELEASE_CLI_NAME: &str = "cua-driver";
pub const LOCAL_CLI_NAME: &str = "cua-driver-local";

#[cfg(target_os = "macos")]
pub const RELEASE_APP_NAME: &str = "CuaDriver";
#[cfg(target_os = "macos")]
pub const LOCAL_APP_NAME: &str = "CuaDriverLocal";
#[cfg(target_os = "macos")]
pub const RELEASE_BUNDLE_ID: &str = "com.trycua.driver";
#[cfg(target_os = "macos")]
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
        LOCAL_NAMESPACE
    } else {
        RELEASE_NAMESPACE
    }
}

/// This installation's per-user locations.
pub fn app_paths() -> AppPaths {
    AppPaths::from_env(state_namespace(), &crate::env_vars::var_os)
}

pub fn user_home_subdirectory() -> &'static str {
    if is_local_installation() {
        ".cua-driver-local"
    } else {
        ".cua-driver"
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

#[cfg(target_os = "macos")]
pub fn app_name() -> &'static str {
    if is_local_installation() {
        LOCAL_APP_NAME
    } else {
        RELEASE_APP_NAME
    }
}

#[cfg(target_os = "macos")]
pub fn app_bundle_path() -> String {
    format!("/Applications/{}.app", app_name())
}

#[cfg(target_os = "macos")]
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
#[cfg(all(target_os = "windows", test))]
pub fn is_env_truthy(name: &str) -> bool {
    match crate::env_vars::var(name) {
        Some(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        None => false,
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

    type Vars = std::collections::HashMap<&'static str, std::path::PathBuf>;

    fn resolve(namespace: &str, vars: &Vars) -> AppPaths {
        AppPaths::from_env(namespace, &|name| {
            vars.get(name).map(|value| value.clone().into_os_string())
        })
    }

    /// The testkit's isolated home, with Windows' profile variables inside it.
    fn isolated_vars(root: &cua_driver_testkit::IsolatedStateRoot) -> Vars {
        let mut vars: Vars = root.env().into_iter().collect();
        if cfg!(windows) {
            vars.insert("USERPROFILE", root.path().to_owned());
            vars.insert("LOCALAPPDATA", root.path().join("AppData").join("Local"));
        }
        vars
    }

    /// The cache directory, socket and pid file as the daemon formatted them
    /// before `AppPaths`, from the base the platform keys them on.
    fn daemon_files(base: &str, namespace: &str) -> (String, String, String) {
        let cache = if cfg!(target_os = "macos") {
            format!("{base}/Library/Caches/{namespace}")
        } else if cfg!(windows) {
            format!("{base}/{namespace}")
        } else {
            format!("{base}/.cache/{namespace}")
        };
        let socket = if cfg!(windows) {
            format!(r"\\.\pipe\{namespace}")
        } else {
            format!("{cache}/{namespace}.sock")
        };
        let pid = format!("{cache}/{namespace}.pid");
        (cache, socket, pid)
    }

    fn windows_pipe_lock(base: &std::path::Path, namespace: &str) -> std::path::PathBuf {
        let digest = match namespace {
            RELEASE_NAMESPACE => "e2f9b10f8fdb6e6fa78c6d655e6af871360b1ff3a8e1f3d988607488997d4916",
            _ => "5b57555139f1db86152f441535e528a6eabd5736bd19ee24f9e8b6a1853a60c6",
        };
        base.join("cua-driver-pipe-locks")
            .join(format!("{digest}.lock"))
    }

    #[test]
    fn app_paths_resolve_the_pre_migration_locations() {
        let root = cua_driver_testkit::IsolatedStateRoot::new().unwrap();
        let home = root.path();
        let isolated = isolated_vars(&root);
        let mut defaults = isolated.clone();
        defaults.remove("CUA_DRIVER_RS_HOME");
        defaults.remove("XDG_STATE_HOME");
        let local_app_data = home.join("AppData").join("Local");
        let base = if cfg!(windows) {
            local_app_data.to_str().unwrap().to_owned()
        } else {
            home.to_str().unwrap().to_owned()
        };
        let state_root = if cfg!(target_os = "macos") {
            home.join("Library").join("Application Support")
        } else if cfg!(windows) {
            local_app_data.clone()
        } else {
            home.join(".local/state")
        };

        for namespace in [RELEASE_NAMESPACE, LOCAL_NAMESPACE] {
            let product_home = home.join(format!(".{namespace}"));
            let (cache, socket, pid) = daemon_files(&base, namespace);
            let pipe_lock = if cfg!(windows) {
                windows_pipe_lock(&local_app_data, namespace)
            } else {
                std::path::PathBuf::from(format!("{socket}.lock"))
            };
            let state_dir = state_root.join(namespace);
            let expected = AppPaths {
                user_home: Some(home.to_owned()),
                home: Some(product_home.clone()),
                cache_dir: cache.into(),
                state_dir: state_dir.clone(),
                config_file: Some(product_home.join("config.json")),
                socket,
                pid_file: pid,
                pipe_lock,
                telemetry_dir: Some(product_home.clone()),
                skills_dir: Some(product_home.join("skills")),
                history_root: state_dir.join("computer-history"),
            };
            assert_eq!(
                resolve(namespace, &defaults),
                expected,
                "{namespace} defaults"
            );

            let overridden = AppPaths {
                skills_dir: Some(home.join(".cua-driver").join("skills")),
                history_root: home.join(".cua-driver").join("computer-history"),
                ..expected.clone()
            };
            assert_eq!(
                resolve(namespace, &isolated),
                overridden,
                "{namespace} under CUA_DRIVER_RS_HOME"
            );

            let mut telemetry = defaults.clone();
            telemetry.insert("CUA_DRIVER_TELEMETRY_HOME", home.join("telemetry"));
            assert_eq!(
                resolve(namespace, &telemetry).telemetry_dir,
                Some(home.join("telemetry"))
            );
        }
    }

    #[test]
    fn app_paths_fall_back_like_the_pre_migration_code_without_a_home() {
        let empty = Vars::new();
        for namespace in [RELEASE_NAMESPACE, LOCAL_NAMESPACE] {
            let paths = resolve(namespace, &empty);
            let base = if cfg!(windows) { "C:/Temp" } else { "/tmp" };
            let (cache, socket, pid) = daemon_files(base, namespace);
            let state_dir = if cfg!(target_os = "macos") {
                std::path::PathBuf::from("/tmp/Library/Application Support").join(namespace)
            } else {
                std::path::PathBuf::from(base).join(namespace)
            };
            let pipe_lock = if cfg!(windows) {
                windows_pipe_lock(&std::env::temp_dir(), namespace)
            } else {
                std::path::PathBuf::from(format!("{socket}.lock"))
            };
            let expected = AppPaths {
                user_home: None,
                home: None,
                cache_dir: cache.into(),
                state_dir: state_dir.clone(),
                config_file: None,
                socket,
                pid_file: pid,
                pipe_lock,
                telemetry_dir: None,
                skills_dir: None,
                history_root: state_dir.join("computer-history"),
            };
            assert_eq!(paths, expected, "{namespace}");
        }
    }

    /// Removed once every caller reads `AppPaths`: the old resolvers and
    /// `AppPaths` agree under this test process's environment.
    #[test]
    fn app_paths_agree_with_the_old_resolvers() {
        let paths = app_paths();
        assert_eq!(paths.socket, crate::serve::default_socket_path());
        assert_eq!(paths.pid_file, crate::serve::default_pid_file_path());
        assert_eq!(
            paths.pipe_lock,
            cua_driver_core::daemon::daemon_lock_path(&paths.socket)
        );
        assert_eq!(paths.history_root, crate::history_runtime::history_root());
        assert_eq!(
            AppPaths::for_namespace(RELEASE_NAMESPACE).config_file,
            cua_driver_core::config_file::default_config_path()
        );
        assert_eq!(
            paths.home,
            paths
                .user_home
                .map(|home| home.join(user_home_subdirectory()))
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn env_truthiness_is_strict() {
        let name = "CUA_DRIVER_RS_TEST_TRUTHY";
        let mut env = crate::env_vars::Overrides::new();
        for value in ["1", "true", "TRUE", "Yes", "on", " 1 "] {
            env.set(name, value);
            assert!(is_env_truthy(name), "expected truthy for {value:?}");
        }
        for value in ["0", "false", "no", "off", ""] {
            env.set(name, value);
            assert!(!is_env_truthy(name), "expected falsy for {value:?}");
        }
    }
}
