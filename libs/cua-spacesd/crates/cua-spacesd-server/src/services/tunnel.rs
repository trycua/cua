// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `TunnelService` plus its ticket-authenticated WebSockets:
//! `/tunnel` (client→guest TCP forwarding) and `/hotspot` (guest→client
//! reverse-SOCKS egress, replacing the unauthenticated rcdp-socks).

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use cua_proto::env::v1::tunnel_service_server::{TunnelService, TunnelServiceServer};
use cua_proto::env::v1::*;
use cua_spacesd_socks::{Hub, Route};
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;
use tonic::{Code, Request, Response as GrpcResponse, Status};

use crate::auth::{caller, TicketScope};
use crate::context::ServerContext;
use crate::error::{session_not_found, status};
use crate::util::{duration, timestamp};

/// Default forward ticket lifetime.
pub const FORWARD_DEFAULT_TTL: Duration = Duration::from_secs(10 * 60);
/// Default hotspot ticket lifetime.
pub const HOTSPOT_DEFAULT_TTL: Duration = Duration::from_secs(60);

struct Forward {
    id: String,
    host: String,
    port: u16,
    expires_at: SystemTime,
    active: AtomicU32,
    bytes_in: AtomicU64,
    bytes_out: AtomicU64,
    closed: CancellationToken,
}

struct Hotspot {
    id: String,
    socks_address: SocketAddr,
    hub: Hub,
    peer_principal: Mutex<String>,
    stop: CancellationToken,
    set_system_proxy: bool,
}

/// State shared by the gRPC service and the WebSocket routes.
#[derive(Clone)]
pub struct TunnelState {
    ctx: ServerContext,
    forwards: Arc<Mutex<HashMap<String, Arc<Forward>>>>,
    hotspot: Arc<Mutex<Option<Arc<Hotspot>>>>,
}

impl TunnelState {
    /// Creates empty state.
    pub fn new(ctx: ServerContext) -> Self {
        let state = Self {
            ctx,
            forwards: Arc::default(),
            hotspot: Arc::default(),
        };
        // Revoked sessions (token file rotated or emptied): stop every
        // forward and the hotspot.
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let mut revoked = state.ctx.session_revocations();
            let shutdown = state.ctx.shutdown_token();
            let weak = (
                Arc::downgrade(&state.forwards),
                Arc::downgrade(&state.hotspot),
            );
            let ctx = state.ctx.clone();
            handle.spawn(async move {
                loop {
                    tokio::select! {
                        changed = revoked.changed() => if changed.is_err() { return },
                        _ = shutdown.cancelled() => return,
                    }
                    let (Some(forwards), Some(hotspot)) = (weak.0.upgrade(), weak.1.upgrade())
                    else {
                        return;
                    };
                    let state = TunnelState {
                        ctx: ctx.clone(),
                        forwards,
                        hotspot,
                    };
                    state.revoke_all();
                }
            });
        }
        state
    }

    /// Closes every forward and stops the hotspot.
    pub fn revoke_all(&self) {
        let forwards: Vec<_> = self
            .forwards
            .lock()
            .expect("forwards")
            .drain()
            .map(|(_, f)| f)
            .collect();
        for forward in &forwards {
            forward.closed.cancel();
        }
        if self.stop_hotspot(None) || !forwards.is_empty() {
            tracing::info!(forwards = forwards.len(), "tunnel sessions revoked");
        }
    }

    fn gc_forwards(&self) {
        let now = SystemTime::now();
        self.forwards
            .lock()
            .expect("forwards")
            .retain(|_, f| f.expires_at > now || f.active.load(Ordering::SeqCst) > 0);
    }

    fn stop_hotspot(&self, id: Option<&str>) -> bool {
        let mut slot = self.hotspot.lock().expect("hotspot");
        let matches = match (slot.as_ref(), id) {
            (Some(h), Some(id)) => h.id == id,
            (Some(_), None) => true,
            (None, _) => false,
        };
        if !matches {
            return false;
        }
        if let Some(hotspot) = slot.take() {
            hotspot.stop.cancel();
            if hotspot.set_system_proxy {
                self.ctx.update_init(|state| state.proxy_env.clear());
            }
            tracing::info!(hotspot = %hotspot.id, "hotspot stopped");
        }
        true
    }
}

