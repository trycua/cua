//! Client side of `TunnelService.Forward`: a loopback TCP listener whose
//! connections are each carried over one ticketed `/tunnel` WebSocket to a
//! guest port.
//!
//! The WebSocket goes to the same endpoint, path prefix and credentials as
//! the gRPC channel, so it works wherever the spacesd is reachable: a
//! direct URL, a relay, or the Fleet gateway (Fleet bearer plus
//! `X-Cua-Fleet-Claim`, env token in `x-cua-env-authorization`). The ticket
//! from `Forward` is reusable until it expires; the forward renews it before
//! then and once more when a socket is refused.
//!
//! Buffers are bounded: at most [`RELAY_CHUNK_BYTES`] is read from the local
//! socket per WebSocket message, a WebSocket message is at most
//! [`MAX_TUNNEL_MESSAGE_BYTES`], and sends wait for the socket to drain.

use std::{
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime},
};

use futures_util::{SinkExt, StreamExt};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Semaphore, watch},
};
use tokio_tungstenite::tungstenite::{
    self, Message, client::IntoClientRequest, protocol::WebSocketConfig,
};

use crate::{
    SpacesdClient,
    error::{Error, ErrorDetails, Result},
    pb,
    transport::{HeaderInjector, ensure_crypto_provider},
};

/// Capability name a spacesd advertises when it serves `Forward`.
pub const TUNNEL_FORWARD_FEATURE: &str = "tunnel.forward";
/// Bytes read from the local socket per WebSocket message.
pub const RELAY_CHUNK_BYTES: usize = 64 * 1024;
/// Largest WebSocket message accepted from the driver (its own cap).
pub const MAX_TUNNEL_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
/// Default cap on concurrently relayed connections per forward.
pub const DEFAULT_MAX_CONNECTIONS: usize = 256;
/// Renew the ticket this long before it expires.
const RENEW_MARGIN: Duration = Duration::from_secs(30);
/// After the local side closes, how long to keep delivering the guest's
/// remaining bytes.
const DRAIN_AFTER_CLOSE: Duration = Duration::from_secs(5);

/// Options for [`SpacesdClient::forward_tcp`].
#[derive(Clone, Debug)]
pub struct ForwardOptions {
    /// Guest TCP port.
    pub port: u16,
    /// Guest host; empty means `127.0.0.1` (the driver allows loopback and
    /// the guest's own addresses only).
    pub host: String,
    /// Local address to listen on (`127.0.0.1:0` picks a free port).
    pub bind: SocketAddr,
    /// Ticket lifetime to request; `None` uses the driver's default. The
    /// forward renews tickets on its own either way.
    pub ttl: Option<Duration>,
    /// Concurrent connections relayed at once; further connections wait in
    /// the listen backlog.
    pub max_connections: usize,
}

impl ForwardOptions {
    /// Forward guest `port` to a free loopback port.
    pub fn new(port: u16) -> Self {
        Self {
            port,
            host: String::new(),
            bind: SocketAddr::from(([127, 0, 0, 1], 0)),
            ttl: None,
            max_connections: DEFAULT_MAX_CONNECTIONS,
        }
    }

    /// Listen on `bind` instead.
    pub fn bind(mut self, bind: SocketAddr) -> Self {
        self.bind = bind;
        self
    }
}

/// Counters of a [`TcpForward`].
#[derive(Debug, Default)]
pub struct ForwardStats {
    /// Connections accepted.
    pub connections: AtomicU64,
    /// Connections currently relayed.
    pub active: AtomicU64,
    /// Bytes sent to the guest.
    pub bytes_to_guest: AtomicU64,
    /// Bytes received from the guest.
    pub bytes_from_guest: AtomicU64,
    /// Connections whose WebSocket could not be opened.
    pub failed: AtomicU64,
}

#[derive(Clone)]
struct Ticket {
    forward_id: String,
    ws_path: String,
    expires_at: Option<SystemTime>,
}

struct Shared {
    client: SpacesdClient,
    options: ForwardOptions,
    ticket: tokio::sync::Mutex<Ticket>,
    /// Every forward id minted (renewals included), closed on shutdown.
    forward_ids: Mutex<Vec<String>>,
    stats: ForwardStats,
}

