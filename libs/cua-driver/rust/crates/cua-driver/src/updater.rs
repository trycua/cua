//! `cua-driver update --apply` implementation.
//!
//! Delegates the actual install work to the canonical installer scripts:
//! - Unix: `libs/cua-driver/scripts/install.sh` (delegates to
//!   `_install-rust.sh` by default)
//! - Windows: `libs/cua-driver/scripts/install.ps1`
//!
//! Why not reimplement the download / atomic-swap / GC in Rust? Those scripts
//! already solve the hard problems:
//! - target-triple → asset-name mapping (per-OS, per-arch)
//! - per-version dir layout (`packages/releases/<version>-<target>/`)
//! - atomic upgrade — symlink retarget on Unix, NTFS directory-junction
//!   retarget on Windows. A running daemon survives the swap because the
//!   kernel keeps the old inode alive (Unix) or the junction flip is a
//!   reparse-point swap that doesn't touch the locked .exe (Windows).
//! - GC of stale per-version dirs (`CUA_DRIVER_RS_KEEP_VERSIONS`)
//! - PATH wiring
//!
//! Treating "update" as a pinned re-install with `CUA_DRIVER_RS_VERSION` set
//! keeps install + update reading from one source of truth. Improvements to
//! the on-disk layout ship in the scripts and benefit both code paths.

use std::process::{Command, ExitStatus};

pub(crate) const PACMAN_UPDATE_GUIDANCE: &str =
    "This executable is managed by pacman. Update with `sudo pacman -Syu`; \
     release selection and availability are controlled by your package repository. \
     The upstream installer and stable/nightly channel switching are disabled.";

/// Only positive package ownership disables the upstream updater. Missing
/// pacman, failed queries, and unresolved paths retain unmanaged behavior.
pub(crate) fn is_pacman_managed() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::env::current_exe()
            .map(|path| pacman_owns_executable(&path, std::path::Path::new("/usr/bin/pacman")))
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

#[cfg(any(target_os = "linux", all(test, unix)))]
fn pacman_owns_executable(executable: &std::path::Path, pacman: &std::path::Path) -> bool {
    if pacman_owns_path(executable, pacman) {
        return true;
    }
    executable
        .canonicalize()
        .is_ok_and(|resolved| resolved != executable && pacman_owns_path(&resolved, pacman))
}

#[cfg(any(target_os = "linux", all(test, unix)))]
fn pacman_owns_path(path: &std::path::Path, pacman: &std::path::Path) -> bool {
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    let Ok(mut child) = Command::new(pacman)
        .args(["-Qoq", "--"])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let deadline = Instant::now() + Duration::from_millis(500);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

/// Canonical install-script URLs. Match what the docs print as the one-liner;
/// users who run `cua-driver update --apply` and re-run the printed manual
/// command land at the exact same script. Per-OS gating keeps the unused
/// constant from triggering `dead_code` on the platform that doesn't use it.
#[cfg(not(windows))]
const CANONICAL_INSTALL_SH: &str = "https://cua.ai/driver/install.sh";
#[cfg(windows)]
const CANONICAL_INSTALL_PS1: &str = "https://cua.ai/driver/install.ps1";

/// The env var both scripts honour to pin the target release tag. Set to a
/// bare version like `"0.2.18"` (no `cua-driver-rs-v` prefix). See
/// `libs/cua-driver/scripts/_install-rust.sh` + `install.ps1`.
const VERSION_PIN_ENV: &str = "CUA_DRIVER_RS_VERSION";
const INSTALL_CHANNEL_ENV: &str = "CUA_DRIVER_INSTALL_CHANNEL";
const RELEASE_VERSION_ENV: &str = "CUA_DRIVER_RELEASE_VERSION";

/// Invoke the canonical installer pinned to `version`. Returns the
/// installer's exit status so the caller can produce the right
/// "succeeded / failed — re-run manually" message.
pub fn run_install_script(version: &str) -> std::io::Result<ExitStatus> {
    run_install_script_with_ownership(version, is_pacman_managed())
}

fn run_install_script_with_ownership(version: &str, managed: bool) -> std::io::Result<ExitStatus> {
    if managed {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            PACMAN_UPDATE_GUIDANCE,
        ));
    }
    #[cfg(windows)]
    {
        // Match the documented Windows one-liner: `irm <url> | iex`.
        // -ExecutionPolicy Bypass lets the downloaded script run on
        // machines with the default restricted policy without requiring
        // the user to Set-ExecutionPolicy first. -NoProfile keeps any
        // user profile script from racing the install.
        let pwsh_cmd = format!("iwr -useb {CANONICAL_INSTALL_PS1} | iex");
        Command::new("powershell.exe")
            .env(VERSION_PIN_ENV, version)
            .env(INSTALL_CHANNEL_ENV, "update_apply")
            .env(RELEASE_VERSION_ENV, version)
            .args([
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                &pwsh_cmd,
            ])
            .status()
    }

    #[cfg(not(windows))]
    {
        // Match the canonical curl-piped-to-bash invocation. install.sh
        // delegates to the Rust implementation by default.
        let bash_cmd = format!("curl -fsSL {CANONICAL_INSTALL_SH} | bash");
        Command::new("bash")
            .env(VERSION_PIN_ENV, version)
            .env(INSTALL_CHANNEL_ENV, "update_apply")
            .env(RELEASE_VERSION_ENV, version)
            .args(["-c", &bash_cmd])
            .status()
    }
}

