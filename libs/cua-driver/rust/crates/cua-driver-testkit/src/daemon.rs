//! Per-test daemon lifecycle for CLI and MCP transport fixtures.

use std::path::Path;
use std::process::{Command, Stdio};
#[cfg(not(unix))]
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::reaper::ChildReaper;

#[cfg(not(unix))]
static DAEMON_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// Keeps the temporary socket directory alive for the daemon lifetime.
pub(crate) struct TestDaemon {
    pub(crate) socket: String,
    #[cfg(unix)]
    _socket_dir: tempfile::TempDir,
}

impl TestDaemon {
    pub(crate) fn spawn(
        binary: &Path,
        reaper: &mut ChildReaper,
        env: &[(&str, &str)],
    ) -> Option<Self> {
        #[cfg(not(unix))]
        let sequence = DAEMON_SEQUENCE.fetch_add(1, Ordering::Relaxed);

        #[cfg(unix)]
        let (socket, socket_dir) = {
            // Unix-domain socket paths are short on macOS, so keep the test
            // directory directly under /tmp with a compact filename.
            let dir = tempfile::Builder::new()
                .prefix("cua-")
                .tempdir_in("/tmp")
                .inspect_err(|error| {
                    eprintln!("[testkit] create daemon socket directory failed: {error}")
                })
                .ok()?;
            (dir.path().join("d.sock").display().to_string(), dir)
        };

        #[cfg(target_os = "windows")]
        let socket = format!(
            r"\\.\pipe\cua-driver-test-{}-{sequence}",
            std::process::id()
        );

        #[cfg(not(any(unix, target_os = "windows")))]
        let socket = format!("cua-driver-test-{}-{sequence}", std::process::id());

        let stderr = if std::env::var_os("CUA_TEST_DRIVER_STDERR").is_some() {
            Stdio::inherit()
        } else {
            Stdio::null()
        };
        let mut command = Command::new(binary);
        command
            .args([
                "serve",
                "--socket",
                &socket,
                "--no-permissions-gate",
                "--no-overlay",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(stderr)
            .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "false");
        for (key, value) in env {
            command.env(key, value);
        }
        reaper
            .spawn(&mut command)
            .inspect_err(|error| eprintln!("[testkit] daemon spawn failed: {error}"))
            .ok()?;

        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if daemon_is_listening(binary, &socket) {
                return Some(Self {
                    socket,
                    #[cfg(unix)]
                    _socket_dir: socket_dir,
                });
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        eprintln!("[testkit] daemon did not become ready on {socket}");
        None
    }
}

#[cfg(unix)]
fn daemon_is_listening(_binary: &Path, socket: &str) -> bool {
    std::os::unix::net::UnixStream::connect(socket).is_ok()
}

#[cfg(not(unix))]
fn daemon_is_listening(binary: &Path, socket: &str) -> bool {
    Command::new(binary)
        .args(["status", "--socket", socket])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}
