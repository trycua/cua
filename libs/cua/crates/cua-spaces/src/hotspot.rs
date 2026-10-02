//! Hotspot: a Space borrows this machine's network.
//!
//! `TunnelService.StartHotspot` starts a SOCKS5 listener on the guest's
//! loopback and returns a ticketed WebSocket. This module keeps that socket
//! open and serves it with `cua_hotspot::peer::serve_egress`, dialing each
//! of the guest's outbound connections from here. The hotspot lives as long
//! as the [`Hotspot`] (in the daemon, until `hotspot_stop` or release), not
//! as long as some app window: it no longer needs the Cua Spaces app.

use crate::error::{Error, Result};
use crate::space::Space;
use cua_spacesd_client::pb;
use futures_util::{SinkExt, StreamExt};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio_tungstenite::tungstenite::{self, client::IntoClientRequest};

/// How this machine dials the guest's outbound connections.
pub type Dialer = Arc<
    dyn Fn(
            String,
            u16,
        ) -> Pin<Box<dyn Future<Output = std::io::Result<tokio::net::TcpStream>> + Send>>
        + Send
        + Sync,
>;

/// The default dialer: plain TCP from this host.
pub fn direct_dialer() -> Dialer {
    Arc::new(|host, port| {
        Box::pin(async move { tokio::net::TcpStream::connect((host.as_str(), port)).await })
    })
}

/// Options for [`Space::start_hotspot`].
#[derive(Clone)]
pub struct HotspotOptions {
    /// Guest loopback SOCKS port (0 = 1080).
    pub socks_port: u32,
    /// Point the guest's system proxy at the hotspot.
    pub set_system_proxy: bool,
    /// Destinations that keep the guest's own network.
    pub bypass: Vec<String>,
    /// How outbound connections are dialed here.
    pub dialer: Dialer,
}

impl Default for HotspotOptions {
    fn default() -> Self {
        Self {
            socks_port: 0,
            set_system_proxy: true,
            bypass: vec![],
            dialer: direct_dialer(),
        }
    }
}

impl std::fmt::Debug for HotspotOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HotspotOptions")
            .field("socks_port", &self.socks_port)
            .field("set_system_proxy", &self.set_system_proxy)
            .field("bypass", &self.bypass)
            .finish()
    }
}

/// A running hotspot. Stops when dropped.
pub struct Hotspot {
    space: Space,
    hotspot_id: String,
    socks_address: String,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl std::fmt::Debug for Hotspot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hotspot")
            .field("space", &self.space.id().to_string())
            .field("hotspot_id", &self.hotspot_id)
            .field("socks_address", &self.socks_address)
            .finish()
    }
}

/// Hotspot state as reported by the Space.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct HotspotStatus {
    /// Space id.
    pub space: String,
    /// `stopped`, `waiting_for_peer` or `active`.
    pub state: String,
    /// Hotspot id.
    pub hotspot_id: String,
    /// Guest SOCKS address.
    pub socks_address: String,
    /// Open relayed connections.
    pub active_connections: u32,
    /// Bytes out of the guest.
    pub bytes_out: u64,
    /// Bytes into the guest.
    pub bytes_in: u64,
    /// Whether this process serves it.
    pub served_here: bool,
}

