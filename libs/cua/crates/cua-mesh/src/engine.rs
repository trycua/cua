// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The data plane: WireGuard sessions per peer over one UDP socket pair,
//! path discovery (direct first, hole punching, encrypted relay fallback)
//! and the virtual network the decrypted packets land on.
//!
//! - Every peer has a boringtun [`Tunn`] (the WireGuard protocol state
//!   machine; this crate never touches a TUN device or the kernel).
//! - A packet to a peer goes to its best direct path when one answered a
//!   disco ping recently, else through the coordinator's relay, which only
//!   ever sees WireGuard or disco ciphertext.
//! - Discovery pings every candidate endpoint of an active peer and asks
//!   the peer (through the relay) to ping back at the same time, which opens
//!   both NATs. The lowest-latency answering path wins; a path that stops
//!   answering falls back to the relay within [`TRUST`].

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use boringtun::noise::handshake::parse_handshake_anon;
use boringtun::noise::{Packet, Tunn, TunnResult};
use tokio::net::UdpSocket;
use tokio::sync::{Notify, mpsc, oneshot};

use crate::disco::{self, Message, Sealer};
use crate::key::{NodeIdentity, PublicKey};
use crate::proto::PeerInfo;
use crate::stun;
use crate::vnet::{Outbound, VNet};

/// How long a direct path stays in use after its last pong.
pub const TRUST: Duration = Duration::from_millis(6500);
/// How often an active peer's paths are re-pinged.
pub const PING_INTERVAL: Duration = Duration::from_millis(2500);
/// How often call-me-maybe goes out while no direct path works.
const CMM_INTERVAL: Duration = Duration::from_secs(5);
/// A peer is active while traffic flowed within this window.
const ACTIVE: Duration = Duration::from_secs(45);
/// Largest datagram handled.
const MAX_DATAGRAM: usize = 65_535;

/// The path a packet takes to a peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Path {
    /// Straight to this UDP endpoint.
    Direct(SocketAddr),
    /// Through the coordinator's relay.
    Relay,
}

impl std::fmt::Display for Path {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Path::Direct(a) => write!(f, "direct {a}"),
            Path::Relay => f.write_str("relay"),
        }
    }
}

#[derive(Debug, Default)]
struct Candidate {
    last_ping: Option<Instant>,
    last_pong: Option<Instant>,
    rtt: Option<Duration>,
    /// From the peer's own endpoint list (kept across netmaps).
    advertised: bool,
}

#[derive(Debug, Default)]
struct PathState {
    candidates: HashMap<SocketAddr, Candidate>,
    pending: HashMap<disco::TxId, (SocketAddr, Instant)>,
    best: Option<(SocketAddr, Duration, Instant)>,
    last_round: Option<Instant>,
    last_cmm: Option<Instant>,
    switches: u64,
}

impl PathState {
    fn current(&self, now: Instant) -> Path {
        match self.best {
            Some((addr, _, at)) if now.duration_since(at) < TRUST => Path::Direct(addr),
            _ => Path::Relay,
        }
    }
}

/// Per-peer counters.
#[derive(Debug, Default)]
pub struct PeerCounters {
    /// Encrypted bytes sent directly.
    pub tx_direct: AtomicU64,
    /// Encrypted bytes sent through the relay.
    pub tx_relay: AtomicU64,
    /// Encrypted bytes received directly.
    pub rx_direct: AtomicU64,
    /// Encrypted bytes received through the relay.
    pub rx_relay: AtomicU64,
}

pub(crate) struct Peer {
    pub(crate) info: RwLock<PeerInfo>,
    index: u32,
    tunn: Mutex<Tunn>,
    sealer: Sealer,
    path: Mutex<PathState>,
    last_activity: Mutex<Instant>,
    pub(crate) counters: PeerCounters,
}

impl Peer {
    pub(crate) fn key(&self) -> PublicKey {
        self.info.read().expect("info").node_key
    }

    pub(crate) fn path(&self) -> Path {
        self.path.lock().expect("path").current(Instant::now())
    }

    pub(crate) fn rtt(&self) -> Option<Duration> {
        let p = self.path.lock().expect("path");
        match p.current(Instant::now()) {
            Path::Direct(_) => p.best.map(|b| b.1),
            Path::Relay => None,
        }
    }