/// Addresses of the guest's own interfaces.
fn own_addresses() -> Vec<IpAddr> {
    let networks = sysinfo::Networks::new_with_refreshed_list();
    networks
        .values()
        .flat_map(|data| {
            data.ip_networks()
                .iter()
                .map(|n| n.addr)
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Forward targets are limited to loopback and the guest's own addresses.
fn allowed_forward_host(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(ip) if ip.is_loopback() => true,
        Ok(ip) => own_addresses().contains(&ip),
        Err(_) => false,
    }
}

fn bypass_matches(bypass: &[String], host: &str) -> bool {
    let ip = host.parse::<IpAddr>().ok();
    bypass.iter().any(|rule| {
        if let (Some(ip), Some((net, bits))) = (ip, rule.split_once('/')) {
            if let (Ok(net), Ok(bits)) = (net.parse::<IpAddr>(), bits.parse::<u32>()) {
                return cidr_contains(net, bits, ip);
            }
        }
        let suffix = rule.trim_start_matches('.').to_ascii_lowercase();
        let host = host.to_ascii_lowercase();
        host == suffix || host.ends_with(&format!(".{suffix}"))
    })
}

fn cidr_contains(net: IpAddr, bits: u32, ip: IpAddr) -> bool {
    match (net, ip) {
        (IpAddr::V4(n), IpAddr::V4(i)) if bits <= 32 => {
            let mask = if bits == 0 {
                0
            } else {
                u32::MAX << (32 - bits)
            };
            u32::from(n) & mask == u32::from(i) & mask
        }
        (IpAddr::V6(n), IpAddr::V6(i)) if bits <= 128 => {
            let mask = if bits == 0 {
                0
            } else {
                u128::MAX << (128 - bits)
            };
            u128::from(n) & mask == u128::from(i) & mask
        }
        _ => false,
    }
}

pub(crate) fn ticket_path(base: &str, ticket: &str) -> String {
    format!(
        "{base}?ticket={}",
        percent_encoding::utf8_percent_encode(ticket, percent_encoding::NON_ALPHANUMERIC)
    )
}

/// The gRPC service.
#[derive(Clone)]
pub struct TunnelServiceImpl {
    state: TunnelState,
}

impl TunnelServiceImpl {
    /// Creates the service.
    pub fn new(state: TunnelState) -> Self {
        Self { state }
    }

    /// Tonic server.
    pub fn into_server(self) -> TunnelServiceServer<Self> {
        TunnelServiceServer::new(self)
    }
}

#[tonic::async_trait]
impl TunnelService for TunnelServiceImpl {
    async fn forward(
        &self,
        request: Request<ForwardRequest>,
    ) -> Result<GrpcResponse<ForwardResponse>, Status> {
        let principal = caller(&request).principal;
        let body = request.into_inner();
        if body.port == 0 || body.port > u16::MAX as u32 {
            return Err(crate::error::invalid("port must be 1-65535"));
        }
        let host = if body.host.is_empty() {
            "127.0.0.1".to_owned()
        } else {
            body.host
        };
        if !allowed_forward_host(&host) {
            return Err(status(
                Code::PermissionDenied,
                ErrorReason::PermissionDenied,
                format!(
                    "forwarding is limited to loopback and the guest's own addresses, not {host:?}"
                ),
            ));
        }
        let ttl = duration(body.ttl.as_ref())
            .filter(|d| !d.is_zero())
            .unwrap_or(FORWARD_DEFAULT_TTL);
        let id = format!("fwd-{}", crate::util::random_id(9));
        let (ticket, expires_at) =
            self.state
                .ctx
                .mint_ticket(TicketScope::Tunnel, &id, principal.as_ref(), ttl);
        self.state.gc_forwards();
        self.state.forwards.lock().expect("forwards").insert(
            id.clone(),
            Arc::new(Forward {
                id: id.clone(),
                host,
                port: body.port as u16,
                expires_at,
                active: AtomicU32::new(0),
                bytes_in: AtomicU64::new(0),
                bytes_out: AtomicU64::new(0),
                closed: CancellationToken::new(),
            }),
        );
        Ok(GrpcResponse::new(ForwardResponse {
            forward_id: id,
            ws_path: ticket_path(cua_proto::metadata::TUNNEL_WS_PATH, &ticket),
            ticket,
            expires_at: Some(timestamp(expires_at)),
        }))
    }

    async fn list_forwards(
        &self,
        _request: Request<ListForwardsRequest>,
    ) -> Result<GrpcResponse<ListForwardsResponse>, Status> {
        self.state.gc_forwards();
        let forwards = self
            .state
            .forwards
            .lock()
            .expect("forwards")
            .values()
            .map(|f| ForwardInfo {
                forward_id: f.id.clone(),
                host: f.host.clone(),
                port: f.port as u32,
                active_connections: f.active.load(Ordering::SeqCst),
                bytes_in: f.bytes_in.load(Ordering::Relaxed),
                bytes_out: f.bytes_out.load(Ordering::Relaxed),
                expires_at: Some(timestamp(f.expires_at)),
            })
            .collect();
        Ok(GrpcResponse::new(ListForwardsResponse { forwards }))
    }

    async fn close_forward(
        &self,
        request: Request<CloseForwardRequest>,
    ) -> Result<GrpcResponse<CloseForwardResponse>, Status> {
        let id = request.into_inner().forward_id;
        let forward = self
            .state
            .forwards
            .lock()
            .expect("forwards")
            .remove(&id)
            .ok_or_else(|| session_not_found("forward", &id))?;
        forward.closed.cancel();
        Ok(GrpcResponse::new(CloseForwardResponse {}))
    }

    async fn start_hotspot(
        &self,
        request: Request<StartHotspotRequest>,
    ) -> Result<GrpcResponse<StartHotspotResponse>, Status> {
        let principal = caller(&request).principal;
        let body = request.into_inner();
        let port = match body.socks_port {
            0 => 1080,
            p if p > u16::MAX as u32 => {
                return Err(crate::error::invalid("socks_port out of range"))
            }
            p => p as u16,
        };
        self.state.stop_hotspot(None);
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .map_err(|e| {
                status(
                    Code::FailedPrecondition,
                    ErrorReason::Unspecified,
                    format!("cannot bind the SOCKS listener on 127.0.0.1:{port}: {e}"),
                )
            })?;
        let socks_address = listener
            .local_addr()
            .map_err(|e| crate::error::internal(e.to_string()))?;
        let id = format!("hs-{}", crate::util::random_id(9));
        let hub = Hub::new();
        let stop = CancellationToken::new();
        let bypass = body.bypass.clone();
        let policy: cua_spacesd_socks::Policy = Arc::new(move |host: &str, _port| {
            if bypass_matches(&bypass, host) {
                Route::Direct
            } else {
                Route::Peer
            }
        });
        {
            let hub = hub.clone();
            let stop = stop.clone();
            tokio::spawn(async move {
                tokio::select! {
                    result = cua_spacesd_socks::serve_socks(listener, hub, policy) => {
                        if let Err(error) = result {
                            tracing::warn!(%error, "hotspot SOCKS listener failed");
                        }
                    }
                    _ = stop.cancelled() => {}
                }
            });
        }
        if body.set_system_proxy {
            let proxy = format!("socks5h://{socks_address}");
            let no_proxy = body.bypass.join(",");
            self.state.ctx.update_init(|state| {
                state.proxy_env.clear();
                for key in [
                    "ALL_PROXY",
                    "all_proxy",
                    "HTTPS_PROXY",
                    "https_proxy",
                    "HTTP_PROXY",
                    "http_proxy",
                ] {
                    state.proxy_env.insert(key.into(), proxy.clone());
                }
                if !no_proxy.is_empty() {
                    state.proxy_env.insert("NO_PROXY".into(), no_proxy.clone());
                    state.proxy_env.insert("no_proxy".into(), no_proxy);
                }
            });
        }
        let ttl = duration(body.ticket_ttl.as_ref())
            .filter(|d| !d.is_zero())
            .unwrap_or(HOTSPOT_DEFAULT_TTL);
        let (ticket, _) =
            self.state
                .ctx
                .mint_ticket(TicketScope::Hotspot, &id, principal.as_ref(), ttl);
        *self.state.hotspot.lock().expect("hotspot") = Some(Arc::new(Hotspot {
            id: id.clone(),
            socks_address,
            hub,
            peer_principal: Mutex::new(String::new()),
            stop,
            set_system_proxy: body.set_system_proxy,
        }));
        Ok(GrpcResponse::new(StartHotspotResponse {
            hotspot_id: id,
            ws_path: ticket_path(cua_proto::metadata::HOTSPOT_WS_PATH, &ticket),
            ticket,
            socks_address: socks_address.to_string(),
        }))
    }

    async fn stop_hotspot(
        &self,
        request: Request<StopHotspotRequest>,
    ) -> Result<GrpcResponse<StopHotspotResponse>, Status> {
        let id = request.into_inner().hotspot_id;
        if !self.state.stop_hotspot(Some(&id)) {
            return Err(session_not_found("hotspot", &id));
        }
        Ok(GrpcResponse::new(StopHotspotResponse {}))
    }

    async fn get_hotspot_status(
        &self,
        _request: Request<GetHotspotStatusRequest>,
    ) -> Result<GrpcResponse<GetHotspotStatusResponse>, Status> {
        let hotspot = self.state.hotspot.lock().expect("hotspot").clone();
        let Some(hotspot) = hotspot else {
            return Ok(GrpcResponse::new(GetHotspotStatusResponse {
                state: HotspotState::Stopped as i32,
                ..Default::default()
            }));
        };
        let stats = hotspot.hub.stats();
        let peer_principal_id = hotspot.peer_principal.lock().expect("peer").clone();
        Ok(GrpcResponse::new(GetHotspotStatusResponse {
            state: if hotspot.hub.is_connected() {
                HotspotState::Active
            } else {
                HotspotState::WaitingForPeer
            } as i32,
            hotspot_id: hotspot.id.clone(),
            peer_principal_id,
            socks_address: hotspot.socks_address.to_string(),
            active_connections: stats.active_connections.load(Ordering::SeqCst),
            bytes_out: stats.bytes_out.load(Ordering::Relaxed),
            bytes_in: stats.bytes_in.load(Ordering::Relaxed),
        }))
    }
}

pub(crate) fn refuse(error: crate::auth::TicketError) -> Response {
    (StatusCode::UNAUTHORIZED, error.to_string()).into_response()
}

pub(crate) fn with_protocol(
    upgrade: WebSocketUpgrade,
    subprotocol: Option<String>,
) -> WebSocketUpgrade {
    match subprotocol {
        Some(p) => upgrade.protocols([p]),
        None => upgrade,
    }
}

/// `GET /tunnel?ticket=…` (WebSocket): one TCP connection to the forward's
/// target; binary frames carry the byte stream both ways.
pub async fn tunnel_ws(
    State(state): State<TunnelState>,
    uri: Uri,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    let (claims, subprotocol) =
        match state
            .ctx
            .validate_request_ticket(&uri, &headers, TicketScope::Tunnel)
        {
            Ok(v) => v,
            Err(e) => return refuse(e),
        };
    let forward = state
        .forwards
        .lock()
        .expect("forwards")
        .get(&claims.resource)
        .cloned();
    let Some(forward) = forward else {
        return (StatusCode::GONE, "forward closed").into_response();
    };
    let target = match tokio::net::TcpStream::connect((forward.host.as_str(), forward.port)).await {
        Ok(t) => t,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                format!("connect {}:{}: {e}", forward.host, forward.port),
            )
                .into_response()
        }
    };
    let _ = target.set_nodelay(true);
    with_protocol(upgrade, subprotocol)
        .max_message_size(16 * 1024 * 1024)
        .on_upgrade(move |socket| splice_tcp(socket, target, forward))
}

