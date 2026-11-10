// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Public TCP shares: a raw TCP service in a Space (a game server, for
//! example a Minecraft Java server on 25565), reachable from anywhere at
//! `<relay tcp host>:<port>`, and through an SRV record at a name such as
//! `mc-1a2b3c4d.m.cua.sh`.
//!
//! Game protocols are plain TCP without TLS or SNI, so the host name cannot
//! route them: every share gets a port of its own from a fixed pool
//! (`CUA_RELAY_TCP_PORTS`, e.g. `26000-26063`).
//!
//! # Path of one connection
//!
//! 1. A client connects to an allocated port. In front of the pod, Traefik
//!    (a TCP entrypoint per port) prepends a PROXY protocol v2 header with
//!    the client's address when `CUA_RELAY_TCP_PROXY_PROTOCOL=on`.
//! 2. The relay checks the share's lease, the per-address connection rate
//!    and the share's connection cap, then opens a WebSocket *through its
//!    own router* (an in-process HTTP connection) to the share's public
//!    path on the computer: `/share/pub/<token>/`. Everything a public link
//!    goes through applies unchanged: the computer must be an account
//!    machine that is sharing, the public-share rate limits, metering, a
//!    "stop sharing" cut.
//! 3. The computer's spacesd hands the WebSocket to the cua daemon's share
//!    gateway, which checks the token and splices it to the one Space port
//!    the share names (never the host, the LAN or another Space).
//! 4. Bytes flow as binary WebSocket messages, under the share's bandwidth
//!    cap.
//!
//! # Lifetime
//!
//! An allocation has a hard end (at most 24 h) and a lease the computer's
//! daemon renews (`POST …/keepalive`). A daemon that stops renewing (it
//! crashed, its machine is gone or cut off) loses the port when the lease
//! runs out. The reconcile sweep (at start and every
//! [`RECONCILE_INTERVAL`]) closes ports whose lease or end passed, whose
//! machine left the directory or stopped sharing, restarts listeners after
//! a relay restart, and makes the zone's tagged SRV records match the live
//! allocations exactly (creating missing ones, deleting any other record
//! with this relay's tag).
//!
//! # API (account mode)
//!
//! - `POST /v1/machines/{id}/tcp-shares`: allocate (owner, or the computer
//!   itself).
//! - `GET /v1/machines/{id}/tcp-shares`: the computer's allocations.
//! - `POST /v1/machines/{id}/tcp-shares/{port}/keepalive`: renew the lease.
//! - `DELETE /v1/machines/{id}/tcp-shares/{port}`: close it.
//!
//! Every open, close and expiry is an audit row in the owner's account log.

use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use futures_util::{SinkExt as _, StreamExt as _};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio_tungstenite::tungstenite::Message;

use crate::api::{error, Caller};
use crate::devices::AuditEvent;
use crate::server::{Registrant, Relay};

/// How often the reconcile sweep runs.
pub const RECONCILE_INTERVAL: Duration = Duration::from_secs(30);

/// Target ports a TCP share may never expose: remote shells and desktops,
/// game-server consoles (Minecraft RCON), container and cluster control
/// planes, and databases. The daemon refuses them too; this is the
/// relay's own copy of the rule.
pub const REFUSED_TARGET_PORTS: &[u16] = &[
    22, 23, 2375, 2376, 3306, 3389, 5432, 5900, 5901, 6379, 6443, 10250, 25575, 27017,
];

/// The share token's length (hex), as for public links.
const TOKEN_LEN: usize = 40;

/// Tag every DNS record this relay makes carries (with its SRV domain), so
/// the sweep only ever touches its own records.
const DNS_TAG: &str = "cua-relay tcp-share";

/// Configuration of public TCP shares. `None` in [`crate::server::RelayConfig`]
/// turns the feature off (the API answers 404).
#[derive(Debug, Clone)]
pub struct TcpShareConfig {
    /// First and last port of the pool (inclusive).
    pub ports: (u16, u16),
    /// Address the listeners bind (inside the pod).
    pub listen: IpAddr,
    /// The host clients connect to (`tcp.m.cua.sh`): the address shown to
    /// people and the SRV records' target.
    pub public_host: String,
    /// Expect a PROXY protocol v2 header first on every connection (from
    /// the TCP proxy in front), and take the client address from it.
    pub proxy_protocol: bool,
    /// Ports one account may hold at once.
    pub per_account: usize,
    /// Ports one computer may hold at once.
    pub per_machine: usize,
    /// New connections per second from one client address (all shares).
    pub conn_rps_per_ip: f64,
    /// Burst of new connections from one client address.
    pub conn_burst_per_ip: f64,
    /// Open connections one share carries at once.
    pub max_conns_per_share: u32,
    /// Bytes per second one share carries, both directions together.
    pub bytes_per_sec: u64,
    /// Longest allocation.
    pub max_ttl: Duration,
    /// How long an allocation lives without a keep-alive.
    pub lease: Duration,
    /// Domain the SRV names live under (`m.cua.sh`); `None`: no SRV records.
    pub srv_domain: Option<String>,
    /// Cloudflare zone and token for the SRV records.
    pub dns: Option<DnsConfig>,
    /// Edge mode (a share edge in the user's own cloud): the only machine
    /// is the user's cua daemon, joined with a static token
    /// (`CUA_RELAY_TOKENS` / `CUA_RELAY_TOKEN_FILE`), which also allocates
    /// ports with that token. No accounts, no directory.
    pub edge: bool,
}

impl Default for TcpShareConfig {
    fn default() -> Self {
        Self {
            ports: (26000, 26063),
            listen: IpAddr::from([0, 0, 0, 0]),
            public_host: String::new(),
            proxy_protocol: false,
            per_account: 2,
            per_machine: 4,
            conn_rps_per_ip: 2.0,
            conn_burst_per_ip: 10.0,
            max_conns_per_share: 64,
            bytes_per_sec: 4 << 20,
            max_ttl: Duration::from_secs(24 * 3600),
            lease: Duration::from_secs(10 * 60),
            srv_domain: None,
            dns: None,
            edge: false,
        }
    }
}

impl TcpShareConfig {
    /// Parses `26000-26063` (or one port).
    pub fn parse_ports(s: &str) -> Result<(u16, u16), String> {
        let s = s.trim();
        let (a, b) = s.split_once('-').unwrap_or((s, s));
        let a: u16 = a
            .trim()
            .parse()
            .map_err(|_| format!("{s:?}: not a port range"))?;
        let b: u16 = b
            .trim()
            .parse()
            .map_err(|_| format!("{s:?}: not a port range"))?;
        if a == 0 || b < a || b - a > 1023 {
            return Err(format!(
                "{s:?}: use first-last, at most 1024 ports, not including 0"
            ));
        }
        Ok((a, b))
    }
}