    pub(crate) fn path_switches(&self) -> u64 {
        self.path.lock().expect("path").switches
    }

    pub(crate) fn handshake_age(&self) -> Option<Duration> {
        self.tunn.lock().expect("tunn").time_since_last_handshake()
    }

    fn touch(&self) {
        *self.last_activity.lock().expect("activity") = Instant::now();
    }

    fn active(&self, now: Instant) -> bool {
        now.duration_since(*self.last_activity.lock().expect("activity")) < ACTIVE
    }
}

#[derive(Default)]
struct PeerTable {
    by_key: HashMap<PublicKey, Arc<Peer>>,
    by_addr: HashMap<Ipv6Addr, Arc<Peer>>,
    by_disco: HashMap<PublicKey, Arc<Peer>>,
    by_index: HashMap<u32, Arc<Peer>>,
}

struct Sockets {
    v4: Arc<UdpSocket>,
    v6: Option<Arc<UdpSocket>>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

/// Outgoing relay packets: `(destination node key, packet)`.
pub(crate) type RelayTx = mpsc::Sender<(PublicKey, Vec<u8>)>;

/// The data plane.
pub(crate) struct Engine {
    identity: NodeIdentity,
    me: PublicKey,
    pub(crate) vnet: Arc<VNet>,
    peers: RwLock<PeerTable>,
    sockets: RwLock<Option<Sockets>>,
    listen_port: u16,
    relay_tx: Mutex<Option<RelayTx>>,
    next_index: AtomicU32,
    stun_waiters: Mutex<HashMap<stun::TxId, oneshot::Sender<SocketAddr>>>,
    local_endpoints: RwLock<Vec<SocketAddr>>,
    disco_kick: Notify,
    /// Never use direct paths (tests and `CUA_MESH_RELAY_ONLY`).
    relay_only: AtomicBool,
    peers_changed: tokio::sync::watch::Sender<u64>,
}

impl Engine {
    pub(crate) async fn new(
        identity: NodeIdentity,
        listen_port: u16,
        relay_only: bool,
    ) -> io::Result<Arc<Self>> {
        let me = identity.public();
        let vnet = VNet::new(identity.addr());
        let (peers_changed, _) = tokio::sync::watch::channel(0);
        let engine = Arc::new(Self {
            identity,
            me,
            vnet,
            peers: RwLock::new(PeerTable::default()),
            sockets: RwLock::new(None),
            listen_port,
            relay_tx: Mutex::new(None),
            next_index: AtomicU32::new(rand::random::<u32>() & 0x00ff_ffff),
            stun_waiters: Mutex::new(HashMap::new()),
            local_endpoints: RwLock::new(Vec::new()),
            disco_kick: Notify::new(),
            relay_only: AtomicBool::new(relay_only),
            peers_changed,
        });
        let out: Arc<dyn Outbound> = engine.clone();
        engine.vnet.set_outbound(Arc::downgrade(&out));
        // `out` is the engine itself; the Weak keeps working as long as the
        // engine lives.
        drop(out);
        engine.bind(listen_port).await?;
        let weak = Arc::downgrade(&engine);
        tokio::spawn(async move { maintenance(weak).await });
        Ok(engine)
    }

