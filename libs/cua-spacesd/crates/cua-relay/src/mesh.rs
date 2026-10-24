// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The mesh coordinator and encrypted relay (`GET /v1/mesh/connect`).
//!
//! Devices and machines run a userspace WireGuard®-based node (the MIT
//! `cua-mesh` crate) and keep one WebSocket here. The relay:
//!
//! - **checks who each node is**: a machine by its machine token (or a
//!   static registration token), a client device by its account token and
//!   enrolled-device session; every node proves its WireGuard key;
//! - **turns accounts, devices and share lists into peer rules**: a client
//!   device peers with the machines its account owns or was shared (and
//!   whose sharing is on); a machine sees each client with the identity a
//!   relay assertion would carry (account, email, role). An account's own
//!   hosts also peer with each other (never another account's machines,
//!   static machines or the Spaces a host provides), each seen with the
//!   [`proto::ROLE_MACHINE`] grant: a gang's rank traffic between the
//!   owner's machines (CUA-1783). In static mode a
//!   client pairs with a machine by proving it holds that machine's env
//!   token; the machine checks the proof and approves (the relay only
//!   passes it along);
//! - **distributes keys and endpoints** (the network map), recomputed when
//!   any of that changes;
//! - **relays packets** between two peers only, and only WireGuard or
//!   disco ciphertext: it never holds a session key, so it cannot read
//!   or alter a session;
//! - **answers STUN** (`--mesh-stun-listen`), so nodes learn their public
//!   address;
//! - **meters mesh sessions** (CUA-1506) like its HTTPS path: an account
//!   machine reports the streams its peers hold open ([`proto::ClientMsg::Usage`]),
//!   and each report feeds the same allowance meter, only for peers in the
//!   machine's own map (owner, share list, team) and as the account the
//!   relay itself knows for that peer's key. Relayed WireGuard packets
//!   between the pair count toward relay hours; a direct path does not.
//!
//! The registry (node keys, endpoints, pairings) persists next to the state
//! file, so a restart loses nothing; peers keep their sessions meanwhile.
//! Writes are coalesced (one compact JSON write per burst of changes, done
//! off the async workers); a new node waits for the write that covers it
//! before it is welcomed, and a pairing is confirmed only once saved. A
//! failed write is counted and reported ([`crate::storage`]) and retried
//! with backoff. The registry is also soft state: every node registers
//! again when it reconnects, so a lost write costs at most a reconnect.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::hash::{Hash as _, Hasher as _};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse as _, Response};
use cua_mesh::proto::{self, ClientMsg, Grant, Mode, NodeKind, PeerInfo, ServerMsg};
use cua_mesh::{PublicKey, SecretKey};
use futures_util::{SinkExt as _, StreamExt as _};
use serde::{Deserialize, Serialize};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, Notify};

use crate::devices::{AuditEvent, DeviceState};
use crate::directory::MACHINE_TOKEN_PREFIX;
use crate::meter::MeterStream;
use crate::oidc::Identity;
use crate::server::{account_token, bearer, Registrant, Relay};
use crate::storage::{store, Storage};
use crate::valid_machine_id;

/// How long an account client's connection lasts before it reconnects with
/// a fresh account token.
const MAX_ACCOUNT_SESSION: Duration = Duration::from_secs(12 * 3600);
/// A connection that says nothing for this long is dropped (nodes ping
/// every 15 s).
const IDLE: Duration = Duration::from_secs(60);
/// Nodes not seen for this long are forgotten.
const FORGET_AFTER_SECS: u64 = 30 * 86_400;
/// Unpaired static-relay clients connected at once.
const MAX_ANONYMOUS: usize = 1024;
/// Queued messages per connection before relayed packets are dropped (at
/// most about 1.5 MiB per node; UDP semantics, QUIC retransmits).
const QUEUE: usize = 1024;
/// Most streams one usage report may hold open for one peer.
const MAX_OPEN_PER_PEER: u32 = 64;
/// How often a machine's metered mesh sessions are checked for relayed
/// packets (relay hours).
/// How long a peer whose new session an enforced plan refused stays out of
/// the machine's map (a retry after that is refused again while over).
const DENY_FOR: Duration = Duration::from_secs(30);

const ATTRIBUTE_EVERY: Duration = Duration::from_secs(5);
/// Relayed packets of a pair are noted at most this often.
const PAIR_MARK_EVERY: Duration = Duration::from_secs(1);
/// Pair activity older than this is forgotten.
const PAIR_FORGET: Duration = Duration::from_secs(120);

/// Who a node is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Owner {
    /// A hosted machine.
    Machine {
        id: String,
        /// `account:<owner>` or `static:<token index>`.
        registrant: String,
    },
    /// A client device of an account.
    Account {
        user: String,
        account: String,
        #[serde(default)]
        email: Option<String>,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        device: Option<String>,
    },
    /// A client of a static-token relay (peers only by pairing).
    Static,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Node {
    node_key: PublicKey,
    disco_key: PublicKey,
    quic_pin: String,
    kind: NodeKind,
    owner: Owner,
    #[serde(default)]
    name: String,
    /// Where the node says it can be reached (up to 32). Never written to
    /// disk: a node reports them again on every connect, and a relay that
    /// restarts has no use for an old address.
    #[serde(default, skip_serializing)]
    endpoints: Vec<SocketAddr>,
    version: u32,
    last_seen: u64,
    #[serde(default)]
    ephemeral: bool,
}

impl Node {
    fn machine_id(&self) -> Option<&str> {
        match &self.owner {
            Owner::Machine { id, .. } => Some(id),
            _ => None,
        }
    }

    fn identity(&self) -> Option<Identity> {
        match &self.owner {
            Owner::Account {
                user,
                account,
                email,
                name,
                ..
            } => Some(Identity {
                user: user.clone(),
                account: account.clone(),
                email: email.clone(),
                name: name.clone(),
                auth_time: None,
                mfa: false,
                orgs: Vec::new(),
            }),
            _ => None,
        }
    }

    fn info(&self, online: bool, grant: Option<Grant>) -> PeerInfo {
        PeerInfo {
            node_key: self.node_key,
            disco_key: self.disco_key,
            quic_pin: self.quic_pin.clone(),
            kind: self.kind,
            machine_id: self.machine_id().map(str::to_owned),
            name: self.name.clone(),
            endpoints: self.endpoints.clone(),
            online,
            version: self.version,
            grant,
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Persisted {
    nodes: Vec<Node>,
    /// `(machine id, client key)` pairings of static machines.
    pairs: Vec<(String, PublicKey)>,
}

struct Conn {
    tx: mpsc::Sender<Message>,
    generation: u64,
    since: Instant,
    /// Peer keys this node may send relayed packets to.
    allowed: Arc<RwLock<HashSet<PublicKey>>>,
    netmap_hash: u64,
}

struct PendingPair {
    client: PublicKey,
    machine_id: String,
    expires: Instant,
}

/// Mesh settings of a relay.
#[derive(Debug, Clone)]
pub struct MeshConfig {
    /// Serve the mesh at all.
    pub enabled: bool,
    /// Relay packets between peers (the encrypted fallback).
    pub relay: bool,
    /// STUN servers advertised to nodes (`host:port`).
    pub stun: Vec<String>,
}

impl Default for MeshConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            relay: true,
            stun: Vec::new(),
        }
    }
}

