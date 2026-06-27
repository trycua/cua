//! MCP transport: one long-lived `cua-driver` server over stdio JSON-RPC.

use std::io::{BufRead, BufReader, Write};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::driver::Driver;
use crate::paths::driver_binary;
use crate::reaper::{spawn_in_job, ChildReaper};
use crate::response::ToolResponse;
use crate::CALL_TIMEOUT;

/// A spawned cua-driver MCP server. State (e.g. `set_config`) persists for the
/// lifetime of this one connection — which is why config-scope tests drive a
/// single `McpDriver`. The server (and any apps spawned through [`reaper`]) are
/// reaped when this value drops.
///
/// [`reaper`]: McpDriver::reaper
pub struct McpDriver {
    reaper: ChildReaper,
    stdin: ChildStdin,
    rx: Receiver<String>,
    next_id: u32,
}

impl McpDriver {
    /// Spawn the driver, start the stdout reader thread, and `initialize`.
    /// Returns `None` (with a skip message) if the binary isn't built — the
    /// caller's test should early-return so an un-built binary skips, not fails.
    pub fn spawn() -> Option<Self> {
        let bin = driver_binary();
        if !bin.exists() {
            eprintln!("[testkit] driver binary not built at {bin:?} — skipping");
            return None;
        }

        let mut reaper = ChildReaper::new();
        let mut driver = spawn_in_job(
            Command::new(&bin)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null()),
        )
        .inspect_err(|e| eprintln!("[testkit] driver spawn failed: {e}"))
        .ok()?;
        let stdin = driver.stdin.take().unwrap();
        let stdout = driver.stdout.take().unwrap();
        reaper.push(driver);

        let (tx, rx) = channel::<String>();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if tx.send(line.trim().to_string()).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        let mut d = McpDriver { reaper, stdin, rx, next_id: 2 };
        d.initialize();
        Some(d)
    }

    fn initialize(&mut self) {
        self.send(serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}
        }));
        let _ = self.rx.recv_timeout(CALL_TIMEOUT);
    }

    fn send(&mut self, req: Value) {
        let _ = writeln!(self.stdin, "{}", serde_json::to_string(&req).unwrap());
        let _ = self.stdin.flush();
    }

    /// Mutable access to the child reaper, e.g. to launch a target app whose
    /// lifetime should be tied to this driver.
    pub fn reaper(&mut self) -> &mut ChildReaper {
        &mut self.reaper
    }

    /// Poll `list_windows` until a window of `pid` whose title contains
    /// `title_substr` appears (up to ~12s). Returns `(window_id, title)`.
    /// Replaces the per-file `find_harness_window` helper.
    pub fn find_window(&mut self, pid: i64, title_substr: &str) -> Option<(u64, String)> {
        let deadline = Instant::now() + Duration::from_secs(12);
        loop {
            let r = self.call("list_windows", serde_json::json!({ "pid": pid }));
            if let Some(wins) = r.structured()["windows"].as_array() {
                for w in wins {
                    if w["pid"].as_i64() != Some(pid) {
                        continue;
                    }
                    let title = w["title"].as_str().unwrap_or("");
                    if title.contains(title_substr) {
                        return Some((w["window_id"].as_u64()?, title.to_string()));
                    }
                }
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(150));
        }
    }

    /// Raw JSON-RPC response envelope, for the rare assertion needing it.
    pub fn call_raw(&mut self, tool: &str, args: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(serde_json::json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": { "name": tool, "arguments": args }
        }));
        match self.rx.recv_timeout(CALL_TIMEOUT) {
            Ok(line) => serde_json::from_str(&line)
                .unwrap_or_else(|_| serde_json::json!({ "error": format!("bad json: {line}") })),
            Err(_) => serde_json::json!({
                "error": format!("TIMEOUT (>{}s) on {tool}", CALL_TIMEOUT.as_secs())
            }),
        }
    }
}

impl Driver for McpDriver {
    fn call(&mut self, tool: &str, args: Value) -> ToolResponse {
        ToolResponse::from_mcp(self.call_raw(tool, args))
    }
}