    /// (Re)binds the UDP sockets: `port` 0 picks a fresh one.
    pub(crate) async fn bind(self: &Arc<Self>, port: u16) -> io::Result<()> {
        let v4 = bind_udp(SocketAddr::from(([0, 0, 0, 0], port)))?;
        let v6 = bind_udp(SocketAddr::from((
            Ipv6Addr::UNSPECIFIED,
            v4.local_addr()?.port(),
        )))
        .or_else(|_| bind_udp(SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0))))
        .ok();
        let mut tasks = Vec::new();
        for sock in std::iter::once(v4.clone()).chain(v6.clone()) {
            let weak = Arc::downgrade(self);
            tasks.push(tokio::spawn(async move { recv_loop(weak, sock).await }));
        }
        let old = self
            .sockets
            .write()
            .expect("sockets")
            .replace(Sockets { v4, v6, tasks });
        if let Some(old) = old {
            for t in old.tasks {
                t.abort();
            }
        }
        Ok(())
    }

    /// Moves to fresh UDP ports (a network change: new NAT mappings) and
    /// forgets every direct path, so traffic stays on the relay until
    /// discovery finds the new ones.
    pub(crate) async fn rebind(self: &Arc<Self>) -> io::Result<()> {
        self.bind(0).await?;
        self.forget_paths();
        Ok(())
    }

    /// Forgets every direct path (the network changed under us).
    pub(crate) fn forget_paths(&self) {
        for p in self.peers.read().expect("peers").by_key.values() {
            let mut path = p.path.lock().expect("path");
            path.best = None;
            path.last_round = None;
            path.last_cmm = None;
            path.pending.clear();
        }
        self.disco_kick.notify_one();
    }

    pub(crate) fn local_ports(&self) -> (u16, Option<u16>) {
        let s = self.sockets.read().expect("sockets");
        let s = s.as_ref().expect("bound");
        (
            s.v4.local_addr()
                .map(|a| a.port())
                .unwrap_or(self.listen_port),
            s.v6.as_ref()
                .and_then(|v| v.local_addr().ok())
                .map(|a| a.port()),
        )
    }

    pub(crate) fn set_relay(&self, tx: Option<RelayTx>) {
        *self.relay_tx.lock().expect("relay") = tx;
    }

    pub(crate) fn set_relay_only(&self, on: bool) {
        self.relay_only.store(on, Ordering::SeqCst);
        if on {
            self.forget_paths();
        }
    }

    pub(crate) fn set_local_endpoints(&self, endpoints: Vec<SocketAddr>) {
        let changed = {
            let mut eps = self.local_endpoints.write().expect("endpoints");
            let changed = *eps != endpoints;
            *eps = endpoints;
            changed
        };
        if changed {
            // Peers still on the relay hear about the new endpoints now,
            // not at the next round.
            for p in self.peers.read().expect("peers").by_key.values() {
                let mut ps = p.path.lock().expect("path");
                if ps.current(Instant::now()) == Path::Relay {
                    ps.last_round = None;
                    ps.last_cmm = None;
                }
            }
            self.disco_kick.notify_one();
        }
    }

    pub(crate) fn subscribe_peers(&self) -> tokio::sync::watch::Receiver<u64> {
        self.peers_changed.subscribe()
    }

    /// Replaces the peer set. Peers whose keys are unchanged keep their
    /// WireGuard sessions and paths.
    pub(crate) fn set_peers(&self, infos: Vec<PeerInfo>) {
        let mut table = self.peers.write().expect("peers");
        let mut next = PeerTable::default();
        for info in infos {
            if info.node_key == self.me || info.addr() == self.vnet_addr() {
                continue;
            }
            let peer = match table.by_key.get(&info.node_key) {
                Some(existing)
                    if existing.info.read().expect("info").disco_key == info.disco_key =>
                {
                    {
                        let mut path = existing.path.lock().expect("path");
                        // Keep learned candidates; refresh advertised ones.
                        path.candidates
                            .retain(|_, c| !c.advertised || c.last_pong.is_some());
                        for e in &info.endpoints {
                            path.candidates.entry(*e).or_default().advertised = true;
                        }
                    }
                    *existing.info.write().expect("info") = info.clone();
                    existing.clone()
                }
                _ => Arc::new(self.new_peer(info.clone())),
            };
            next.by_addr.insert(info.addr(), peer.clone());
            next.by_disco.insert(info.disco_key, peer.clone());
            next.by_index.insert(peer.index, peer.clone());
            next.by_key.insert(info.node_key, peer);
        }
        *table = next;
        drop(table);
        self.peers_changed.send_modify(|v| *v += 1);
        self.disco_kick.notify_one();
    }

    fn vnet_addr(&self) -> Ipv6Addr {
        self.identity.addr()
    }

    fn new_peer(&self, info: PeerInfo) -> Peer {
        let index = self.next_index.fetch_add(1, Ordering::Relaxed) & 0x00ff_ffff;
        let tunn = Tunn::new(
            self.identity.node.x25519(),
            info.node_key.x25519(),
            None,
            None,
            index,
            None,
        );
        let sealer = Sealer::new(&self.identity.disco, &info.disco_key);
        let mut path = PathState::default();
        for e in &info.endpoints {
            path.candidates.entry(*e).or_default().advertised = true;
        }
        Peer {
            info: RwLock::new(info),
            index,
            tunn: Mutex::new(tunn),
            sealer,
            path: Mutex::new(path),
            last_activity: Mutex::new(Instant::now() - ACTIVE),
            counters: PeerCounters::default(),
        }
    }

    /// Drops one peer now (a revoked pairing), before the next map.
    pub(crate) fn remove_peer(&self, key: &PublicKey) {
        let mut table = self.peers.write().expect("peers");
        if let Some(p) = table.by_key.remove(key) {
            let info = p.info.read().expect("info").clone();
            table.by_addr.remove(&info.addr());
            table.by_disco.remove(&info.disco_key);
            table.by_index.remove(&p.index);
        }
        drop(table);
        self.peers_changed.send_modify(|v| *v += 1);
    }

    pub(crate) fn peer(&self, key: &PublicKey) -> Option<Arc<Peer>> {
        self.peers.read().expect("peers").by_key.get(key).cloned()
    }

    pub(crate) fn peer_by_addr(&self, addr: &Ipv6Addr) -> Option<Arc<Peer>> {
        self.peers.read().expect("peers").by_addr.get(addr).cloned()
    }

    pub(crate) fn peers(&self) -> Vec<Arc<Peer>> {
        self.peers
            .read()
            .expect("peers")
            .by_key
            .values()
            .cloned()
            .collect()
    }

    /// Marks `key` as about to be used, so discovery starts now.
    pub(crate) fn want(&self, key: &PublicKey) {
        if let Some(p) = self.peer(key) {
            p.touch();
            self.disco_kick.notify_one();
        }
    }

    // ---- sending ----

    fn send_udp(&self, dst: SocketAddr, data: &[u8]) -> io::Result<()> {
        let s = self.sockets.read().expect("sockets");
        let s = s
            .as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "unbound"))?;
        let sock = match dst {
            SocketAddr::V4(_) => &s.v4,
            SocketAddr::V6(_) => {
                s.v6.as_ref()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported, "no IPv6 socket"))?
            }
        };
        sock.try_send_to(data, dst).map(|_| ())
    }

    fn send_relay(&self, dst: PublicKey, data: &[u8]) -> io::Result<()> {
        let tx = self.relay_tx.lock().expect("relay").clone();
        match tx {
            Some(tx) => tx
                .try_send((dst, data.to_vec()))
                .map_err(|_| io::Error::new(io::ErrorKind::WouldBlock, "relay queue full")),
            None => Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "relay unavailable",
            )),
        }
    }

    fn send_via(&self, peer: &Peer, path: Path, data: &[u8]) -> io::Result<()> {
        match path {
            Path::Direct(addr) => {
                peer.counters
                    .tx_direct
                    .fetch_add(data.len() as u64, Ordering::Relaxed);
                self.send_udp(addr, data)
            }
            Path::Relay => {
                peer.counters
                    .tx_relay
                    .fetch_add(data.len() as u64, Ordering::Relaxed);
                self.send_relay(peer.key(), data)
            }
        }
    }

    fn send_to_peer(&self, peer: &Peer, data: &[u8]) -> io::Result<()> {
        let path = if self.relay_only.load(Ordering::Relaxed) {
            Path::Relay
        } else {
            peer.path()
        };
        let r = self.send_via(peer, path, data);
        if matches!(&r, Err(e) if e.kind() != io::ErrorKind::WouldBlock) && path != Path::Relay {
            // A direct send failing outright (no route after a network
            // change): fall back to the relay for this packet.
            return self.send_via(peer, Path::Relay, data);
        }
        r
    }

    fn drain(&self, peer: &Peer, path: Path, tunn: &mut Tunn, buf: &mut [u8]) {
        while let TunnResult::WriteToNetwork(p) = tunn.decapsulate(None, &[], buf) {
            let _ = self.send_via(peer, path, p);
        }
    }

    // ---- receiving ----

    fn on_datagram(&self, path: Path, relay_src: Option<PublicKey>, data: &[u8]) {
        if matches!(path, Path::Direct(_)) && stun::is_stun(data) {
            if let Some((tx, mapped)) = stun::parse_response(data)
                && let Some(w) = self.stun_waiters.lock().expect("stun").remove(&tx)
            {
                let _ = w.send(mapped);
            }
            return;
        }
        if disco::is_disco(data) {
            self.on_disco(path, relay_src, data);
            return;
        }
        self.on_wireguard(path, relay_src, data);
    }

    fn on_wireguard(&self, path: Path, relay_src: Option<PublicKey>, data: &[u8]) {
        let peer = {
            let table = self.peers.read().expect("peers");
            match Tunn::parse_incoming_packet(data) {
                Ok(Packet::HandshakeInit(ref init)) => {
                    let me = self.identity.node.public();
                    match parse_handshake_anon(&self.identity.node.x25519(), &me.x25519(), init) {
                        Ok(half) => table
                            .by_key
                            .get(&PublicKey(half.peer_static_public))
                            .cloned(),
                        Err(_) => None,
                    }
                }
                Ok(Packet::HandshakeResponse(r)) => {
                    table.by_index.get(&(r.receiver_idx >> 8)).cloned()
                }
                Ok(Packet::PacketCookieReply(c)) => {
                    table.by_index.get(&(c.receiver_idx >> 8)).cloned()
                }
                Ok(Packet::PacketData(d)) => table.by_index.get(&(d.receiver_idx >> 8)).cloned(),
                Err(_) => None,
            }
        };
        let Some(peer) = peer else {
            return;
        };
        if let Some(src) = relay_src
            && src != peer.key()
        {
            // A relayed packet must come from the peer it names.
            return;
        }
        match path {
            Path::Direct(_) => peer
                .counters
                .rx_direct
                .fetch_add(data.len() as u64, Ordering::Relaxed),
            Path::Relay => peer
                .counters
                .rx_relay
                .fetch_add(data.len() as u64, Ordering::Relaxed),
        };
        let src_ip = match path {
            Path::Direct(a) => Some(a.ip()),
            Path::Relay => None,
        };
        // Decryption never grows a packet; queued packets flushed after a
        // handshake are at most one tunnel MTU plus overhead.
        let mut buf = vec![0u8; (data.len() + 64).max(2048)];
        let mut tunn = peer.tunn.lock().expect("tunn");
        match tunn.decapsulate(src_ip, data, &mut buf) {
            TunnResult::WriteToNetwork(p) => {
                let _ = self.send_via(&peer, path, p);
                self.drain(&peer, path, &mut tunn, &mut buf);
            }
            TunnResult::WriteToTunnelV6(p, src) => {
                // Cryptokey routing: the inner source must be the peer's
                // own address.
                if src == peer.info.read().expect("info").addr() {
                    peer.touch();
                    self.vnet.deliver(p);
                }
            }
            TunnResult::WriteToTunnelV4(..) | TunnResult::Done | TunnResult::Err(_) => {}
        }
        drop(tunn);
        if let Path::Direct(addr) = path {
            // Authenticated traffic from an endpoint: worth probing.
            let mut ps = peer.path.lock().expect("path");
            if ps.best.is_none() && !ps.candidates.contains_key(&addr) {
                ps.candidates.insert(addr, Candidate::default());
                drop(ps);
                self.disco_kick.notify_one();
            }
        }
    }

    fn on_disco(&self, path: Path, relay_src: Option<PublicKey>, data: &[u8]) {
        let Some(sender) = disco::sender(data) else {
            return;
        };
        let Some(peer) = self
            .peers
            .read()
            .expect("peers")
            .by_disco
            .get(&sender)
            .cloned()
        else {
            return;
        };
        if let Some(src) = relay_src
            && src != peer.key()
        {
            return;
        }
        let Some(msg) = peer.sealer.open(data) else {
            return;
        };
        let now = Instant::now();
        match msg {
            Message::Ping { tx, node } => {
                if node != peer.key() {
                    return;
                }
                let observed = match path {
                    Path::Direct(a) => a,
                    Path::Relay => SocketAddr::from(([0, 0, 0, 0], 0)),
                };
                let pong = peer.sealer.seal(&Message::Pong { tx, src: observed });
                let _ = match path {
                    Path::Direct(a) => self.send_udp(a, &pong),
                    Path::Relay => self.send_relay(peer.key(), &pong),
                };
                if let Path::Direct(a) = path
                    && !self.relay_only.load(Ordering::Relaxed)
                {
                    let mut ps = peer.path.lock().expect("path");
                    let fresh = !ps.candidates.contains_key(&a);
                    ps.candidates.entry(a).or_default();
                    let unconfirmed = ps.candidates.get(&a).is_none_or(|c| c.last_pong.is_none());
                    drop(ps);
                    if fresh || unconfirmed {
                        // Ping back right away: a ping that got through
                        // means our ping back likely gets through too.
                        self.ping(&peer, a);
                        // The peer may have moved: check the current path
                        // too, so a silent one is noticed within a round
                        // trip or two (see `reselect`).
                        let best = peer.path.lock().expect("path").best.map(|b| b.0);
                        if let Some(best) = best
                            && best != a
                        {
                            self.ping(&peer, best);
                        }
                    }
                }
            }
            Message::Pong { tx, src } => {
                let mut ps = peer.path.lock().expect("path");
                let Some((addr, sent)) = ps.pending.remove(&tx) else {
                    return;
                };
                if path != Path::Direct(addr) {
                    return;
                }
                let rtt = now.duration_since(sent);
                let c = ps.candidates.entry(addr).or_default();
                c.last_pong = Some(now);
                c.rtt = Some(rtt);
                let switch = match ps.best {
                    None => true,
                    Some((b, _, at)) if b == addr => {
                        ps.best = Some((addr, rtt, now));
                        let _ = at;
                        false
                    }
                    Some((best, brtt, at)) => {
                        // The current path leaves a ping unanswered well past
                        // its round trip (the peer moved: a new NAT mapping
                        // or network), it missed its last ping round, or
                        // this one is clearly faster.
                        let overdue = brtt * 2 + Duration::from_millis(100);
                        let best_silent = ps.pending.values().any(|(a, t)| {
                            *a == best && *t > at && now.duration_since(*t) > overdue
                        });
                        best_silent
                            || now.duration_since(at) > PING_INTERVAL + Duration::from_secs(1)
                            || rtt.as_secs_f64() < brtt.as_secs_f64() * 0.8
                    }
                };
                if switch {
                    ps.best = Some((addr, rtt, now));
                    ps.switches += 1;
                    tracing::debug!(peer = %peer.key().short(), %addr, ?rtt, "direct path");
                }
                drop(ps);
                if !src.ip().is_unspecified() {
                    let mut eps = self.local_endpoints.write().expect("endpoints");
                    if !eps.contains(&src) && eps.len() < 32 {
                        // Peer-reflexive: how the peer sees us.
                        eps.push(src);
                    }
                }
            }
            Message::CallMeMaybe { endpoints } => {
                if self.relay_only.load(Ordering::Relaxed) {
                    return;
                }
                // Ping what it sent and what the map says, all at once: the
                // peer is pinging us right now, so both NATs open.
                let targets: Vec<SocketAddr> = {
                    let mut ps = peer.path.lock().expect("path");
                    for e in &endpoints {
                        ps.candidates.entry(*e).or_default();
                    }
                    ps.candidates.keys().copied().collect()
                };
                peer.touch();
                for e in targets.into_iter().take(32) {
                    if e.port() != 0 && !e.ip().is_unspecified() {
                        self.ping(&peer, e);
                    }
                }
            }
        }
    }

    fn ping(&self, peer: &Peer, addr: SocketAddr) {
        let tx: disco::TxId = rand::random();
        {
            let mut ps = peer.path.lock().expect("path");
            let now = Instant::now();
            ps.pending
                .retain(|_, (_, at)| now.duration_since(*at) < Duration::from_secs(10));
            ps.pending.insert(tx, (addr, now));
            ps.candidates.entry(addr).or_default().last_ping = Some(now);
        }
        let msg = peer.sealer.seal(&Message::Ping { tx, node: self.me });
        let _ = self.send_udp(addr, &msg);
    }

    /// One discovery round for every active peer.
    fn discover(&self) {
        if self.relay_only.load(Ordering::Relaxed) {
            return;
        }
        let now = Instant::now();
        let endpoints = self.local_endpoints.read().expect("endpoints").clone();
        for peer in self.peers() {
            if !peer.active(now) {
                continue;
            }
            let (targets, send_cmm) = {
                let mut ps = peer.path.lock().expect("path");
                let fresh_best =
                    matches!(ps.best, Some((_, _, at)) if now.duration_since(at) < PING_INTERVAL);
                let due = ps
                    .last_round
                    .is_none_or(|t| now.duration_since(t) >= PING_INTERVAL);
                if fresh_best || !due {
                    continue;
                }
                ps.last_round = Some(now);
                let trusted = matches!(ps.current(now), Path::Direct(_));
                let targets: Vec<SocketAddr> = match (trusted, ps.best) {
                    // Keep the working path alive; re-probe the others
                    // less often.
                    (true, Some((b, _, _))) => std::iter::once(b)
                        .chain(ps.candidates.iter().filter_map(|(a, c)| {
                            (*a != b
                                && c.last_ping.is_none_or(|t| {
                                    now.duration_since(t) > Duration::from_secs(20)
                                }))
                            .then_some(*a)
                        }))
                        .collect(),
                    _ => ps.candidates.keys().copied().collect(),
                };
                let send_cmm = !trusted
                    && !endpoints.is_empty()
                    && ps
                        .last_cmm
                        .is_none_or(|t| now.duration_since(t) >= CMM_INTERVAL);
                if send_cmm {
                    ps.last_cmm = Some(now);
                }
                (targets, send_cmm)
            };
            if send_cmm {
                let cmm = peer.sealer.seal(&Message::CallMeMaybe {
                    endpoints: endpoints.clone(),
                });
                let _ = self.send_relay(peer.key(), &cmm);
            }
            for t in targets.into_iter().take(16) {
                if t.port() == 0 || t.ip().is_unspecified() {
                    continue;
                }
                self.ping(&peer, t);
            }
        }
    }

    /// Leaves a direct path that stopped answering for the candidate that
    /// answered most recently, without waiting for [`TRUST`] to run out.
    fn reselect(&self) {
        let now = Instant::now();
        for peer in self.peers() {
            let mut ps = peer.path.lock().expect("path");
            let Some((best, brtt, at)) = ps.best else {
                continue;
            };
            let overdue = brtt * 2 + Duration::from_millis(150);
            let silent = ps
                .pending
                .values()
                .any(|(a, t)| *a == best && *t > at && now.duration_since(*t) > overdue);
            if !silent {
                continue;
            }
            let next = ps
                .candidates
                .iter()
                .filter(|(a, c)| **a != best && c.last_pong.is_some_and(|p| p > at))
                .max_by_key(|(_, c)| c.last_pong)
                .map(|(a, c)| (*a, c.rtt.unwrap_or(brtt), c.last_pong.unwrap_or(now)));
            if let Some(next) = next {
                tracing::debug!(peer = %peer.key().short(), from = %best, to = %next.0, "direct path moved");
                ps.best = Some(next);
                ps.switches += 1;
            }
        }
    }

    fn timers(&self) {
        let mut buf = vec![0u8; 256];
        for peer in self.peers() {
            let mut tunn = peer.tunn.lock().expect("tunn");
            if let TunnResult::WriteToNetwork(p) = tunn.update_timers(&mut buf) {
                let p = p.to_vec();
                drop(tunn);
                let _ = self.send_to_peer(&peer, &p);
            }
        }
    }

    /// A packet the relay delivered from `src`.
    pub(crate) fn on_relay(&self, src: PublicKey, data: &[u8]) {
        self.on_datagram(Path::Relay, Some(src), data);
    }

    /// Asks a STUN server what our v4 socket maps to.
    pub(crate) async fn stun(&self, server: SocketAddr, timeout: Duration) -> Option<SocketAddr> {
        let tx: stun::TxId = rand::random();
        let (otx, orx) = oneshot::channel();
        self.stun_waiters.lock().expect("stun").insert(tx, otx);
        let sent = self.send_udp(server, &stun::request(tx)).is_ok();
        let got = if sent {
            tokio::time::timeout(timeout, orx)
                .await
                .ok()
                .and_then(Result::ok)
        } else {
            None
        };
        self.stun_waiters.lock().expect("stun").remove(&tx);
        got
    }

    pub(crate) fn shutdown(&self) {
        if let Some(s) = self.sockets.write().expect("sockets").take() {
            for t in s.tasks {
                t.abort();
            }
        }
        self.set_relay(None);
    }
}