/// The coordinator's state.
pub struct MeshState {
    path: Option<PathBuf>,
    nodes: Mutex<HashMap<PublicKey, Node>>,
    pairs: Mutex<BTreeSet<(String, PublicKey)>>,
    conns: Mutex<HashMap<PublicKey, Conn>>,
    pending: Mutex<HashMap<String, PendingPair>>,
    generation: AtomicU64,
    kick: Notify,
    dirty: Notify,
    /// Bytes relayed (ciphertext).
    pub relayed_bytes: AtomicU64,
    /// Packets dropped because the destination was offline, not a peer or
    /// slow.
    pub dropped: AtomicU64,
    /// When WireGuard packets between two nodes (the pair, in key order)
    /// were last relayed: mesh sessions crossing the relay use relay hours.
    pair_seen: Mutex<HashMap<(PublicKey, PublicKey), Instant>>,
    /// Usage reports machines sent (CUA-1506).
    usage_reports: AtomicU64,
    /// Report entries ignored: a peer outside the machine's map, a user that
    /// is not the peer's own, or not an account machine.
    usage_rejected: AtomicU64,
    /// Mesh streams metered now.
    metered_streams: AtomicU64,
    /// `(machine node, peer node)` pairs whose new session an enforced plan
    /// refused, until the instant: the peer is left out of that machine's
    /// map (and its relayed packets dropped) meanwhile.
    denied: Mutex<HashMap<(PublicKey, PublicKey), Instant>>,
    /// Sessions refused by an enforced plan since start.
    sessions_refused: AtomicU64,
    /// Where write failures are counted and reported.
    storage: Arc<Storage>,
    /// Saves asked for (a counter) ...
    save_asked: AtomicU64,
    /// ... and the last one done: (the counter it covered, whether it was
    /// written).
    saved: tokio::sync::watch::Sender<(u64, bool)>,
}

impl std::fmt::Debug for MeshState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MeshState")
            .field("nodes", &self.nodes.lock().expect("nodes").len())
            .finish()
    }
}

impl MeshState {
    /// Loads the registry from `path` (missing: empty).
    pub fn open(path: Option<PathBuf>) -> std::io::Result<Self> {
        Self::open_with(path, Arc::default())
    }

    /// [`MeshState::open`], reporting writes to `storage`. Temp files an
    /// interrupted write left are removed first.
    pub fn open_with(path: Option<PathBuf>, storage: Arc<Storage>) -> std::io::Result<Self> {
        if let Some(p) = &path {
            crate::assertion::remove_stale_temps(p)?;
        }
        let persisted: Persisted = match &path {
            Some(p) => match std::fs::read_to_string(p) {
                Ok(raw) => serde_json::from_str(&raw)
                    .map_err(|e| std::io::Error::other(format!("{}: {e}", p.display())))?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Persisted::default(),
                Err(e) => return Err(e),
            },
            None => Persisted::default(),
        };
        let now = crate::assertion::now_secs();
        Ok(Self {
            path,
            nodes: Mutex::new(
                persisted
                    .nodes
                    .into_iter()
                    .filter(|n| !n.ephemeral && n.last_seen + FORGET_AFTER_SECS > now)
                    .map(|mut n| {
                        // Registries written before endpoints stopped being
                        // persisted still carry them.
                        n.endpoints.clear();
                        (n.node_key, n)
                    })
                    .collect(),
            ),
            pairs: Mutex::new(persisted.pairs.into_iter().collect()),
            conns: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            generation: AtomicU64::new(0),
            kick: Notify::new(),
            dirty: Notify::new(),
            relayed_bytes: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            pair_seen: Mutex::new(HashMap::new()),
            usage_reports: AtomicU64::new(0),
            usage_rejected: AtomicU64::new(0),
            metered_streams: AtomicU64::new(0),
            denied: Mutex::new(HashMap::new()),
            sessions_refused: AtomicU64::new(0),
            storage,
            save_asked: AtomicU64::new(0),
            saved: tokio::sync::watch::channel((0, true)).0,
        })
    }

    /// Emails the registry holds for `account`'s client devices.
    pub fn emails_of(&self, account: &str) -> Vec<String> {
        self.nodes
            .lock()
            .expect("nodes")
            .values()
            .filter_map(|n| match &n.owner {
                Owner::Account {
                    account: a,
                    user,
                    email: Some(e),
                    ..
                } if a == account || user == account => Some(e.clone()),
                _ => None,
            })
            .collect()
    }

    /// Account deletion: forgets `account`'s nodes (its client devices and
    /// the machines it registered, or any of `machines`), their pairings,
    /// connections, pending pairs and activity, then rewrites the registry.
    /// Returns the nodes removed. Callers follow with [`Self::changed`] so
    /// the remaining nodes' maps drop the removed peers.
    pub fn erase_account(&self, account: &str, machines: &[String]) -> std::io::Result<usize> {
        let registrant = format!("account:{account}");
        let mut removed_keys: HashSet<PublicKey> = HashSet::new();
        let mut removed_machines: HashSet<String> = machines.iter().cloned().collect();
        {
            let mut nodes = self.nodes.lock().expect("nodes");
            nodes.retain(|key, n| {
                let mine = match &n.owner {
                    Owner::Account {
                        user, account: a, ..
                    } => a == account || user == account,
                    Owner::Machine {
                        id,
                        registrant: reg,
                    } => *reg == registrant || machines.contains(id),
                    Owner::Static => false,
                };
                if mine {
                    removed_keys.insert(*key);
                    if let Some(id) = n.machine_id() {
                        removed_machines.insert(id.to_owned());
                    }
                }
                !mine
            });
        }
        self.pairs
            .lock()
            .expect("pairs")
            .retain(|(m, c)| !removed_machines.contains(m) && !removed_keys.contains(c));
        {
            let mut conns = self.conns.lock().expect("conns");
            for key in &removed_keys {
                if let Some(conn) = conns.remove(key) {
                    let _ = conn.tx.try_send(Message::Close(None));
                }
            }
        }
        self.pending.lock().expect("pending").retain(|_, p| {
            !removed_keys.contains(&p.client) && !removed_machines.contains(&p.machine_id)
        });
        self.denied
            .lock()
            .expect("denied")
            .retain(|(a, b), _| !removed_keys.contains(a) && !removed_keys.contains(b));
        self.pair_seen
            .lock()
            .expect("pair_seen")
            .retain(|(a, b), _| !removed_keys.contains(a) && !removed_keys.contains(b));
        // Always rewritten: a retry after a failed write finishes the job.
        let saved = self.save_now();
        self.storage.record(store::MESH, &saved);
        saved?;
        Ok(removed_keys.len())
    }

    /// Forgets the endpoints of every node that has been offline for more
    /// than `days` (0: never), keeping its key registration. Returns how
    /// many nodes lost their endpoints.
    pub fn forget_stale_endpoints(&self, days: u64) -> usize {
        if days == 0 {
            return 0;
        }
        let cutoff = crate::assertion::now_secs().saturating_sub(days.saturating_mul(86_400));
        let live: HashSet<PublicKey> = self.conns.lock().expect("conns").keys().copied().collect();
        let mut nodes = self.nodes.lock().expect("nodes");
        let mut forgotten = 0;
        for n in nodes.values_mut() {
            if !n.endpoints.is_empty() && !live.contains(&n.node_key) && n.last_seen < cutoff {
                n.endpoints.clear();
                forgotten += 1;
            }
        }
        forgotten
    }

    /// Recompute every network map now (the directory or devices changed).
    pub fn changed(&self) {
        self.kick.notify_one();
    }

