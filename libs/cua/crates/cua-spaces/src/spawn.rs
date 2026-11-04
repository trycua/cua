//! The host's end of a Space's spawn channel (`TunnelService.StartSpawn`).
//!
//! The guest listens on its own loopback and relays each connection over a
//! WebSocket this host opens to the Space's cua-spacesd. Every stream that
//! arrives is handed to the caller's dialer, which serves this Space's
//! child-Space API on it. The host knows the Space by the connection it
//! opened, so nothing the guest sends can name another Space.

use cua_spacesd_client::pb;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::{self, client::IntoClientRequest};

use crate::error::{Error, Result};
use crate::space::Space;

/// The guest feature.
pub const SPAWN_FEATURE: &str = "spawn";

/// A started spawn channel. Dropping it leaves the guest's listener in
/// place but closes the host's socket (new connections are refused);
/// [`SpawnChannel::stop`] also removes the listener and the capability.
pub struct SpawnChannel {
    space: Space,
    spawn_id: String,
    address: String,
    token_path: String,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl SpawnChannel {
    /// The guest's listener URL (`http://127.0.0.1:<port>`).
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Where the guest keeps the capability.
    pub fn token_path(&self) -> &str {
        &self.token_path
    }

    /// Waits until the host's socket closes (the guest stopped, restarted
    /// or replaced the channel).
    pub async fn closed(&mut self) {
        if let Some(task) = self.task.as_mut() {
            let _ = task.await;
        }
        self.task = None;
    }

    /// Stops the channel in the guest (listener and capability file).
    pub async fn stop(mut self) -> Result<()> {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        self.space
            .spacesd()?
            .tunnel()
            .stop_spawn(pb::StopSpawnRequest {
                spawn_id: self.spawn_id.clone(),
            })
            .await
            .map_err(cua_spacesd_client::Error::from)?;
        Ok(())
    }
}

impl Drop for SpawnChannel {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

impl Space {
    /// Starts this Space's spawn channel with `capability`, serving every
    /// stream the guest opens with `dial`.
    pub async fn start_spawn_with<D, F, T>(&self, capability: &str, dial: D) -> Result<SpawnChannel>
    where
        D: Fn(String, u16) -> F + Send + Sync + 'static,
        F: std::future::Future<Output = std::io::Result<T>> + Send + 'static,
        T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + 'static,
    {
        self.require(SPAWN_FEATURE)?;
        let resp = self
            .spacesd()?
            .tunnel()
            .start_spawn(pb::StartSpawnRequest {
                capability: capability.to_owned(),
                ticket_ttl: None,
                space_id: self.id().to_string(),
            })
            .await
            .map_err(cua_spacesd_client::Error::from)?
            .into_inner();
        let mut request = self
            .websocket_url(&resp.ws_path)?
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
                        .tunnel()
                        .stop_spawn(pb::StopSpawnRequest {
                            spawn_id: resp.spawn_id.clone(),
                        })
                        .await;
                    return Err(Error::Stream(format!("spawn socket: {e}")));
                }
            };
        let (sink, stream) = ws.split();
        let sink = sink.with(|bytes: Vec<u8>| async move {
            Ok::<_, tungstenite::Error>(tungstenite::Message::Binary(bytes.into()))
        });
        let source = stream
            .take_while(|m| {
                futures_util::future::ready(!matches!(
                    m,
                    Err(_) | Ok(tungstenite::Message::Close(_))
                ))
            })
            .filter_map(|m| async move {
                match m {
                    Ok(tungstenite::Message::Binary(b)) => Some(b.to_vec()),
                    _ => None,
                }
            });
        let space_id = self.id().to_string();
        let task = tokio::spawn(async move {
            cua_hotspot::peer::serve_streams(Box::pin(source), Box::pin(sink), dial).await;
            tracing::debug!(space = %space_id, "spawn socket closed");
        });
        Ok(SpawnChannel {
            space: self.clone(),
            spawn_id: resp.spawn_id,
            address: resp.address,
            token_path: resp.token_path,
            task: Some(task),
        })
    }
}