/// Cloudflare DNS access for SRV records (a token scoped to the zone).
#[derive(Clone)]
pub struct DnsConfig {
    /// Zone id.
    pub zone_id: String,
    /// API token (Zone Read, DNS Write on that zone only).
    pub token: String,
    /// API base (tests point it at a fake).
    pub api_base: String,
}

impl std::fmt::Debug for DnsConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DnsConfig")
            .field("zone_id", &self.zone_id)
            .field("token", &"<redacted>")
            .field("api_base", &self.api_base)
            .finish()
    }
}

/// One allocated port.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Allocation {
    /// The public port.
    pub port: u16,
    /// Machine whose daemon serves it.
    pub machine: String,
    /// Account that owns the computer.
    pub owner: String,
    /// The share's token (the credential of its public path).
    pub token: String,
    /// The port inside the Space (audit and the refused-port rule).
    pub target_port: u16,
    /// SRV service (`minecraft`), when the share asked for a name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    /// The name people type (`mc-1a2b3c4d.m.cua.sh`), when there is an SRV
    /// record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub srv_name: Option<String>,
    /// A name for lists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Unix seconds.
    pub created: u64,
    /// The lease: closed after this unless renewed.
    pub lease_until: u64,
    /// The hard end (at most the relay's longest allocation).
    pub ends: u64,
    /// Who may connect (CIDRs; empty: anyone).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow: Vec<String>,
    /// Drop connections whose Minecraft Java handshake does not name this
    /// share's address (`srv_name` or `host:port`'s host).
    #[serde(default)]
    pub check_host: bool,
    /// The most bytes the share may move (both ways); 0: no cap. Reaching
    /// it ends every connection and refuses new ones (the owner's daily
    /// data cap, which the daemon splits across its shares).
    #[serde(default)]
    pub max_bytes: u64,
}

/// The API's view of an allocation (no token).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AllocationView {
    /// Public port.
    pub port: u16,
    /// `host:port`.
    pub address: String,
    /// Public host.
    pub host: String,
    /// SRV name people type, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub srv_name: Option<String>,
    /// Port in the Space.
    pub target_port: u16,
    /// Name for lists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Unix seconds.
    pub created: u64,
    /// Unix seconds.
    pub lease_until: u64,
    /// Unix seconds.
    pub ends: u64,
    /// Open connections now.
    pub connections: u32,
    /// Who may connect (CIDRs; empty: anyone).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow: Vec<String>,
    /// Bytes moved since the port opened (this relay run).
    #[serde(default)]
    pub bytes: u64,
    /// The share's data cap (0: none).
    #[serde(default)]
    pub max_bytes: u64,
}

/// Body of `POST /v1/machines/{id}/tcp-shares`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllocateRequest {
    /// The share's token (40 hex), made by the daemon.
    pub token: String,
    /// Port in the Space.
    pub target_port: u16,
    /// Lifetime in seconds (default and maximum: the relay's longest).
    #[serde(default)]
    pub ttl_secs: Option<u64>,
    /// SRV service name (`minecraft`): also makes an SRV record.
    #[serde(default)]
    pub service: Option<String>,
    /// Name for lists.
    #[serde(default)]
    pub name: Option<String>,
    /// Who may connect (CIDRs, at most 32; empty: anyone).
    #[serde(default)]
    pub allow: Vec<String>,
    /// Check the Minecraft Java handshake's host (needs a `service` of
    /// `minecraft`).
    #[serde(default)]
    pub check_host: bool,
    /// The most bytes the share may move (0 or unset: no cap).
    #[serde(default)]
    pub max_bytes: Option<u64>,
}

#[derive(Default, Serialize, Deserialize)]
struct Persisted {
    allocations: Vec<Allocation>,
}

struct Bucket {
    tokens: f64,
    last: Instant,
}

/// A per-share byte budget shared by every connection of the share.
struct ByteBucket {
    rate: f64,
    tokens: Mutex<(f64, Instant)>,
    /// Bytes moved both ways since the port opened (the data cap).
    moved: std::sync::atomic::AtomicU64,
}

impl ByteBucket {
    fn new(rate: u64) -> Self {
        Self {
            rate: rate.max(1) as f64,
            tokens: Mutex::new((rate.max(1) as f64, Instant::now())),
            moved: std::sync::atomic::AtomicU64::new(0),
        }
    }