    /// Asks for a save and waits for the write that covers it: whether it
    /// was written (true when nothing is persisted; false after 10 s).
    pub async fn persist(&self) -> bool {
        if self.path.is_none() {
            return true;
        }
        let asked = self.save_asked.fetch_add(1, Ordering::SeqCst) + 1;
        let mut rx = self.saved.subscribe();
        self.dirty.notify_one();
        let wait = async {
            loop {
                let (done, ok) = *rx.borrow_and_update();
                if done >= asked {
                    return ok;
                }
                if rx.changed().await.is_err() {
                    return false;
                }
            }
        };
        tokio::time::timeout(Duration::from_secs(10), wait)
            .await
            .unwrap_or(false)
    }

    /// The registry as compact JSON, encoded under the locks straight from
    /// the live maps (no copy of the nodes).
    fn encode(&self) -> serde_json::Result<Vec<u8>> {
        #[derive(Serialize)]
        struct Out<'a> {
            nodes: Vec<&'a Node>,
            pairs: &'a BTreeSet<(String, PublicKey)>,
        }
        let nodes = self.nodes.lock().expect("nodes");
        let pairs = self.pairs.lock().expect("pairs");
        let paired: HashSet<PublicKey> = pairs.iter().map(|(_, c)| *c).collect();
        serde_json::to_vec(&Out {
            nodes: nodes
                .values()
                .filter(|n| !n.ephemeral)
                .filter(|n| n.owner != Owner::Static || paired.contains(&n.node_key))
                .collect(),
            pairs: &pairs,
        })
    }

    /// Writes the registry now (blocking).
    fn save_now(&self) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let json = self.encode().map_err(std::io::Error::other)?;
        crate::assertion::write_private(path, &json)
    }

    /// Closes every node's connection (they reconnect on their own; a
    /// shutdown, or a test standing in for a crash).
    pub fn disconnect_all(&self) {
        for (_, conn) in self.conns.lock().expect("conns").drain() {
            let _ = conn.tx.try_send(Message::Close(None));
        }
    }

    /// Connected static-relay clients no machine has accepted yet.
    fn anonymous(&self) -> usize {
        let nodes = self.nodes.lock().expect("nodes");
        let pairs = self.pairs.lock().expect("pairs");
        self.conns
            .lock()
            .expect("conns")
            .keys()
            .filter(|k| {
                nodes.get(k).is_some_and(|n| n.owner == Owner::Static)
                    && !pairs.iter().any(|(_, c)| c == *k)
            })
            .count()
    }

    /// Nodes connected right now.
    pub fn connected(&self) -> usize {
        self.conns.lock().expect("conns").len()
    }
}

/// Starts the background tasks: map recomputation and registry saving.
pub fn spawn(relay: &Relay) {
    let r = relay.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(2));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = tick.tick() => {}
                _ = r.mesh.kick.notified() => {}
            }
            recompute(&r);
        }
    });
    let r = relay.clone();
    tokio::spawn(async move {
        let mut backoff = Duration::from_secs(1);
        loop {
            r.mesh.dirty.notified().await;
            // Coalesce bursts (connects, endpoint updates) into one write.
            tokio::time::sleep(Duration::from_millis(200)).await;
            let asked = r.mesh.save_asked.load(Ordering::SeqCst);
            let mesh = r.mesh.clone();
            let written = tokio::task::spawn_blocking(move || mesh.save_now())
                .await
                .unwrap_or_else(|e| Err(std::io::Error::other(e.to_string())));
            if r.mesh.path.is_some() {
                r.mesh.storage.record(store::MESH, &written);
            }
            let ok = written.is_ok();
            r.mesh.saved.send_replace((asked, ok));
            if ok {
                backoff = Duration::from_secs(1);
            } else {
                // Still dirty: try again, less often while it keeps failing.
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(30));
                r.mesh.dirty.notify_one();
            }
        }
    });
}

/// The peers `node` may talk to.
fn peers_for(
    relay: &Relay,
    node: &Node,
    nodes: &HashMap<PublicKey, Node>,
    online: &HashSet<PublicKey>,
    pairs: &BTreeSet<(String, PublicKey)>,
) -> Vec<PeerInfo> {
    let mut out = Vec::new();
    match (&node.owner, node.kind) {
        (Owner::Machine { id, registrant }, NodeKind::Machine) => {
            let record = registrant
                .starts_with("account:")
                .then(|| relay.directory.get(id))
                .flatten();
            for c in nodes.values().filter(|c| c.kind == NodeKind::Client) {
                let paired = pairs.contains(&(id.clone(), c.node_key));
                let device = match &c.owner {
                    Owner::Account { device, .. } => device.as_deref(),
                    _ => None,
                };
                let grant = match (&record, c.identity()) {
                    (Some(r), Some(who)) if r.sharing && client_device_ok(relay, c) => {
                        mesh_role(relay, r, &who, device).map(|role| Grant {
                            user: who.user.clone(),
                            account: who.account.clone(),
                            email: who.email.clone(),
                            name: who.name.clone(),
                            role: role.as_str().to_owned(),
                            device: device.map(str::to_owned),
                        })
                    }
                    _ => None,
                };
                if grant.is_some() {
                    out.push(c.info(online.contains(&c.node_key), grant));
                } else if paired && record.is_none() {
                    out.push(c.info(online.contains(&c.node_key), None));
                }
            }
            // The account's other machines (CUA-1783): its own hosts reach
            // each other (a gang's rank traffic), never another account's,
            // never a static machine, and never a Space a host provides.
            if let Some(r) = record.as_ref().filter(|r| r.host.is_none()) {
                for m in nodes
                    .values()
                    .filter(|m| m.kind == NodeKind::Machine && m.node_key != node.node_key)
                {
                    let Owner::Machine {
                        id: other,
                        registrant: theirs,
                    } = &m.owner
                    else {
                        continue;
                    };
                    if theirs != registrant || other == id {
                        continue;
                    }
                    let Some(o) = relay.directory.get(other) else {
                        continue;
                    };
                    if o.owner.id != r.owner.id || o.host.is_some() {
                        continue;
                    }
                    out.push(m.info(
                        online.contains(&m.node_key),
                        Some(Grant {
                            user: r.owner.id.clone(),
                            account: r.owner.id.clone(),
                            email: r.owner.email.clone(),
                            name: Some(o.name.clone()),
                            role: proto::ROLE_MACHINE.to_owned(),
                            device: None,
                        }),
                    ));
                }
            }
        }
        (_, NodeKind::Client) => {
            let who = node.identity();
            let device_ok = who.is_none() || client_device_ok(relay, node);
            let device = match &node.owner {
                Owner::Account { device, .. } => device.as_deref(),
                _ => None,
            };
            for m in nodes.values().filter(|m| m.kind == NodeKind::Machine) {
                let Owner::Machine { id, registrant } = &m.owner else {
                    continue;
                };
                let allowed = if registrant.starts_with("account:") {
                    device_ok
                        && who.as_ref().is_some_and(|who| {
                            relay.directory.get(id).is_some_and(|r| {
                                r.sharing && mesh_role(relay, &r, who, device).is_some()
                            })
                        })
                } else {
                    pairs.contains(&(id.clone(), node.node_key))
                };
                if allowed {
                    out.push(m.info(online.contains(&m.node_key), None));
                }
            }
        }
        _ => {}
    }
    out.sort_by_key(|p| p.node_key);
    out
}