impl Space {
    /// Starts the hotspot and serves its egress from this machine.
    pub async fn start_hotspot(&self, options: HotspotOptions) -> Result<Hotspot> {
        self.require("hotspot")?;
        let resp = self
            .spacesd()?
            .tunnel()
            .start_hotspot(pb::StartHotspotRequest {
                socks_port: options.socks_port,
                set_system_proxy: options.set_system_proxy,
                bypass: options.bypass.clone(),
                ticket_ttl: None,
            })
            .await
            .map_err(cua_spacesd_client::Error::from)?
            .into_inner();
        let ws_path = if resp.ws_path.is_empty() {
            format!(
                "{}?ticket={}",
                cua_proto::metadata::HOTSPOT_WS_PATH,
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
        let ws = match tokio_tungstenite::connect_async(request).await {
            Ok((ws, _)) => ws,
            Err(e) => {
                let _ = self
                    .spacesd()?
                    .tunnel()
                    .stop_hotspot(pb::StopHotspotRequest {
                        hotspot_id: resp.hotspot_id.clone(),
                    })
                    .await;
                return Err(Error::Stream(format!("hotspot socket: {e}")));
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
        let dialer = options.dialer.clone();
        let space_id = self.id().to_string();
        let task = tokio::spawn(async move {
            cua_hotspot::peer::serve_egress(Box::pin(source), Box::pin(sink), move |host, port| {
                dialer(host, port)
            })
            .await;
            tracing::info!(space = %space_id, "hotspot socket closed");
        });
        Ok(Hotspot {
            space: self.clone(),
            hotspot_id: resp.hotspot_id,
            socks_address: resp.socks_address,
            task: Some(task),
        })
    }

    /// `TunnelService.GetHotspotStatus`.
    pub async fn hotspot_status(&self) -> Result<HotspotStatus> {
        self.require("hotspot")?;
        let s = self
            .spacesd()?
            .tunnel()
            .get_hotspot_status(pb::GetHotspotStatusRequest {})
            .await
            .map_err(cua_spacesd_client::Error::from)?
            .into_inner();
        Ok(HotspotStatus {
            space: self.id().to_string(),
            state: match pb::HotspotState::try_from(s.state).unwrap_or_default() {
                pb::HotspotState::Active => "active",
                pb::HotspotState::WaitingForPeer => "waiting_for_peer",
                pb::HotspotState::Stopped => "stopped",
                pb::HotspotState::Unspecified => "unknown",
            }
            .into(),
            hotspot_id: s.hotspot_id,
            socks_address: s.socks_address,
            active_connections: s.active_connections,
            bytes_out: s.bytes_out,
            bytes_in: s.bytes_in,
            served_here: false,
        })
    }
}

impl Hotspot {
    /// Hotspot id.
    pub fn id(&self) -> &str {
        &self.hotspot_id
    }

    /// Guest-side SOCKS5 address (for example `127.0.0.1:1080`).
    pub fn socks_address(&self) -> &str {
        &self.socks_address
    }

    /// The Space.
    pub fn space(&self) -> &Space {
        &self.space
    }

    /// Whether the egress socket is still being served.
    pub fn is_running(&self) -> bool {
        self.task.as_ref().is_some_and(|t| !t.is_finished())
    }

    /// Current status from the Space, marked as served here.
    pub async fn status(&self) -> Result<HotspotStatus> {
        let mut s = self.space.hotspot_status().await?;
        s.served_here = self.is_running();
        Ok(s)
    }

    /// Stops the hotspot in the Space and closes the socket.
    pub async fn stop(mut self) -> Result<()> {
        let r = self
            .space
            .spacesd()?
            .tunnel()
            .stop_hotspot(pb::StopHotspotRequest {
                hotspot_id: self.hotspot_id.clone(),
            })
            .await;
        if let Some(t) = self.task.take() {
            t.abort();
        }
        r.map_err(cua_spacesd_client::Error::from)?;
        Ok(())
    }
}

impl Drop for Hotspot {
    fn drop(&mut self) {
        if let Some(t) = self.task.take() {
            t.abort();
        }
    }
}

impl crate::Spaces {
    /// Starts (or restarts) the hotspot for a Space and keeps it running in
    /// this process until [`crate::Spaces::hotspot_stop`] or release.
    pub async fn hotspot_start(
        &self,
        space: &str,
        options: HotspotOptions,
    ) -> Result<HotspotStatus> {
        let s = self.space(space).await?;
        let key = s.id().to_string();
        if let Some(old) = self.inner.hotspots.lock().await.remove(&key) {
            let _ = old.stop().await;
        }
        let hotspot = s.start_hotspot(options.clone()).await?;
        let status = HotspotStatus {
            space: key.clone(),
            state: "waiting_for_peer".into(),
            hotspot_id: hotspot.id().into(),
            socks_address: hotspot.socks_address().into(),
            active_connections: 0,
            bytes_out: 0,
            bytes_in: 0,
            served_here: true,
        };
        let id = hotspot.id().to_string();
        self.inner
            .hotspots
            .lock()
            .await
            .insert(key.clone(), hotspot);
        self.supervise_hotspot(key, id, options);
        Ok(status)
    }

    /// Re-starts the hotspot of `key` when its guest daemon restarts (the
    /// socket closes and the new daemon has no hotspot), until it is
    /// stopped or replaced.
    fn supervise_hotspot(&self, key: String, id: String, options: HotspotOptions) {
        use crate::reattach::{Attempt, Health, Policy, supervise};
        let spaces = self.clone();
        // The hotspot this supervisor last saw; another id means a caller
        // replaced it (and started its own supervisor).
        let mine = Arc::new(std::sync::Mutex::new(Some(id)));
        tokio::spawn(async move {
            let health = {
                let (spaces, key, mine) = (spaces.clone(), key.clone(), mine.clone());
                move || {
                    let (spaces, key, mine) = (spaces.clone(), key.clone(), mine.clone());
                    async move {
                        let (id, running) = {
                            let map = spaces.inner.hotspots.lock().await;
                            match map.get(&key) {
                                Some(h) => (h.id().to_string(), h.is_running()),
                                None => return Health::Gone,
                            }
                        };
                        {
                            let mut m = mine.lock().expect("hotspot id");
                            match m.as_deref() {
                                None => *m = Some(id.clone()),
                                Some(seen) if seen != id => return Health::Gone,
                                _ => {}
                            }
                        }
                        if !running {
                            return Health::Down;
                        }
                        // The socket is open; does the guest still have its
                        // half? A restarted daemon reports no hotspot.
                        let Ok(space) = spaces.space(&key).await else {
                            return Health::Healthy;
                        };
                        match space.hotspot_status().await {
                            Ok(s) if s.state == "stopped" || s.hotspot_id != id => Health::Down,
                            _ => Health::Healthy,
                        }
                    }
                }
            };
            let reattach = {
                let (spaces, key, mine) = (spaces.clone(), key.clone(), mine.clone());
                move || {
                    let (spaces, key, mine, options) =
                        (spaces.clone(), key.clone(), mine.clone(), options.clone());
                    async move {
                        let space = spaces.space(&key).await.map_err(|e| e.to_string())?;
                        // Hold the map across the start so a stop cannot slip in.
                        let mut map = spaces.inner.hotspots.lock().await;
                        if !map.contains_key(&key) {
                            return Ok(Attempt::Unsupported);
                        }
                        let fresh = space
                            .start_hotspot(options)
                            .await
                            .map_err(|e| e.to_string())?;
                        tracing::info!(space = %key, "hotspot re-attached after the guest daemon restarted");
                        *mine.lock().expect("hotspot id") = Some(fresh.id().to_string());
                        if let Some(old) = map.insert(key, fresh) {
                            drop(old);
                        }
                        Ok(Attempt::Attached)
                    }
                }
            };
            supervise(Policy::default(), health, reattach).await;
        });
    }

    /// Stops the hotspot of one Space, or of every Space when `None`.
    /// Returns the Spaces that were stopped.
    pub async fn hotspot_stop(&self, space: Option<&str>) -> Result<Vec<String>> {
        let mut map = self.inner.hotspots.lock().await;
        let keys: Vec<String> = match space {
            Some(s) => vec![self.resolve(s)?.to_string()],
            None => map.keys().cloned().collect(),
        };
        let mut stopped = vec![];
        for k in keys {
            if let Some(h) = map.remove(&k) {
                h.stop().await?;
                stopped.push(k);
            }
        }
        Ok(stopped)
    }

    /// Status of the hotspots this process serves (or of one Space).
    pub async fn hotspot_statuses(&self, space: Option<&str>) -> Result<Vec<HotspotStatus>> {
        let map = self.inner.hotspots.lock().await;
        let mut out = vec![];
        match space {
            Some(s) => {
                let key = self.resolve(s)?.to_string();
                match map.get(&key) {
                    Some(h) => out.push(h.status().await?),
                    None => {
                        drop(map);
                        out.push(self.space(s).await?.hotspot_status().await?);
                    }
                }
            }
            None => {
                for h in map.values() {
                    out.push(h.status().await?);
                }
            }
        }
        Ok(out)
    }
}