    fn moved(&self) -> u64 {
        self.moved.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Whether moving `n` more bytes would pass the data cap `max` (0: no
    /// cap), so a chunk that crosses it is not carried at all. When it
    /// would, the share counts as full: no new connection is carried and
    /// the daemon sees the cap reached.
    fn would_pass(&self, n: usize, max: u64) -> bool {
        let pass = max > 0 && self.moved().saturating_add(n as u64) > max;
        if pass {
            self.moved
                .fetch_max(max, std::sync::atomic::Ordering::Relaxed);
        }
        pass
    }

    /// Waits until `n` bytes may pass.
    async fn take(&self, n: usize) {
        self.moved
            .fetch_add(n as u64, std::sync::atomic::Ordering::Relaxed);
        let n = n as f64;
        loop {
            let wait = {
                let mut g = self.tokens.lock().expect("bytes");
                let now = Instant::now();
                let refill = now.duration_since(g.1).as_secs_f64() * self.rate;
                g.0 = (g.0 + refill).min(self.rate);
                g.1 = now;
                if g.0 >= n.min(self.rate) {
                    g.0 -= n;
                    return;
                }
                Duration::from_secs_f64(((n.min(self.rate) - g.0) / self.rate).max(0.001))
            };
            tokio::time::sleep(wait).await;
        }
    }
}

struct Live {
    task: tokio::task::AbortHandle,
    connections: Arc<std::sync::atomic::AtomicU32>,
    bytes: Arc<ByteBucket>,
    /// Cancelled when the allocation closes: ends every connection.
    cut: tokio_util::sync::CancellationToken,
}

/// The relay's TCP shares.
pub struct TcpShares {
    config: TcpShareConfig,
    file: Option<PathBuf>,
    allocations: Mutex<BTreeMap<u16, Allocation>>,
    live: Mutex<HashMap<u16, Live>>,
    per_ip: Mutex<HashMap<IpAddr, Bucket>>,
    /// DNS record ids by SRV name, as last listed or created.
    dns_ids: Mutex<HashMap<String, String>>,
    http: reqwest::Client,
}

fn now_secs() -> u64 {
    crate::assertion::now_secs()
}

impl TcpShares {
    /// Opens the allocations file (`<state>.tcp.json`), when there is one.
    pub fn open(config: TcpShareConfig, file: Option<PathBuf>) -> std::io::Result<Self> {
        let mut allocations = BTreeMap::new();
        if let Some(path) = &file {
            match std::fs::read(path) {
                Ok(bytes) => {
                    let p: Persisted = serde_json::from_slice(&bytes).map_err(|e| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!("{}: {e}", path.display()),
                        )
                    })?;
                    for a in p.allocations {
                        allocations.insert(a.port, a);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        Ok(Self {
            config,
            file,
            allocations: Mutex::new(allocations),
            live: Mutex::default(),
            per_ip: Mutex::default(),
            dns_ids: Mutex::default(),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap_or_default(),
        })
    }

    /// The configuration.
    pub fn config(&self) -> &TcpShareConfig {
        &self.config
    }

    fn save(&self, allocations: &BTreeMap<u16, Allocation>) {
        let Some(path) = &self.file else { return };
        let body = Persisted {
            allocations: allocations.values().cloned().collect(),
        };
        let Ok(bytes) = serde_json::to_vec_pretty(&body) else {
            return;
        };
        let tmp = path.with_extension("tcp.json.tmp");
        let result = std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, path));
        if let Err(e) = result {
            tracing::warn!(error = %e, "tcp shares: could not save allocations");
        }
    }

    /// Current allocations.
    pub fn list(&self) -> Vec<Allocation> {
        self.allocations
            .lock()
            .expect("allocations")
            .values()
            .cloned()
            .collect()
    }

    fn view(&self, a: &Allocation) -> AllocationView {
        let (connections, bytes) = self
            .live
            .lock()
            .expect("live")
            .get(&a.port)
            .map(|l| {
                (
                    l.connections.load(std::sync::atomic::Ordering::Relaxed),
                    l.bytes.moved(),
                )
            })
            .unwrap_or((0, 0));
        AllocationView {
            port: a.port,
            address: format!("{}:{}", self.config.public_host, a.port),
            host: self.config.public_host.clone(),
            srv_name: a.srv_name.clone(),
            target_port: a.target_port,
            name: a.name.clone(),
            created: a.created,
            lease_until: a.lease_until,
            ends: a.ends,
            connections,
            allow: a.allow.clone(),
            bytes,
            max_bytes: a.max_bytes,
        }
    }

    /// Takes one new connection from `ip`; `false` when it is over its rate.
    fn admit_ip(&self, ip: IpAddr) -> bool {
        let rate = self.config.conn_rps_per_ip;
        let burst = self.config.conn_burst_per_ip;
        let now = Instant::now();
        let mut g = self.per_ip.lock().expect("per ip");
        if g.len() > 50_000 {
            g.retain(|_, b| now.duration_since(b.last) < Duration::from_secs(300));
        }
        let b = g.entry(ip).or_insert(Bucket {
            tokens: burst,
            last: now,
        });
        b.tokens = (b.tokens + now.duration_since(b.last).as_secs_f64() * rate).min(burst);
        b.last = now;
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Allocates a port for `machine` (owned by `owner`).
    pub fn allocate(
        &self,
        machine: &str,
        owner: &str,
        req: &AllocateRequest,
    ) -> Result<Allocation, (StatusCode, String)> {
        let bad = |m: &str| (StatusCode::BAD_REQUEST, m.to_owned());
        if req.token.len() != TOKEN_LEN || !req.token.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(bad("token must be 40 hex characters"));
        }
        if req.target_port == 0 {
            return Err(bad("target_port must be 1-65535"));
        }
        if REFUSED_TARGET_PORTS.contains(&req.target_port) {
            return Err((
                StatusCode::FORBIDDEN,
                format!(
                    "port {} is an admin or remote-access port and cannot be shared publicly",
                    req.target_port
                ),
            ));
        }
        let service = match req.service.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(s) if valid_service(s) => Some(s.to_ascii_lowercase()),
            Some(_) => return Err(bad("service must be 1-15 of a-z, 0-9 and -")),
        };
        if req.allow.len() > 32 {
            return Err(bad("at most 32 allowlist entries"));
        }
        let mut allow = Vec::with_capacity(req.allow.len());
        for c in &req.allow {
            match normalize_cidr(c) {
                Some(n) => allow.push(n),
                None => return Err(bad("allow takes addresses or CIDRs (203.0.113.0/24)")),
            }
        }
        if req.check_host && service.as_deref() != Some("minecraft") {
            return Err(bad(
                "check_host is for Minecraft Java shares (service minecraft)",
            ));
        }
        let max_ttl = self.config.max_ttl.as_secs();
        let ttl = req.ttl_secs.unwrap_or(max_ttl).clamp(60, max_ttl);
        let now = now_secs();
        let mut g = self.allocations.lock().expect("allocations");
        if g.values().any(|a| a.token == req.token) {
            return Err((
                StatusCode::CONFLICT,
                "this share already has a port".to_owned(),
            ));
        }
        let by_owner = g.values().filter(|a| a.owner == owner).count();
        if by_owner >= self.config.per_account {
            return Err((
                StatusCode::TOO_MANY_REQUESTS,
                format!(
                    "your account already has {by_owner} public TCP shares open (the limit is {}); stop one first",
                    self.config.per_account
                ),
            ));
        }
        let by_machine = g.values().filter(|a| a.machine == machine).count();
        if by_machine >= self.config.per_machine {
            return Err((
                StatusCode::TOO_MANY_REQUESTS,
                format!(
                    "this computer already has {by_machine} public TCP shares open (the limit is {})",
                    self.config.per_machine
                ),
            ));
        }
        let (first, last) = self.config.ports;
        let free: Vec<u16> = (first..=last).filter(|p| !g.contains_key(p)).collect();
        if free.is_empty() {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                "every public TCP port of this relay is in use; try again later".to_owned(),
            ));
        }
        // A random free port, so a port number says nothing about when (or
        // whether) a share was made.
        let port = free[rand::random::<u32>() as usize % free.len()];
        let srv_name = match (&service, &self.config.srv_domain) {
            (Some(s), Some(domain)) => Some(format!(
                "{}-{}.{domain}",
                short_service(s),
                hex::encode(rand::random::<[u8; 4]>())
            )),
            _ => None,
        };
        let a = Allocation {
            port,
            machine: machine.to_owned(),
            owner: owner.to_owned(),
            token: req.token.to_ascii_lowercase(),
            target_port: req.target_port,
            service,
            srv_name,
            name: req
                .name
                .as_deref()
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .map(|n| n.chars().take(64).collect()),
            created: now,
            lease_until: (now + self.config.lease.as_secs()).min(now + ttl),
            ends: now + ttl,
            allow,
            check_host: req.check_host,
            max_bytes: req.max_bytes.unwrap_or(0),
        };
        g.insert(port, a.clone());
        self.save(&g);
        Ok(a)
    }

    /// Renews the lease of `port` (machine `machine`).
    pub fn keepalive(&self, machine: &str, port: u16) -> Option<Allocation> {
        let mut g = self.allocations.lock().expect("allocations");
        let a = g.get_mut(&port).filter(|a| a.machine == machine)?;
        a.lease_until = (now_secs() + self.config.lease.as_secs()).min(a.ends);
        let a = a.clone();
        self.save(&g);
        Some(a)
    }

    /// Removes `port` from the table (the caller closes it).
    fn take(&self, port: u16) -> Option<Allocation> {
        let mut g = self.allocations.lock().expect("allocations");
        let a = g.remove(&port)?;
        self.save(&g);
        Some(a)
    }

    /// Stops the listener of `port` and every connection on it.
    fn stop_listener(&self, port: u16) {
        if let Some(l) = self.live.lock().expect("live").remove(&port) {
            l.cut.cancel();
            l.task.abort();
        }
    }
}

