// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Pins every location `AppPaths` resolves to the value the driver resolved
//! before the locations moved into one place, per OS, from the testkit's
//! isolated home. A change here moves where clients look for the daemon and
//! where the driver keeps its state.

use cua_driver_core::paths::{AppPaths, LOCAL_NAMESPACE, RELEASE_NAMESPACE};

type Vars = std::collections::HashMap<&'static str, std::path::PathBuf>;

fn resolve(namespace: &str, vars: &Vars) -> AppPaths {
    AppPaths::from_env(namespace, &|name| {
        vars.get(name).map(|value| value.clone().into_os_string())
    })
}

/// The testkit's isolated home, with Windows' profile variables inside it.
/// `USERPROFILE` differs from `HOME` so the tests see which one each
/// location reads.
fn isolated_vars(root: &cua_driver_testkit::IsolatedStateRoot) -> Vars {
    let mut vars: Vars = root.env().into_iter().collect();
    if cfg!(windows) {
        vars.insert("USERPROFILE", root.path().join("profile"));
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
    // The daemon formats its strings from the variable as given, which
    // for the testkit's `HOME` ends in a separator.
    let base_variable = if cfg!(windows) {
        "LOCALAPPDATA"
    } else {
        "HOME"
    };
    let base = isolated[base_variable].to_str().unwrap().to_owned();
    let state_root = if cfg!(target_os = "macos") {
        home.join("Library").join("Application Support")
    } else if cfg!(windows) {
        local_app_data.clone()
    } else {
        home.join(".local/state")
    };

    // Skills alone read `USERPROFILE` on Windows even when `HOME` is set.
    let skills_home = if cfg!(windows) {
        home.join("profile")
    } else {
        home.to_owned()
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
            skills_dir: Some(skills_home.join(format!(".{namespace}")).join("skills")),
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

        // Skills take an empty override as given; history ignores it.
        let mut empty_override = defaults.clone();
        empty_override.insert("CUA_DRIVER_RS_HOME", std::path::PathBuf::new());
        assert_eq!(
            resolve(namespace, &empty_override),
            AppPaths {
                skills_dir: Some(std::path::PathBuf::from("skills")),
                ..expected.clone()
            },
            "{namespace} under an empty CUA_DRIVER_RS_HOME"
        );

        // Only Linux reads `XDG_STATE_HOME`, and only when it is not empty.
        let mut xdg_state = defaults.clone();
        xdg_state.insert("XDG_STATE_HOME", home.join("xdg-state"));
        let xdg_expected = if cfg!(target_os = "linux") {
            let state_dir = home.join("xdg-state").join(namespace);
            AppPaths {
                history_root: state_dir.join("computer-history"),
                state_dir,
                ..expected.clone()
            }
        } else {
            expected.clone()
        };
        assert_eq!(
            resolve(namespace, &xdg_state),
            xdg_expected,
            "{namespace} under XDG_STATE_HOME"
        );
        xdg_state.insert("XDG_STATE_HOME", std::path::PathBuf::new());
        assert_eq!(
            resolve(namespace, &xdg_state),
            expected,
            "{namespace} under an empty XDG_STATE_HOME"
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
