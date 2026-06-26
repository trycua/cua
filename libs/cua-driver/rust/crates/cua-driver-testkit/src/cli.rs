//! CLI transport: a stateless `cua-driver call <tool>` process per action.
//!
//! Each call is its own process — no state carries between calls (the property
//! that makes `set_config` disk-persistence observable here but not over MCP).
//! Args are piped via **stdin** rather than a positional arg, which the CLI
//! accepts and which dodges PowerShell 5.1's quote-stripping on JSON (see #1637).

use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::driver::Driver;
use crate::paths::driver_binary;
use crate::response::ToolResponse;

/// Drives cua-driver over the stateless CLI surface.
pub struct CliDriver {
    bin: std::path::PathBuf,
}

impl CliDriver {
    pub fn new() -> Self {
        CliDriver { bin: driver_binary() }
    }

    /// Whether the driver binary exists (caller should skip the test if not).
    pub fn available(&self) -> bool {
        self.bin.exists()
    }
}

impl Default for CliDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl Driver for CliDriver {
    fn call(&mut self, tool: &str, args: Value) -> ToolResponse {
        let mut child = match Command::new(&self.bin)
            .arg("call")
            .arg(tool)
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