/// A running TCP forward. Dropping it stops the listener and every relayed
/// connection; [`TcpForward::close`] also revokes the driver-side forward.
pub struct TcpForward {
    local_addr: SocketAddr,
    guest_port: u16,
    shared: Arc<Shared>,
    stop: watch::Sender<bool>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl std::fmt::Debug for TcpForward {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TcpForward")
            .field("local_addr", &self.local_addr)
            .field("guest_port", &self.guest_port)
            .finish_non_exhaustive()
    }
}

impl TcpForward {
    /// Loopback address accepting connections.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Guest port.
    pub fn guest_port(&self) -> u16 {
        self.guest_port
    }

    /// Driver-side forward id of the current ticket.
    pub async fn forward_id(&self) -> String {
        self.shared.ticket.lock().await.forward_id.clone()
    }

    /// Counters.
    pub fn stats(&self) -> &ForwardStats {
        &self.shared.stats
    }

    /// Stops listening, closes every relayed connection and revokes the
    /// driver-side forwards (best effort).
    pub async fn close(mut self) -> Result<()> {
        self.shutdown();
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
        let ids = std::mem::take(&mut *self.shared.forward_ids.lock().unwrap());
        close_forwards(&self.shared.client, ids).await;
        Ok(())
    }

    fn shutdown(&self) {
        let _ = self.stop.send(true);
    }
}

impl Drop for TcpForward {
    fn drop(&mut self) {
        self.shutdown();
        let Some(task) = self.task.take() else {
            return;
        };
        task.abort();
        let ids = std::mem::take(&mut *self.shared.forward_ids.lock().unwrap());
        if ids.is_empty() {
            return;
        }
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let client = self.shared.client.clone();
            handle.spawn(async move { close_forwards(&client, ids).await });
        }
    }
}

async fn close_forwards(client: &SpacesdClient, ids: Vec<String>) {
    for forward_id in ids {
        let _ = tokio::time::timeout(
            Duration::from_secs(5),
            client
                .tunnel()
                .close_forward(pb::CloseForwardRequest { forward_id }),
        )
        .await;
    }
}

fn unsupported(message: String) -> Error {
    Error::FeatureUnsupported {
        feature: TUNNEL_FORWARD_FEATURE.into(),
        details: ErrorDetails {
            code: tonic::Code::Unimplemented as i32,
            message,
            metadata: Default::default(),
        },
    }
}

impl SpacesdClient {
    /// Whether the driver serves TCP forwarding ([`TUNNEL_FORWARD_FEATURE`]).
    pub async fn supports_tunnel_forward(&self) -> Result<bool> {
        self.has_feature(TUNNEL_FORWARD_FEATURE).await
    }

    /// Forwards a local TCP port to a guest port through the driver's
    /// `/tunnel` WebSocket. Fails with [`Error::FeatureUnsupported`] when the
    /// driver does not advertise [`TUNNEL_FORWARD_FEATURE`].
    pub async fn forward_tcp(&self, options: ForwardOptions) -> Result<TcpForward> {
        if options.port == 0 {
            return Err(Error::Protocol("guest port must be 1-65535".into()));
        }
        if !self.supports_tunnel_forward().await? {
            let version = self
                .capabilities()
                .await
                .map(|c| c.version)
                .unwrap_or_default();
            return Err(unsupported(format!(
                "cua-spacesd {version} does not advertise {TUNNEL_FORWARD_FEATURE:?}; \
                 upgrade the driver in the sandbox image"
            )));
        }
        let ticket = mint(self, &options).await?;
        let listener = TcpListener::bind(options.bind)
            .await
            .map_err(|e| Error::Transport(format!("cannot listen on {}: {e}", options.bind)))?;
        let local_addr = listener
            .local_addr()
            .map_err(|e| Error::Transport(e.to_string()))?;
        let shared = Arc::new(Shared {
            client: self.clone(),
            forward_ids: Mutex::new(vec![ticket.forward_id.clone()]),
            ticket: tokio::sync::Mutex::new(ticket),
            options: options.clone(),
            stats: ForwardStats::default(),
        });
        let (stop, stopped) = watch::channel(false);
        let task = tokio::spawn(accept_loop(listener, shared.clone(), stopped));
        Ok(TcpForward {
            local_addr,
            guest_port: options.port,
            shared,
            stop,
            task: Some(task),
        })
    }

