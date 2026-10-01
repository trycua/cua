//! A volume mounted in a Space's guest, served from this host.
//!
//! `VolumeService.AttachVolume` prepares the guest mount and returns a
//! ticketed WebSocket. [`Space::attach_volume`] keeps that socket open and
//! answers the guest's mount streams, which name a target (`nfs` for a macOS
//! guest, `fs` for Linux), with the caller's dialer. This crate carries the
//! bytes only; what serves them (Cua Volume, whose access rules, audit and
//! secret scan stay on the host) is the caller's, and the guest never holds
//! storage keys. The mount lives as long as the [`VolumeAttachment`].

use std::time::Duration;

use cua_spacesd_client::pb;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::{self, client::IntoClientRequest};

use crate::error::{Error, Result};
use crate::space::Space;

/// The guest feature.
pub const VOLUME_FEATURE: &str = "volume.mount";

/// A mounted volume in a Space.
pub struct VolumeAttachment {
    space: Space,
    volume_id: String,
    mount_path: String,
    backend: String,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl std::fmt::Debug for VolumeAttachment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VolumeAttachment")
            .field("space", &self.space.id())
            .field("volume_id", &self.volume_id)
            .field("mount_path", &self.mount_path)
            .finish()
    }
}

/// The guest's view of its mount.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct VolumeGuestStatus {
    /// `detached`, `waiting_for_client`, `mounting`, `mounted`, `error`.
    pub state: String,
    pub mount_path: String,
    pub backend: String,
    pub detail: String,
    pub bytes_in: u64,
    pub bytes_out: u64,
}

impl VolumeAttachment {
    pub fn volume_id(&self) -> &str {
        &self.volume_id
    }

    /// Where the guest mounts the volume.
    pub fn mount_path(&self) -> &str {
        &self.mount_path
    }

    /// `nfs` or `fs`.
    pub fn backend(&self) -> &str {
        &self.backend
    }

    /// Whether the socket to the guest is still open.
    pub fn is_live(&self) -> bool {
        self.task.as_ref().is_some_and(|t| !t.is_finished())
    }

