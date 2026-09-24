//! CLI transport: one `cua-driver call <tool>` process per action, all backed
//! by a shared test-owned daemon.
//!
//! Each shell call is its own process, while tool state and enforcement live in
//! the daemon just as they do in production.
//! Args are piped via **stdin** rather than a positional arg, which the CLI
//! accepts and which dodges PowerShell 5.1's quote-stripping on JSON (see #1637).

use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::daemon::TestDaemon;
use crate::driver::Driver;
use crate::paths::driver_binary;
use crate::reaper::ChildReaper;
use crate::response::ToolResponse;

/// Drives cua-driver over the stateless CLI surface.
pub struct CliDriver {
    bin: std::path::PathBuf,
    _reaper: Option<ChildReaper>,
    daemon: Option<TestDaemon>,
}

impl CliDriver {
    pub fn new() -> Self {
        Self::with_daemon_env(&[])
    }

    /// Drive an explicitly selected binary.
    ///
    /// Integration tests that use Cargo's `CARGO_BIN_EXE_*` value should use
    /// this constructor so a restored `target/` artifact cannot be mistaken
    /// for the binary Cargo built for the test.
    pub fn with_binary(bin: impl Into<std::path::PathBuf>) -> Self {
        Self::with_binary_and_daemon_env(bin.into(), &[])
    }

    /// Start a test-owned daemon with explicit immutable startup settings.
    /// This is used for permission-mode and policy tests; tool-call child
    /// processes remain ordinary clients and do not receive these values.
    pub fn with_daemon_env(env: &[(&str, &str)]) -> Self {
        Self::with_binary_and_daemon_env(driver_binary(), env)
    }

    fn with_binary_and_daemon_env(bin: std::path::PathBuf, env: &[(&str, &str)]) -> Self {
        if !bin.exists() {
            return CliDriver {
                bin,
                _reaper: None,
                daemon: None,
            };
        }
        let mut reaper = ChildReaper::new();
        let daemon = TestDaemon::spawn(&bin, &mut reaper, env);
        CliDriver {
            bin,
            _reaper: Some(reaper),
            daemon,
        }
    }

    /// Whether the driver binary exists (caller should skip the test if not).
    pub fn available(&self) -> bool {
        self.bin.exists() && self.daemon.is_some()
    }

    /// Isolated per-user state root given to the test-owned daemon, or `None`
    /// when the caller passed [`crate::SHARE_HOST_STATE`].
    pub fn state_root(&self) -> Option<&std::path::Path> {
        self.daemon.as_ref().and_then(TestDaemon::state_root)
    }

    pub fn daemon_socket(&self) -> Option<&str> {
        self.daemon.as_ref().map(|daemon| daemon.socket.as_str())
    }
}

impl Default for CliDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl Driver for CliDriver {
    fn call(&mut self, tool: &str, args: Value) -> ToolResponse {
        let Some(daemon) = &self.daemon else {
            return ToolResponse::new(
                "test daemon unavailable".into(),
                Value::Null,
                true,
                Value::Null,
            );
        };
        let mut command = Command::new(&self.bin);
        command
            .arg("call")
            .arg(tool)
            .args(["--socket", &daemon.socket])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        daemon.apply_state_root(&mut command);
        let mut child = match command.spawn() {
            Ok(c) => c,
            Err(e) => {
                let msg = format!("spawn failed: {e}");
                return ToolResponse::new(msg, Value::Null, true, Value::Null);
            }
        };

        if let Some(mut stdin) = child.stdin.take() {
            let _ = writeln!(stdin, "{}", serde_json::to_string(&args).unwrap());
        }
        let out = match child.wait_with_output() {
            Ok(o) => o,
            Err(e) => {
                let msg = format!("wait failed: {e}");
                return ToolResponse::new(msg, Value::Null, true, Value::Null);
            }
        };

        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        // The CLI prints structuredContent (pretty JSON) or plain text — parse
        // when it's JSON, else keep it as text.
        let structured = serde_json::from_str::<Value>(&stdout).unwrap_or(Value::Null);
        let is_error = !out.status.success();
        let text = if stdout.is_empty() && is_error {
            stderr
        } else {
            stdout
        };
        ToolResponse::new(text, structured, is_error, Value::Null)
    }
}
