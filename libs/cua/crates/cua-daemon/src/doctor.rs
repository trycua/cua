//! `cua runtime doctor`: inspects local runtime components without changing
//! anything. It only looks for executables on `PATH`; it never runs them.
//!
//! `setup` needs cua-vmm (the zero-pre-setup orchestrator) and reports
//! `Unsupported` until it is merged.

use cua_proto::daemon::v1::{CheckStatus, RuntimeCheck};
use std::path::{Path, PathBuf};

/// Components the local runtimes use, per host OS.
fn components() -> Vec<(&'static str, &'static [&'static str], bool)> {
    // (name, executables, applicable on this host)
    let mac = cfg!(target_os = "macos");
    let linux = cfg!(target_os = "linux");
    vec![
        ("lume", &["lume"][..], mac),
        (
            "qemu",
            &["qemu-system-x86_64", "qemu-system-aarch64"][..],
            true,
        ),
        ("qemu-img", &["qemu-img"][..], true),
        ("docker", &["docker", "podman"][..], true),
        ("runsc", &["runsc"][..], linux),
        ("colima", &["colima"][..], mac),
    ]
}

fn exe_names(name: &str) -> Vec<String> {
    if cfg!(windows) {
        vec![format!("{name}.exe"), name.to_string()]
    } else {
        vec![name.to_string()]
    }
}

/// First `name` on `path`.
pub fn which_in(path: &std::ffi::OsStr, name: &str) -> Option<PathBuf> {
    for dir in std::env::split_paths(path) {
        for n in exe_names(name) {
            let candidate = dir.join(&n);
            if is_executable(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

fn is_executable(p: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(p) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    true
}

/// Runs every check against `PATH` (or `path` when given).
pub fn doctor(path: Option<&std::ffi::OsStr>) -> Vec<RuntimeCheck> {
    let env_path = std::env::var_os("PATH").unwrap_or_default();
    let path = path.unwrap_or(&env_path);
    components()
        .into_iter()
        .map(|(name, exes, applicable)| {
            let found = exes.iter().find_map(|e| which_in(path, e));
            let (status, detail) = match (found, applicable) {
                (Some(p), _) => (CheckStatus::Ok, format!("found {}", p.display())),
                (None, false) => (
                    CheckStatus::NotApplicable,
                    "not used on this host".to_string(),
                ),
                (None, true) => (
                    CheckStatus::Installable,
                    "missing; `cua runtime setup` will provision it once cua-vmm lands".to_string(),
                ),
            };
            RuntimeCheck {
                name: name.into(),
                status: status as i32,
                version: String::new(),
                detail,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_executables_on_a_fake_path() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join(if cfg!(windows) {
            "docker.exe"
        } else {
            "docker"
        });
        std::fs::write(&exe, b"#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let checks = doctor(Some(dir.path().as_os_str()));
        let docker = checks.iter().find(|c| c.name == "docker").unwrap();
        assert_eq!(docker.status, CheckStatus::Ok as i32);
        let qemu = checks.iter().find(|c| c.name == "qemu").unwrap();
        assert_eq!(qemu.status, CheckStatus::Installable as i32);
    }
}
