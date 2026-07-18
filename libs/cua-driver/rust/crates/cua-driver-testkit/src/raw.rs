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
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::Value;

use crate::paths::driver_binary;

/// A spawned cua-driver with raw stdio access and no handshake performed.
/// Killed on drop.
pub struct RawDriver {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl RawDriver {
    /// Spawn the driver with piped stdio. Returns `None` (with a skip eprintln)
    /// if the binary isn't built — callers early-return so an un-built binary
    /// skips rather than fails.
    pub fn spawn() -> Option<Self> {
        let bin = driver_binary();
        if !bin.exists() {
            eprintln!("[testkit] driver binary not built at {bin:?} — skipping");
            return None;
        }
        let mut child = Command::new(&bin)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .inspect_err(|e| eprintln!("[testkit] driver spawn failed: {e}"))
            .ok()?;
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Some(RawDriver {
            child,
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

impl Drop for RawDriver {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