/// `who`'s role on `record` for the mesh, by the HTTPS path's rules: its
/// own role (owner, share lists) or, through a team the machine is shared
/// with, only from a device a team admin approved (when devices are
/// enrolled at all).
fn mesh_role(
    relay: &Relay,
    record: &crate::directory::MachineRecord,
    who: &Identity,
    device: Option<&str>,
) -> Option<crate::directory::Role> {
    let (role, teams) = relay.machine_role(record, who)?;
    if teams.is_empty() || !relay.config.device_enrollment || relay.config.oidc.is_none() {
        return Some(role);
    }
    device
        .is_some_and(|d| {
            if relay.org.source() == crate::org_access::TeamsSource::Org {
                relay.org.cached_device_approved(&who.user, &teams, d)
            } else {
                relay.teams.device_approved_any(&teams, &who.account, d)
            }
        })
        .then_some(role)
}

/// An account client's device is still good (enrolled, not revoked, or
/// enrollment is off / in grace).
fn client_device_ok(relay: &Relay, node: &Node) -> bool {
    let Owner::Account { device, .. } = &node.owner else {
        return true;
    };
    if !relay.config.device_enrollment || relay.config.oidc.is_none() {
        return true;
    }
    match device {
        Some(d) => relay
            .devices
            .get(d)
            .is_some_and(|r| r.state(crate::assertion::now_secs()) == DeviceState::Enrolled),
        None => relay.devices.in_grace(crate::assertion::now_secs()),
    }
}

fn recompute(relay: &Relay) {
    let mesh = &relay.mesh;
    let nodes = mesh.nodes.lock().expect("nodes").clone();
    let pairs = mesh.pairs.lock().expect("pairs").clone();
    let mut conns = mesh.conns.lock().expect("conns");
    let online: HashSet<PublicKey> = conns.keys().copied().collect();
    let mut cut = Vec::new();
    let denied = {
        let mut d = mesh.denied.lock().expect("denied");
        let now = Instant::now();
        d.retain(|_, until| *until > now);
        d.clone()
    };
    for (key, conn) in conns.iter_mut() {
        let Some(node) = nodes.get(key) else { continue };
        if matches!(node.owner, Owner::Account { .. })
            && (!client_device_ok(relay, node) || conn.since.elapsed() > MAX_ACCOUNT_SESSION)
        {
            cut.push(*key);
            continue;
        }
        let mut peers = peers_for(relay, node, &nodes, &online, &pairs);
        // A peer whose new session an enforced plan refused is left out of
        // the machine's map for a while: the machine drops it (no WireGuard
        // session, no relayed packets) and the peer is told the plan's
        // limit nowhere else, so the same rule holds for a modified client.
        peers.retain(|p| !denied.contains_key(&(*key, p.node_key)));
        let json = serde_json::to_string(&ServerMsg::Netmap {
            peers: peers.clone(),
        })
        .expect("netmap json");
        let mut h = std::collections::hash_map::DefaultHasher::new();
        json.hash(&mut h);
        let hash = h.finish();
        *conn.allowed.write().expect("allowed") = peers.iter().map(|p| p.node_key).collect();
        if hash != conn.netmap_hash {
            conn.netmap_hash = hash;
            let _ = conn.tx.try_send(Message::Text(json.into()));
        }
    }
    for key in cut {
        if let Some(conn) = conns.remove(&key) {
            let _ = conn.tx.try_send(Message::Close(None));
        }
    }
    drop(conns);
    let now = Instant::now();
    mesh.pending
        .lock()
        .expect("pending")
        .retain(|_, p| p.expires > now);
    mesh.pair_seen
        .lock()
        .expect("pair seen")
        .retain(|_, t| now.saturating_duration_since(*t) < PAIR_FORGET);
}

fn pair(a: PublicKey, b: PublicKey) -> (PublicKey, PublicKey) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

/// One metered mesh peer of a machine connection.
struct MeteredPeer {
    user: String,
    streams: Vec<MeterStream>,
}

/// What one machine connection's usage reports hold open in the meter.
/// Dropped with the connection: every stream it reported closes.
struct MachineUsage {
    mesh: Arc<MeshState>,
    machine: String,
    peers: HashMap<PublicKey, MeteredPeer>,
    /// Warned about an ignored entry (once per connection).
    warned: bool,
}

impl MachineUsage {
    fn streams(&self) -> u64 {
        self.peers.values().map(|p| p.streams.len() as u64).sum()
    }

    /// Applies a full snapshot from the machine.
    fn apply(
        &mut self,
        relay: &Relay,
        allowed: &RwLock<HashSet<PublicKey>>,
        node_key: &PublicKey,
        peers: Vec<proto::PeerUsage>,
    ) {
        let mesh = &relay.mesh;
        let before = self.streams();
        let mut seen = HashSet::new();
        let mut rejected = 0u64;
        for entry in peers.into_iter().take(proto::MAX_USAGE_PEERS) {
            if !seen.insert(entry.peer) {
                continue;
            }
            // Only a peer the relay put in this machine's map (owner, share
            // list, approved team device), and only as that peer's own
            // account: a machine cannot charge anyone else.
            let who = allowed
                .read()
                .expect("allowed")
                .contains(&entry.peer)
                .then(|| {
                    mesh.nodes
                        .lock()
                        .expect("nodes")
                        .get(&entry.peer)
                        .and_then(Node::identity)
                })
                .flatten()
                .filter(|who| who.user == entry.user);
            let Some(who) = who else {
                rejected += 1;
                seen.remove(&entry.peer);
                continue;
            };
            let open = entry.open.min(MAX_OPEN_PER_PEER) as usize;
            let p = self.peers.entry(entry.peer).or_insert_with(|| MeteredPeer {
                user: who.user.clone(),
                streams: Vec::new(),
            });
            while p.streams.len() < open {
                match relay.meter.enter_mesh_enforced(&who, &self.machine) {
                    Ok(Some(s)) => p.streams.push(s),
                    Ok(None) => break,
                    Err(refusal) => {
                        // A new session past an enforced plan's cap: leave
                        // the peer out of the machine's map so the machine
                        // drops it. Sessions already held are never here.
                        mesh.denied
                            .lock()
                            .expect("denied")
                            .insert((*node_key, entry.peer), Instant::now() + DENY_FOR);
                        mesh.sessions_refused.fetch_add(1, Ordering::Relaxed);
                        mesh.changed();
                        tracing::info!(
                            machine = %self.machine, peer = %entry.peer.short(),
                            code = refusal.code, "mesh session refused by the plan limit"
                        );
                        break;
                    }
                }
            }
            p.streams.truncate(open);
            if entry.live && !p.streams.is_empty() {
                relay.meter.mark_live(&who.user, &self.machine);
            }
        }
        // Peers the snapshot no longer names close.
        self.peers
            .retain(|k, p| seen.contains(k) && !p.streams.is_empty());
        let after = self.streams();
        mesh.metered_streams.fetch_add(after, Ordering::Relaxed);
        mesh.metered_streams.fetch_sub(before, Ordering::Relaxed);
        if rejected > 0 {
            mesh.usage_rejected.fetch_add(rejected, Ordering::Relaxed);
            if !std::mem::replace(&mut self.warned, true) {
                tracing::warn!(
                    machine = %self.machine, rejected,
                    "mesh usage report names peers outside this machine's map; ignored"
                );
            }
        }
    }

