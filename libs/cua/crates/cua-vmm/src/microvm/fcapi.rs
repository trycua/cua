//! Firecracker's REST API over its Unix socket (`--api-sock`).
//!
//! Only the calls the runtime makes: pause/resume, snapshot create/load and
//! Ctrl-Alt-Del. Each call opens its own connection (Firecracker serves one
//! request per connection happily, and a stale keep-alive cannot outlive a
//! restarted VMM this way).

use std::path::{Path, PathBuf};
use std::time::Duration;

use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};

use crate::error::{Result, VmmError};

/// A Firecracker API socket.
#[derive(Clone, Debug)]
pub struct FcApi {
    sock: PathBuf,
}

impl FcApi {
    pub fn new(sock: impl Into<PathBuf>) -> Self {
        Self { sock: sock.into() }
    }

    pub fn path(&self) -> &Path {
        &self.sock
    }

    /// Sends one request; `Ok` on a 2xx, the API's `fault_message` otherwise.
    pub async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
        let fut = async {
            let stream = super::connect_unix(&self.sock).await.map_err(|e| {
                VmmError::other(format!("firecracker API {}: {e}", self.sock.display()))
            })?;
            let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
                .await
                .map_err(|e| VmmError::other(format!("firecracker API handshake: {e}")))?;
            tokio::spawn(async move {
                let _ = conn.await;
            });
            let bytes = body
                .map(|b| Bytes::from(serde_json::to_vec(&b).unwrap_or_default()))
                .unwrap_or_default();
            let req = hyper::Request::builder()
                .method(method)
                .uri(format!("http://localhost{path}"))
                .header("Host", "localhost")
                .header("Content-Type", "application/json")
                .header("Accept", "application/json")
                .body(Full::new(bytes))
                .map_err(|e| VmmError::other(e.to_string()))?;
            let resp = sender
                .send_request(req)
                .await
                .map_err(|e| VmmError::other(format!("firecracker API {method} {path}: {e}")))?;
            let status = resp.status();
            let body = resp
                .into_body()
                .collect()
                .await
                .map_err(|e| VmmError::other(e.to_string()))?
                .to_bytes();
            let v: Value = if body.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&body)
                    .unwrap_or(Value::String(String::from_utf8_lossy(&body).into_owned()))
            };
            if status.is_success() {
                Ok(v)
            } else {
                let msg = v
                    .get("fault_message")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| v.to_string());
                Err(VmmError::other(format!(
                    "firecracker {method} {path}: {status}: {msg}"
                )))
            }
        };
        tokio::time::timeout(Duration::from_secs(120), fut)
            .await
            .map_err(|_| VmmError::other(format!("firecracker {method} {path}: timed out")))?
    }

    /// Waits until the socket accepts connections (the VMM is up).
    pub async fn wait_ready(&self, timeout: Duration) -> Result<()> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if super::connect_unix(&self.sock).await.is_ok() {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(VmmError::other(format!(
                    "firecracker did not open its API socket {} within {timeout:?}",
                    self.sock.display()
                )));
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }

    pub async fn pause(&self) -> Result<()> {
        self.call("PATCH", "/vm", Some(json!({"state": "Paused"})))
            .await
            .map(drop)
    }

    pub async fn resume(&self) -> Result<()> {
        self.call("PATCH", "/vm", Some(json!({"state": "Resumed"})))
            .await
            .map(drop)
    }

    /// A snapshot of a paused VM: all of its memory, or (`diff`, for a VM
    /// loaded with dirty-page tracking) only the pages written since it was
    /// loaded or last snapshotted, at their offsets in a sparse file.
    pub async fn snapshot(&self, vmstate: &Path, mem: &Path, diff: bool) -> Result<()> {
        self.call(
            "PUT",
            "/snapshot/create",
            Some(json!({
                "snapshot_type": if diff { "Diff" } else { "Full" },
                "snapshot_path": vmstate,
                "mem_file_path": mem,
            })),
        )
        .await
        .map(drop)
    }

    /// Loads a snapshot into this (fresh) VMM and resumes it, tracking
    /// the pages the guest writes (for diff snapshots). The memory file is
    /// mapped private: VMs restored from one file share its pages.
    pub async fn load(&self, vmstate: &Path, mem: &Path) -> Result<()> {
        self.call(
            "PUT",
            "/snapshot/load",
            Some(json!({
                "snapshot_path": vmstate,
                "mem_backend": {"backend_type": "File", "backend_path": mem},
                "track_dirty_pages": true,
                "resume_vm": true,
            })),
        )
        .await
        .map(drop)
    }

    /// Ctrl-Alt-Del: the guest init shuts down cleanly.
    pub async fn ctrl_alt_del(&self) -> Result<()> {
        self.call(
            "PUT",
            "/actions",
            Some(json!({"action_type": "SendCtrlAltDel"})),
        )
        .await
        .map(drop)
    }
}
