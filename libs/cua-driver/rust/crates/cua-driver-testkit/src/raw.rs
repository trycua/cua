//! Raw JSON-RPC transport — lockstep send/recv with **no** auto-initialize.
//!
//! [`crate::McpDriver`] auto-`initialize`s on spawn and only exposes
//! `tools/call`, which is the right ergonomics for behavior tests. The MCP
//! *protocol* tests need the opposite: drive the `initialize` handshake
//! themselves, send arbitrary methods (`tools/list`, `unknown/method`,
//! malformed frames), and read each raw response line in order. `RawDriver`
//! gives them exactly that, shared across the `protocol_*` test files so they
//! don't each re-implement `send_request`/`read_response`.

use std::io::{BufRead, BufReader, Write};
use std::process::{ChildStdin, ChildStdout, Command, Stdio};

use serde_json::Value;

use crate::daemon::TestDaemon;
use crate::paths::driver_binary;
use crate::reaper::{spawn_in_job, ChildReaper};

/// A spawned cua-driver with raw stdio access and no handshake performed.
/// Killed on drop.
pub struct RawDriver {
    _reaper: ChildReaper,
    _daemon: Option<TestDaemon>,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl RawDriver {
    /// Spawn the driver with piped stdio. Returns `None` (with a skip eprintln)
    /// if the binary isn't built — callers early-return so an un-built binary
    /// skips rather than fails.
    pub fn spawn() -> Option<Self> {
        Self::spawn_daemon_backed(false, &[])
    }

    /// Spawn the daemon-backed driver with an explicit test environment.
    ///
    /// Permission-mode tests use this to model a trusted host's launch-time
    /// configuration without mutating the test process environment.
    pub fn spawn_with_env(env: &[(&str, &str)]) -> Option<Self> {
        Self::spawn_daemon_backed(false, env)
    }

    /// Spawn a daemon-backed raw driver with the certified platform overlay
    /// host enabled. Cursor protocol tests use this deliberately; ordinary
    /// protocol tests keep the no-overlay daemon so they remain headless.
    pub fn spawn_with_overlay() -> Option<Self> {
        Self::spawn_daemon_backed(true, &[])
    }

    /// Spawn an overlay-enabled daemon with explicit trusted launch settings.
    pub fn spawn_with_overlay_and_env(env: &[(&str, &str)]) -> Option<Self> {
        Self::spawn_daemon_backed(true, env)
    }

    fn spawn_daemon_backed(overlay_enabled: bool, env: &[(&str, &str)]) -> Option<Self> {
        let bin = driver_binary();
        if !bin.exists() {
            eprintln!("[testkit] driver binary not built at {bin:?} — skipping");
            return None;
        }
        let mut reaper = ChildReaper::new();
        let daemon = if overlay_enabled {
            TestDaemon::spawn_with_overlay(&bin, &mut reaper, env)?
        } else {
            TestDaemon::spawn(&bin, &mut reaper, env)?
        };
        let mut command = Command::new(&bin);
        command
            .args(["mcp", "--socket", &daemon.socket])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = spawn_in_job(&mut command)
            .inspect_err(|e| eprintln!("[testkit] driver spawn failed: {e}"))
            .ok()?;
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        reaper.push(child);
        Some(RawDriver {
            _reaper: reaper,
            _daemon: Some(daemon),
            stdin,
            stdout,
        })
    }

    /// Spawn the platform-default direct MCP runtime without a service.
    ///
    /// Standalone macOS intentionally retains the signed app/service topology,
    /// so this helper is available only where bare `mcp` owns its runtime.
    #[cfg(not(target_os = "macos"))]
    pub fn spawn_direct() -> Option<Self> {
        Self::spawn_direct_with_args(&["mcp"])
    }

    /// Spawn the explicitly selected direct MCP runtime without a service.
    ///
    /// Unlike [`Self::spawn_direct`], this is available on macOS because the
    /// caller has deliberately opted out of the signed app/service default.
    pub fn spawn_explicit_direct() -> Option<Self> {
        Self::spawn_direct_with_args(&["mcp", "--direct"])
    }

    fn spawn_direct_with_args(args: &[&str]) -> Option<Self> {
        let bin = driver_binary();
        if !bin.exists() {
            eprintln!("[testkit] driver binary not built at {bin:?} — skipping");
            return None;
        }
        let mut reaper = ChildReaper::new();
        let mut command = Command::new(&bin);
        command
            .args(args)
            .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "false")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = spawn_in_job(&mut command)
            .inspect_err(|e| eprintln!("[testkit] direct driver spawn failed: {e}"))
            .ok()?;
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        reaper.push(child);
        Some(Self {
            _reaper: reaper,
            _daemon: None,
            stdin,
            stdout,
        })
    }

    /// Write one JSON-RPC frame (newline-delimited) and flush.
    pub fn send(&mut self, req: &Value) {
        writeln!(self.stdin, "{}", serde_json::to_string(req).unwrap()).unwrap();
        let _ = self.stdin.flush();
    }

    /// Read and parse the next response line. Panics on read/parse failure
    /// (a malformed or missing response is a protocol-test failure).
    pub fn recv(&mut self) -> Value {
        let mut line = String::new();
        self.stdout
            .read_line(&mut line)
            .expect("read response line");
        serde_json::from_str(line.trim()).expect("parse JSON response")
    }
}
