//! Presenting a Space to the human: picture-in-picture, the viewer, and one
//! streamed window on the operator's desktop.
//!
//! This is UI, so it stays in the app (plan §4): Spaces only asks. The
//! default [`ControlServerDisplay`] asks the Cua Spaces app over its
//! loopback control server (`~/.cua/spaces-control.json` holds its port and
//! bearer), exactly as `spaces_mcp.py` did; the Tauri app, when it hosts
//! Spaces in-process, passes its own [`OperatorDisplay`] instead.

use crate::error::{Error, Result};
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A request to draw one Space window on the operator's desktop.
#[derive(Clone, Debug, serde::Serialize)]
pub struct WindowStreamRequest {
    /// Space id.
    pub space_id: String,
    /// Window handle (`WindowRef.id`).
    pub window_id: String,
    /// Owning app.
    pub app_name: String,
    /// Label.
    pub title: String,
    /// Ticketed media WebSocket URL for that window (rcdp wire v2). The
    /// ticket is short-lived and bound to this one session.
    pub media_url: String,
    /// Media session id (for `CloseMedia`).
    pub media_session_id: String,
}

/// Something that can present Spaces on the operator's desktop.
#[async_trait::async_trait]
pub trait OperatorDisplay: Send + Sync {
    /// Opens a bidirectional window showing one Space window.
    async fn stream_window(&self, request: WindowStreamRequest) -> Result<()>;
    /// Pins the Space as picture-in-picture.
    async fn pin_pip(&self, space_id: &str) -> Result<()>;
    /// Removes the picture-in-picture overlay.
    async fn unpin_pip(&self, space_id: &str) -> Result<()>;
    /// Opens the full viewer.
    async fn open_viewer(&self, space_id: &str) -> Result<()>;
}

/// No display: every call reports `host_capability_missing`.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoDisplay;

#[async_trait::async_trait]
impl OperatorDisplay for NoDisplay {
    async fn stream_window(&self, _: WindowStreamRequest) -> Result<()> {
        Err(no_display())
    }
    async fn pin_pip(&self, _: &str) -> Result<()> {
        Err(no_display())
    }
    async fn unpin_pip(&self, _: &str) -> Result<()> {
        Err(no_display())
    }
    async fn open_viewer(&self, _: &str) -> Result<()> {
        Err(no_display())
    }
}

fn no_display() -> Error {
    Error::host(
        cua_spaces_contract::host::OPERATOR_DISPLAY,
        "no operator display is attached; open the Cua Spaces app",
    )
}

/// The Cua Spaces app's loopback control server.
#[derive(Clone, Debug)]
pub struct ControlServerDisplay {
    config: PathBuf,
}

#[derive(serde::Deserialize)]
struct ControlConfig {
    port: u16,
    token: String,
}

impl ControlServerDisplay {
    /// Reads the port and bearer from `config` (normally
    /// `~/.cua/spaces-control.json`) on every call, so an app restart that
    /// rotates them is picked up.
    pub fn new(config: impl Into<PathBuf>) -> Self {
        Self {
            config: config.into(),
        }
    }

    async fn post(&self, path: &str, body: serde_json::Value) -> Result<serde_json::Value> {
        let raw = tokio::fs::read(&self.config).await.map_err(|e| {
            Error::host(
                cua_spaces_contract::host::OPERATOR_DISPLAY,
                format!(
                    "the Cua Spaces app isn't running (no control server at {}: {e})",
                    self.config.display()
                ),
            )
        })?;
        let cfg: ControlConfig = serde_json::from_slice(&raw)?;
        let body = serde_json::to_vec(&body)?;
        let mut stream = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            tokio::net::TcpStream::connect(("127.0.0.1", cfg.port)),
        )
        .await
        .map_err(|_| Error::Timeout("connecting to the Cua Spaces app".into()))?
        .map_err(|e| {
            Error::host(
                cua_spaces_contract::host::OPERATOR_DISPLAY,
                format!("the Cua Spaces app control server is unreachable: {e}"),
            )
        })?;
        let head = format!(
            "POST {path} HTTP/1.1\r\nhost: 127.0.0.1\r\nauthorization: Bearer {}\r\n\
             content-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            cfg.token,
            body.len()
        );
        stream.write_all(head.as_bytes()).await?;
        stream.write_all(&body).await?;
        let mut response = Vec::new();
        // Bounded read: a control reply is small.
        let mut limited = (&mut stream).take(1 << 20);
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            limited.read_to_end(&mut response),
        )
        .await
        .map_err(|_| Error::Timeout(format!("control {path}")))??;
        let text = String::from_utf8_lossy(&response);
        let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
        let status: u16 = head
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        if !(200..300).contains(&status) {
            return Err(Error::host(
                cua_spaces_contract::host::OPERATOR_DISPLAY,
                format!(
                    "control {path} -> {status}: {}",
                    body.chars().take(200).collect::<String>()
                ),
            ));
        }
        Ok(serde_json::from_str(body.trim()).unwrap_or(serde_json::Value::Null))
    }
}

#[async_trait::async_trait]
impl OperatorDisplay for ControlServerDisplay {
    async fn stream_window(&self, request: WindowStreamRequest) -> Result<()> {
        self.post("/window/stream", serde_json::to_value(&request)?)
            .await
            .map(|_| ())
    }
    async fn pin_pip(&self, space_id: &str) -> Result<()> {
        self.post("/pip/pin", serde_json::json!({ "space_id": space_id }))
            .await
            .map(|_| ())
    }
    async fn unpin_pip(&self, space_id: &str) -> Result<()> {
        self.post("/pip/unpin", serde_json::json!({ "space_id": space_id }))
            .await
            .map(|_| ())
    }
    async fn open_viewer(&self, space_id: &str) -> Result<()> {
        self.post("/viewer/open", serde_json::json!({ "space_id": space_id }))
            .await
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_missing_app_is_a_host_capability_error() {
        let d = ControlServerDisplay::new("/nonexistent/cua/spaces-control.json");
        let e = d.pin_pip("local:x").await.unwrap_err();
        assert_eq!(e.tag(), "host_capability_missing");
    }

    #[tokio::test]
    async fn posts_to_the_control_server_with_its_bearer() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("spaces-control.json");
        std::fs::write(&cfg, format!(r#"{{"port":{port},"token":"tok"}}"#)).unwrap();
        let server = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut got = Vec::new();
            let mut buf = vec![0u8; 4096];
            // Bounded: the request is one head and a small JSON body.
            for _ in 0..16 {
                let n = s.read(&mut buf).await.unwrap();
                got.extend_from_slice(&buf[..n]);
                if n == 0 || got.ends_with(b"}") {
                    break;
                }
            }
            s.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 11\r\n\r\n{\"ok\":true}")
                .await
                .unwrap();
            String::from_utf8_lossy(&got).into_owned()
        });
        ControlServerDisplay::new(&cfg)
            .open_viewer("direct:h:1")
            .await
            .unwrap();
        let request = server.await.unwrap();
        assert!(request.starts_with("POST /viewer/open "), "{request}");
        assert!(request.contains("authorization: Bearer tok"));
        assert!(request.contains("direct:h:1"));
    }
}