    /// Opens a WebSocket to a driver-relative path (such as the `ws_path` of
    /// a tunnel, hotspot or media ticket) with this client's endpoint, path
    /// prefix and gateway credentials.
    pub async fn open_websocket(
        &self,
        ws_path: &str,
    ) -> Result<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>>
    {
        let options = &self.inner.options;
        let url = options.endpoint.ws_url(ws_path);
        let mut request = url
            .as_str()
            .into_client_request()
            .map_err(|e| Error::Protocol(format!("invalid WebSocket URL: {e}")))?;
        HeaderInjector::new(&options.channel_config())?
            .apply(&mut request)
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;
        ensure_crypto_provider();
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_TUNNEL_MESSAGE_BYTES))
            .max_frame_size(Some(MAX_TUNNEL_MESSAGE_BYTES));
        let connect = tokio_tungstenite::connect_async_with_config(request, Some(config), true);
        match tokio::time::timeout(options.connect_timeout.max(Duration::from_secs(5)), connect)
            .await
        {
            Ok(Ok((ws, _))) => Ok(ws),
            Ok(Err(tungstenite::Error::Http(resp))) => {
                let status = resp.status();
                let body = resp
                    .body()
                    .as_deref()
                    .map(|b| String::from_utf8_lossy(&b[..b.len().min(512)]).into_owned())
                    .unwrap_or_default();
                let details = ErrorDetails {
                    code: match status.as_u16() {
                        401 => tonic::Code::Unauthenticated,
                        403 => tonic::Code::PermissionDenied,
                        404 | 410 => tonic::Code::NotFound,
                        _ => tonic::Code::Unavailable,
                    } as i32,
                    message: format!("WebSocket {status}: {}", body.trim()),
                    metadata: Default::default(),
                };
                Err(match status.as_u16() {
                    401 => Error::Unauthenticated(details),
                    403 => Error::PermissionDenied(details),
                    _ => Error::Transport(details.message),
                })
            }
            Ok(Err(e)) => Err(Error::Transport(format!("WebSocket {url}: {e}"))),
            Err(_) => Err(Error::Transport(format!("WebSocket {url}: timed out"))),
        }
    }
}

async fn mint(client: &SpacesdClient, options: &ForwardOptions) -> Result<Ticket> {
    let resp = client
        .tunnel()
        .forward(pb::ForwardRequest {
            port: options.port as u32,
            host: options.host.clone(),
            ttl: options.ttl.map(|d| pbjson_types::Duration {
                seconds: d.as_secs() as i64,
                nanos: d.subsec_nanos() as i32,
            }),
        })
        .await
        .map_err(|status| {
            if status.code() == tonic::Code::Unimplemented {
                unsupported(format!(
                    "cua-spacesd does not implement TunnelService.Forward: {}",
                    status.message()
                ))
            } else {
                Error::from(status)
            }
        })?
        .into_inner();
    let ws_path = if resp.ws_path.is_empty() {
        format!(
            "{}?ticket={}",
            cua_proto::metadata::TUNNEL_WS_PATH,
            resp.ticket
        )
    } else {
        resp.ws_path
    };
    let expires_at = resp.expires_at.and_then(|t| {
        let secs = u64::try_from(t.seconds).ok()?;
        Some(SystemTime::UNIX_EPOCH + Duration::new(secs, t.nanos.max(0) as u32))
    });
    Ok(Ticket {
        forward_id: resp.forward_id,
        ws_path,
        expires_at,
    })
}

impl Shared {
    /// The current ticket, renewed when it is about to expire or `stale`
    /// names the one a socket just refused.
    async fn ticket(&self, stale: Option<&str>) -> Result<Ticket> {
        let mut current = self.ticket.lock().await;
        let expiring = current
            .expires_at
            .is_some_and(|at| at <= SystemTime::now() + RENEW_MARGIN);
        if expiring || stale == Some(current.ws_path.as_str()) {
            let fresh = mint(&self.client, &self.options).await?;
            self.forward_ids
                .lock()
                .unwrap()
                .push(fresh.forward_id.clone());
            *current = fresh;
        }
        Ok(current.clone())
    }