    /// Waits until the guest reports the mount (or its failure).
    pub async fn wait_mounted(&self, timeout: Duration) -> Result<VolumeGuestStatus> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let s = self.space.volume_status().await?;
            match s.state.as_str() {
                "mounted" => return Ok(s),
                "error" => {
                    return Err(Error::Stream(format!(
                        "the guest could not mount the volume: {}",
                        s.detail
                    )));
                }
                "detached" => {
                    return Err(Error::Stream("the guest detached the volume".into()));
                }
                _ => {}
            }
            if tokio::time::Instant::now() > deadline {
                return Err(Error::Stream(format!(
                    "the guest did not mount the volume in {timeout:?} (state {})",
                    s.state
                )));
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// Unmounts in the guest and closes the socket. What the dialer served
    /// is the caller's to stop.
    pub async fn detach(mut self) -> Result<()> {
        let r = self
            .space
            .spacesd()?
            .volume()
            .detach_volume(pb::DetachVolumeRequest {
                volume_id: self.volume_id.clone(),
            })
            .await;
        if let Some(t) = self.task.take() {
            let _ = tokio::time::timeout(Duration::from_secs(10), t).await;
        }
        r.map(|_| ())
            .map_err(|e| Error::from(cua_spacesd_client::Error::from(e)))
    }
}

impl Drop for VolumeAttachment {
    fn drop(&mut self) {
        if let Some(t) = self.task.take() {
            t.abort();
        }
    }
}

impl Space {
    /// Mounts a volume in the guest, served by `dial`: each guest mount
    /// stream names a target (`nfs` or `fs`, as `backend` says) and `dial`
    /// connects it (refusing names it does not serve). Fails with
    /// `capability_missing` when the guest has no mount backend.
    pub async fn attach_volume<D, F>(
        &self,
        dial: D,
        mount_path: Option<String>,
    ) -> Result<VolumeAttachment>
    where
        D: Fn(String, u16) -> F + Send + Sync + 'static,
        F: std::future::Future<Output = std::io::Result<tokio::net::TcpStream>> + Send + 'static,
    {
        self.require(VOLUME_FEATURE)?;
        let resp = self
            .spacesd()?
            .volume()
            .attach_volume(pb::AttachVolumeRequest {
                mount_path: mount_path.unwrap_or_default(),
                ticket_ttl: None,
            })
            .await
            .map_err(cua_spacesd_client::Error::from)?
            .into_inner();
        let ws_path = if resp.ws_path.is_empty() {
            format!(
                "{}?ticket={}",
                cua_proto::metadata::VOLUME_WS_PATH,
                resp.ticket
            )
        } else {
            resp.ws_path.clone()
        };
        let mut request = self
            .websocket_url(&ws_path)?
            .into_client_request()
            .map_err(|e| Error::Stream(e.to_string()))?;
        for (k, v) in self.websocket_headers().await? {
            if let (Ok(k), Ok(v)) = (
                http::HeaderName::from_bytes(k.as_bytes()),
                http::HeaderValue::from_str(&v),
            ) {
                request.headers_mut().insert(k, v);
            }
        }
        cua_spacesd_client::transport::ensure_crypto_provider();
        let config = tungstenite::protocol::WebSocketConfig::default()
            .max_message_size(Some(16 << 20))
            .max_frame_size(Some(16 << 20));
        let ws =
            match tokio_tungstenite::connect_async_with_config(request, Some(config), true).await {
                Ok((ws, _)) => ws,
                Err(e) => {
                    let _ = self
                        .spacesd()?
                        .volume()
                        .detach_volume(pb::DetachVolumeRequest {
                            volume_id: resp.volume_id.clone(),
                        })
                        .await;
                    return Err(Error::Stream(format!("volume socket: {e}")));
                }
            };
        let (sink, stream) = ws.split();
        let sink = sink.with(|bytes: Vec<u8>| async move {
            Ok::<_, tungstenite::Error>(tungstenite::Message::Binary(bytes.into()))
        });
        let source = stream.filter_map(|m| async move {
            match m {
                Ok(tungstenite::Message::Binary(b)) => Some(b.to_vec()),
                _ => None,
            }
        });
        let space_id = self.id().to_string();
        let task = tokio::spawn(async move {
            cua_hotspot::peer::serve_egress(Box::pin(source), Box::pin(sink), dial).await;
            tracing::info!(space = %space_id, "volume socket closed");
        });
        Ok(VolumeAttachment {
            space: self.clone(),
            volume_id: resp.volume_id,
            mount_path: resp.mount_path,
            backend: resp.backend,
            task: Some(task),
        })
    }

    /// The mount backend the guest claims (`nfs`, `fs`), when it has one.
    pub fn volume_backend(&self) -> Option<String> {
        self.feature(VOLUME_FEATURE)
            .filter(|f| f.supported)
            .and_then(|f| f.attributes.get("backend").cloned())
    }

    /// `VolumeService.GetVolumeStatus`.
    pub async fn volume_status(&self) -> Result<VolumeGuestStatus> {
        self.require(VOLUME_FEATURE)?;
        let s = self
            .spacesd()?
            .volume()
            .get_volume_status(pb::GetVolumeStatusRequest {})
            .await
            .map_err(cua_spacesd_client::Error::from)?
            .into_inner();
        Ok(VolumeGuestStatus {
            state: match pb::VolumeState::try_from(s.state).unwrap_or_default() {
                pb::VolumeState::Detached => "detached",
                pb::VolumeState::WaitingForClient => "waiting_for_client",
                pb::VolumeState::Mounting => "mounting",
                pb::VolumeState::Mounted => "mounted",
                pb::VolumeState::Error => "error",
                pb::VolumeState::Unspecified => "unknown",
            }
            .into(),
            mount_path: s.mount_path,
            backend: s.backend,
            detail: s.detail,
            bytes_in: s.bytes_in,
            bytes_out: s.bytes_out,
        })
    }
}
