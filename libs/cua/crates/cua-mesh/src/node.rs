// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! [`Mesh`]: one node on the mesh (a client device or a hosted machine).

use std::collections::{BTreeSet, HashMap};
use std::net::{IpAddr, SocketAddr, SocketAddrV6};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::mpsc;

use crate::coord::{self, AuthProvider, Control, ControlConfig, ControlState, PairVerifier};
use crate::engine::{Engine, Path};
use crate::key::{NodeIdentity, PublicKey};
use crate::proto::{self, NodeKind, PeerInfo, PeerUsage};
use crate::quic::{self, MeshStream, StreamHeader};
use crate::vnet::VSocket;
use crate::{Error, Result};

/// The default mesh UDP port a machine forwards to its local QUIC media
/// port, and the one a client forward sends media datagrams to.
///
/// It is a port of the mesh's own overlay network (an in-process socket of
/// the node's own mesh address), not a port of the operating system, so two
/// nodes on one computer never compete for it. A host and the clients that
/// dial it must agree, so both take it from [`MeshConfig::media_udp_port`].
/// Every Cua build leaves it at 3212 (guest Spaces and older cua-spacesd
/// fix it there); the setting is an override for a deployment that must
/// move it.
pub const MEDIA_UDP_PORT: u16 = 3212;

/// How long a connection to a peer the map lists online may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// ... and to one it lists offline (only a direct path could carry it).
const OFFLINE_CONNECT_TIMEOUT: Duration = Duration::from_millis(1500);

/// How a node is set up.
#[derive(Clone)]
pub struct MeshConfig {
    /// The coordinator (the relay's base URL: `https://relay.example`).
    pub coordinator: String,
    /// Machine or client.
    pub kind: NodeKind,
    /// Display name.
    pub name: String,
    /// Where the node identity lives (`0600` JSON, created on first use).
    /// `None`: a fresh in-memory identity the coordinator forgets when this
    /// node disconnects (a one-off CLI run).
    pub identity_path: Option<PathBuf>,
    /// Credentials for the coordinator, fetched before every connect.
    pub auth: AuthProvider,
    /// UDP port (0: any).
    pub listen_port: u16,
    /// Static mode, machines: decides pairing requests.
    pub pair_verifier: Option<PairVerifier>,
    /// Static mode, machines: where approved client keys persist.
    pub approved_path: Option<PathBuf>,
    /// Extra STUN servers (`host:port`) besides the coordinator's.
    pub stun: Vec<String>,
    /// Never use direct paths (tests; `CUA_MESH_RELAY_ONLY=1`).
    pub relay_only: bool,
    /// The mesh UDP port of the peers' QUIC media service: client forwards
    /// send media datagrams to it, and a machine serves it with
    /// [`Mesh::serve_udp`] on the same number. Default
    /// [`MEDIA_UDP_PORT`].
    pub media_udp_port: u16,
    /// Divides the wait before reconnecting to the coordinator (tests
    /// only; leave at 1).
    #[doc(hidden)]
    pub reconnect_speedup: u32,
}

impl std::fmt::Debug for MeshConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MeshConfig")
            .field("coordinator", &self.coordinator)
            .field("kind", &self.kind)
            .field("name", &self.name)
            .finish()
    }
}

impl MeshConfig {
    /// Defaults for a node of `kind` at `coordinator`.
    pub fn new(
        coordinator: impl Into<String>,
        kind: NodeKind,
        identity_path: Option<PathBuf>,
        auth: AuthProvider,
    ) -> Self {
        Self {
            coordinator: coordinator.into(),
            kind,
            name: hostname(),
            identity_path,
            auth,
            listen_port: 0,
            pair_verifier: None,
            approved_path: None,
            stun: Vec::new(),
            relay_only: std::env::var("CUA_MESH_RELAY_ONLY").is_ok_and(|v| v == "1"),
            media_udp_port: MEDIA_UDP_PORT,
            reconnect_speedup: 1,
        }
    }
}

/// The machine's hostname, for display.
pub fn hostname() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|h| !h.is_empty())
        .or_else(|| {
            std::process::Command::new("hostname")
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        })
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "cua".into())
}

/// Static-mode client keys a machine approved (persisted).
#[derive(Debug, Default)]
pub struct Approved {
    path: Option<PathBuf>,
    keys: RwLock<BTreeSet<PublicKey>>,
}

impl Approved {
    /// Loads from `path` (missing: empty).
    pub fn load(path: Option<PathBuf>) -> Self {
        let keys = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|raw| serde_json::from_str::<Vec<PublicKey>>(&raw).ok())
            .unwrap_or_default();
        Self {
            path,
            keys: RwLock::new(keys.into_iter().collect()),
        }
    }

    /// Approved?
    pub fn contains(&self, k: &PublicKey) -> bool {
        self.keys.read().expect("keys").contains(k)
    }

    /// Approves `k` (and saves).
    pub fn add(&self, k: PublicKey) {
        let snapshot: Vec<PublicKey> = {
            let mut keys = self.keys.write().expect("keys");
            if !keys.insert(k) {
                return;
            }
            keys.iter().copied().collect()
        };
        if let Some(path) = &self.path
            && let Ok(json) = serde_json::to_vec_pretty(&snapshot)
            && let Err(e) = crate::key::write_private(path, &json)
        {
            tracing::warn!(error = %e, "cannot save approved mesh peers");
        }
    }

    /// Revokes `k`.
    pub fn remove(&self, k: &PublicKey) -> bool {
        let snapshot: Vec<PublicKey> = {
            let mut keys = self.keys.write().expect("keys");
            if !keys.remove(k) {
                return false;
            }
            keys.iter().copied().collect()
        };
        if let Some(path) = &self.path
            && let Ok(json) = serde_json::to_vec_pretty(&snapshot)
        {
            let _ = crate::key::write_private(path, &json);
        }
        true
    }
}

