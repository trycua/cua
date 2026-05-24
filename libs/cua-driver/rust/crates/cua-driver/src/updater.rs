//! `cua-driver update --apply` implementation.
//!
//! Delegates the actual install work to the canonical installer scripts:
//! - Unix:    `libs/cua-driver/scripts/install.sh` (delegates to
//!            `_install-rust.sh` when `--backend=rust`)
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

/// Canonical install-script URLs. Match what the docs print as the one-liner;
/// users who run `cua-driver update --apply` and re-run the printed manual
/// command land at the exact same script. Per-OS gating keeps the unused
/// constant from triggering `dead_code` on the platform that doesn't use it.
#[cfg(not(windows))]
const CANONICAL_INSTALL_SH: &str =
    "https://raw.githubusercontent.com/trycua/cua/main/libs/cua-driver/scripts/install.sh";
#[cfg(windows)]
const CANONICAL_INSTALL_PS1: &str =
    "https://raw.githubusercontent.com/trycua/cua/main/libs/cua-driver/scripts/install.ps1";

/// The env var both scripts honour to pin the target release tag. Set to a
/// bare version like `"0.2.18"` (no `cua-driver-rs-v` prefix). See
/// `libs/cua-driver/scripts/_install-rust.sh` + `install.ps1`.
const VERSION_PIN_ENV: &str = "CUA_DRIVER_RS_VERSION";

/// Invoke the canonical installer pinned to `version`. Returns the
/// installer's exit status so the caller can produce the right
/// "succeeded / failed — re-run manually" message.
pub fn run_install_script(version: &str) -> std::io::Result<ExitStatus> {
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
        // Match the canonical curl-piped-to-bash invocation. `--backend=rust`
        // is the explicit selector — without it the canonical install.sh
        // auto-detects on macOS and would install the Swift driver instead.
        let bash_cmd = format!(
            "curl -fsSL {CANONICAL_INSTALL_SH} | bash -s -- install --backend=rust"
        );
        Command::new("bash")
            .env(VERSION_PIN_ENV, version)
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
        format!("curl -fsSL {CANONICAL_INSTALL_SH} | bash -s -- install --backend=rust")
    }
}
