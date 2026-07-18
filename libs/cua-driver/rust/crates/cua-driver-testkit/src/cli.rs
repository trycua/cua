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
        let bin = driver_binary();
        if !bin.exists() {
            return CliDriver {
                bin,
                _reaper: None,
                daemon: None,
            };
        }
        let mut reaper = ChildReaper::new();
        let daemon = TestDaemon::spawn(&bin, &mut reaper, &[]);
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
        let mut child = match Command::new(&self.bin)
            .arg("call")
            .arg(tool)
            .args(["--socket", &daemon.socket])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
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
        // The CLI prints structuredContent (pretty JSON) or plain text — parse
        // when it's JSON, else keep it as text.
        let structured = serde_json::from_str::<Value>(&stdout).unwrap_or(Value::Null);
        let is_error = !out.status.success();
        ToolResponse::new(stdout, structured, is_error, Value::Null)
    }
}