/// A stream a peer opened to this node.
pub struct IncomingStream {
    /// Who.
    pub peer: PeerInfo,
    /// What it asked for.
    pub header: StreamHeader,
    /// The stream.
    pub stream: MeshStream,
}

impl std::fmt::Debug for IncomingStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IncomingStream")
            .field("peer", &self.peer.node_key)
            .field("service", &self.header.service)
            .finish()
    }
}

/// A peer, for status displays and measurements.
#[derive(Debug, Clone, Serialize)]
pub struct PeerStatus {
    /// Node key.
    pub node_key: PublicKey,
    /// Name.
    pub name: String,
    /// Machine id (machines).
    pub machine_id: Option<String>,
    /// Kind.
    pub kind: NodeKind,
    /// Online at the coordinator.
    pub online: bool,
    /// `direct <addr>` or `relay`.
    pub path: String,
    /// Direct path round trip, when direct.
    pub rtt_ms: Option<f64>,
    /// Seconds since the last WireGuard handshake.
    pub handshake_age_s: Option<f64>,
    /// Encrypted bytes sent and received, direct and relayed.
    pub tx_direct: u64,
    /// See `tx_direct`.
    pub tx_relay: u64,
    /// See `tx_direct`.
    pub rx_direct: u64,
    /// See `tx_direct`.
    pub rx_relay: u64,
    /// Path switches so far.
    pub path_switches: u64,
}

/// The node's status.
#[derive(Debug, Clone, Serialize)]
pub struct Status {
    /// Mesh address.
    pub addr: std::net::Ipv6Addr,
    /// Node key.
    pub node_key: PublicKey,
    /// Coordinator connection.
    pub connected: bool,
    /// Account or static.
    pub mode: Option<proto::Mode>,
    /// Our candidate endpoints.
    pub endpoints: Vec<SocketAddr>,
    /// Peers.
    pub peers: Vec<PeerStatus>,
    /// Last coordinator error.
    pub last_error: Option<String>,
}

struct Inner {
    identity: NodeIdentity,
    engine: Arc<Engine>,
    control: Arc<Control>,
    endpoint: quinn::Endpoint,
    conns: tokio::sync::Mutex<HashMap<PublicKey, quinn::Connection>>,
    /// One connect at a time per peer (callers to the same peer share it);
    /// peers never wait on each other's connects.
    connecting: std::sync::Mutex<HashMap<PublicKey, Arc<tokio::sync::Mutex<()>>>>,
    incoming: tokio::sync::Mutex<mpsc::Receiver<IncomingStream>>,
    stun: Vec<String>,
    shutdown: coord::tokio_util_cancel::Token,
    approved: Option<Arc<Approved>>,
    /// [`MeshConfig::media_udp_port`].
    media_udp_port: u16,
    udp_services: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    /// The peer flows of every [`Mesh::serve_udp`] service.
    udp_flows: Mutex<Vec<UdpFlows>>,
    endpoints: Mutex<Vec<SocketAddr>>,
    /// Connections peers opened to us, so a peer leaving the map is cut.
    inbound: Mutex<Vec<(PublicKey, quinn::Connection)>>,
}

/// One node on the mesh. Cheap to clone.
#[derive(Clone)]
pub struct Mesh {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Mesh {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mesh")
            .field("addr", &self.inner.identity.addr())
            .finish()
    }
}

impl Mesh {
    /// Starts the node: binds its UDP sockets, connects to the coordinator
    /// in the background and serves its stream endpoint. Use
    /// [`Mesh::wait_ready`] to wait for the first network map.
    pub async fn start(cfg: MeshConfig) -> Result<Self> {
        let identity = match &cfg.identity_path {
            Some(path) => NodeIdentity::load_or_create(path)?,
            None => NodeIdentity::generate()?,
        };
        let engine = Engine::new(identity.clone(), cfg.listen_port, cfg.relay_only)
            .await
            .map_err(|e| Error::Io(format!("udp: {e}")))?;
        let control = Control::new();
        let approved = match cfg.kind {
            NodeKind::Machine => Some(Arc::new(Approved::load(cfg.approved_path.clone()))),
            NodeKind::Client => None,
        };
        let sock = engine
            .vnet
            .bind(quic::STREAM_PORT)
            .map_err(|e| Error::Io(e.to_string()))?;
        let endpoint = quic::endpoint(&identity, sock)?;
        let (in_tx, in_rx) = mpsc::channel(256);
        let shutdown = coord::tokio_util_cancel::Token::new();
        let inner = Arc::new(Inner {
            identity: identity.clone(),
            engine: engine.clone(),
            control: control.clone(),
            endpoint: endpoint.clone(),
            conns: tokio::sync::Mutex::new(HashMap::new()),
            connecting: std::sync::Mutex::new(HashMap::new()),
            incoming: tokio::sync::Mutex::new(in_rx),
            stun: cfg.stun.clone(),
            shutdown: shutdown.clone(),
            approved: approved.clone(),
            media_udp_port: cfg.media_udp_port,
            udp_services: Mutex::new(Vec::new()),
            udp_flows: Mutex::new(Vec::new()),
            endpoints: Mutex::new(Vec::new()),
            inbound: Mutex::new(Vec::new()),
        });
        // Coordinator connection.
        tokio::spawn(coord::run(
            ControlConfig {
                url: cfg.coordinator.clone(),
                kind: cfg.kind,
                name: cfg.name.clone(),
                auth: cfg.auth.clone(),
                pair_verifier: cfg.pair_verifier.clone(),
                approved,
                ephemeral: cfg.identity_path.is_none(),
                reconnect_speedup: cfg.reconnect_speedup.max(1),
            },
            control,
            engine.clone(),
            identity,
            shutdown.clone(),
        ));
        // Incoming streams (machines serve; a client refuses any
        // connection a peer opens to it).
        tokio::spawn(accept_loop(
            Arc::downgrade(&inner),
            endpoint,
            in_tx,
            cfg.kind == NodeKind::Machine,
        ));
        // Endpoint discovery.
        tokio::spawn(discovery_loop(Arc::downgrade(&inner)));
        // A refused device loses every connection at once.
        tokio::spawn(denial_loop(
            Arc::downgrade(&inner),
            inner.control.state.subscribe(),
        ));
        // Peers that leave the map lose their connections.
        tokio::spawn(prune_loop(Arc::downgrade(&inner), engine.subscribe_peers()));
        Ok(Self { inner })
    }

