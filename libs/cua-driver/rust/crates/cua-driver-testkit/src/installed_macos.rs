// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A dedicated `--no-overlay` instance of the installed, TCC-authorized macOS
//! app, for E2E rows that need its grants without the canonical shared daemon.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// The installed Driver binary named by `CUA_E2E_INSTALLED_DRIVER_BIN`,
/// resolved through the `~/.local/bin` symlink to the app bundle.
pub fn installed_macos_binary() -> PathBuf {
    let binary = PathBuf::from(
        std::env::var_os("CUA_E2E_INSTALLED_DRIVER_BIN")
            .expect("CUA_E2E_INSTALLED_DRIVER_BIN must identify the installed, TCC-authorized app"),
    );
    assert!(binary.is_file(), "installed Driver is missing: {binary:?}");
    fs::canonicalize(&binary)
        .unwrap_or_else(|error| panic!("resolve installed Driver {binary:?}: {error}"))
}

/// A second instance of the installed app, sharing its TCC grants but not its
/// home, socket, or PID file. It serves with `--no-overlay`. The canonical
/// shared daemon is left untouched; this instance is stopped on drop.
pub struct InstalledMacosDaemon {
    binary: PathBuf,
    socket: String,
    _socket_dir: tempfile::TempDir,
}

impl InstalledMacosDaemon {
    pub fn start(home: &Path) -> Self {
        let binary = installed_macos_binary();
        let app = binary
            .ancestors()
            .nth(3)
            .filter(|path| path.extension().is_some_and(|ext| ext == "app"))
            .unwrap_or_else(|| panic!("installed Driver is not inside an app bundle: {binary:?}"))
            .to_path_buf();
        // Unix socket paths are short on macOS.
        let socket_dir = tempfile::Builder::new()
            .prefix("cua-p-")
            .tempdir_in("/tmp")
            .expect("create daemon socket directory");
        let socket = socket_dir.path().join("d.sock").display().to_string();
        let pid_file = socket_dir.path().join("d.pid").display().to_string();
        let log = home.join("daemon.log");
        let status = Command::new("open")
            .args(["-n", "-g"])
            .arg("--env")
            .arg(format!("CUA_DRIVER_RS_HOME={}", home.display()))
            .args(["--env", "CUA_DRIVER_RS_TELEMETRY_ENABLED=false"])
            .arg("--stderr")
            .arg(&log)
            .arg(&app)
            .args(["--args", "--socket", &socket, "--pid-file", &pid_file])
            .args([
                "serve",
                "--permission-mode",
                "unrestricted",
                "--dangerously-bypass-approvals",
                "--no-permissions-gate",
                "--no-overlay",
            ])
            .status()
            .expect("launch dedicated installed Driver instance");
        assert!(status.success(), "open failed for {app:?}: {status}");
        let daemon = Self {
            binary,
            socket,
            _socket_dir: socket_dir,
        };
        let deadline = Instant::now() + Duration::from_secs(45);
        loop {
            let output = Command::new(&daemon.binary)
                .args(["--socket", &daemon.socket, "status"])
                .stdin(Stdio::null())
                .output();
            if output.as_ref().is_ok_and(|output| {
                output.status.success()
                    && String::from_utf8_lossy(&output.stdout)
                        .contains("permission mode: unrestricted")
            }) {
                return daemon;
            }
            if Instant::now() >= deadline {
                panic!(
                    "dedicated installed Driver did not become ready: status={:?} log={}",
                    output.map(|output| String::from_utf8_lossy(&output.stdout).into_owned()),
                    fs::read_to_string(&log).unwrap_or_default()
                );
            }
            thread::sleep(Duration::from_millis(250));
        }
    }

    pub fn socket(&self) -> &str {
        &self.socket
    }
}

impl Drop for InstalledMacosDaemon {
    fn drop(&mut self) {
        let _ = Command::new(&self.binary)
            .args(["--socket", &self.socket, "stop"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}
