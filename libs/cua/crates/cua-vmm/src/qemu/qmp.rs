//! Async QMP (QEMU Machine Protocol) client.
//!
//! Gives agentless access to a VM: framebuffer screenshots, keyboard/mouse
//! injection, power and pause control, and internal snapshots.

use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

use crate::error::{Result, VmmError};

const IO_TIMEOUT: Duration = Duration::from_secs(30);

/// One negotiated QMP session.
pub struct QmpClient {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
    /// Asynchronous events received while waiting for command replies.
    pub events: Vec<Value>,
}

impl QmpClient {
    /// Connect to `addr` (`127.0.0.1:port`) and negotiate capabilities.
    pub async fn connect(addr: &str) -> Result<Self> {
        let stream = tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(addr))
            .await
            .map_err(|_| VmmError::Qmp(format!("connect to {addr} timed out")))?
            .map_err(|e| VmmError::Qmp(format!("connect to {addr}: {e}")))?;
        let (r, w) = stream.into_split();
        let mut c = Self {
            reader: BufReader::new(r),
            writer: w,
            events: Vec::new(),
        };
        let greeting = c.read_msg().await?;
        if greeting.get("QMP").is_none() {
            return Err(VmmError::Qmp(format!("unexpected greeting: {greeting}")));
        }
        c.execute("qmp_capabilities", None).await?;
        Ok(c)
    }

    /// Connect, retrying until `timeout` (QEMU opens the socket slightly after
    /// the process starts).
    pub async fn connect_retry(addr: &str, timeout: Duration) -> Result<Self> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            match Self::connect(addr).await {
                Ok(c) => return Ok(c),
                Err(e) if tokio::time::Instant::now() >= deadline => return Err(e),
                Err(_) => tokio::time::sleep(Duration::from_millis(250)).await,
            }
        }
    }

    async fn read_msg(&mut self) -> Result<Value> {
        let mut line = String::new();
        let n = tokio::time::timeout(IO_TIMEOUT, self.reader.read_line(&mut line))
            .await
            .map_err(|_| VmmError::Qmp("timed out waiting for QEMU".into()))??;
        if n == 0 {
            return Err(VmmError::Qmp("connection closed by QEMU".into()));
        }
        Ok(serde_json::from_str(&line)?)
    }

    /// Execute a command and return its `return` value.
    pub async fn execute(&mut self, command: &str, arguments: Option<Value>) -> Result<Value> {
        let msg = encode(command, arguments);
        self.writer.write_all(msg.as_bytes()).await?;
        self.writer.flush().await?;
        loop {
            let v = self.read_msg().await?;
            if v.get("event").is_some() {
                self.events.push(v);
                continue;
            }
            return decode_reply(v);
        }
    }

    /// Run a human-monitor (HMP) command; returns its text output.
    pub async fn hmp(&mut self, command_line: &str) -> Result<String> {
        let v = self
            .execute(
                "human-monitor-command",
                Some(json!({ "command-line": command_line })),
            )
            .await?;
        Ok(v.as_str().unwrap_or_default().to_string())
    }

    /// `query-status` → e.g. `"running"`, `"paused"`, `"shutdown"`.
    pub async fn status(&mut self) -> Result<String> {
        let v = self.execute("query-status", None).await?;
        Ok(v.get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string())
    }

    /// Save the framebuffer to a PNG file (path is on the host).
    pub async fn screendump(&mut self, path: &Path) -> Result<()> {
        self.execute(
            "screendump",
            Some(json!({ "filename": path.display().to_string(), "format": "png" })),
        )
        .await?;
        Ok(())
    }

    /// Press and release a combination of QEMU qcodes (e.g. `["ctrl","alt","delete"]`).
    pub async fn send_keys(&mut self, qcodes: &[&str]) -> Result<()> {
        let keys: Vec<Value> = qcodes
            .iter()
            .map(|k| json!({"type": "qcode", "data": k}))
            .collect();
        self.execute("send-key", Some(json!({ "keys": keys })))
            .await?;
        Ok(())
    }

    /// Move the absolute pointer (`x`,`y` in 0..=32767 tablet units) and
    /// optionally click `button` (`left`/`right`/`middle`).
    pub async fn pointer(&mut self, x: u32, y: u32, button: Option<&str>) -> Result<()> {
        let mut events = vec![
            json!({"type": "abs", "data": {"axis": "x", "value": x}}),
            json!({"type": "abs", "data": {"axis": "y", "value": y}}),
        ];
        if let Some(b) = button {
            events.push(json!({"type": "btn", "data": {"down": true, "button": b}}));
        }
        self.execute("input-send-event", Some(json!({ "events": events })))
            .await?;
        if let Some(b) = button {
            let up = json!([{"type": "btn", "data": {"down": false, "button": b}}]);
            self.execute("input-send-event", Some(json!({ "events": up })))
                .await?;
        }
        Ok(())
    }

    /// ACPI power button.
    pub async fn system_powerdown(&mut self) -> Result<()> {
        self.execute("system_powerdown", None).await.map(|_| ())
    }
    /// Pause vCPUs.
    pub async fn stop(&mut self) -> Result<()> {
        self.execute("stop", None).await.map(|_| ())
    }
    /// Resume vCPUs.
    pub async fn cont(&mut self) -> Result<()> {
        self.execute("cont", None).await.map(|_| ())
    }
    /// Terminate QEMU immediately.
    pub async fn quit(&mut self) -> Result<()> {
        // The connection may drop before the reply arrives.
        match self.execute("quit", None).await {
            Ok(_) | Err(VmmError::Qmp(_)) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Internal snapshot of RAM + device state + disks (`savevm`). HMP reports
    /// failures as text, so non-empty output is treated as an error.
    pub async fn savevm(&mut self, name: &str) -> Result<()> {
        hmp_ok(self.hmp(&format!("savevm {name}")).await?)
    }
    pub async fn loadvm(&mut self, name: &str) -> Result<()> {
        hmp_ok(self.hmp(&format!("loadvm {name}")).await?)
    }
    pub async fn delvm(&mut self, name: &str) -> Result<()> {
        hmp_ok(self.hmp(&format!("delvm {name}")).await?)
    }
}

fn hmp_ok(out: String) -> Result<()> {
    let t = out.trim();
    if t.is_empty() {
        Ok(())
    } else {
        Err(VmmError::Qmp(t.to_string()))
    }
}

/// Encode one QMP command line.
pub fn encode(command: &str, arguments: Option<Value>) -> String {
    let mut msg = json!({ "execute": command });
    if let Some(a) = arguments {
        msg["arguments"] = a;
    }
    format!("{msg}\n")
}

/// Turn a QMP reply into `Ok(return)` / `Err(error.desc)`.
pub fn decode_reply(v: Value) -> Result<Value> {
    if let Some(err) = v.get("error") {
        let class = err.get("class").and_then(Value::as_str).unwrap_or("Error");
        let desc = err
            .get("desc")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        return Err(VmmError::Qmp(format!("{class}: {desc}")));
    }
    Ok(v.get("return").cloned().unwrap_or(Value::Null))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[test]
    fn encodes_commands_with_and_without_arguments() {
        assert_eq!(encode("stop", None), "{\"execute\":\"stop\"}\n");
        let e = encode("screendump", Some(json!({"filename": "/x.png"})));
        let v: Value = serde_json::from_str(&e).unwrap();
        assert_eq!(v["arguments"]["filename"], "/x.png");
    }

    #[test]
    fn decodes_errors() {
        let err =
            decode_reply(json!({"error": {"class": "GenericError", "desc": "nope"}})).unwrap_err();
        assert!(err.to_string().contains("GenericError: nope"));
        assert_eq!(
            decode_reply(json!({"return": {"status": "running"}})).unwrap()["status"],
            "running"
        );
    }

    /// Fake QMP server: greeting, capabilities, an interleaved event, replies.
    #[tokio::test]
    async fn client_negotiates_and_skips_events() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            let (s, _) = l.accept().await.unwrap();
            let (r, mut w) = s.into_split();
            let mut r = BufReader::new(r);
            w.write_all(b"{\"QMP\": {\"version\": {}, \"capabilities\": []}}\n")
                .await
                .unwrap();
            let mut line = String::new();
            while r.read_line(&mut line).await.unwrap() > 0 {
                let v: Value = serde_json::from_str(&line).unwrap();
                let reply = match v["execute"].as_str().unwrap() {
                    "qmp_capabilities" => json!({"return": {}}),
                    "query-status" => {
                        w.write_all(b"{\"event\": \"RESUME\", \"timestamp\": {}}\n")
                            .await
                            .unwrap();
                        json!({"return": {"status": "running", "running": true}})
                    }
                    "human-monitor-command" => {
                        json!({"return": "Error: no block device can store vmstate\r\n"})
                    }
                    _ => json!({"error": {"class": "CommandNotFound", "desc": "x"}}),
                };
                w.write_all(format!("{reply}\n").as_bytes()).await.unwrap();
                line.clear();
            }
        });
        let mut c = QmpClient::connect(&addr).await.unwrap();
        assert_eq!(c.status().await.unwrap(), "running");
        assert_eq!(c.events.len(), 1);
        assert!(
            c.savevm("x")
                .await
                .unwrap_err()
                .to_string()
                .contains("vmstate")
        );
        assert!(c.execute("bogus", None).await.is_err());
    }
}
