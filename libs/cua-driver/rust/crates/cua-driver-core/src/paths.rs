// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Where the driver keeps its own per-user files.
//!
//! [`AppPaths`] is the one place that reads `HOME`, `USERPROFILE`,
//! `LOCALAPPDATA` and the XDG base directories to locate driver state. Paths
//! that find other apps (installed-app discovery, browser profiles) stay in
//! the platform crates.
//!
//! Each field keeps the exact rule its old call site used, including the
//! fallbacks, so moving a caller here changes no resolved path.

use std::ffi::OsString;
use std::path::PathBuf;

/// Namespace of the released product.
pub const RELEASE_NAMESPACE: &str = "cua-driver";
/// Namespace of the explicitly installed source build.
pub const LOCAL_NAMESPACE: &str = "cua-driver-local";

/// Overrides every product-home location that honors it.
const DRIVER_HOME_OVERRIDE: &str = "CUA_DRIVER_RS_HOME";
/// Overrides the telemetry directory only.
const TELEMETRY_HOME_OVERRIDE: &str = "CUA_DRIVER_TELEMETRY_HOME";

/// The driver's per-user locations for one installed-product namespace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppPaths {
    /// The user's home: `HOME`, else `USERPROFILE`.
    pub user_home: Option<PathBuf>,
    /// The product home, `<user_home>/.<namespace>`.
    pub home: Option<PathBuf>,
    /// Per-user cache directory that holds the socket and pid file.
    pub cache_dir: PathBuf,
    /// Per-user state directory.
    pub state_dir: PathBuf,
    /// `<home>/config.json`.
    pub config_file: Option<PathBuf>,
    /// Daemon socket path, or the named pipe on Windows.
    pub socket: String,
    /// Daemon pid file.
    pub pid_file: String,
    /// Lock the daemon holds on [`Self::socket`] while it runs.
    pub pipe_lock: PathBuf,
    /// Telemetry identity and markers: `CUA_DRIVER_TELEMETRY_HOME`, else
    /// [`Self::home`].
    pub telemetry_dir: Option<PathBuf>,
    /// Local skill-pack stage: `CUA_DRIVER_RS_HOME/skills`, else the product
    /// home's `skills`. On Windows this product home is under `USERPROFILE`
    /// even when `HOME` is set.
    pub skills_dir: Option<PathBuf>,
    /// Computer-history store: `CUA_DRIVER_RS_HOME/computer-history`, else
    /// [`Self::state_dir`]'s.
    pub history_root: PathBuf,
}

impl AppPaths {
    /// Resolve every location for `namespace` from the process environment.
    pub fn for_namespace(namespace: &str) -> Self {
        Self::from_env(namespace, &process_env)
    }

    /// Resolve every location for `namespace`, reading variables through
    /// `env`.
    pub fn from_env(namespace: &str, env: &dyn Fn(&str) -> Option<OsString>) -> Self {
        let user_home = env("HOME")
            .or_else(|| env("USERPROFILE"))
            .map(PathBuf::from);
        let home = user_home
            .as_ref()
            .map(|user_home| user_home.join(format!(".{namespace}")));
        let cache_dir = cache_dir(namespace, env);
        let socket = socket(namespace, &cache_dir);
        let pid_file = pid_file(namespace, &cache_dir);
        let state_dir = state_dir(namespace, env);
        let history_root = env(DRIVER_HOME_OVERRIDE)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| state_dir.clone())
            .join("computer-history");
        let skills_dir = skills_home(namespace, env).map(|home| home.join("skills"));
        Self {
            config_file: home.as_ref().map(|home| home.join("config.json")),
            telemetry_dir: env(TELEMETRY_HOME_OVERRIDE)
                .map(PathBuf::from)
                .or_else(|| home.clone()),
            pipe_lock: pipe_lock(&socket, env),
            cache_dir: PathBuf::from(cache_dir),
            user_home,
            home,
            state_dir,
            socket,
            pid_file,
            skills_dir,
            history_root,
        }
    }

    /// The lock a daemon holds on an arbitrary `socket` path, such as one
    /// given with `--socket`.
    pub fn pipe_lock_for(socket: &str) -> PathBuf {
        pipe_lock(socket, &process_env)
    }
}

fn process_env(name: &str) -> Option<OsString> {
    std::env::var_os(name)
}

/// A variable as UTF-8, the way `std::env::var(..).ok()` reads it.
fn var_string(env: &dyn Fn(&str) -> Option<OsString>, name: &str) -> Option<String> {
    env(name).and_then(|value| value.into_string().ok())
}

fn cache_dir(namespace: &str, env: &dyn Fn(&str) -> Option<OsString>) -> String {
    #[cfg(target_os = "macos")]
    {
        let home = var_string(env, "HOME").unwrap_or_else(|| "/tmp".into());
        format!("{home}/Library/Caches/{namespace}")
    }
    #[cfg(target_os = "linux")]
    {
        let home = var_string(env, "HOME").unwrap_or_else(|| "/tmp".into());
        format!("{home}/.cache/{namespace}")
    }
    #[cfg(target_os = "windows")]
    {
        let local = var_string(env, "LOCALAPPDATA").unwrap_or_else(|| "C:/Temp".into());
        format!("{local}/{namespace}")
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = (namespace, env);
        "/tmp".to_owned()
    }
}

fn socket(namespace: &str, cache_dir: &str) -> String {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        format!("{cache_dir}/{namespace}.sock")
    }
    #[cfg(target_os = "windows")]
    {
        let _ = cache_dir;
        format!(r"\\.\pipe\{namespace}")
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = cache_dir;
        format!("/tmp/{namespace}.sock")
    }
}

fn pid_file(namespace: &str, cache_dir: &str) -> String {
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    {
        format!("{cache_dir}/{namespace}.pid")
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = (namespace, cache_dir);
        "/tmp/cua-driver.pid".to_owned()
    }
}

fn state_dir(namespace: &str, env: &dyn Fn(&str) -> Option<OsString>) -> PathBuf {
    #[cfg(target_os = "macos")]
    let root = env("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("Library")
        .join("Application Support");
    #[cfg(target_os = "linux")]
    let root = env("XDG_STATE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| env("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    #[cfg(target_os = "windows")]
    let root = env("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:/Temp"));
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let root = {
        let _ = env;
        PathBuf::from("/tmp")
    };
    root.join(namespace)
}

fn skills_home(namespace: &str, env: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    if let Some(home) = var_string(env, DRIVER_HOME_OVERRIDE) {
        return Some(PathBuf::from(home));
    }
    let user_home = if cfg!(windows) {
        var_string(env, "USERPROFILE")
    } else {
        var_string(env, "HOME")
    }?;
    Some(PathBuf::from(user_home).join(format!(".{namespace}")))
}

fn pipe_lock(socket: &str, env: &dyn Fn(&str) -> Option<OsString>) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        // `%LOCALAPPDATA%` stays the same across a user's sessions where
        // `%TEMP%` may not. The file name is a fixed-length digest of the
        // uppercased name, because Windows compares pipe names
        // case-insensitively and a name can outgrow a path component.
        use sha2::Digest as _;
        let base = env("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let digest = sha2::Sha256::digest(socket.to_uppercase().as_bytes());
        base.join("cua-driver-pipe-locks")
            .join(format!("{digest:x}.lock"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = env;
        PathBuf::from(format!("{socket}.lock"))
    }
}