impl Outbound for Engine {
    /// `Ok` unless the direct path's socket is full (`WouldBlock`, which
    /// QUIC waits out). A datagram that cannot go out otherwise (no path,
    /// the relay reconnecting or its queue full) is dropped like any lost
    /// UDP datagram and QUIC retransmits it: any other error would end the
    /// QUIC connection, and a session must outlive a relay or network blip.
    fn send_ip(&self, dst: Ipv6Addr, packet: &[u8]) -> io::Result<()> {
        let Some(peer) = self.peer_by_addr(&dst) else {
            tracing::trace!(%dst, "no mesh peer; dropping");
            return Ok(());
        };
        peer.touch();
        let mut buf = vec![0u8; packet.len() + 160];
        let out = {
            let mut tunn = peer.tunn.lock().expect("tunn");
            match tunn.encapsulate(packet, &mut buf) {
                TunnResult::WriteToNetwork(p) => Some(p.len()),
                _ => None,
            }
        };
        if let Some(n) = out
            && let Err(e) = self.send_to_peer(&peer, &buf[..n])
        {
            if e.kind() == io::ErrorKind::WouldBlock && matches!(peer.path(), Path::Direct(_)) {
                return Err(e);
            }
            tracing::trace!(error = %e, "mesh send dropped");
        }
        Ok(())
    }