    /// Waits until the coordinator welcomed this node and sent a map.
    pub async fn wait_ready(&self, timeout: Duration) -> Result<()> {
        let mut rx = self.inner.control.state.subscribe();
        // Ready, or a coordinator that says it has no mesh or refuses this
        // node (a revoked device): no point waiting for either to change.
        let ready = rx.wait_for(|s| {
            (s.connected && s.has_netmap)
                || s.denied.is_some()
                || s.last_error
                    .as_deref()
                    .is_some_and(|e| e.contains("no mesh coordinator"))
        });
        match tokio::time::timeout(timeout, ready).await {
            Ok(Ok(s)) if s.connected && s.has_netmap => Ok(()),
            _ => {
                let s = self.inner.control.state.borrow().clone();
                if let Some(reason) = s.denied {
                    return Err(Error::Refused(reason));
                }
                Err(match s.last_error {
                    Some(e) if e.contains("no mesh coordinator") => Error::Unsupported(e),
                    Some(e) => Error::Unreachable(e),
                    None => Error::Unreachable("the mesh coordinator did not answer".into()),
                })
            }
        }
    }

    /// The coordinator connection's state.
    pub fn control_state(&self) -> ControlState {
        self.inner.control.state.borrow().clone()
    }

    /// This node's key.
    pub fn node_key(&self) -> PublicKey {
        self.inner.identity.public()
    }

    /// This node's mesh address.
    pub fn addr(&self) -> std::net::Ipv6Addr {
        self.inner.identity.addr()
    }

    /// The mesh UDP port client forwards of this node send media datagrams
    /// to ([`MeshConfig::media_udp_port`]).
    pub fn media_udp_port(&self) -> u16 {
        self.inner.media_udp_port
    }

    /// Current peers.
    pub fn peers(&self) -> Vec<PeerInfo> {
        self.inner
            .engine
            .peers()
            .iter()
            .map(|p| p.info.read().expect("info").clone())
            .collect()
    }

    /// The peer serving machine `machine_id`, if it is in the map.
    pub fn machine(&self, machine_id: &str) -> Option<PeerInfo> {
        self.peers()
            .into_iter()
            .find(|p| p.kind == NodeKind::Machine && p.machine_id.as_deref() == Some(machine_id))
    }