fn valid_service(s: &str) -> bool {
    (1..=15).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !s.starts_with('-')
}

/// The short label prefix of a service (`minecraft` -> `mc`).
fn short_service(s: &str) -> &str {
    match s {
        "minecraft" => "mc",
        "terraria" => "tr",
        other => &other[..other.len().min(8)],
    }
}

// ---- Listeners -------------------------------------------------------------

/// Starts the listeners of every allocation and the reconcile sweep. Called
/// when the relay starts serving (never in standby).
pub fn spawn(relay: &Relay) {
    let Some(tcp) = relay.tcp.clone() else {
        return;
    };
    let relay = relay.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(RECONCILE_INTERVAL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            reconcile(&relay, &tcp).await;
        }
    });
}

/// One pass of the reconcile sweep (see the module docs).
pub async fn reconcile(relay: &Relay, tcp: &Arc<TcpShares>) {
    let now = now_secs();
    let mut ended = Vec::new();
    for a in tcp.list() {
        let reason = if a.ends <= now {
            Some("ended")
        } else if a.lease_until <= now {
            Some("lease_lapsed")
        } else if tcp.config.edge {
            // No directory on an edge: the lease is the computer's word.
            None
        } else {
            match relay.directory.get(&a.machine) {
                None => Some("machine_removed"),
                Some(r) if r.owner.id != a.owner => Some("machine_changed_owner"),
                Some(r) if relay.directory.is_detached(&r) => Some("machine_removed"),
                Some(r) if !r.sharing => Some("sharing_stopped"),
                Some(_) => None,
            }
        };
        if let Some(reason) = reason {
            ended.push((a, reason));
        }
    }
    for (a, reason) in ended {
        if tcp.take(a.port).is_some() {
            tcp.stop_listener(a.port);
            audit(relay, &a, "tcp_share_closed", reason);
        }
    }
    // Listeners for every allocation (after a restart, or one that failed).
    for a in tcp.list() {
        let running = tcp.live.lock().expect("live").contains_key(&a.port);
        if !running {
            start_listener(relay, tcp, &a).await;
        }
    }
    if let Err(e) = reconcile_dns(tcp).await {
        tracing::warn!(error = %e, "tcp shares: DNS reconcile failed (retried next sweep)");
    }
}

async fn start_listener(relay: &Relay, tcp: &Arc<TcpShares>, a: &Allocation) {
    let addr = SocketAddr::new(tcp.config.listen, a.port);
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!(port = a.port, error = %e, "tcp shares: cannot listen (retried next sweep)");
            return;
        }
    };
    let connections = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let bytes = Arc::new(ByteBucket::new(tcp.config.bytes_per_sec));
    let cut = tokio_util::sync::CancellationToken::new();
    let port = a.port;
    let (relay2, tcp2, conns2, bytes2, cut2) = (
        relay.clone(),
        tcp.clone(),
        connections.clone(),
        bytes.clone(),
        cut.clone(),
    );
    let task = tokio::spawn(async move {
        loop {
            let (stream, peer) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                }
            };
            let _ = stream.set_nodelay(true);
            let (relay, tcp, conns, bytes, cut) = (
                relay2.clone(),
                tcp2.clone(),
                conns2.clone(),
                bytes2.clone(),
                cut2.clone(),
            );
            tokio::spawn(async move {
                serve_connection(relay, tcp, port, stream, peer, conns, bytes, cut).await;
            });
        }
    })
    .abort_handle();
    tcp.live.lock().expect("live").insert(
        port,
        Live {
            task,
            connections,
            bytes,
            cut,
        },
    );
}

/// Decrements the share's connection count when a connection ends.
struct ConnGuard(Arc<std::sync::atomic::AtomicU32>);

impl Drop for ConnGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

#[allow(clippy::too_many_arguments)]
async fn serve_connection(
    relay: Relay,
    tcp: Arc<TcpShares>,
    port: u16,
    mut stream: tokio::net::TcpStream,
    peer: SocketAddr,
    connections: Arc<std::sync::atomic::AtomicU32>,
    bytes: Arc<ByteBucket>,
    cut: tokio_util::sync::CancellationToken,
) {
    let client = if tcp.config.proxy_protocol {
        match tokio::time::timeout(Duration::from_secs(5), read_proxy_v2(&mut stream)).await {
            Ok(Ok(Some(addr))) => addr,
            Ok(Ok(None)) => peer,
            // A connection without the header did not come through the
            // proxy in front: refuse it.
            _ => return,
        }
    } else {
        peer
    };
    if !tcp.admit_ip(client.ip()) {
        return;
    }
    let Some(a) = tcp
        .allocations
        .lock()
        .expect("allocations")
        .get(&port)
        .cloned()
    else {
        return;
    };
    let now = now_secs();
    if a.lease_until <= now || a.ends <= now {
        return;
    }
    // Over its data cap: no new connections.
    if a.max_bytes > 0 && bytes.moved() >= a.max_bytes {
        return;
    }
    // The share's allowlist, on the real client address.
    if !a.allow.is_empty() && !a.allow.iter().any(|c| in_cidr(c, client.ip())) {
        return;
    }
    // A Minecraft share only takes connections that dialed its address.
    let mut first = Vec::new();
    if a.check_host {
        let mut expected = vec![tcp.config.public_host.clone()];
        if let Some(n) = &a.srv_name {
            expected.push(n.clone());
        }
        let read = async {
            let mut buf = [0u8; 256];
            loop {
                match crate::mc_handshake::parse(&first) {
                    crate::mc_handshake::Handshake::NeedMore => {}
                    other => return other,
                }
                match stream.read(&mut buf).await {
                    Ok(0) | Err(_) => return crate::mc_handshake::Handshake::NotMinecraft,
                    Ok(n) => first.extend_from_slice(&buf[..n]),
                }
            }
        };
        match tokio::time::timeout(Duration::from_secs(5), read).await {
            Ok(crate::mc_handshake::Handshake::Host(h))
                if crate::mc_handshake::matches(&h, &expected) => {}
            _ => return,
        }
    }
    let n = connections.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let _guard = ConnGuard(connections.clone());
    if n >= tcp.config.max_conns_per_share {
        return;
    }
    let ws = match open_machine_socket(&relay, &a, client).await {
        Ok(ws) => ws,
        Err(e) => {
            tracing::debug!(port, error = %e, "tcp shares: could not reach the share");
            return;
        }
    };
    splice(stream, ws, bytes, cut, first, a.max_bytes).await;
}