    /// Relayed packets between this machine and a metered peer: those
    /// sessions cross the relay (relay hours).
    fn attribute(&self, relay: &Relay, machine_key: &PublicKey) {
        if self.peers.is_empty() {
            return;
        }
        let seen = relay.mesh.pair_seen.lock().expect("pair seen");
        for (peer, p) in &self.peers {
            if let Some(at) = seen.get(&pair(*machine_key, *peer)) {
                relay.meter.relayed(&p.user, &self.machine, *at);
            }
        }
    }
}

impl Drop for MachineUsage {
    fn drop(&mut self) {
        // The streams close with their guards; the gauge follows.
        self.mesh
            .metered_streams
            .fetch_sub(self.streams(), Ordering::Relaxed);
    }
}

fn send(tx: &mpsc::Sender<Message>, msg: &ServerMsg) {
    let _ = tx.try_send(Message::Text(
        serde_json::to_string(msg).expect("json").into(),
    ));
}

fn refuse(status: StatusCode, message: &str) -> Response {
    (status, message.to_owned()).into_response()
}

/// `GET /v1/mesh/connect`.
pub(crate) async fn connect(
    State(relay): State<Relay>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    if !relay.config.mesh.enabled {
        return refuse(StatusCode::NOT_FOUND, "this relay has no mesh coordinator");
    }
    let owner = match authenticate(&relay, &headers).await {
        Ok(o) => o,
        Err(r) => return r,
    };
    upgrade
        .max_message_size(proto::MAX_RELAY_PACKET + 64)
        .on_upgrade(move |socket| async move { session(relay, owner, socket).await })
}

#[allow(clippy::result_large_err)]
async fn authenticate(relay: &Relay, headers: &HeaderMap) -> Result<Owner, Response> {
    if let Some(id) = headers
        .get(proto::MACHINE_ID_HEADER)
        .and_then(|v| v.to_str().ok())
    {
        if !valid_machine_id(id) {
            return Err(refuse(StatusCode::BAD_REQUEST, "invalid machine id"));
        }
        if let Some(index) = relay.token_index(headers) {
            if relay.directory.get(id).is_some() {
                return Err(refuse(
                    StatusCode::CONFLICT,
                    "machine id is registered to an account",
                ));
            }
            if relay
                .machine(id)
                .is_some_and(|m| m.registrant != Registrant::Static(index))
            {
                return Err(refuse(
                    StatusCode::CONFLICT,
                    "machine id joined with another token",
                ));
            }
            return Ok(Owner::Machine {
                id: id.to_owned(),
                registrant: format!("static:{index}"),
            });
        }
        let record = bearer(headers, header::AUTHORIZATION.as_str())
            .filter(|t| t.starts_with(MACHINE_TOKEN_PREFIX))
            .and_then(|t| relay.directory.machine_for_token(t));
        return match record {
            Some(r) if r.id == id => Ok(Owner::Machine {
                id: id.to_owned(),
                registrant: format!("account:{}", r.owner.id),
            }),
            Some(_) => Err(refuse(
                StatusCode::FORBIDDEN,
                "machine token belongs to another machine id",
            )),
            None => Err(refuse(StatusCode::UNAUTHORIZED, "invalid machine token")),
        };
    }
    // A client device.
    match (account_token(headers), &relay.config.oidc) {
        (Some(token), Some(oidc)) => {
            let who = oidc.validate(token).await.map_err(|e| {
                refuse(
                    StatusCode::UNAUTHORIZED,
                    &format!("invalid account token: {e}"),
                )
            })?;
            let gate =
                crate::device_api::gate(relay, crate::device_api::session_header(headers), &who)
                    .map_err(|m| refuse(StatusCode::FORBIDDEN, &m))?;
            Ok(Owner::Account {
                user: who.user.clone(),
                account: who.account.clone(),
                email: who.email.clone(),
                name: who.name.clone(),
                device: gate.device().map(str::to_owned),
            })
        }
        // Static-token relays: anyone may connect, and peers with nothing
        // until a machine accepts its pairing proof.
        _ if !relay.config.tokens.is_empty() => {
            if relay.mesh.anonymous() >= MAX_ANONYMOUS {
                return Err(refuse(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "too many unpaired mesh clients; retry later",
                ));
            }
            Ok(Owner::Static)
        }
        _ => Err(refuse(
            StatusCode::UNAUTHORIZED,
            "sign in: this relay accepts account tokens",
        )),
    }
}