    /// Waits up to `timeout` for machine `machine_id` to be in the map.
    pub async fn wait_machine(&self, machine_id: &str, timeout: Duration) -> Option<PeerInfo> {
        let mut rx = self.inner.engine.subscribe_peers();
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if let Some(p) = self.machine(machine_id) {
                return Some(p);
            }
            if tokio::time::timeout_at(deadline, rx.changed())
                .await
                .is_err()
            {
                return None;
            }
        }
    }

    /// [`Self::wait_machine`], but answers as soon as the coordinator's
    /// network map has arrived: a machine missing from it is not on the
    /// mesh (an older cua-spacesd, or a machine with the mesh off), so a
    /// caller that can fall back does not wait out `timeout` for it.
    pub async fn wait_machine_in_map(
        &self,
        machine_id: &str,
        timeout: Duration,
    ) -> Option<PeerInfo> {
        let mut peers = self.inner.engine.subscribe_peers();
        let mut control = self.inner.control.state.subscribe();
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if let Some(p) = self.machine(machine_id) {
                return Some(p);
            }
            if control.borrow_and_update().has_netmap {
                return None;
            }
            let changed = async {
                tokio::select! {
                    r = peers.changed() => r.is_ok(),
                    r = control.changed() => r.is_ok(),
                }
            };
            match tokio::time::timeout_at(deadline, changed).await {
                Ok(true) => {}
                _ => return self.machine(machine_id),
            }
        }
    }

    /// The path traffic to `peer` takes right now.
    pub fn path(&self, peer: &PublicKey) -> Option<Path> {
        self.inner.engine.peer(peer).map(|p| p.path())
    }

    /// The round trip of the path to `peer` when it is direct (the last
    /// disco pong), else `None`.
    pub fn rtt(&self, peer: &PublicKey) -> Option<Duration> {
        self.inner
            .engine
            .peer(peer)
            .filter(|p| matches!(p.path(), Path::Direct(_)))
            .and_then(|p| p.rtt())
    }

    /// Starts looking for a direct path to `peer` now (before any traffic)
    /// and waits up to `within` for one: the path then (`None` when the
    /// peer is not in the map).
    pub async fn warm(&self, peer: &PublicKey, within: Duration) -> Option<Path> {
        self.inner.engine.want(peer);
        let deadline = tokio::time::Instant::now() + within;
        loop {
            let path = self.path(peer)?;
            if matches!(path, Path::Direct(_)) || tokio::time::Instant::now() >= deadline {
                return Some(path);
            }
            self.inner.engine.want(peer);
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Status for displays and measurements.
    pub fn status(&self) -> Status {
        let state = self.inner.control.state.borrow().clone();
        let peers = self
            .inner
            .engine
            .peers()
            .iter()
            .map(|p| {
                let info = p.info.read().expect("info").clone();
                use std::sync::atomic::Ordering::Relaxed;
                PeerStatus {
                    node_key: info.node_key,
                    name: info.name.clone(),
                    machine_id: info.machine_id.clone(),
                    kind: info.kind,
                    online: info.online,
                    path: p.path().to_string(),
                    rtt_ms: p.rtt().map(|d| d.as_secs_f64() * 1000.0),
                    handshake_age_s: p.handshake_age().map(|d| d.as_secs_f64()),
                    tx_direct: p.counters.tx_direct.load(Relaxed),
                    tx_relay: p.counters.tx_relay.load(Relaxed),
                    rx_direct: p.counters.rx_direct.load(Relaxed),
                    rx_relay: p.counters.rx_relay.load(Relaxed),
                    path_switches: p.path_switches(),
                }
            })
            .collect();
        Status {
            addr: self.addr(),
            node_key: self.node_key(),
            connected: state.connected,
            mode: state.mode,
            endpoints: self.inner.control_endpoints(),
            peers,
            last_error: state.last_error,
        }
    }

    async fn connection(&self, peer: &PeerInfo) -> Result<quinn::Connection> {
        if let Some(c) = self.live_connection(&peer.node_key).await {
            return Ok(c);
        }
        // Per peer: a peer that does not answer holds up only its own
        // callers, never connections to the others (an offline computer
        // used to stall every other one for the whole connect timeout).
        let gate = self
            .inner
            .connecting
            .lock()
            .expect("connecting")
            .entry(peer.node_key)
            .or_default()
            .clone();
        let _turn = gate.lock().await;
        if let Some(c) = self.live_connection(&peer.node_key).await {
            return Ok(c);
        }
        self.inner.engine.want(&peer.node_key);
        // A peer the map lists offline gets a short try (a direct path may
        // still carry it), not the full wait.
        let within = if peer.online {
            CONNECT_TIMEOUT
        } else {
            OFFLINE_CONNECT_TIMEOUT
        };
        let c = tokio::time::timeout(
            within,
            quic::connect(&self.inner.endpoint, peer.addr(), &peer.quic_pin),
        )
        .await
        .map_err(|_| {
            Error::Unreachable(if peer.online {
                format!("{} did not answer on the mesh", peer.name)
            } else {
                format!("{} is offline", peer.name)
            })
        })??;
        // Refused, or the peer left the map, while connecting: as if the
        // connection had been open then (`denial_loop`, `prune_loop`).
        if let Some(reason) = self.denied() {
            c.close(3u32.into(), b"device refused");
            return Err(Error::Refused(reason));
        }
        if self.inner.engine.peer(&peer.node_key).is_none() {
            c.close(2u32.into(), b"peer removed");
            return Err(Error::NotFound(format!("{} left the mesh", peer.name)));
        }
        self.inner
            .conns
            .lock()
            .await
            .insert(peer.node_key, c.clone());
        Ok(c)
    }

    async fn live_connection(&self, peer: &PublicKey) -> Option<quinn::Connection> {
        self.inner
            .conns
            .lock()
            .await
            .get(peer)
            .filter(|c| c.close_reason().is_none())
            .cloned()
    }

    /// Why this node can no longer use the mesh, while it holds: its
    /// device was refused, or the relay turned its mesh off. Callers with
    /// a fallback (the relay's HTTPS path) switch to it.
    pub fn unusable(&self) -> Option<String> {
        let s = self.inner.control.state.borrow();
        if s.connected {
            return None;
        }
        s.denied.clone().or_else(|| {
            s.unsupported.then(|| {
                s.last_error
                    .clone()
                    .unwrap_or_else(|| "the relay has no mesh coordinator".into())
            })
        })
    }

    /// The coordinator's refusal of this node, while it stands.
    fn denied(&self) -> Option<String> {
        let s = self.inner.control.state.borrow();
        (!s.connected).then(|| s.denied.clone()).flatten()
    }

    /// Opens a stream to `service` on `peer`.
    pub async fn open_stream(
        &self,
        peer: &PublicKey,
        service: &str,
        session: Option<String>,
    ) -> Result<MeshStream> {
        // A revoked device: say so now, not after a peer that already
        // dropped it fails to answer.
        if let Some(reason) = self.denied() {
            return Err(Error::Refused(reason));
        }
        let info = self
            .inner
            .engine
            .peer(peer)
            .map(|p| p.info.read().expect("info").clone())
            .ok_or_else(|| Error::NotFound(format!("peer {} is not in the mesh", peer.short())))?;
        let header = StreamHeader {
            service: service.to_owned(),
            session,
        };
        for attempt in 0..2 {
            let conn = self.connection(&info).await?;
            match conn.open_bi().await {
                Ok((mut send, recv)) => {
                    quic::write_header(&mut send, &header).await?;
                    return Ok(quic::join(send, recv));
                }
                Err(e) if attempt == 0 => {
                    tracing::debug!(error = %e, "mesh connection went away; reconnecting");
                    self.inner.conns.lock().await.remove(&info.node_key);
                }
                Err(e) => return Err(Error::Unreachable(e.to_string())),
            }
        }
        unreachable!("two attempts")
    }

    /// The next stream a peer opened to this node.
    pub async fn accept(&self) -> Option<IncomingStream> {
        self.inner.incoming.lock().await.recv().await
    }

    /// Forwards a loopback port to `service` on `peer`: every TCP
    /// connection to the returned address becomes one mesh stream, and
    /// datagrams to the same port number on UDP go to the peer's
    /// [`MeshConfig::media_udp_port`] (default [`MEDIA_UDP_PORT`]). Dropping
    /// the [`Forward`] stops it.
    pub async fn forward(
        &self,
        peer: PublicKey,
        service: &str,
        session: Option<String>,
    ) -> Result<Forward> {
        self.forward_on(peer, service, session, std::net::Ipv4Addr::LOCALHOST.into())
            .await
    }

    /// [`Mesh::forward`] on a port of `ip` (an address of this machine)
    /// instead of loopback: a guest of this machine (a VM on its NAT, a
    /// container on its bridge) reaches the forward at its gateway address.
    pub async fn forward_on(
        &self,
        peer: PublicKey,
        service: &str,
        session: Option<String>,
        ip: std::net::IpAddr,
    ) -> Result<Forward> {
        let (tcp, udp) = bind_pair(ip).await?;
        let local = tcp.local_addr().map_err(|e| Error::Io(e.to_string()))?;
        let me = self.clone();
        let service = service.to_owned();
        let session2 = session.clone();
        let tcp_task = tokio::spawn(async move {
            loop {
                let Ok((conn, _)) = tcp.accept().await else {
                    continue;
                };
                let me = me.clone();
                let service = service.clone();
                let session = session2.clone();
                tokio::spawn(async move {
                    match me.open_stream(&peer, &service, session).await {
                        Ok(stream) => quic::splice(conn, stream).await,
                        // The process or the node is going away (a short
                        // `cua` run exiting): nothing to warn about.
                        Err(e) if stopping(&e, me.inner.shutdown.is_cancelled()) => {
                            tracing::debug!(error = %e, "mesh forward while stopping")
                        }
                        Err(e) => tracing::warn!(error = %e, "mesh forward"),
                    }
                });
            }
        });
        let udp_task = udp.map(|udp| {
            let me = self.clone();
            tokio::spawn(udp_client_forward(me, Arc::new(udp), peer))
        });
        Ok(Forward {
            local,
            tasks: [Some(tcp_task), udp_task].into_iter().flatten().collect(),
        })
    }

    /// Serves mesh UDP `port` by relaying each peer flow to `target` (a
    /// local UDP service, e.g. QUIC media).
    pub fn serve_udp(&self, port: u16, target: SocketAddr) -> Result<()> {
        let sock = self
            .inner
            .engine
            .vnet
            .bind(port)
            .map_err(|e| Error::Io(e.to_string()))?;
        let flows = UdpFlows {
            started: Instant::now(),
            map: Arc::default(),
        };
        self.inner
            .udp_flows
            .lock()
            .expect("udp flows")
            .push(flows.clone());
        let task = tokio::spawn(udp_server_forward(sock, target, flows));
        self.inner.udp_services.lock().expect("udp").push(task);
        Ok(())
    }

    /// Peers that sent a datagram to a [`Mesh::serve_udp`] service (QUIC
    /// media) within `within`.
    pub fn udp_active_peers(&self, within: Duration) -> Vec<PublicKey> {
        let within = within.as_millis() as u64;
        let addrs: BTreeSet<std::net::Ipv6Addr> = self
            .inner
            .udp_flows
            .lock()
            .expect("udp flows")
            .iter()
            .flat_map(|f| {
                let now = f.started.elapsed().as_millis() as u64;
                f.map
                    .lock()
                    .expect("flows")
                    .iter()
                    .filter(|(_, flow)| {
                        now.saturating_sub(flow.last_ms.load(Ordering::Relaxed)) <= within
                    })
                    .map(|(src, _)| *src.ip())
                    .collect::<Vec<_>>()
            })
            .collect();
        if addrs.is_empty() {
            return Vec::new();
        }
        self.peers()
            .into_iter()
            .filter(|p| addrs.contains(&p.addr()))
            .map(|p| p.node_key)
            .collect()
    }

    /// Account mode, machines: reports the streams peers hold open now
    /// (a full snapshot) to the coordinator, which meters them as it
    /// meters its own HTTPS path. Sent only when it changed, and again
    /// after a reconnect; a coordinator that does not meter is sent
    /// nothing. Cheap to call often.
    pub fn report_usage(&self, peers: Vec<PeerUsage>) {
        self.inner.control.set_usage(peers);
    }

    /// Static mode: pairs this client with machine `machine_id` by proving
    /// its env token (the proof goes through the coordinator; the token does
    /// not).
    pub async fn pair(&self, machine_id: &str, env_token: &str) -> Result<()> {
        let ts = proto::now_secs();
        let proof = proto::pair_proof(env_token, machine_id, &self.node_key(), ts);
        self.inner
            .control
            .pair(machine_id, ts, proof, Duration::from_secs(15))
            .await
    }

    /// Static mode, machines: forgets an approved client.
    pub fn revoke(&self, client: &PublicKey) -> bool {
        let removed = self
            .inner
            .approved
            .as_ref()
            .is_some_and(|a| a.remove(client));
        if removed {
            self.inner.engine.remove_peer(client);
        }
        removed
    }

    /// Moves to new UDP ports (a network change) and re-discovers paths.
    pub async fn rebind(&self) -> Result<()> {
        self.inner
            .engine
            .rebind()
            .await
            .map_err(|e| Error::Io(e.to_string()))?;
        refresh_endpoints(&self.inner).await;
        Ok(())
    }

    /// Never use direct paths from now on (or again).
    pub fn set_relay_only(&self, on: bool) {
        self.inner.engine.set_relay_only(on);
    }

    /// Stops the node.
    pub fn shutdown(&self) {
        self.inner.shutdown.cancel();
        self.inner.endpoint.close(0u32.into(), b"shutdown");
        self.inner.engine.shutdown();
        for t in self.inner.udp_services.lock().expect("udp").drain(..) {
            t.abort();
        }
    }
}

/// Whether a forward's failure only means this node is stopping: it was
/// shut down, or the QUIC endpoint is (the runtime is ending).
fn stopping(e: &Error, shut_down: bool) -> bool {
    shut_down || matches!(e, Error::Unreachable(m) if m.contains("endpoint stopping"))
}

impl Inner {
    fn control_endpoints(&self) -> Vec<SocketAddr> {
        self.endpoints.lock().expect("endpoints").clone()
    }
}

/// A loopback forward ([`Mesh::forward`]).
#[derive(Debug)]
pub struct Forward {
    /// Where to connect (TCP, and UDP on the same port when it was free).
    pub local: SocketAddr,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Drop for Forward {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

async fn bind_pair(ip: std::net::IpAddr) -> Result<(TcpListener, Option<UdpSocket>)> {
    for _ in 0..16 {
        let tcp = TcpListener::bind((ip, 0))
            .await
            .map_err(|e| Error::Io(e.to_string()))?;
        let port = tcp
            .local_addr()
            .map_err(|e| Error::Io(e.to_string()))?
            .port();
        if let Ok(udp) = UdpSocket::bind((ip, port)).await {
            return Ok((tcp, Some(udp)));
        }
    }
    let tcp = TcpListener::bind((ip, 0))
        .await
        .map_err(|e| Error::Io(e.to_string()))?;
    Ok((tcp, None))
}

async fn accept_loop(
    inner: std::sync::Weak<Inner>,
    endpoint: quinn::Endpoint,
    tx: mpsc::Sender<IncomingStream>,
    serves: bool,
) {
    while let Some(incoming) = endpoint.accept().await {
        if !serves {
            incoming.refuse();
            continue;
        }
        let inner = inner.clone();
        let tx = tx.clone();
        tokio::spawn(async move {
            let Ok(conn) = incoming.await else { return };
            let SocketAddr::V6(remote) = conn.remote_address() else {
                return;
            };
            {
                let Some(strong) = inner.upgrade() else {
                    return;
                };
                let Some(peer) = strong.engine.peer_by_addr(remote.ip()) else {
                    conn.close(1u32.into(), b"not a peer");
                    return;
                };
                let mut inbound = strong.inbound.lock().expect("inbound");
                inbound.retain(|(_, c)| c.close_reason().is_none());
                inbound.push((peer.key(), conn.clone()));
            }
            loop {
                let Ok((send, mut recv)) = conn.accept_bi().await else {
                    return;
                };
                // The peer is whoever owns the source address: WireGuard
                // already authenticated it.
                let Some(strong) = inner.upgrade() else {
                    return;
                };
                let Some(peer) = strong.engine.peer_by_addr(remote.ip()) else {
                    conn.close(1u32.into(), b"not a peer");
                    return;
                };
                let info = peer.info.read().expect("info").clone();
                drop(strong);
                let tx = tx.clone();
                tokio::spawn(async move {
                    let header = match tokio::time::timeout(
                        Duration::from_secs(10),
                        quic::read_header(&mut recv),
                    )
                    .await
                    {
                        Ok(Ok(h)) => h,
                        _ => return,
                    };
                    let _ = tx
                        .send(IncomingStream {
                            peer: info,
                            header,
                            stream: quic::join(send, recv),
                        })
                        .await;
                });
            }
        });
    }
}

/// Closes every connection when the coordinator refuses this node (a
/// revoked device): its peers have dropped it, so open streams would only
/// hang until the idle timeout.
async fn denial_loop(
    inner: std::sync::Weak<Inner>,
    mut rx: tokio::sync::watch::Receiver<coord::ControlState>,
) {
    while rx.changed().await.is_ok() {
        let denied = {
            let s = rx.borrow_and_update();
            !s.connected && s.denied.is_some()
        };
        if !denied {
            continue;
        }
        let Some(inner) = inner.upgrade() else { return };
        for (_, c) in inner.conns.lock().await.drain() {
            c.close(3u32.into(), b"device refused");
        }
        for (_, c) in inner.inbound.lock().expect("inbound").drain(..) {
            c.close(3u32.into(), b"device refused");
        }
    }
}

async fn prune_loop(inner: std::sync::Weak<Inner>, mut rx: tokio::sync::watch::Receiver<u64>) {
    while rx.changed().await.is_ok() {
        let Some(inner) = inner.upgrade() else { return };
        let current: BTreeSet<PublicKey> = inner.engine.peers().iter().map(|p| p.key()).collect();
        let mut conns = inner.conns.lock().await;
        conns.retain(|k, c| {
            let keep = current.contains(k);
            if !keep {
                c.close(2u32.into(), b"peer removed");
            }
            keep
        });
        drop(conns);
        inner
            .connecting
            .lock()
            .expect("connecting")
            .retain(|k, _| current.contains(k));
        // Connections removed peers opened to us end too (a share revoked,
        // a device removed: cut at once, not at the idle timeout).
        inner.inbound.lock().expect("inbound").retain(|(k, c)| {
            let keep = current.contains(k) && c.close_reason().is_none();
            if !keep {
                c.close(2u32.into(), b"peer removed");
            }
            keep
        });
    }
}

/// An address of another overlay network's tunnel: Tailscale's
/// 100.64.0.0/10 or fd7a:115c:a1e0::/48 on its interface (`tailscale0`,
/// a macOS `utun`, Windows' `Tailscale`). Never a mesh endpoint: a path
/// through it is a tunnel in a tunnel, and often runs through that
/// network's own relays (DERP), slower than this mesh's relay.
fn is_overlay(name: &str, ip: IpAddr) -> bool {
    let name = name.to_ascii_lowercase();
    if !(name.starts_with("tailscale") || name.starts_with("utun")) {
        return false;
    }
    match ip {
        IpAddr::V4(a) => {
            let o = a.octets();
            o[0] == 100 && (o[1] & 0xc0) == 64
        }
        IpAddr::V6(a) => {
            let s = a.segments();
            s[0] == 0xfd7a && s[1] == 0x115c && s[2] == 0xa1e0
        }
    }
}

fn interface_endpoints(v4: u16, v6: Option<u16>) -> Vec<SocketAddr> {
    let mut out = Vec::new();
    if let Ok(ifs) = if_addrs::get_if_addrs() {
        for i in ifs {
            if i.is_loopback() || is_overlay(&i.name, i.ip()) {
                continue;
            }
            match i.ip() {
                IpAddr::V4(a) if !a.is_link_local() && !a.is_unspecified() => {
                    out.push(SocketAddr::new(IpAddr::V4(a), v4));
                }
                IpAddr::V6(a) => {
                    let s = a.segments();
                    let link_local = (s[0] & 0xffc0) == 0xfe80;
                    if let Some(p) = v6
                        && !link_local
                        && !a.is_unspecified()
                        && !crate::key::is_mesh_addr(&a)
                    {
                        out.push(SocketAddr::new(IpAddr::V6(a), p));
                    }
                }
                _ => {}
            }
        }
    }
    out
}

async fn resolve(server: &str) -> Option<SocketAddr> {
    tokio::net::lookup_host(server)
        .await
        .ok()?
        .find(SocketAddr::is_ipv4)
}

async fn refresh_endpoints(inner: &Arc<Inner>) -> Vec<SocketAddr> {
    let (v4, v6) = inner.engine.local_ports();
    let mut eps = interface_endpoints(v4, v6);
    let mut servers: Vec<String> = inner.control.state.borrow().stun.clone();
    servers.extend(inner.stun.iter().cloned());
    for s in servers.iter().take(4) {
        let t = std::time::Instant::now();
        let addr = resolve(s).await;
        let resolved = t.elapsed();
        let mapped = match addr {
            Some(addr) => inner.engine.stun(addr, Duration::from_secs(2)).await,
            None => None,
        };
        tracing::debug!(server = %s, ?resolved, took = ?t.elapsed(), ?mapped, "stun");
        if let Some(mapped) = mapped
            && !eps.contains(&mapped)
        {
            eps.push(mapped);
        }
    }
    eps.sort();
    eps.dedup();
    inner.engine.set_local_endpoints(eps.clone());
    inner.control.set_endpoints(eps.clone());
    *inner.endpoints.lock().expect("endpoints") = eps.clone();
    eps
}

async fn discovery_loop(inner: std::sync::Weak<Inner>) {
    let mut last_ifs: Vec<SocketAddr> = Vec::new();
    let mut last_full = None::<tokio::time::Instant>;
    let mut was_connected = false;
    loop {
        let Some(strong) = inner.upgrade() else {
            return;
        };
        if strong.shutdown.is_cancelled() {
            return;
        }
        let (v4, v6) = strong.engine.local_ports();
        let ifs = interface_endpoints(v4, v6);
        let connected = strong.control.state.borrow().connected;
        let changed = ifs != last_ifs;
        if changed && !last_ifs.is_empty() {
            // The network changed under us (roaming, Wi-Fi to Ethernet):
            // stop trusting direct paths and find new ones.
            tracing::info!("network change: re-discovering mesh paths");
            strong.engine.forget_paths();
        }
        let due = last_full.is_none_or(|t| t.elapsed() > Duration::from_secs(30));
        if changed || due || (connected && !was_connected) {
            refresh_endpoints(&strong).await;
            last_full = Some(tokio::time::Instant::now());
            last_ifs = ifs;
        }
        was_connected = connected;
        let shutdown = strong.shutdown.clone();
        let mut state = strong.control.state.subscribe();
        drop(strong);
        // Every 2 s, or at once when the coordinator connection changes
        // (a fresh connection learns its public endpoint right away).
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(2)) => {}
            _ = state.wait_for(|s| s.connected != connected) => {}
            _ = shutdown.cancelled() => return,
        }
    }
}

/// UDP flows of a client forward: local source to its mesh socket.
type Flows = HashMap<SocketAddr, (Arc<VSocket>, tokio::time::Instant)>;

async fn udp_client_forward(mesh: Mesh, local: Arc<UdpSocket>, peer: PublicKey) {
    // One mesh socket per local source, so replies find their way back.
    let flows: Arc<Mutex<Flows>> = Arc::default();
    let mut buf = vec![0u8; 65_535];
    loop {
        let Ok((n, src)) = local.recv_from(&mut buf).await else {
            continue;
        };
        let Some(info) = mesh
            .inner
            .engine
            .peer(&peer)
            .map(|p| p.info.read().expect("info").clone())
        else {
            continue;
        };
        mesh.inner.engine.want(&peer);
        let dst = SocketAddrV6::new(info.addr(), mesh.inner.media_udp_port, 0, 0);
        let sock = {
            let mut f = flows.lock().expect("flows");
            f.retain(|_, (_, at)| at.elapsed() < Duration::from_secs(180));
            match f.get_mut(&src) {
                Some((s, at)) => {
                    *at = tokio::time::Instant::now();
                    s.clone()
                }
                None => {
                    let Ok(s) = mesh.inner.engine.vnet.bind(0) else {
                        continue;
                    };
                    f.insert(src, (s.clone(), tokio::time::Instant::now()));
                    let back = local.clone();
                    let reader = s.clone();
                    let flows = flows.clone();
                    tokio::spawn(async move {
                        loop {
                            let r =
                                tokio::time::timeout(Duration::from_secs(180), reader.recv_from())
                                    .await;
                            match r {
                                Ok(Ok((data, _))) => {
                                    let _ = back.send_to(&data, src).await;
                                }
                                _ => {
                                    flows.lock().expect("flows").remove(&src);
                                    return;
                                }
                            }
                        }
                    });
                    s
                }
            }
        };
        let _ = sock.send_to(&buf[..n], dst);
    }
}

/// One peer flow of a [`Mesh::serve_udp`] service.
struct UdpFlow {
    local: Arc<UdpSocket>,
    /// When the peer last sent on it (ms since the service started).
    last_ms: AtomicU64,
}

/// A [`Mesh::serve_udp`] service's flows by peer address.
#[derive(Clone)]
struct UdpFlows {
    started: Instant,
    map: Arc<Mutex<HashMap<SocketAddrV6, Arc<UdpFlow>>>>,
}

async fn udp_server_forward(sock: Arc<VSocket>, target: SocketAddr, flows: UdpFlows) {
    let started = flows.started;
    let flows = flows.map;
    loop {
        let Ok((data, src)) = sock.recv_from().await else {
            return;
        };
        let now_ms = started.elapsed().as_millis() as u64;
        let local = {
            let existing = flows.lock().expect("flows").get(&src).cloned();
            match existing {
                Some(f) => {
                    f.last_ms.store(now_ms, Ordering::Relaxed);
                    f.local.clone()
                }
                None => {
                    let bind = if target.is_ipv4() {
                        "127.0.0.1:0"
                    } else {
                        "[::1]:0"
                    };
                    let Ok(l) = UdpSocket::bind(bind).await else {
                        continue;
                    };
                    if l.connect(target).await.is_err() {
                        continue;
                    }
                    let l = Arc::new(l);
                    flows.lock().expect("flows").insert(
                        src,
                        Arc::new(UdpFlow {
                            local: l.clone(),
                            last_ms: AtomicU64::new(now_ms),
                        }),
                    );
                    let back = sock.clone();
                    let reader = l.clone();
                    let flows = flows.clone();
                    tokio::spawn(async move {
                        let mut buf = vec![0u8; 65_535];
                        loop {
                            match tokio::time::timeout(
                                Duration::from_secs(180),
                                reader.recv(&mut buf),
                            )
                            .await
                            {
                                Ok(Ok(n)) => {
                                    let _ = back.send_to(&buf[..n], src);
                                }
                                _ => {
                                    flows.lock().expect("flows").remove(&src);
                                    return;
                                }
                            }
                        }
                    });
                    l
                }
            }
        };
        let _ = local.send(&data).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stopping_node_is_not_a_warning() {
        let stopping_endpoint = Error::Unreachable("quic connect: endpoint stopping".into());
        assert!(stopping(&stopping_endpoint, false));
        assert!(stopping(&Error::Refused("revoked".into()), true));
        assert!(!stopping(&Error::Refused("revoked".into()), false));
        assert!(!stopping(&Error::Unreachable("timed out".into()), false));
    }

    #[test]
    fn tailscale_tunnel_addresses_are_not_mesh_endpoints() {
        let v4 = |s: &str| IpAddr::V4(s.parse().unwrap());
        let v6 = |s: &str| IpAddr::V6(s.parse().unwrap());
        assert!(is_overlay("utun4", v4("100.101.102.103")));
        assert!(is_overlay("tailscale0", v4("100.127.1.2")));
        assert!(is_overlay("Tailscale", v6("fd7a:115c:a1e0::7b38:3e77")));
        // The same ranges elsewhere (a cloud VPC using 100.64/10) stay.
        assert!(!is_overlay("eth0", v4("100.101.102.103")));
        // Other addresses on a tunnel interface stay (another VPN's).
        assert!(!is_overlay("utun2", v4("10.8.0.2")));
        assert!(!is_overlay("utun4", v4("100.128.0.1")));
        assert!(!is_overlay("en0", v4("192.168.1.100")));
    }
}