/// True if the local cua-driver daemon is currently accepting connections
/// on its default socket / named pipe. Used post-install to decide whether
/// to print the "restart the daemon to pick up the new binary" hint.
pub fn daemon_is_running() -> bool {
    crate::serve::is_daemon_listening(&crate::serve::default_socket_path())
}

/// The platform-appropriate manual re-install command, used in both the
/// "available, run --apply" preview and the "apply failed, retry manually"
/// error message. Kept here so both messages stay in sync.
pub fn manual_install_one_liner() -> String {
    #[cfg(windows)]
    {
        format!("irm {CANONICAL_INSTALL_PS1} | iex")
    }
    #[cfg(not(windows))]
    {
        format!("curl -fsSL {CANONICAL_INSTALL_SH} | bash")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_install_never_starts_installer() {
        let error = run_install_script_with_ownership("0.24.0", true).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(error.to_string().contains("sudo pacman -Syu"));
    }

    #[cfg(unix)]
    fn fake_pacman(root: &std::path::Path, script: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = root.join("pacman");
        std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(unix)]
    #[test]
    fn ownership_requires_successful_query_with_literal_path() {
        let root = tempfile::tempdir().unwrap();
        let pacman = fake_pacman(
            root.path(),
            "[ \"$1\" = -Qoq ] && [ \"$2\" = -- ] && [ \"$3\" = '/usr/bin/driver ; $(false)' ]",
        );
        assert!(pacman_owns_executable(
            std::path::Path::new("/usr/bin/driver ; $(false)"),
            &pacman
        ));
        assert!(!pacman_owns_executable(
            std::path::Path::new("/usr/bin/unmanaged"),
            &pacman
        ));
    }

    #[cfg(unix)]
    #[test]
    fn ownership_queries_symlink_target() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("packaged-driver");
        std::fs::write(&target, "fixture").unwrap();
        let link = root.path().join("driver");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let pacman = fake_pacman(
            root.path(),
            "case \"$3\" in */packaged-driver) exit 0;; *) exit 1;; esac",
        );
        assert!(pacman_owns_executable(&link, &pacman));
    }

    #[cfg(unix)]
    #[test]
    fn missing_failed_and_timed_out_queries_are_not_ownership() {
        let root = tempfile::tempdir().unwrap();
        let executable = root.path().join("missing-driver");
        assert!(!pacman_owns_executable(
            &executable,
            &root.path().join("missing-pacman")
        ));
        let pacman = fake_pacman(root.path(), "exit 2");
        assert!(!pacman_owns_executable(&executable, &pacman));
        fake_pacman(root.path(), "while :; do :; done");
        let started = std::time::Instant::now();
        assert!(!pacman_owns_executable(&executable, &pacman));
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }
}