async fn session(relay: Relay, owner: Owner, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();
    let server = SecretKey::generate();
    let nonce = hex::encode(rand::random::<[u8; 16]>());
    let challenge = ServerMsg::Challenge {
        version: proto::VERSION,
        server_key: server.public(),
        nonce: nonce.clone(),
    };
    if sink
        .send(Message::Text(
            serde_json::to_string(&challenge).expect("json").into(),
        ))
        .await
        .is_err()
    {
        return;
    }
    let hello = match tokio::time::timeout(Duration::from_secs(10), stream.next()).await {
        Ok(Some(Ok(Message::Text(t)))) => serde_json::from_str::<ClientMsg>(t.as_str()).ok(),
        _ => None,
    };
    let Some(ClientMsg::Hello {
        version,
        kind,
        node_key,
        disco_key,
        quic_pin,
        name,
        endpoints,
        proof,
        ephemeral,
    }) = hello
    else {
        return;
    };
    let error = |message: &str| {
        Message::Text(
            serde_json::to_string(&ServerMsg::Error {
                message: message.to_owned(),
            })
            .expect("json")
            .into(),
        )
    };
    if version != proto::VERSION {
        let _ = sink
            .send(error(&format!(
                "this relay speaks mesh v{}; the node speaks v{version}",
                proto::VERSION
            )))
            .await;
        return;
    }
    if !proto::verify_hello_proof(&server, &node_key, &nonce, &proof) {
        let _ = sink.send(error("the node key proof does not verify")).await;
        return;
    }
    let expected_kind = match owner {
        Owner::Machine { .. } => NodeKind::Machine,
        _ => NodeKind::Client,
    };
    if kind != expected_kind {
        let _ = sink
            .send(error("node kind does not match its credentials"))
            .await;
        return;
    }
    let mesh = &relay.mesh;
    let (conflict, fresh) = {
        let mut nodes = mesh.nodes.lock().expect("nodes");
        // A key belongs to one owner (a device or a machine) for life.
        let conflict =
            nodes
                .get(&node_key)
                .is_some_and(|existing| !match (&existing.owner, &owner) {
                    (Owner::Machine { id: a, .. }, Owner::Machine { id: b, .. }) => a == b,
                    (Owner::Account { account: a, .. }, Owner::Account { account: b, .. }) => {
                        a == b
                    }
                    (Owner::Static, Owner::Static) => true,
                    _ => false,
                });
        // Whether what the registry keeps of this node changes (not only
        // when it was last seen or where it is).
        let mut fresh = nodes.get(&node_key).is_none_or(|old| {
            old.disco_key != disco_key
                || old.quic_pin != quic_pin
                || old.kind != kind
                || old.owner != owner
                || old.name != name.chars().take(64).collect::<String>()
                || old.version != version
                || old.ephemeral != ephemeral
        });
        if !conflict {
            if let Owner::Machine { id, .. } = &owner {
                // One node per machine: a new key replaces the old one.
                let before = nodes.len();
                nodes.retain(|k, n| *k == node_key || n.machine_id() != Some(id.as_str()));
                fresh |= nodes.len() != before;
            }
            nodes.insert(
                node_key,
                Node {
                    node_key,
                    disco_key,
                    quic_pin,
                    kind,
                    owner: owner.clone(),
                    name: name.chars().take(64).collect(),
                    endpoints: endpoints.into_iter().take(32).collect(),
                    version,
                    last_seen: crate::assertion::now_secs(),
                    ephemeral,
                },
            );
        }
        (conflict, fresh)
    };
    if conflict {
        let _ = sink
            .send(error("this node key belongs to another owner"))
            .await;
        return;
    }
    // A new key or owner is saved before the node is welcomed, so a crash
    // right after cannot forget it (endpoint updates are batched instead).
    // A failed save is reported by the saver; the node is welcomed anyway:
    // it registers again whenever it reconnects.
    if fresh {
        if !mesh.persist().await {
            tracing::warn!(node = %node_key, "mesh node welcomed before it could be saved");
        }
    } else {
        mesh.dirty.notify_one();
    }
    let mode = match &owner {
        Owner::Machine { registrant, .. } if registrant.starts_with("static:") => Mode::Static,
        Owner::Static => Mode::Static,
        _ => Mode::Account,
    };
    // Account machines report their peers' streams for the meter (a
    // relay that meters nothing asks for none).
    let account_machine =
        matches!(&owner, Owner::Machine { registrant, .. } if registrant.starts_with("account:"));
    let meters = account_machine && relay.config.oidc.is_some() && relay.meter.config().enabled;
    let welcome = ServerMsg::Welcome {
        version: proto::VERSION,
        addr: node_key.mesh_addr(),
        mode,
        stun: relay.config.mesh.stun.clone(),
        usage: if meters { proto::USAGE_VERSION } else { 0 },
    };
    if sink
        .send(Message::Text(
            serde_json::to_string(&welcome).expect("json").into(),
        ))
        .await
        .is_err()
    {
        return;
    }
    if let Owner::Account {
        account, device, ..
    } = &owner
    {
        relay.devices.audit_throttled(
            &format!("mesh:{}", node_key.short()),
            account,
            AuditEvent::new("mesh_connected")
                .device(device.as_deref())
                .detail(format!("node {}", node_key.short())),
        );
    }
    let (tx, mut rx) = mpsc::channel::<Message>(QUEUE);
    let generation = mesh.generation.fetch_add(1, Ordering::SeqCst);
    let allowed: Arc<RwLock<HashSet<PublicKey>>> = Arc::default();
    if let Some(old) = mesh.conns.lock().expect("conns").insert(
        node_key,
        Conn {
            tx: tx.clone(),
            generation,
            since: Instant::now(),
            allowed: allowed.clone(),
            netmap_hash: 0,
        },
    ) {
        // The same node reconnected: the newer connection wins.
        let _ = old.tx.try_send(Message::Close(None));
    }
    tracing::info!(node = %node_key.short(), ?kind, "mesh node connected");
    mesh.changed();
    let writer = tokio::spawn(async move {
        while let Some(m) = rx.recv().await {
            let close = matches!(m, Message::Close(_));
            if sink.send(m).await.is_err() || close {
                return;
            }
        }
    });

    let mut usage = meters.then(|| MachineUsage {
        mesh: mesh.clone(),
        machine: match &owner {
            Owner::Machine { id, .. } => id.clone(),
            _ => String::new(),
        },
        peers: HashMap::new(),
        warned: false,
    });
    let mut marks: HashMap<PublicKey, Instant> = HashMap::new();
    let mut attribute = tokio::time::interval(ATTRIBUTE_EVERY);
    attribute.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut heard = tokio::time::Instant::now();
    loop {
        let metering = usage.as_ref().is_some_and(|u| !u.peers.is_empty());
        let msg = tokio::select! {
            m = tokio::time::timeout_at(heard + IDLE, stream.next()) => match m {
                Ok(Some(Ok(m))) => m,
                _ => break,
            },
            _ = attribute.tick(), if metering => {
                if let Some(u) = &usage {
                    u.attribute(&relay, &node_key);
                }
                continue;
            }
        };
        heard = tokio::time::Instant::now();
        match msg {
            Message::Binary(b) => relay_packet(&relay, &node_key, &allowed, &mut marks, &b),
            Message::Text(t) => {
                let Ok(msg) = serde_json::from_str::<ClientMsg>(t.as_str()) else {
                    continue;
                };
                if let ClientMsg::Usage { v, peers } = msg {
                    // Unknown versions are ignored (a newer machine).
                    if v == proto::USAGE_VERSION {
                        mesh.usage_reports.fetch_add(1, Ordering::Relaxed);
                        match &mut usage {
                            Some(u) => {
                                u.apply(&relay, &allowed, &node_key, peers);
                                u.attribute(&relay, &node_key);
                            }
                            None => {
                                mesh.usage_rejected
                                    .fetch_add(peers.len() as u64, Ordering::Relaxed);
                            }
                        }
                    }
                    continue;
                }
                control(&relay, &node_key, &owner, &tx, msg);
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
    drop(usage);
    writer.abort();
    {
        let mut conns = mesh.conns.lock().expect("conns");
        if conns
            .get(&node_key)
            .is_some_and(|c| c.generation == generation)
        {
            conns.remove(&node_key);
        }
    }
    {
        // The same key may have reconnected meanwhile: then keep it.
        let live = mesh.conns.lock().expect("conns").contains_key(&node_key);
        let paired = mesh
            .pairs
            .lock()
            .expect("pairs")
            .iter()
            .any(|(_, c)| *c == node_key);
        let mut nodes = mesh.nodes.lock().expect("nodes");
        // A short-lived node, and an anonymous static-relay client no
        // machine accepted, are forgotten when they leave.
        let forget = ephemeral || (owner == Owner::Static && !paired);
        if forget && !live {
            nodes.remove(&node_key);
        } else if let Some(n) = nodes.get_mut(&node_key) {
            n.last_seen = crate::assertion::now_secs();
        }
    }
    mesh.dirty.notify_one();
    mesh.changed();
    tracing::info!(node = %node_key.short(), "mesh node disconnected");
}

fn relay_packet(
    relay: &Relay,
    src: &PublicKey,
    allowed: &RwLock<HashSet<PublicKey>>,
    marks: &mut HashMap<PublicKey, Instant>,
    b: &[u8],
) {
    let mesh = &relay.mesh;
    let Some((proto::FRAME_SEND, dst, packet)) = proto::parse_frame(b) else {
        return;
    };
    if !relay.config.mesh.relay
        || packet.len() > proto::MAX_RELAY_PACKET
        || !allowed.read().expect("allowed").contains(&dst)
    {
        mesh.dropped.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let target = mesh
        .conns
        .lock()
        .expect("conns")
        .get(&dst)
        .map(|c| c.tx.clone());
    match target {
        Some(t) => {
            let frame = proto::frame(proto::FRAME_RECV, src, packet);
            if t.try_send(Message::Binary(frame.into())).is_ok() {
                mesh.relayed_bytes
                    .fetch_add(packet.len() as u64, Ordering::Relaxed);
                // WireGuard traffic of the pair crossed the relay (disco
                // pings only look for a direct path): noted at most once a
                // second per destination, for relay hours.
                if !cua_mesh::disco::is_disco(packet) {
                    let now = Instant::now();
                    let due = marks
                        .get(&dst)
                        .is_none_or(|t| now.saturating_duration_since(*t) >= PAIR_MARK_EVERY);
                    if due {
                        if marks.len() > 4096 {
                            marks.clear();
                        }
                        marks.insert(dst, now);
                        mesh.pair_seen
                            .lock()
                            .expect("pair seen")
                            .insert(pair(*src, dst), now);
                    }
                }
            } else {
                mesh.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
        None => {
            mesh.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

fn control(
    relay: &Relay,
    me: &PublicKey,
    owner: &Owner,
    tx: &mpsc::Sender<Message>,
    msg: ClientMsg,
) {
    let mesh = &relay.mesh;
    match msg {
        ClientMsg::Ping => send(tx, &ServerMsg::Pong),
        ClientMsg::Endpoints { endpoints } => {
            if let Some(n) = mesh.nodes.lock().expect("nodes").get_mut(me) {
                n.endpoints = endpoints.into_iter().take(32).collect();
                n.last_seen = crate::assertion::now_secs();
            }
            mesh.dirty.notify_one();
            mesh.changed();
        }
        ClientMsg::Pair {
            machine_id,
            ts,
            proof,
        } => {
            if !matches!(owner, Owner::Static | Owner::Account { .. }) {
                return;
            }
            let fail = |reason: &str| {
                send(
                    tx,
                    &ServerMsg::PairResult {
                        machine_id: machine_id.clone(),
                        ok: false,
                        reason: reason.to_owned(),
                    },
                )
            };
            let machine = mesh
                .nodes
                .lock()
                .expect("nodes")
                .values()
                .find(|n| n.machine_id() == Some(machine_id.as_str()))
                .cloned();
            let Some(machine) = machine else {
                return fail("that machine is not on the mesh");
            };
            if matches!(&machine.owner, Owner::Machine { registrant, .. } if registrant.starts_with("account:"))
            {
                return fail("an account machine is shared from its owner's account, not paired");
            }
            let conn = mesh
                .conns
                .lock()
                .expect("conns")
                .get(&machine.node_key)
                .map(|c| c.tx.clone());
            let Some(conn) = conn else {
                return fail("that machine is offline");
            };
            let id = hex::encode(rand::random::<[u8; 12]>());
            {
                let mut pending = mesh.pending.lock().expect("pending");
                if pending.len() > 1024 {
                    return fail("too many pairing requests");
                }
                pending.insert(
                    id.clone(),
                    PendingPair {
                        client: *me,
                        machine_id: machine_id.clone(),
                        expires: Instant::now() + Duration::from_secs(30),
                    },
                );
            }
            let name = mesh
                .nodes
                .lock()
                .expect("nodes")
                .get(me)
                .map(|n| n.name.clone())
                .unwrap_or_default();
            send(
                &conn,
                &ServerMsg::PairRequest {
                    request_id: id,
                    client_key: *me,
                    ts,
                    proof,
                    name,
                },
            );
        }
        ClientMsg::PairReply {
            request_id,
            accept,
            reason,
        } => {
            let Owner::Machine { id, .. } = owner else {
                return;
            };
            let Some(p) = mesh.pending.lock().expect("pending").remove(&request_id) else {
                return;
            };
            if &p.machine_id != id {
                return;
            }
            if accept {
                mesh.pairs
                    .lock()
                    .expect("pairs")
                    .insert((id.clone(), p.client));
                // Saved before the client hears about it: a pairing the
                // relay could not save is not confirmed.
                let relay = relay.clone();
                let id = id.clone();
                tokio::spawn(async move {
                    let mesh = &relay.mesh;
                    let saved = mesh.persist().await;
                    if !saved {
                        mesh.pairs
                            .lock()
                            .expect("pairs")
                            .remove(&(id.clone(), p.client));
                    }
                    mesh.changed();
                    let client = mesh
                        .conns
                        .lock()
                        .expect("conns")
                        .get(&p.client)
                        .map(|c| c.tx.clone());
                    if let Some(c) = client {
                        send(
                            &c,
                            &ServerMsg::PairResult {
                                machine_id: id,
                                ok: saved,
                                reason: if saved {
                                    reason
                                } else {
                                    "the relay could not save the pairing; try again".into()
                                },
                            },
                        );
                    }
                });
                return;
            }
            let client = mesh
                .conns
                .lock()
                .expect("conns")
                .get(&p.client)
                .map(|c| c.tx.clone());
            if let Some(c) = client {
                send(
                    &c,
                    &ServerMsg::PairResult {
                        machine_id: id.clone(),
                        ok: accept,
                        reason,
                    },
                );
            }
        }
        ClientMsg::Hello { .. } | ClientMsg::Usage { .. } => {}
    }
}

/// Answers STUN binding requests on `sock` until it fails.
pub async fn serve_stun(sock: UdpSocket) {
    let mut buf = [0u8; 1500];
    loop {
        let (n, src) = match sock.recv_from(&mut buf).await {
            Ok(v) => v,
            Err(e) => {
                tracing::debug!(error = %e, "stun recv");
                continue;
            }
        };
        if let Some(tx) = cua_mesh::stun::parse_request(&buf[..n]) {
            let _ = sock.send_to(&cua_mesh::stun::response(tx, src), src).await;
        }
    }
}

/// `GET /relay/v1/mesh` (admin token): what the coordinator carries.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MeshStatus {
    /// Nodes in the registry.
    pub nodes: usize,
    /// Machines among them.
    pub machines: usize,
    /// Nodes connected now.
    pub connected: usize,
    /// Static pairings.
    pub pairings: usize,
    /// Ciphertext bytes relayed since start.
    pub relayed_bytes: u64,
    /// Relayed packets dropped (not a peer, offline, slow) since start.
    pub dropped: u64,
    /// Mesh streams machines report open now (metered, CUA-1506).
    #[serde(default)]
    pub metered_streams: u64,
    /// Usage reports received since start.
    #[serde(default)]
    pub usage_reports: u64,
    /// Report entries ignored since start (a peer outside the machine's
    /// map, or a user that is not the peer's own).
    #[serde(default)]
    pub usage_rejected: u64,
    /// New mesh sessions refused by an enforced plan's cap.
    pub sessions_refused: u64,
}

/// The coordinator's counters.
pub fn status(relay: &Relay) -> MeshStatus {
    let mesh = &relay.mesh;
    let nodes = mesh.nodes.lock().expect("nodes");
    MeshStatus {
        nodes: nodes.len(),
        machines: nodes
            .values()
            .filter(|n| n.kind == NodeKind::Machine)
            .count(),
        connected: mesh.connected(),
        pairings: mesh.pairs.lock().expect("pairs").len(),
        relayed_bytes: mesh.relayed_bytes.load(Ordering::Relaxed),
        dropped: mesh.dropped.load(Ordering::Relaxed),
        metered_streams: mesh.metered_streams.load(Ordering::Relaxed),
        usage_reports: mesh.usage_reports.load(Ordering::Relaxed),
        usage_rejected: mesh.usage_rejected.load(Ordering::Relaxed),
        sessions_refused: mesh.sessions_refused.load(Ordering::Relaxed),
    }
}

/// What `/v1/info` says about the mesh.
pub(crate) fn info(relay: &Relay) -> Option<proto::MeshInfo> {
    relay.config.mesh.enabled.then(|| proto::MeshInfo {
        version: proto::VERSION,
        stun: relay.config.mesh.stun.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_is_written_compact_and_read_back_and_temps_are_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("relay.mesh.json");
        let stale = dir.path().join(format!(
            ".relay.mesh.json.{}.tmp",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::write(&stale, vec![b'{'; 4096]).unwrap();
        let storage = Arc::new(Storage::new(Some(dir.path().into()), Default::default()));
        let mesh = MeshState::open_with(Some(path.clone()), storage.clone()).unwrap();
        assert!(!stale.exists(), "an interrupted write's temp is removed");
        let key = SecretKey::generate().public();
        mesh.nodes.lock().unwrap().insert(
            key,
            Node {
                node_key: key,
                disco_key: SecretKey::generate().public(),
                quic_pin: "pin".into(),
                kind: NodeKind::Machine,
                owner: Owner::Machine {
                    id: "machine-0001".into(),
                    registrant: "account:ada".into(),
                },
                name: "m".into(),
                endpoints: vec![],
                version: 1,
                last_seen: crate::assertion::now_secs(),
                ephemeral: false,
            },
        );
        mesh.save_now().unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains('\n'), "compact JSON: {raw}");
        let again = MeshState::open(Some(path)).unwrap();
        assert!(again.nodes.lock().unwrap().contains_key(&key));
    }

    #[test]
    fn endpoints_are_not_persisted_and_stale_ones_are_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("relay.mesh.json");
        let mesh = MeshState::open(Some(path.clone())).unwrap();
        let day = 86_400;
        let now = crate::assertion::now_secs();
        let ep: SocketAddr = "203.0.113.9:41641".parse().unwrap();
        let mut keys = Vec::new();
        for (name, age_days) in [("fresh", 1u64), ("stale", 10), ("live", 10)] {
            let key = SecretKey::generate().public();
            keys.push(key);
            mesh.nodes.lock().unwrap().insert(
                key,
                Node {
                    node_key: key,
                    disco_key: SecretKey::generate().public(),
                    quic_pin: "pin".into(),
                    kind: NodeKind::Machine,
                    owner: Owner::Machine {
                        id: format!("machine-{name}"),
                        registrant: "account:ada".into(),
                    },
                    name: name.into(),
                    endpoints: vec![ep],
                    version: 1,
                    last_seen: now - age_days * day,
                    ephemeral: false,
                },
            );
        }
        // "live" is connected: its old last_seen does not matter.
        let (tx, _rx) = mpsc::channel(1);
        mesh.conns.lock().unwrap().insert(
            keys[2],
            Conn {
                tx,
                generation: 1,
                since: Instant::now(),
                allowed: Arc::default(),
                netmap_hash: 0,
            },
        );
        assert_eq!(mesh.forget_stale_endpoints(0), 0);
        assert_eq!(mesh.forget_stale_endpoints(7), 1);
        let nodes = mesh.nodes.lock().unwrap();
        assert!(nodes[&keys[0]].endpoints == vec![ep], "recent keeps them");
        assert!(nodes[&keys[1]].endpoints.is_empty(), "stale loses them");
        assert!(
            nodes[&keys[2]].endpoints == vec![ep],
            "connected keeps them"
        );
        assert_eq!(nodes[&keys[1]].name, "stale", "the registration stays");
        drop(nodes);
        // Nothing the node reported reaches the disk.
        mesh.save_now().unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(
            !raw.contains("203.0.113.9") && !raw.contains("endpoints"),
            "{raw}"
        );
        // A registry written by an older relay, with endpoints, loads without
        // them.
        let legacy = raw.replacen(
            "\"version\":1",
            "\"endpoints\":[\"203.0.113.9:41641\"],\"version\":1",
            1,
        );
        std::fs::write(&path, legacy).unwrap();
        let again = MeshState::open(Some(path)).unwrap();
        assert!(again
            .nodes
            .lock()
            .unwrap()
            .values()
            .all(|n| n.endpoints.is_empty()));
    }

    #[cfg(feature = "hosted")]
    fn node(kind: NodeKind, owner: Owner) -> Node {
        Node {
            node_key: SecretKey::generate().public(),
            disco_key: SecretKey::generate().public(),
            quic_pin: "pin".into(),
            kind,
            owner,
            name: "n".into(),
            endpoints: vec![],
            version: 1,
            last_seen: crate::assertion::now_secs(),
            ephemeral: false,
        }
    }

    // Plan limits are an optional hosted-relay feature (`hosted`).
    #[cfg(feature = "hosted")]
    #[tokio::test]
    async fn a_mesh_session_past_an_enforced_cap_is_dropped_by_the_machine_never_a_held_one() {
        use crate::meter::{parse_allowances, parse_plan_set, MeterConfig};
        let relay = Relay::new(crate::server::RelayConfig {
            meter: MeterConfig {
                allowances: parse_allowances("free=1/20").unwrap(),
                enforce_plans: parse_plan_set("free"),
                live_after: Duration::ZERO,
                ..MeterConfig::default()
            },
            ..crate::server::RelayConfig::default()
        });
        let mesh = relay.mesh.clone();
        let ada = node(
            NodeKind::Client,
            Owner::Account {
                user: "ada".into(),
                account: "ada".into(),
                email: Some("ada@example.com".into()),
                name: None,
                device: None,
            },
        );
        let mut machines = Vec::new();
        for i in 0..2 {
            let m = node(
                NodeKind::Machine,
                Owner::Machine {
                    id: format!("machine-{i}"),
                    registrant: "static:0".into(),
                },
            );
            mesh.pairs
                .lock()
                .unwrap()
                .insert((format!("machine-{i}"), ada.node_key));
            let allowed: Arc<RwLock<HashSet<PublicKey>>> = Arc::default();
            mesh.conns.lock().unwrap().insert(
                m.node_key,
                Conn {
                    tx: mpsc::channel(8).0,
                    generation: 1,
                    since: Instant::now(),
                    allowed: allowed.clone(),
                    netmap_hash: 0,
                },
            );
            mesh.nodes.lock().unwrap().insert(m.node_key, m.clone());
            let usage = MachineUsage {
                mesh: mesh.clone(),
                machine: format!("machine-{i}"),
                peers: HashMap::new(),
                warned: false,
            };
            machines.push((m, allowed, usage));
        }
        mesh.nodes.lock().unwrap().insert(ada.node_key, ada.clone());
        recompute(&relay);
        let report = || {
            vec![proto::PeerUsage {
                peer: ada.node_key,
                user: "ada".into(),
                open: 1,
                live: true,
            }]
        };

        // The first machine's session fits Free's one.
        let (m0, a0, u0) = &mut machines[0];
        u0.apply(&relay, a0, &m0.node_key, report());
        assert_eq!(u0.streams(), 1);
        // A second machine's new session is refused: nothing is metered, and
        // the machine's map (and relay path) no longer has the peer.
        let (m1, a1, u1) = &mut machines[1];
        assert!(a1.read().unwrap().contains(&ada.node_key));
        u1.apply(&relay, a1, &m1.node_key, report());
        assert_eq!(u1.streams(), 0);
        assert_eq!(status(&relay).sessions_refused, 1);
        recompute(&relay);
        assert!(!a1.read().unwrap().contains(&ada.node_key));
        // The held session is never cut: its machine still has the peer.
        let (m0, a0, u0) = &mut machines[0];
        u0.apply(&relay, a0, &m0.node_key, report());
        assert_eq!(u0.streams(), 1);
        assert!(a0.read().unwrap().contains(&ada.node_key));
        // The denial lapses, so the peer can ask again (and is refused again).
        for until in mesh.denied.lock().unwrap().values_mut() {
            *until = Instant::now();
        }
        recompute(&relay);
        assert!(machines[1].1.read().unwrap().contains(&ada.node_key));
    }
}