    fn poll_send_ready(&self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let s = self.sockets.read().expect("sockets");
        match s.as_ref() {
            Some(s) => s.v4.poll_send_ready(cx),
            // Shut down: sends are dropped (see `send_ip`), never an error
            // QUIC would report.
            None => Poll::Ready(Ok(())),
        }
    }
}

/// Socket buffers: room for a window's worth of datagrams at speed, so
/// bursts are not dropped by the kernel before the receive loop runs.
const SOCKET_BUFFER: usize = 8 << 20;

fn bind_udp(addr: SocketAddr) -> io::Result<Arc<UdpSocket>> {
    use socket2::{Domain, Protocol, Socket, Type};
    let domain = if addr.is_ipv4() {
        Domain::IPV4
    } else {
        Domain::IPV6
    };
    let s = Socket::new(domain, Type::DGRAM, Some(Protocol::UDP))?;
    if addr.is_ipv6() {
        // Native IPv6 only; IPv4 has its own socket on the same port.
        s.set_only_v6(true)?;
    }
    // Best effort: the kernel caps these (net.core.*mem_max on Linux).
    let _ = s.set_recv_buffer_size(SOCKET_BUFFER);
    let _ = s.set_send_buffer_size(SOCKET_BUFFER);
    s.set_nonblocking(true)?;
    s.bind(&addr.into())?;
    Ok(Arc::new(UdpSocket::from_std(s.into())?))
}