/// Whether `ip` is in `cidr` (`a.b.c.d/n`, IPv6 too).
fn in_cidr(cidr: &str, ip: IpAddr) -> bool {
    let Some((net, len)) = cidr.split_once('/') else {
        return false;
    };
    let (Ok(net), Ok(len)) = (net.parse::<IpAddr>(), len.parse::<u32>()) else {
        return false;
    };
    match (net, ip) {
        (IpAddr::V4(n), IpAddr::V4(i)) => {
            let mask = if len == 0 {
                0
            } else {
                u32::MAX << (32 - len.min(32))
            };
            u32::from(n) & mask == u32::from(i) & mask
        }
        (IpAddr::V6(n), IpAddr::V6(i)) => {
            let mask = if len == 0 {
                0
            } else {
                u128::MAX << (128 - len.min(128))
            };
            u128::from(n) & mask == u128::from(i) & mask
        }
        _ => false,
    }
}

/// `a.b.c.d/n` or a bare address (as `/32` or `/128`); `None` when invalid.
fn normalize_cidr(s: &str) -> Option<String> {
    let s = s.trim();
    let (ip, len) = match s.split_once('/') {
        Some((ip, len)) => (ip, Some(len)),
        None => (s, None),
    };
    let ip: IpAddr = ip.parse().ok()?;
    let max = if ip.is_ipv4() { 32 } else { 128 };
    let len: u8 = match len {
        Some(l) => l.parse().ok().filter(|l| *l <= max)?,
        None => max,
    };
    Some(format!("{ip}/{len}"))
}

/// Reads a PROXY protocol v2 header. `Ok(None)`: a LOCAL command (a health
/// check of the proxy itself); the peer address stands.
async fn read_proxy_v2(stream: &mut tokio::net::TcpStream) -> std::io::Result<Option<SocketAddr>> {
    const SIG: [u8; 12] = [
        0x0D, 0x0A, 0x0D, 0x0A, 0x00, 0x0D, 0x0A, 0x51, 0x55, 0x49, 0x54, 0x0A,
    ];
    let mut head = [0u8; 16];
    stream.read_exact(&mut head).await?;
    if head[..12] != SIG || head[12] >> 4 != 2 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "no PROXY v2 header",
        ));
    }
    let len = u16::from_be_bytes([head[14], head[15]]) as usize;
    if len > 512 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "PROXY header too long",
        ));
    }
    let mut body = vec![0u8; len];
    stream.read_exact(&mut body).await?;
    let command = head[12] & 0x0F;
    if command == 0 {
        return Ok(None);
    }
    Ok(match head[13] {
        // TCP over IPv4: src(4) dst(4) sport(2) dport(2).
        0x11 if body.len() >= 12 => Some(SocketAddr::new(
            IpAddr::from([body[0], body[1], body[2], body[3]]),
            u16::from_be_bytes([body[8], body[9]]),
        )),
        // TCP over IPv6: src(16) dst(16) sport(2) dport(2).
        0x21 if body.len() >= 36 => {
            let mut ip = [0u8; 16];
            ip.copy_from_slice(&body[..16]);
            Some(SocketAddr::new(
                IpAddr::from(ip),
                u16::from_be_bytes([body[32], body[33]]),
            ))
        }
        _ => None,
    })
}

type MachineSocket = tokio_tungstenite::WebSocketStream<tokio::io::DuplexStream>;

/// A WebSocket to the share's public path on its machine, made through the
/// relay's own router over an in-process connection, so every rule of a
/// public link applies (see the module docs). `client` is the address the
/// router sees (rate limits, logs).
async fn open_machine_socket(
    relay: &Relay,
    a: &Allocation,
    client: SocketAddr,
) -> Result<MachineSocket, String> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;

    let (ours, theirs) = tokio::io::duplex(256 * 1024);
    let router = relay
        .router()
        .layer(axum::Extension(axum::extract::ConnectInfo(client)));
    tokio::spawn(async move {
        let service = hyper_util::service::TowerToHyperService::new(router);
        let _ = hyper::server::conn::http1::Builder::new()
            .serve_connection(hyper_util::rt::TokioIo::new(theirs), service)
            .with_upgrades()
            .await;
    });
    // The computer's own origin when the relay has a host domain (path mode
    // on the main origin would be redirected for a request without an
    // account header), else path mode.
    let url = match relay.config.host_domain.as_deref() {
        Some(domain) => format!("ws://{}.{domain}/share/pub/{}/", a.machine, a.token),
        None => format!("ws://relay.internal/m/{}/share/pub/{}/", a.machine, a.token),
    };
    let mut request = url
        .into_client_request()
        .map_err(|e| format!("request: {e}"))?;
    request.headers_mut().insert(
        "x-cua-share-transport",
        axum::http::HeaderValue::from_static("tcp"),
    );
    let (ws, response) = tokio::time::timeout(
        Duration::from_secs(15),
        tokio_tungstenite::client_async(request, ours),
    )
    .await
    .map_err(|_| "the share did not answer in time".to_string())?
    .map_err(|e| format!("handshake: {e}"))?;
    if response.status() != StatusCode::SWITCHING_PROTOCOLS {
        return Err(format!("the share answered {}", response.status()));
    }
    Ok(ws)
}