async fn splice_tcp(socket: WebSocket, target: tokio::net::TcpStream, forward: Arc<Forward>) {
    forward.active.fetch_add(1, Ordering::SeqCst);
    let (mut ws_tx, mut ws_rx) = socket.split();
    let (mut tcp_rx, mut tcp_tx) = target.into_split();
    let closed = forward.closed.clone();
    let up = {
        let forward = forward.clone();
        async move {
            while let Some(Ok(message)) = ws_rx.next().await {
                match message {
                    Message::Binary(data) => {
                        forward
                            .bytes_in
                            .fetch_add(data.len() as u64, Ordering::Relaxed);
                        if tcp_tx.write_all(&data).await.is_err() {
                            break;
                        }
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
            let _ = tcp_tx.shutdown().await;
        }
    };
    let down = {
        let forward = forward.clone();
        async move {
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                match tcp_rx.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        forward.bytes_out.fetch_add(n as u64, Ordering::Relaxed);
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
        }
    };
    // Half-close aware: finish when the guest side is done sending, or when
    // the forward is revoked.
    tokio::select! {
        _ = async { tokio::join!(up, down) } => {}
        _ = closed.cancelled() => {}
    }
    forward.active.fetch_sub(1, Ordering::SeqCst);
}

/// `GET /hotspot?ticket=…` (WebSocket): attaches the caller as the egress
/// peer; the hotspot stops when this socket closes.
pub async fn hotspot_ws(
    State(state): State<TunnelState>,
    uri: Uri,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    let (claims, subprotocol) =
        match state
            .ctx
            .validate_request_ticket(&uri, &headers, TicketScope::Hotspot)
        {
            Ok(v) => v,
            Err(e) => return refuse(e),
        };
    let hotspot = state.hotspot.lock().expect("hotspot").clone();
    let Some(hotspot) = hotspot.filter(|h| h.id == claims.resource) else {
        return (StatusCode::GONE, "hotspot stopped").into_response();
    };
    if hotspot.hub.is_connected() {
        return (StatusCode::CONFLICT, "a peer is already attached").into_response();
    }
    *hotspot.peer_principal.lock().expect("peer") = claims.principal_id.clone();
    with_protocol(upgrade, subprotocol)
        .max_message_size(16 * 1024 * 1024)
        .on_upgrade(move |socket| async move {
            let (ws_tx, ws_rx) = socket.split();
            let source = ws_rx
                .take_while(|m| {
                    futures_util::future::ready(!matches!(m, Err(_) | Ok(Message::Close(_))))
                })
                .filter_map(|m| async move {
                    match m {
                        Ok(Message::Binary(data)) => Some(data.to_vec()),
                        _ => None,
                    }
                });
            let sink = ws_tx.with(|data: Vec<u8>| async move {
                Ok::<_, axum::Error>(Message::Binary(data.into()))
            });
            let source = Box::pin(source);
            let sink = Box::pin(sink);
            tokio::select! {
                _ = hotspot.hub.run_peer(source, sink) => {}
                _ = hotspot.stop.cancelled() => {}
            }
            state.stop_hotspot(Some(&hotspot.id));
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_hosts_are_limited() {
        assert!(allowed_forward_host("127.0.0.1"));
        assert!(allowed_forward_host("localhost"));
        assert!(allowed_forward_host("::1"));
        assert!(!allowed_forward_host("8.8.8.8"));
        assert!(!allowed_forward_host("example.com"));
    }

    #[test]
    fn bypass_rules() {
        let rules = vec!["10.0.0.0/8".to_owned(), ".corp.example".to_owned()];
        assert!(bypass_matches(&rules, "10.1.2.3"));
        assert!(!bypass_matches(&rules, "11.1.2.3"));
        assert!(bypass_matches(&rules, "git.corp.example"));
        assert!(bypass_matches(&rules, "corp.example"));
        assert!(!bypass_matches(&rules, "notcorp.example"));
    }
}