async fn recv_loop(engine: std::sync::Weak<Engine>, sock: Arc<UdpSocket>) {
    let mut buf = vec![0u8; MAX_DATAGRAM];
    loop {
        let (n, src) = match sock.recv_from(&mut buf).await {
            Ok(v) => v,
            Err(e) => {
                // ICMP-induced errors (connection refused/reset) are
                // per-datagram on some platforms; keep going.
                tracing::trace!(error = %e, "udp recv");
                tokio::time::sleep(Duration::from_millis(5)).await;
                continue;
            }
        };
        let Some(engine) = engine.upgrade() else {
            return;
        };
        let src = match src {
            SocketAddr::V6(v6) => match v6.ip().to_ipv4_mapped() {
                Some(v4) => SocketAddr::new(IpAddr::V4(v4), v6.port()),
                None => src,
            },
            v4 => v4,
        };
        engine.on_datagram(Path::Direct(src), None, &buf[..n]);
    }
}

async fn maintenance(engine: std::sync::Weak<Engine>) {
    let mut timers = tokio::time::interval(Duration::from_millis(250));
    timers.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        let kick = {
            let Some(e) = engine.upgrade() else { return };
            async move { e.disco_kick.notified().await }
        };
        tokio::select! {
            _ = timers.tick() => {}
            _ = kick => {}
        }
        let Some(e) = engine.upgrade() else { return };
        if e.sockets.read().expect("sockets").is_none() {
            return;
        }
        e.timers();
        e.reselect();
        e.discover();
    }
}