/// Copies bytes between the client's TCP connection and the share's
/// WebSocket until either side closes or the allocation is closed.
async fn splice(
    stream: tokio::net::TcpStream,
    ws: MachineSocket,
    bytes: Arc<ByteBucket>,
    cut: tokio_util::sync::CancellationToken,
    first: Vec<u8>,
    max_bytes: u64,
) {
    let (mut tcp_read, mut tcp_write) = stream.into_split();
    let (mut ws_write, mut ws_read) = ws.split();
    let up_bytes = bytes.clone();
    let up_cut = cut.clone();
    // The data cap: every connection of the share ends once it is reached.
    let watch_bytes = bytes.clone();
    let over_cap = async move {
        if max_bytes == 0 {
            return std::future::pending::<()>().await;
        }
        loop {
            if watch_bytes.moved() >= max_bytes {
                return;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    };
    let up = async move {
        // What was read to check the handshake goes first.
        if !first.is_empty() {
            up_bytes.take(first.len()).await;
            if ws_write.send(Message::Binary(first)).await.is_err() {
                return;
            }
        }
        let mut buf = vec![0u8; 32 * 1024];
        loop {
            let n = match tcp_read.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            // A chunk that would pass the cap is not carried: the share ends.
            if up_bytes.would_pass(n, max_bytes) {
                up_cut.cancel();
                break;
            }
            up_bytes.take(n).await;
            if ws_write
                .send(Message::Binary(buf[..n].to_vec()))
                .await
                .is_err()
            {
                break;
            }
        }
        let _ = ws_write.close().await;
    };
    let down_cut = cut.clone();
    let down = async move {
        while let Some(Ok(msg)) = ws_read.next().await {
            let data = match msg {
                Message::Binary(d) => d,
                Message::Close(_) => break,
                _ => continue,
            };
            if bytes.would_pass(data.len(), max_bytes) {
                down_cut.cancel();
                break;
            }
            bytes.take(data.len()).await;
            if tcp_write.write_all(&data).await.is_err() {
                break;
            }
        }
        let _ = tcp_write.shutdown().await;
    };
    tokio::select! {
        _ = async { tokio::join!(up, down) } => {}
        _ = cut.cancelled() => {}
        _ = over_cap => {
            tracing::info!("tcp shares: a share reached its data cap");
            cut.cancel();
        }
    }
}

// ---- DNS -------------------------------------------------------------------

#[derive(Deserialize)]
struct CfList {
    #[serde(default)]
    result: Vec<CfRecord>,
    #[serde(default)]
    result_info: Option<CfInfo>,
    success: bool,
    #[serde(default)]
    errors: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct CfInfo {
    total_pages: u32,
}

#[derive(Deserialize, Clone)]
struct CfRecord {
    id: String,
    name: String,
    #[serde(default)]
    comment: Option<String>,
    #[serde(default)]
    data: Option<serde_json::Value>,
}

/// The tag of this relay's records: the tag plus its SRV domain, so a
/// relay for another environment never touches this one's records in the same zone.
fn dns_tag(srv_domain: &str) -> String {
    format!("{DNS_TAG} {srv_domain}")
}

/// The SRV owner name for an allocation (`_minecraft._tcp.mc-….m.cua.sh`).
fn srv_owner(a: &Allocation) -> Option<String> {
    Some(format!(
        "_{}._tcp.{}",
        a.service.as_deref()?,
        a.srv_name.as_deref()?
    ))
}

async fn reconcile_dns(tcp: &TcpShares) -> Result<(), String> {
    let (Some(dns), Some(domain)) = (&tcp.config.dns, &tcp.config.srv_domain) else {
        return Ok(());
    };
    let tag = dns_tag(domain);
    let mut existing: Vec<CfRecord> = Vec::new();
    let mut page = 1u32;
    loop {
        let url = format!(
            "{}/zones/{}/dns_records?type=SRV&per_page=500&page={page}",
            dns.api_base, dns.zone_id
        );
        let r: CfList = tcp
            .http
            .get(&url)
            .bearer_auth(&dns.token)
            .send()
            .await
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())?;
        if !r.success {
            return Err(format!("list: {:?}", r.errors));
        }
        existing.extend(
            r.result
                .into_iter()
                .filter(|rec| rec.comment.as_deref().is_some_and(|c| c.starts_with(&tag))),
        );
        let pages = r.result_info.map(|i| i.total_pages).unwrap_or(1);
        if page >= pages {
            break;
        }
        page += 1;
    }
    let want: HashMap<String, Allocation> = tcp
        .list()
        .into_iter()
        .filter_map(|a| Some((srv_owner(&a)?, a)))
        .collect();
    // Delete what no live allocation explains (closed shares, a crash
    // between allocate and close, a port that moved).
    for rec in &existing {
        let keep = want.get(&rec.name).is_some_and(|a| {
            rec.data
                .as_ref()
                .and_then(|d| d.get("port"))
                .and_then(serde_json::Value::as_u64)
                == Some(a.port as u64)
        });
        if !keep {
            let url = format!(
                "{}/zones/{}/dns_records/{}",
                dns.api_base, dns.zone_id, rec.id
            );
            tcp.http
                .delete(&url)
                .bearer_auth(&dns.token)
                .send()
                .await
                .map_err(|e| e.to_string())?;
            tcp.dns_ids.lock().expect("dns ids").remove(&rec.name);
        }
    }
    let present: std::collections::HashSet<&str> = existing
        .iter()
        .filter(|r| want.contains_key(&r.name))
        .map(|r| r.name.as_str())
        .collect();
    for (name, a) in &want {
        if present.contains(name.as_str()) {
            continue;
        }
        let body = serde_json::json!({
            "type": "SRV",
            "name": name,
            "ttl": 60,
            "proxied": false,
            "comment": format!("{tag} {}", a.port),
            "data": {
                "priority": 0,
                "weight": 5,
                "port": a.port,
                "target": tcp.config.public_host,
            },
        });
        let url = format!("{}/zones/{}/dns_records", dns.api_base, dns.zone_id);
        let r: serde_json::Value = tcp
            .http
            .post(&url)
            .bearer_auth(&dns.token)
            .json(&body)
            .send()
            .await
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())?;
        if r.get("success").and_then(serde_json::Value::as_bool) != Some(true) {
            return Err(format!(
                "create {name}: {}",
                r.get("errors").cloned().unwrap_or_default()
            ));
        }
        if let Some(id) = r.pointer("/result/id").and_then(serde_json::Value::as_str) {
            tcp.dns_ids
                .lock()
                .expect("dns ids")
                .insert(name.clone(), id.to_owned());
        }
    }
    Ok(())
}

// ---- API -------------------------------------------------------------------

/// The API routes (the feature off: every route answers 404).
pub(crate) fn routes() -> Router<Relay> {
    Router::new()
        .route(
            "/v1/machines/{id}/tcp-shares",
            get(list_route).post(allocate_route),
        )
        .route("/v1/machines/{id}/tcp-shares/{port}", delete(close_route))
        .route(
            "/v1/machines/{id}/tcp-shares/{port}/keepalive",
            post(keepalive_route),
        )
}

#[allow(clippy::result_large_err)] // axum's Response is the route error everywhere here.
fn feature(relay: &Relay) -> Result<Arc<TcpShares>, Response> {
    relay.tcp.clone().ok_or_else(|| {
        error(
            StatusCode::NOT_FOUND,
            "public TCP shares are not available on this relay",
        )
    })
}

/// The computer's owner account, when the caller is its owner (from an
/// enrolled device) or the computer itself.
#[allow(clippy::result_large_err)] // axum's Response is the route error everywhere here.
async fn owner_of(
    relay: &Relay,
    headers: &HeaderMap,
    id: &str,
) -> Result<(String, Option<String>), Response> {
    if relay.tcp.as_ref().is_some_and(|t| t.config.edge) {
        return edge_owner(relay, headers, id);
    }
    let caller = crate::api::caller(relay, headers).await?;
    caller.require_device()?;
    let record = relay
        .directory
        .get(id)
        .ok_or_else(|| error(StatusCode::NOT_FOUND, "no such computer"))?;
    match &caller {
        Caller::Machine(m) if m.id == id => {}
        Caller::Machine(_) => {
            return Err(error(
                StatusCode::FORBIDDEN,
                "computer token for another computer",
            ))
        }
        Caller::Account(who, _) => {
            if who.user != record.owner.id {
                // Not the owner: indistinguishable from a missing machine
                // unless it is shared with the caller.
                return Err(error(StatusCode::FORBIDDEN, "only the owner can do this"));
            }
        }
    }
    Ok((record.owner.id.clone(), caller.actor()))
}

/// On an edge: the caller must hold the edge's static token, and the
/// machine must be joined with it.
#[allow(clippy::result_large_err)] // axum's Response is the route error everywhere here.
fn edge_owner(
    relay: &Relay,
    headers: &HeaderMap,
    id: &str,
) -> Result<(String, Option<String>), Response> {
    use subtle::ConstantTimeEq as _;
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    let known = relay
        .config
        .tokens
        .iter()
        .any(|t| bool::from(t.as_bytes().ct_eq(token.as_bytes())));
    if token.is_empty() || !known {
        return Err(error(
            StatusCode::UNAUTHORIZED,
            "missing or wrong edge token",
        ));
    }
    if relay.machine(id).is_none() {
        return Err(error(
            StatusCode::NOT_FOUND,
            "that computer is not joined to this edge",
        ));
    }
    Ok(("edge".into(), Some("machine".into())))
}

fn audit(relay: &Relay, a: &Allocation, kind: &str, detail: &str) {
    let host = relay
        .tcp
        .as_ref()
        .map(|t| t.config.public_host.clone())
        .unwrap_or_default();
    relay.devices.audit(
        &a.owner,
        AuditEvent::new(kind)
            .machine(&a.machine)
            .subject(match &a.srv_name {
                Some(n) => format!("{host}:{} ({n})", a.port),
                None => format!("{host}:{}", a.port),
            })
            .detail(format!(
                "space port {}{}; {detail}",
                a.target_port,
                a.service
                    .as_deref()
                    .map(|s| format!(", {s}"))
                    .unwrap_or_default()
            )),
    );
}

async fn allocate_route(
    State(relay): State<Relay>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(req): Json<AllocateRequest>,
) -> Response {
    let tcp = match feature(&relay) {
        Ok(t) => t,
        Err(r) => return r,
    };
    let (owner, actor) = match owner_of(&relay, &headers, &id).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    // Only machines that joined as the account (not static-token machines)
    // carry public shares.
    if let Some(m) = relay.machine(&id).filter(|_| !tcp.config.edge) {
        if !matches!(&m.registrant, Registrant::Account(o) if o == &owner) {
            return error(
                StatusCode::FORBIDDEN,
                "this computer cannot carry public shares",
            );
        }
    }
    let a = match tcp.allocate(&id, &owner, &req) {
        Ok(a) => a,
        Err((status, message)) => return error(status, message),
    };
    start_listener(&relay, &tcp, &a).await;
    audit(
        &relay,
        &a,
        "tcp_share_opened",
        &format!("by {}", actor.as_deref().unwrap_or("owner")),
    );
    // The SRV record is made in the background; the sweep retries it.
    {
        let tcp = tcp.clone();
        tokio::spawn(async move {
            if let Err(e) = reconcile_dns(&tcp).await {
                tracing::warn!(error = %e, "tcp shares: DNS update failed (retried next sweep)");
            }
        });
    }
    (StatusCode::CREATED, Json(tcp.view(&a))).into_response()
}

async fn list_route(
    State(relay): State<Relay>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let tcp = match feature(&relay) {
        Ok(t) => t,
        Err(r) => return r,
    };
    if let Err(r) = owner_of(&relay, &headers, &id).await {
        return r;
    }
    let rows: Vec<AllocationView> = tcp
        .list()
        .iter()
        .filter(|a| a.machine == id)
        .map(|a| tcp.view(a))
        .collect();
    Json(serde_json::json!({ "shares": rows })).into_response()
}

async fn keepalive_route(
    State(relay): State<Relay>,
    Path((id, port)): Path<(String, u16)>,
    headers: HeaderMap,
) -> Response {
    let tcp = match feature(&relay) {
        Ok(t) => t,
        Err(r) => return r,
    };
    if let Err(r) = owner_of(&relay, &headers, &id).await {
        return r;
    }
    match tcp.keepalive(&id, port) {
        Some(a) => Json(tcp.view(&a)).into_response(),
        None => error(
            StatusCode::NOT_FOUND,
            "no such TCP share (it may have ended)",
        ),
    }
}

async fn close_route(
    State(relay): State<Relay>,
    Path((id, port)): Path<(String, u16)>,
    headers: HeaderMap,
) -> Response {
    let tcp = match feature(&relay) {
        Ok(t) => t,
        Err(r) => return r,
    };
    if let Err(r) = owner_of(&relay, &headers, &id).await {
        return r;
    }
    let owned = tcp
        .allocations
        .lock()
        .expect("allocations")
        .get(&port)
        .is_some_and(|a| a.machine == id);
    if !owned {
        return error(
            StatusCode::NOT_FOUND,
            "no such TCP share (it may have ended)",
        );
    }
    if let Some(a) = tcp.take(port) {
        tcp.stop_listener(port);
        audit(&relay, &a, "tcp_share_closed", "stopped");
        let tcp = tcp.clone();
        tokio::spawn(async move {
            if let Err(e) = reconcile_dns(&tcp).await {
                tracing::warn!(error = %e, "tcp shares: DNS update failed (retried next sweep)");
            }
        });
    }
    Json(serde_json::json!({ "closed": port })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shares(cfg: TcpShareConfig) -> TcpShares {
        TcpShares::open(cfg, None).unwrap()
    }

    fn req(token: char, port: u16) -> AllocateRequest {
        AllocateRequest {
            token: token.to_string().repeat(40),
            target_port: port,
            ttl_secs: None,
            service: Some("minecraft".into()),
            name: None,
            allow: Vec::new(),
            check_host: false,
            max_bytes: None,
        }
    }

    #[test]
    fn allowlists_and_host_checks_are_checked_at_allocation() {
        let t = shares(TcpShareConfig {
            public_host: "tcp.m.example".into(),
            srv_domain: Some("m.example".into()),
            ..TcpShareConfig::default()
        });
        let mut r = req('a', 25565);
        r.allow = vec!["203.0.113.0/24".into(), "198.51.100.7".into()];
        r.check_host = true;
        let a = t.allocate("m1", "ada", &r).unwrap();
        assert_eq!(a.allow, vec!["203.0.113.0/24", "198.51.100.7/32"]);
        assert!(a.check_host);
        let mut bad = req('b', 25565);
        bad.allow = vec!["not an address".into()];
        assert_eq!(
            t.allocate("m1", "ada", &bad).unwrap_err().0,
            StatusCode::BAD_REQUEST
        );
        let mut not_mc = req('c', 7777);
        not_mc.service = Some("terraria".into());
        not_mc.check_host = true;
        assert_eq!(
            t.allocate("m1", "ada", &not_mc).unwrap_err().0,
            StatusCode::BAD_REQUEST
        );
        let mut many = req('d', 25565);
        many.allow = (0..33).map(|i| format!("10.0.0.{i}")).collect();
        assert_eq!(
            t.allocate("m1", "ada", &many).unwrap_err().0,
            StatusCode::BAD_REQUEST
        );
    }

    #[test]
    fn cidrs_match_the_real_client_address() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        assert!(in_cidr("203.0.113.0/24", ip("203.0.113.9")));
        assert!(!in_cidr("203.0.113.0/24", ip("203.0.114.9")));
        assert!(in_cidr("0.0.0.0/0", ip("8.8.8.8")));
        assert!(in_cidr("2001:db8::/32", ip("2001:db8::7")));
        assert!(!in_cidr("2001:db8::/32", ip("203.0.113.9")));
        assert!(!in_cidr("garbage", ip("203.0.113.9")));
        assert_eq!(
            normalize_cidr("198.51.100.7").as_deref(),
            Some("198.51.100.7/32")
        );
        assert_eq!(normalize_cidr("10.0.0.0/33"), None);
    }

    #[test]
    fn ports_parse() {
        assert_eq!(
            TcpShareConfig::parse_ports("26000-26063"),
            Ok((26000, 26063))
        );
        assert_eq!(TcpShareConfig::parse_ports("31000"), Ok((31000, 31000)));
        assert!(TcpShareConfig::parse_ports("0-10").is_err());
        assert!(TcpShareConfig::parse_ports("200-100").is_err());
        assert!(TcpShareConfig::parse_ports("1-5000").is_err());
    }

    #[test]
    fn allocations_respect_the_pool_the_limits_and_refused_ports() {
        let t = shares(TcpShareConfig {
            ports: (26000, 26002),
            public_host: "tcp.m.example".into(),
            srv_domain: Some("m.example".into()),
            per_account: 2,
            per_machine: 2,
            ..TcpShareConfig::default()
        });
        let a = t.allocate("m1", "ada", &req('a', 25565)).unwrap();
        assert!((26000..=26002).contains(&a.port));
        let srv = a.srv_name.clone().unwrap();
        assert!(
            srv.starts_with("mc-") && srv.ends_with(".m.example"),
            "{srv}"
        );
        assert_eq!(srv_owner(&a).unwrap(), format!("_minecraft._tcp.{srv}"));
        // The same share twice: refused.
        assert_eq!(
            t.allocate("m1", "ada", &req('a', 25565)).unwrap_err().0,
            StatusCode::CONFLICT
        );
        // Admin ports: refused.
        assert_eq!(
            t.allocate("m1", "ada", &req('b', 25575)).unwrap_err().0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            t.allocate("m1", "ada", &req('b', 22)).unwrap_err().0,
            StatusCode::FORBIDDEN
        );
        t.allocate("m2", "ada", &req('c', 7777)).unwrap();
        // Per account.
        assert_eq!(
            t.allocate("m3", "ada", &req('d', 7777)).unwrap_err().0,
            StatusCode::TOO_MANY_REQUESTS
        );
        // The pool runs out.
        t.allocate("m4", "bob", &req('e', 7777)).unwrap();
        assert_eq!(
            t.allocate("m5", "cy", &req('f', 7777)).unwrap_err().0,
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[test]
    fn a_lease_never_outlives_the_end_and_renews_for_its_machine_only() {
        let t = shares(TcpShareConfig::default());
        let mut r = req('a', 25565);
        r.ttl_secs = Some(120);
        let a = t.allocate("m1", "ada", &r).unwrap();
        assert!(a.lease_until <= a.ends);
        assert!(t.keepalive("other", a.port).is_none());
        let b = t.keepalive("m1", a.port).unwrap();
        assert!(b.lease_until <= b.ends);
    }

    #[test]
    fn allocations_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("relay.tcp.json");
        let t = TcpShares::open(TcpShareConfig::default(), Some(file.clone())).unwrap();
        let a = t.allocate("m1", "ada", &req('a', 25565)).unwrap();
        let again = TcpShares::open(TcpShareConfig::default(), Some(file)).unwrap();
        assert_eq!(again.list(), vec![a]);
    }

    #[test]
    fn per_address_connection_rate() {
        let t = shares(TcpShareConfig {
            conn_rps_per_ip: 0.001,
            conn_burst_per_ip: 3.0,
            ..TcpShareConfig::default()
        });
        let ip = IpAddr::from([203, 0, 113, 7]);
        assert!(t.admit_ip(ip) && t.admit_ip(ip) && t.admit_ip(ip));
        assert!(!t.admit_ip(ip));
        assert!(t.admit_ip(IpAddr::from([203, 0, 113, 8])));
    }

    #[tokio::test]
    async fn the_byte_budget_holds_a_share_to_its_rate() {
        // 64 KiB/s with one second of burst.
        let b = ByteBucket::new(64 * 1024);
        let start = Instant::now();
        // The burst passes at once.
        b.take(32 * 1024).await;
        b.take(32 * 1024).await;
        assert!(
            start.elapsed() < Duration::from_millis(500),
            "{:?}",
            start.elapsed()
        );
        // Then 128 KiB more takes about two seconds.
        for _ in 0..4 {
            b.take(32 * 1024).await;
        }
        let all = start.elapsed();
        assert!(all >= Duration::from_millis(1500), "{all:?}");
        assert!(all < Duration::from_secs(6), "{all:?}");
        // A single write larger than the whole budget still passes (it waits
        // for a full bucket, never forever).
        tokio::time::timeout(Duration::from_secs(5), b.take(256 * 1024))
            .await
            .expect("an oversized write is not stuck");
    }

    #[test]
    fn the_connection_count_drops_when_a_connection_ends() {
        let n = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let guards: Vec<ConnGuard> = (0..64)
            .map(|_| {
                n.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                ConnGuard(n.clone())
            })
            .collect();
        assert_eq!(n.load(std::sync::atomic::Ordering::SeqCst), 64);
        drop(guards);
        assert_eq!(n.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn proxy_v2_header_gives_the_client_address() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
            let mut h = vec![
                0x0D, 0x0A, 0x0D, 0x0A, 0x00, 0x0D, 0x0A, 0x51, 0x55, 0x49, 0x54, 0x0A, 0x21, 0x11,
                0x00, 0x0C,
            ];
            h.extend_from_slice(&[198, 51, 100, 9, 10, 0, 0, 1, 0x30, 0x39, 0x75, 0x30]);
            s.write_all(&h).await.unwrap();
            s.write_all(b"hello").await.unwrap();
        });
        let (mut s, _) = listener.accept().await.unwrap();
        let got = read_proxy_v2(&mut s).await.unwrap().unwrap();
        assert_eq!(got, "198.51.100.9:12345".parse().unwrap());
        let mut rest = [0u8; 5];
        s.read_exact(&mut rest).await.unwrap();
        assert_eq!(&rest, b"hello");
    }
}