    async fn open(
        &self,
    ) -> Result<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>>
    {
        let ticket = self.ticket(None).await?;
        match self.client.open_websocket(&ticket.ws_path).await {
            Ok(ws) => Ok(ws),
            // Expired or revoked ticket (401), or a forward the driver no
            // longer knows (410): mint a fresh one and try once more.
            Err(Error::Unauthenticated(_)) | Err(Error::Transport(_)) => {
                let ticket = self.ticket(Some(&ticket.ws_path)).await?;
                self.client.open_websocket(&ticket.ws_path).await
            }
            Err(e) => Err(e),
        }
    }
}

async fn accept_loop(
    listener: TcpListener,
    shared: Arc<Shared>,
    mut stopped: watch::Receiver<bool>,
) {
    let permits = Arc::new(Semaphore::new(shared.options.max_connections.max(1)));
    let mut connections = tokio::task::JoinSet::new();
    loop {
        let permit = tokio::select! {
            p = permits.clone().acquire_owned() => match p { Ok(p) => p, Err(_) => break },
            _ = stopped.changed() => break,
        };
        let inbound = tokio::select! {
            r = listener.accept() => match r {
                Ok((s, _)) => s,
                Err(e) => {
                    tracing::warn!(error = %e, "tunnel listener failed");
                    break;
                }
            },
            _ = stopped.changed() => break,
        };
        // Reap finished connections so the set stays bounded.
        while connections.try_join_next().is_some() {}
        let shared = shared.clone();
        let stopped = stopped.clone();
        connections.spawn(async move {
            let _permit = permit;
            shared.stats.connections.fetch_add(1, Ordering::Relaxed);
            let _ = inbound.set_nodelay(true);
            let ws = match shared.open().await {
                Ok(ws) => ws,
                Err(e) => {
                    shared.stats.failed.fetch_add(1, Ordering::Relaxed);
                    tracing::warn!(error = %e, port = shared.options.port, "tunnel socket failed");
                    return;
                }
            };
            shared.stats.active.fetch_add(1, Ordering::Relaxed);
            relay(inbound, ws, &shared.stats, stopped).await;
            shared.stats.active.fetch_sub(1, Ordering::Relaxed);
        });
    }
    // Dropping the set aborts every relayed connection.
    connections.shutdown().await;
}

/// Splices one local connection and its WebSocket until either side ends
/// or the forward stops.
async fn relay(
    inbound: TcpStream,
    ws: tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>,
    stats: &ForwardStats,
    mut stopped: watch::Receiver<bool>,
) {
    let (mut local_rx, mut local_tx) = inbound.into_split();
    let (mut ws_tx, mut ws_rx) = ws.split();
    // Set when the local side reaches EOF: from then on the guest gets at
    // most DRAIN_AFTER_CLOSE to finish.
    let (local_done_tx, mut local_done) = watch::channel(false);
    let up = async {
        let mut buf = vec![0u8; RELAY_CHUNK_BYTES];
        loop {
            match local_rx.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    stats.bytes_to_guest.fetch_add(n as u64, Ordering::Relaxed);
                    if ws_tx
                        .send(Message::Binary(bytes::Bytes::copy_from_slice(&buf[..n])))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }
        let _ = ws_tx.send(Message::Close(None)).await;
        let _ = local_done_tx.send(true);
    };
    let down = async {
        loop {
            let next = tokio::select! {
                m = ws_rx.next() => m,
                _ = async {
                    let _ = local_done.wait_for(|d| *d).await;
                    tokio::time::sleep(DRAIN_AFTER_CLOSE).await;
                } => None,
            };
            match next {
                Some(Ok(Message::Binary(data))) => {
                    stats
                        .bytes_from_guest
                        .fetch_add(data.len() as u64, Ordering::Relaxed);
                    if local_tx.write_all(&data).await.is_err() {
                        break;
                    }
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            }
        }
        let _ = local_tx.shutdown().await;
    };
    // The guest closing ends the connection; the local side closing first
    // leaves the guest a bounded drain.
    let spliced = async {
        tokio::pin!(up);
        tokio::pin!(down);
        tokio::select! {
            _ = &mut down => {}
            _ = &mut up => down.await,
        }
    };
    tokio::select! {
        _ = spliced => {}
        _ = stopped.wait_for(|s| *s) => {}
    }
}
