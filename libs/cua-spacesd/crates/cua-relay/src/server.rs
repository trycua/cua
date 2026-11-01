// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The relay: machines register over WebSocket with a relay token; clients
//! reach them at `/m/<machine-id>/…` (path mode, prefix stripped) or
//! `<machine-id>.<domain>` (host mode). Every client request becomes one
//! yamux stream carrying HTTP/1.1 (including WebSocket upgrades) or h2
//! (gRPC) to the machine's spacesd.
//!
//! Two registration modes share one relay:
//! - **Static relay tokens** (self-hosting): machines register with a
//!   shared secret and clients present the env token end to end; the relay
//!   forwards it unmodified and never stores it, but it does parse it in
//!   transit like every request it proxies (TLS terminates in front of the
//!   relay). See the crate README's "What the relay can see".
//! - **Accounts** (cua.ai): machines are registered by a signed-in user
//!   (`POST /v1/machines`) and join with the machine token they got back.
//!   Clients present their account token; the relay checks ownership /
//!   allowlist / sharing, strips the client's credentials and forwards a
//!   short-lived relay-signed principal assertion instead (see
//!   [`crate::assertion`]). The machine directory API lives in
//!   [`crate::api`].

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use axum::body::Body;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode, Uri, Version};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use futures_util::io::{AsyncReadExt as _, AsyncWriteExt as _};
use hyper_util::rt::{TokioExecutor, TokioIo};
use serde::Serialize;
use tokio::sync::mpsc;
use tokio_util::compat::FuturesAsyncReadCompatExt as _;
use tokio_util::sync::CancellationToken;

use crate::assertion::{self, AssertionClaims, RelayKey, ASSERTION_HEADER};
use crate::auth_log::{self, Reason, Route};
use crate::devices::{DevicePolicy, DeviceStore, DEVICE_SESSION_HEADER};
use crate::directory::{Directory, MACHINE_TOKEN_PREFIX};
use crate::meter::{Meter, MeterStream};
use crate::mux::{self, Counted, Counters, MuxHandle};
use crate::oidc::{Identity, OidcValidator};
use crate::ws::WsIo;
use crate::{valid_machine_id, CONNECT_PATH, HEARTBEAT_BYTE, MACHINE_ID_HEADER, VERSION_HEADER};

/// Header a client may use for its account token instead of
/// `authorization` (so an env token can travel in `authorization`).
pub const RELAY_AUTHORIZATION_HEADER: &str = "x-cua-relay-authorization";
/// How long a client stays in a machine's presence list after its last
/// stream closed.
const PRESENCE_LINGER: Duration = Duration::from_secs(30);

/// Default for [`RelayConfig::audit_retention_days`].
pub const DEFAULT_AUDIT_RETENTION_DAYS: u64 = 400;
/// Default for [`RelayConfig::mesh_endpoint_retention_days`].
pub const DEFAULT_MESH_ENDPOINT_RETENTION_DAYS: u64 = 7;

/// Relay configuration.
#[derive(Debug, Clone)]
pub struct RelayConfig {
    /// Accepted machine-registration tokens.
    pub tokens: Vec<String>,
    /// Maximum machines connected per registration token.
    pub max_machines_per_token: usize,
    /// Machine joins admitted per second once the burst is used; the rest are
    /// refused with 503 and a `Retry-After`. 0: no limit. See [`crate::join_limit`].
    pub join_rate_per_sec: f64,
    /// Machine joins admitted at once.
    pub join_burst: u32,
    /// Maximum concurrent client streams per machine.
    pub max_streams_per_machine: u32,
    /// A machine without a heartbeat for this long is dropped.
    pub machine_idle_timeout: Duration,
    /// Host-mode domain: `<id>.<domain>` routes to machine `<id>`.
    pub host_domain: Option<String>,
    /// With a host domain, keep forwarding `/m/<id>/…` on the main origin
    /// (for clients that build path-mode URLs), except that a browser
    /// opening a path-mode share link is redirected to the share's own
    /// origin (see [`legacy_share_redirect`]). Off: a host domain turns
    /// path mode off (S6).
    pub keep_path_mode: bool,
    /// Token for `/relay/v1/machines`.
    pub admin_token: Option<String>,
    /// Refuse client requests that carry no spacesd credential
    /// (bearer header, ticket or signed URL). The relay cannot validate it;
    /// this only stops anonymous scanning from reaching machines.
    pub require_client_credentials: bool,
    /// Account mode: validates cua.ai account tokens. `None` disables the
    /// machine directory API and account machines.
    pub oidc: Option<Arc<OidcValidator>>,
    /// Where the machine directory is persisted (in memory when unset).
    pub state_file: Option<PathBuf>,
    /// Assertion signing key (a fresh one per process when unset).
    pub signing_key: Option<Arc<RelayKey>>,
    /// Public base URL (`https://relay.cua.ai`): assertion issuer and the
    /// machine URLs in the directory. Derived from `Host` when unset.
    pub public_url: Option<String>,
    /// Maximum machines one account may register.
    pub max_machines_per_account: usize,
    /// Account mode: client devices must be enrolled (with a second factor)
    /// to list or reach machines. See [`crate::devices`].
    pub device_enrollment: bool,
    /// Enrollment lifetime, migration grace period, bootstrap window and
    /// session lifetime.
    pub device_policy: DevicePolicy,
    /// Where devices and the audit log are persisted (default: next to the
    /// state file, `<state>.devices.json`; in memory without one).
    pub devices_file: Option<PathBuf>,
    /// Largest number of concurrent client streams across *every* machine
    /// at once (S7): a memory budget independent of how many machines are
    /// connected, enforced with backpressure (a new stream is refused, not
    /// buffered, once the budget is spent). Bytes are bounded per machine
    /// by `machine_window_bytes` × connected machines; this bounds them
    /// across all machines at once regardless of how many are connected, so
    /// one user's worth of machines and slow readers cannot, by itself,
    /// exhaust the pod and drop every other machine's connection with it.
    pub max_global_streams: u64,
    /// yamux's own per-machine-connection stream cap (a hard ceiling under
    /// `max_streams_per_machine`, which is the friendlier, app-level check
    /// in `proxy()`). Must satisfy `machine_window_bytes >= machine_max_streams * 256 KiB`.
    pub machine_max_streams: usize,
    /// yamux's per-machine connection receive window: the most one
    /// machine's connection buffers across all of its streams (S7).
    pub machine_window_bytes: usize,
    /// The mesh coordinator and encrypted relay (see [`crate::mesh`]).
    pub mesh: crate::mesh::MeshConfig,
    /// Where the mesh registry persists (default: next to the state file,
    /// `<state>.mesh.json`; in memory without one).
    pub mesh_file: Option<PathBuf>,
    /// Free-space reserve and per-account quota for pool writes (see
    /// [`crate::storage`]).
    pub storage_limits: crate::storage::StorageLimits,
    /// Allowance metering (account mode). Its usage file defaults to
    /// `<state>.usage.json` next to the state file.
    #[cfg(feature = "hosted")]
    pub meter: crate::meter::MeterConfig,
    /// Where team access comes from (`teams.json`, shadow, organizations)
    /// and how the Spaces API is reached.
    #[cfg(feature = "hosted")]
    pub org_access: crate::org_access::OrgAccessConfig,
    /// For this long after start, a request for a known machine that is
    /// not connected waits for it to reconnect instead of failing: the
    /// machines of the relay this one took over from are on their way
    /// (CUA-1774). Zero: fail at once.
    pub reconnect_grace: Duration,
    /// The OIDC web client behind the browser sign-in of `cua share`
    /// links (see [`crate::share_web`]). `None`: no browser sign-in.
    pub share_web: Option<crate::share_web::ShareWebConfig>,
    /// Rate limits for anonymous requests to public share links.
    pub share_limits: crate::share_web::ShareLimits,
    /// Key file of the analytics ids and log tags ([`crate::ident`]).
    /// `None`: next to the state file, or in memory without one.
    pub analytics_key_file: Option<PathBuf>,
    /// Audit entries older than this many days are dropped (0: only the
    /// per-account count limit applies).
    pub audit_retention_days: u64,
    /// A mesh node offline for more than this many days loses its stored
    /// endpoints (its key registration stays). 0: never.
    pub mesh_endpoint_retention_days: u64,
    /// Reverse proxies whose `X-Forwarded-For` is believed. Empty: never,
    /// the socket peer is the client address.
    pub trusted_proxies: crate::proxies::TrustedProxies,
}

impl Default for RelayConfig {
    fn default() -> Self {
        Self {
            tokens: Vec::new(),
            max_machines_per_token: 16,
            join_rate_per_sec: crate::join_limit::DEFAULT_JOIN_RATE_PER_SEC,
            join_burst: crate::join_limit::DEFAULT_JOIN_BURST,
            // Down from an earlier 1000 (S7): yamux's own per-connection
            // cap (machine_max_streams) is now the smaller crate::mux::
            // MAX_STREAMS, so this app-level check (which answers with a
            // friendlier error) should not be looser than it.
            max_streams_per_machine: crate::mux::MAX_STREAMS as u32,
            machine_idle_timeout: Duration::from_secs(90),
            host_domain: None,
            keep_path_mode: false,
            admin_token: None,
            require_client_credentials: true,
            oidc: None,
            state_file: None,
            signing_key: None,
            public_url: None,
            max_machines_per_account: 32,
            device_enrollment: true,
            device_policy: DevicePolicy::default(),
            devices_file: None,
            // 4096 streams: at a default 256 KiB yamux starting window this
            // is at most ~1 GiB actively buffered relay-wide even if every
            // stream's window grew to the per-machine ceiling at once,
            // comfortably inside a 1 GiB pod alongside the rest of the
            // process. Tune to the deployed pod's actual memory limit.
            max_global_streams: 4096,
            machine_max_streams: crate::mux::MAX_STREAMS,
            machine_window_bytes: crate::mux::DEFAULT_WINDOW_BYTES,
            mesh: crate::mesh::MeshConfig::default(),
            mesh_file: None,
            storage_limits: crate::storage::StorageLimits::default(),
            #[cfg(feature = "hosted")]
            meter: crate::meter::MeterConfig::default(),
            #[cfg(feature = "hosted")]
            org_access: crate::org_access::OrgAccessConfig::default(),
            reconnect_grace: Duration::ZERO,
            share_web: None,
            share_limits: crate::share_web::ShareLimits::default(),
            analytics_key_file: None,
            audit_retention_days: DEFAULT_AUDIT_RETENTION_DAYS,
            mesh_endpoint_retention_days: DEFAULT_MESH_ENDPOINT_RETENTION_DAYS,
            trusted_proxies: crate::proxies::TrustedProxies::default(),
        }
    }
}

/// Who registered a connected machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Registrant {
    /// A static relay token (its index).
    Static(usize),
    /// A machine token from the account directory; the owning account.
    Account(String),
}

/// One client (account user) using a machine.
#[derive(Debug, Clone)]
pub(crate) struct ClientPresence {
    pub email: Option<String>,
    pub name: Option<String>,
    pub streams: u32,
    pub since: u64,
    pub last_seen: Instant,
}

pub(crate) struct Machine {
    pub(crate) id: String,
    pub(crate) registrant: Registrant,
    pub(crate) version: String,
    pub(crate) connected_at: SystemTime,
    generation: u64,
    handle: MuxHandle,
    counters: Arc<Counters>,
    active: AtomicU32,
    last_heartbeat: Mutex<Instant>,
    pub(crate) abort: tokio::task::AbortHandle,
    /// Cancelled to cut every client stream ("stop sharing").
    clients_cut: Mutex<CancellationToken>,
    /// Per-account children of `clients_cut`: cancelled to cut one
    /// account's streams (its access was revoked) and no one else's.
    user_cuts: Mutex<HashMap<String, CancellationToken>>,
    /// Account users with open (or recently closed) streams.
    pub(crate) clients: Mutex<HashMap<String, ClientPresence>>,
}

impl Machine {
    /// Time since the machine's last heartbeat.
    pub(crate) fn heartbeat_age(&self) -> Duration {
        self.last_heartbeat.lock().expect("heartbeat").elapsed()
    }

    /// Cuts every client stream and forgets presence.
    pub(crate) fn cut_clients(&self) {
        // Same lock order as `cut_token`, held across both, so no stream
        // can attach a per-account token to the old one in between.
        let old = {
            let mut all = self.clients_cut.lock().expect("cut");
            self.user_cuts.lock().expect("user cuts").clear();
            std::mem::take(&mut *all)
        };
        old.cancel();
        self.clients.lock().expect("clients").clear();
    }

    /// Cuts the streams of account user `user` only, and drops it from
    /// presence (CUA-1417: a revoke must not cut anyone else).
    pub(crate) fn cut_user(&self, user: &str) {
        if let Some(token) = self.user_cuts.lock().expect("user cuts").remove(user) {
            token.cancel();
        }
        self.clients.lock().expect("clients").remove(user);
    }

    /// The token a new stream watches: its account's own (a child of the
    /// machine-wide one), or the machine-wide one when it is not attributed.
    fn cut_token(&self, user: Option<&str>) -> CancellationToken {
        let all = self.clients_cut.lock().expect("cut");
        let Some(user) = user else {
            return all.clone();
        };
        self.user_cuts
            .lock()
            .expect("user cuts")
            .entry(user.to_owned())
            .or_insert_with(|| all.child_token())
            .clone()
    }

    fn client_enter(&self, who: &Identity) {
        let mut clients = self.clients.lock().expect("clients");
        let entry = clients
            .entry(who.user.clone())
            .or_insert_with(|| ClientPresence {
                email: who.email.clone(),
                name: who.name.clone(),
                streams: 0,
                since: assertion::now_secs(),
                last_seen: Instant::now(),
            });
        entry.streams += 1;
        entry.last_seen = Instant::now();
    }

    fn client_leave(&self, user: &str) {
        if let Some(entry) = self.clients.lock().expect("clients").get_mut(user) {
            entry.streams = entry.streams.saturating_sub(1);
            entry.last_seen = Instant::now();
        }
    }

    /// The account user who most recently used this machine (within the
    /// presence linger): who a machine-minted ticket (a viewer's media
    /// WebSocket) most likely belongs to, for metering only.
    fn recent_client(&self) -> Option<Identity> {
        let clients = self.clients.lock().expect("clients");
        clients
            .iter()
            .filter(|(_, c)| c.streams > 0 || c.last_seen.elapsed() < PRESENCE_LINGER)
            .max_by_key(|(_, c)| c.last_seen)
            .map(|(user, c)| Identity {
                user: user.clone(),
                account: user.clone(),
                email: c.email.clone(),
                name: c.name.clone(),
                auth_time: None,
                mfa: false,
                orgs: Vec::new(),
            })
    }

    /// Clients with open streams or a stream closed within the linger.
    pub(crate) fn presence(&self) -> Vec<(String, ClientPresence)> {
        let mut clients = self.clients.lock().expect("clients");
        clients.retain(|_, c| c.streams > 0 || c.last_seen.elapsed() < PRESENCE_LINGER);
        let mut rows: Vec<_> = clients
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        rows
    }
}

/// Relay state.
#[derive(Clone)]
pub struct Relay {
    pub(crate) config: Arc<RelayConfig>,
    pub(crate) machines: Arc<Mutex<HashMap<String, Arc<Machine>>>>,
    generation: Arc<AtomicU64>,
    pub(crate) directory: Arc<Directory>,
    pub(crate) devices: Arc<DeviceStore>,
    pub(crate) teams: Arc<crate::teams::Teams>,
    /// Team access from Keycloak Organizations and the Spaces API.
    pub(crate) org: Arc<crate::org_access::OrgAccess>,
    /// Plans from the Spaces API's entitlements, when that is the source.
    #[cfg(feature = "hosted")]
    pub(crate) plans: Option<crate::plan_source::EntitlementPlans>,
    /// Who reached each machine, and whose access was revoked (tickets).
    pub(crate) grants: Arc<crate::grants::Grants>,
    /// Pairing codes ("add a machine" by link or QR).
    pub(crate) pairing: Arc<crate::pairing::PairingStore>,
    pub(crate) key: Arc<RelayKey>,
    /// Open client streams across every machine (S7): the global memory
    /// budget's live counter.
    pub(crate) global_streams: Arc<AtomicU64>,
    /// The mesh coordinator's registry and connections.
    pub(crate) mesh: Arc<crate::mesh::MeshState>,
    /// Each account's pool: coordinator lease, job log, mailbox.
    pub(crate) pool: Arc<crate::pool::PoolState>,
    /// The state volume's health (write failures, budget, probe).
    pub(crate) storage: Arc<crate::storage::Storage>,
    /// Allowance metering.
    pub(crate) meter: Arc<Meter>,
    /// Connection outcome counters (`cua_relay_connections_total`).
    pub(crate) conns: Arc<crate::conn_metrics::ConnMetrics>,
    /// Admission control for machine joins (a reconnect storm after a restart).
    join_limiter: Arc<Mutex<crate::join_limit::JoinLimiter>>,
    /// When this relay started (the reconnect grace counts from it).
    started: Instant,
    /// Share links: browser sign-in state and public-link rate limits.
    pub(crate) share: Arc<crate::share_web::ShareState>,
}

/// One row of `/relay/v1/machines`.
#[derive(Debug, Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct MachineStatus {
    /// Machine id.
    pub id: String,
    /// Reported spacesd version.
    pub version: String,
    /// Seconds since the Unix epoch.
    pub connected_at: u64,
    /// Open client streams.
    pub active_streams: u32,
    /// Bytes from clients to the machine.
    pub bytes_to_machine: u64,
    /// Bytes from the machine to clients.
    pub bytes_from_machine: u64,
}

impl Relay {
    /// Creates a relay. Panics if the configured state file is unreadable;
    /// use [`Relay::try_new`] to handle that.
    pub fn new(config: RelayConfig) -> Self {
        Self::try_new(config).expect("relay state file")
    }

    /// Creates a relay, loading the machine directory from the state file.
    /// Temp files interrupted writes left in the state directory are
    /// removed first (one relay per state directory).
    pub fn try_new(config: RelayConfig) -> std::io::Result<Self> {
        let devices_file = config.devices_file.clone().or_else(|| {
            config
                .state_file
                .as_ref()
                .map(|p| p.with_extension("devices.json"))
        });
        let state_dir = config
            .state_file
            .as_ref()
            .or(devices_file.as_ref())
            .map(|p| match p.parent() {
                Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
                _ => PathBuf::from("."),
            });
        if let Some(dir) = &state_dir {
            std::fs::create_dir_all(dir)?;
        }
        // The analytics id / log tag key, before anything can log a tag.
        let ident_path = config.analytics_key_file.clone().or_else(|| {
            crate::ident::default_path(config.state_file.as_deref(), devices_file.as_deref())
        });
        crate::ident::install(ident_path.as_deref())?;
        let storage = Arc::new(crate::storage::Storage::new(
            state_dir.clone(),
            config.storage_limits,
        ));
        if let Some(dir) = &state_dir {
            // The pools sweep their own directories when they open.
            let (files, bytes) = crate::storage::sweep_temps(dir, 0)?;
            storage.temps_removed(files, bytes);
        }
        let directory = match &config.state_file {
            Some(path) => Directory::open_with(
                path.clone(),
                config.max_machines_per_account,
                storage.clone(),
            )?,
            None => Directory::in_memory(config.max_machines_per_account),
        };
        let devices = match devices_file {
            Some(path) => DeviceStore::open_with(path, config.device_policy, storage.clone())?,
            None => DeviceStore::in_memory(config.device_policy),
        };
        let grants = match &config.state_file {
            Some(path) => crate::grants::Grants::open_with(
                path.with_extension("grants.json"),
                storage.clone(),
            )?,
            None => crate::grants::Grants::in_memory(),
        };
        let pairing = match &config.state_file {
            Some(path) => crate::pairing::PairingStore::open_with(
                path.with_extension("pairing.json"),
                storage.clone(),
            )?,
            None => crate::pairing::PairingStore::in_memory(),
        };
        let key = config
            .signing_key
            .clone()
            .unwrap_or_else(|| Arc::new(RelayKey::generate()));
        let mesh_file = config.mesh_file.clone().or_else(|| {
            config
                .state_file
                .as_ref()
                .map(|p| p.with_extension("mesh.json"))
        });
        let mesh = Arc::new(crate::mesh::MeshState::open_with(
            mesh_file,
            storage.clone(),
        )?);
        let pool = Arc::new(crate::pool::PoolState::open_with(
            config.state_file.as_ref().map(|p| p.with_extension("pool")),
            storage.clone(),
        )?);
        #[cfg(feature = "hosted")]
        let crate::hosted::HostedState {
            meter,
            teams,
            org,
            plans,
        } = crate::hosted::open(&config, &storage)?;
        #[cfg(not(feature = "hosted"))]
        let (meter, teams, org) = crate::free::open();
        let join_limiter = Arc::new(Mutex::new(crate::join_limit::JoinLimiter::new(
            config.join_rate_per_sec,
            config.join_burst,
            Instant::now(),
        )));
        let relay = Self {
            config: Arc::new(config),
            machines: Arc::default(),
            generation: Arc::default(),
            directory: Arc::new(directory),
            devices: Arc::new(devices),
            teams,
            org,
            #[cfg(feature = "hosted")]
            plans,
            grants: Arc::new(grants),
            pairing: Arc::new(pairing),
            key,
            global_streams: Arc::default(),
            mesh,
            pool,
            storage,
            meter,
            conns: Arc::default(),
            join_limiter,
            started: Instant::now(),
            share: Arc::default(),
        };
        relay.sweep_retention();
        Ok(relay)
    }

    /// Drops what has outlived its retention: audit entries older than
    /// `audit_retention_days`, stored mesh endpoints of nodes offline longer
    /// than `mesh_endpoint_retention_days`. At start and then hourly.
    pub fn sweep_retention(&self) {
        let audit = self.devices.trim_audit(self.config.audit_retention_days);
        let endpoints = self
            .mesh
            .forget_stale_endpoints(self.config.mesh_endpoint_retention_days);
        #[cfg(feature = "hosted")]
        let audit = audit + self.teams.trim_audit(self.config.audit_retention_days);
        if audit + endpoints > 0 {
            tracing::info!(audit, endpoints, "retention sweep dropped expired data");
        }
    }

    /// The HTTP router.
    pub fn router(&self) -> Router {
        Router::new()
            // Liveness: the process answers. Never touches the disk.
            .route("/healthz", get(|| async { StatusCode::NO_CONTENT }))
            // The state volume: a write probe, free space, each store's
            // last write (503 when the probe or a store fails).
            .route("/readyz", get(readyz))
            .route("/metrics", get(metrics))
            .route(CONNECT_PATH, get(machine_connect))
            .route(cua_mesh::proto::CONNECT_PATH, get(crate::mesh::connect))
            .route("/relay/v1/machines", get(machine_list))
            .route("/relay/v1/mesh", get(mesh_status))
            .merge(crate::api::routes())
            .merge(crate::device_api::routes())
            .merge(crate::pool::routes())
            .merge(crate::meter::routes())
            .merge(self.hosted_routes())
            .merge(crate::pairing::routes())
            .merge(if self.share_web_enabled() {
                crate::share_web::routes()
            } else {
                Router::new()
            })
            .fallback(proxy)
            .layer(axum::middleware::from_fn_with_state(
                self.clone(),
                share_hosts,
            ))
            .layer(axum::middleware::from_fn_with_state(
                self.clone(),
                count_connections,
            ))
            .with_state(self.clone())
    }

    /// The hosted relay's Teams and internal routes. Without the
    /// `hosted` feature the Teams routes answer 410: not on this relay.
    fn hosted_routes(&self) -> Router<Relay> {
        #[cfg(feature = "hosted")]
        return crate::hosted::routes(self);
        #[cfg(not(feature = "hosted"))]
        return crate::free::routes();
    }

    /// The mesh coordinator (registry, connections, counters).
    pub fn mesh(&self) -> &crate::mesh::MeshState {
        &self.mesh
    }

    /// The relay's allowance meter.
    pub fn meter(&self) -> &Arc<Meter> {
        &self.meter
    }

    /// The relay's assertion signing key.
    pub fn signing_key(&self) -> &RelayKey {
        &self.key
    }

    /// The caller's role on `record`: its own (owner, allowlist, viewers)
    /// first, else an editor's through the teams it is in that the machine
    /// is shared with (their ids; empty for a direct role).
    pub(crate) fn machine_role(
        &self,
        record: &crate::directory::MachineRecord,
        who: &crate::oidc::Identity,
    ) -> Option<(crate::directory::Role, Vec<String>)> {
        if let Some(role) = record.role_of(who) {
            // Team-only sharing: a direct share counts only while the
            // owner's team still has the person (from the answers cached
            // when they opened it; nothing cached leaves it as it is).
            let outside = role != crate::directory::Role::Owner
                && self.org.team_only_sharing()
                && self
                    .org
                    .cached_share_allowed(&record.owner.id, &crate::team_sharing::who_keys(who))
                    == Some(false);
            if !outside {
                return Some((role, Vec::new()));
            }
        }
        let host = record.host.as_deref().and_then(|h| self.directory.get(h));
        let teams: Vec<String> = if self.org.source() == crate::org_access::TeamsSource::Org {
            // Organizations: what the Spaces API last answered (asked on
            // every open; dropped by a membership push), only while the
            // record's owner is a member of the same org.
            let orgs = self
                .org
                .cached_org_roles(&who.user, &record.owner.id, &record.id);
            // A team share can be watch only; one editor team is enough
            // to edit.
            if !orgs.is_empty() && orgs.iter().all(|(_, viewer)| *viewer) {
                return Some((
                    crate::directory::Role::Viewer,
                    orgs.into_iter().map(|(o, _)| o).collect(),
                ));
            }
            orgs.into_iter().map(|(o, _)| o).collect()
        } else {
            self.teams
                .access(record, host.as_ref(), &who.account)
                .into_iter()
                .map(|(t, _)| t)
                .collect()
        };
        (!teams.is_empty()).then_some((crate::directory::Role::Shared, teams))
    }

    /// Re-checks every account recently authorized on `machines` (all of
    /// them when `None`) after a sharing or team change, and revokes only
    /// the ones that lost access or dropped to a lower role: their streams
    /// are cut and their outstanding tickets refused. Everyone else,
    /// including the owner, keeps their sessions (CUA-1416, CUA-1417).
    pub(crate) fn reauthorize(&self, machines: Option<&[String]>) {
        use crate::grants::rank;
        let at = crate::grants::now_ms();
        let ids = match machines {
            Some(ids) => ids.to_vec(),
            None => {
                let mut ids = self.grants.machines();
                ids.extend(self.machine_ids());
                ids.sort();
                ids.dedup();
                ids
            }
        };
        for id in ids {
            let record = self.directory.get(&id);
            let role_of = |who: &Identity| {
                record
                    .as_ref()
                    .and_then(|r| self.machine_role(r, who))
                    .map(|(role, _)| rank(role))
            };
            let seen = self.grants.seen_on(&id);
            for (user, s) in &seen {
                let now = role_of(&s.identity(user));
                match now {
                    Some(n) if n == s.role => {}
                    // Raised (a viewer made an editor): reconnect to get
                    // sessions opened with the new role.
                    Some(n) if n > s.role => {
                        self.grants.set_role(&id, user, n);
                        if let Some(m) = self.machine(&id) {
                            m.cut_user(user);
                        }
                    }
                    _ => self.revoke_user(&id, user, s.role, now, at),
                }
            }
            // Live clients the store does not know (pruned): check them too.
            let Some(machine) = self.machine(&id) else {
                continue;
            };
            for (user, c) in machine.presence() {
                if seen.iter().any(|(u, _)| u == &user) {
                    continue;
                }
                let who = Identity {
                    user: user.clone(),
                    account: user.clone(),
                    email: c.email,
                    name: c.name,
                    auth_time: None,
                    mfa: false,
                    orgs: Vec::new(),
                };
                if role_of(&who).is_none() {
                    self.revoke_user(&id, &user, rank(crate::directory::Role::Shared), None, at);
                }
            }
        }
    }

    /// Revokes account user `user` on machine `id` (it had role rank
    /// `before`, now `now`): cuts its streams and refuses its older tickets.
    pub(crate) fn revoke_user(&self, id: &str, user: &str, before: u8, now: Option<u8>, at: u64) {
        self.grants.revoke(id, user, before, now, at);
        if let Some(m) = self.machine(id) {
            m.cut_user(user);
        }
        tracing::info!(machine = %id, user = %crate::ident::tag(user), "access revoked; the account's streams cut");
    }

    /// Cuts every stream and outstanding ticket of the account `account`
    /// on `machines` (all when `None`) without changing its access: a
    /// device of it was revoked or denied, and tickets don't say which
    /// device they were minted for.
    pub(crate) fn cut_account(&self, account: &str, machines: Option<&[String]>) {
        let at = crate::grants::now_ms();
        let ids = match machines {
            Some(ids) => ids.to_vec(),
            None => self.grants.machines(),
        };
        for id in ids {
            for (user, s) in self.grants.seen_on(&id) {
                if s.account == account || user == account {
                    self.revoke_user(&id, &user, s.role, Some(s.role), at);
                }
            }
        }
        for machine in self.machines.lock().expect("machines").values() {
            if machines.is_none_or(|m| m.contains(&machine.id)) {
                machine.cut_user(account);
            }
        }
    }

    /// Whether a machine-minted ticket may still be used: refused when it
    /// may predate a revoke of the account it was minted for.
    fn ticket_allowed(
        &self,
        id: &str,
        record: &crate::directory::MachineRecord,
        ticket: &crate::grants::TicketInfo,
    ) -> bool {
        use crate::grants::rank;
        if ticket.user.is_empty() {
            return true;
        }
        // An account deleted since the ticket was minted (CUA-1950).
        if self.grants.erased_since(&ticket.user, ticket.issued) {
            return false;
        }
        let (seen, mark) = self.grants.lookup(id, &ticket.user);
        let Some(s) = seen else {
            return crate::grants::ticket_allowed(ticket, None, mark, None);
        };
        let now = self
            .machine_role(record, &s.identity(&ticket.user))
            .map(|(role, _)| rank(role));
        if now.is_none_or(|n| n < s.role) {
            // A change the relay hasn't acted on yet: act on it now.
            self.revoke_user(id, &ticket.user, s.role, now, crate::grants::now_ms());
            return false;
        }
        crate::grants::ticket_allowed(ticket, Some(&s), mark, now)
    }

    /// The public base URL for a request (config, else `Host`).
    pub(crate) fn public_url(&self, headers: &HeaderMap) -> String {
        if let Some(url) = &self.config.public_url {
            return url.trim_end_matches('/').to_owned();
        }
        let host = headers
            .get(header::HOST)
            .and_then(|h| h.to_str().ok())
            .unwrap_or("localhost");
        let proto = headers
            .get("x-forwarded-proto")
            .and_then(|h| h.to_str().ok())
            .filter(|p| *p == "https" || *p == "http")
            .unwrap_or("http");
        format!("{proto}://{host}")
    }

    /// The client URL for machine `id`: its own subdomain when a host-mode
    /// domain is configured (S6: a separate origin per machine), else the
    /// shared origin's `/m/<id>` path.
    pub(crate) fn machine_url(&self, headers: &HeaderMap, id: &str) -> String {
        let Some(domain) = &self.config.host_domain else {
            return format!("{}/m/{id}", self.public_url(headers));
        };
        let scheme = self
            .config
            .public_url
            .as_deref()
            .and_then(|u| u.split_once("://"))
            .map(|(s, _)| s.to_owned())
            .unwrap_or_else(|| {
                let https = headers
                    .get("x-forwarded-proto")
                    .and_then(|h| h.to_str().ok())
                    == Some("https");
                if https { "https" } else { "http" }.to_owned()
            });
        format!("{scheme}://{id}.{domain}")
    }

    /// The connected machine `id`, if any.
    pub(crate) fn machine(&self, id: &str) -> Option<Arc<Machine>> {
        self.machines.lock().expect("machines").get(id).cloned()
    }

    /// Within the reconnect grace after start, waits for the known machine
    /// `id` to connect (it was connected to the relay this one replaced and
    /// is reconnecting); `None` once the grace is over or for an unknown id.
    pub(crate) async fn await_reconnect(&self, id: &str) -> Option<Arc<Machine>> {
        let until = self.started + self.config.reconnect_grace;
        if Instant::now() >= until || self.directory.get(id).is_none() {
            return None;
        }
        while Instant::now() < until {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if let Some(machine) = self.machine(id) {
                return Some(machine);
            }
        }
        None
    }

    /// Drops machine `id` and cuts its clients.
    pub(crate) fn drop_machine(&self, id: &str) {
        if let Some(m) = self.machines.lock().expect("machines").remove(id) {
            m.cut_clients();
            m.abort.abort();
        }
    }

    /// Settle overlapping upgrades where the machine joins the registry.
    fn insert_machine(&self, machine: Arc<Machine>) -> (Option<Arc<Machine>>, bool) {
        let mut machines = self.machines.lock().expect("machines");
        // Generation allocation can precede this lock in a different order.
        // Admit against occupied slots, excluding the session being replaced.
        let over_limit = matches!(machine.registrant, Registrant::Static(_))
            && machines
                .values()
                .filter(|m| m.registrant == machine.registrant && m.id != machine.id)
                .count()
                >= self.config.max_machines_per_token;
        if over_limit {
            return (None, true);
        }
        (machines.insert(machine.id.clone(), machine), false)
    }

    /// Connected machine ids.
    pub fn machine_ids(&self) -> Vec<String> {
        let mut ids: Vec<_> = self
            .machines
            .lock()
            .expect("machines")
            .keys()
            .cloned()
            .collect();
        ids.sort();
        ids
    }

    /// Status rows.
    pub fn status(&self) -> Vec<MachineStatus> {
        let mut rows: Vec<_> = self
            .machines
            .lock()
            .expect("machines")
            .values()
            .map(|m| MachineStatus {
                id: m.id.clone(),
                version: m.version.clone(),
                connected_at: m
                    .connected_at
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
                active_streams: m.active.load(Ordering::SeqCst),
                bytes_to_machine: m.counters.to_machine.load(Ordering::Relaxed),
                bytes_from_machine: m.counters.from_machine.load(Ordering::Relaxed),
            })
            .collect();
        rows.sort_by(|a, b| a.id.cmp(&b.id));
        rows
    }

    /// Drops every machine session (as if the relay process died).
    pub fn disconnect_all(&self) {
        for machine in self
            .machines
            .lock()
            .expect("machines")
            .drain()
            .map(|(_, m)| m)
        {
            machine.abort.abort();
        }
    }

    /// Serves on `listener` until the future `shutdown` resolves.
    pub async fn serve(
        self,
        listener: tokio::net::TcpListener,
        shutdown: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> std::io::Result<()> {
        if self.config.mesh.enabled {
            crate::mesh::spawn(&self);
        }
        // Small frames (gRPC calls, yamux window updates, heartbeats) must
        // not wait for Nagle: with delayed ACKs each relayed call took about
        // 80 ms longer than the network round trip.
        use axum::serve::ListenerExt as _;
        let listener = listener.tap_io(|tcp| {
            let _ = tcp.set_nodelay(true);
        });
        self.meter.spawn();
        {
            let relay = self.clone();
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(Duration::from_secs(3600));
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                tick.tick().await; // the first tick is at start (done in try_new)
                loop {
                    tick.tick().await;
                    let relay = relay.clone();
                    let _ = tokio::task::spawn_blocking(move || relay.sweep_retention()).await;
                }
            });
        }
        #[cfg(feature = "hosted")]
        crate::team_sharing::spawn(&self);
        let handle = self.clone();
        axum::serve(
            listener,
            self.router()
                .into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            shutdown.await;
            // Machines and mesh nodes reconnect at once, to the relay that
            // replaces this one (CUA-1774), instead of holding the shutdown
            // open until the process exits.
            let machines = handle.machine_ids().len();
            let mesh = handle.mesh.connected();
            handle.disconnect_all();
            handle.mesh.disconnect_all();
            tracing::info!(machines, mesh, "closed machine and mesh connections");
            // Keep the last minutes of usage across a restart.
            handle.meter.persist(true);
        })
        .await
    }

    pub(crate) fn token_index(&self, headers: &HeaderMap) -> Option<usize> {
        let presented = bearer(headers, header::AUTHORIZATION.as_str())?;
        self.config.tokens.iter().position(|t| {
            use subtle::ConstantTimeEq as _;
            t.len() == presented.len() && bool::from(t.as_bytes().ct_eq(presented.as_bytes()))
        })
    }
}

/// `<token>` of `name: Bearer <token>`.
pub(crate) fn bearer<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            let (scheme, rest) = v.split_once(' ')?;
            scheme.eq_ignore_ascii_case("bearer").then_some(rest.trim())
        })
        .filter(|t| !t.is_empty())
}

/// The account token a client presented, if any.
pub(crate) fn account_token(headers: &HeaderMap) -> Option<&str> {
    bearer(headers, RELAY_AUTHORIZATION_HEADER)
        .or_else(|| bearer(headers, header::AUTHORIZATION.as_str()))
}

pub(crate) fn text(status: StatusCode, message: impl Into<String>) -> Response {
    let mut response = (status, message.into()).into_response();
    response
        .extensions_mut()
        .insert(crate::conn_metrics::RelayStatus(status));
    response
}

/// gRPC status codes the relay produces itself.
#[derive(Clone, Copy)]
enum GrpcCode {
    Unauthenticated = 16,
    PermissionDenied = 7,
    Unavailable = 14,
    ResourceExhausted = 8,
    NotFound = 5,
    InvalidArgument = 3,
}

/// gRPC methods that hold a session open (process and terminal streams,
/// window and directory watches). Everything else (list, health, file
/// transfers, unary calls) is short and never counts as a session.
const SESSION_GRPC_METHODS: &[&str] = &[
    "/cua.env.v1.ProcessService/StartProcess",
    "/cua.env.v1.ProcessService/ConnectProcess",
    "/cua.env.v1.ProcessService/StreamInput",
    "/cua.env.v1.WindowsService/WatchWindows",
    "/cua.env.v1.FilesystemService/WatchDir",
];

/// Whether a request is a gRPC or gRPC-Web call to a session-holding method.
/// Matched on the path suffix, since the relay may sit under a prefix.
fn holds_grpc_session(headers: &HeaderMap, path: &str) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("application/grpc"))
        && SESSION_GRPC_METHODS.iter().any(|m| path.ends_with(m))
}

/// Refuses a client request. gRPC and gRPC-Web callers get a trailers-only
/// gRPC status (HTTP 200 + `grpc-status`), so SDKs see the right code
/// instead of an opaque HTTP error; everyone else gets `status`.
fn refuse(
    headers: &HeaderMap,
    status: StatusCode,
    code: GrpcCode,
    message: impl Into<String>,
) -> Response {
    refuse_as(headers, status, code, message, None)
}

/// The `ErrorInfo.metadata["kind"]` of a refused thread grant
/// ([`thread_grant`]): clients tell it from any other refusal by it.
pub const THREAD_GRANT_REFUSED: &str = "thread_grant_refused";

/// [`refuse`], typed with `kind` (`ErrorInfo.metadata["kind"]`) when given.
fn refuse_as(
    headers: &HeaderMap,
    status: StatusCode,
    code: GrpcCode,
    message: impl Into<String>,
    kind: Option<&str>,
) -> Response {
    let message = message.into();
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .filter(|ct| ct.starts_with("application/grpc"));
    let Some(content_type) = content_type else {
        return text(status, message);
    };
    // gRPC callers see HTTP 200; connection metrics keep the relay's status.
    let mark = |mut response: Response| {
        response
            .extensions_mut()
            .insert(crate::conn_metrics::RelayStatus(status));
        response
    };
    let encoded = percent_encode_grpc(&message);
    let details = {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD_NO_PAD
            .encode(status_details(code, &message, kind))
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header("grpc-status", (code as i32).to_string())
        .header("grpc-message", encoded)
        .header("grpc-status-details-bin", details)
        .body(Body::empty())
        .map(mark)
        .unwrap_or_else(|_| text(status, message))
}

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn put_bytes(out: &mut Vec<u8>, field: u64, bytes: &[u8]) {
    put_varint(out, (field << 3) | 2);
    put_varint(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

/// `google.rpc.Status{code, message, details: [Any(cua.env.v1.ErrorInfo)]}`,
/// encoded by hand so the relay needs no protobuf toolchain. `ErrorInfo`
/// reasons follow `cua.env.v1.ErrorReason`.
fn status_details(code: GrpcCode, message: &str, kind: Option<&str>) -> Vec<u8> {
    let reason: u64 = match code {
        GrpcCode::Unauthenticated => 1,  // ERROR_REASON_UNAUTHENTICATED
        GrpcCode::PermissionDenied => 3, // ERROR_REASON_PERMISSION_DENIED
        GrpcCode::Unavailable | GrpcCode::NotFound => 8, // ERROR_REASON_TARGET_UNAVAILABLE
        GrpcCode::ResourceExhausted => 20, // ERROR_REASON_RATE_LIMITED
        GrpcCode::InvalidArgument => 0,
    };
    let mut info = Vec::new();
    if reason != 0 {
        put_varint(&mut info, 1 << 3);
        put_varint(&mut info, reason);
    }
    put_bytes(&mut info, 2, message.as_bytes());
    if let Some(kind) = kind {
        // `map<string, string> metadata = 4`: one {key, value} entry.
        let mut entry = Vec::new();
        put_bytes(&mut entry, 1, b"kind");
        put_bytes(&mut entry, 2, kind.as_bytes());
        put_bytes(&mut info, 4, &entry);
    }
    let mut any = Vec::new();
    put_bytes(&mut any, 1, b"type.googleapis.com/cua.env.v1.ErrorInfo");
    put_bytes(&mut any, 2, &info);
    let mut status = Vec::new();
    put_varint(&mut status, 1 << 3);
    put_varint(&mut status, code as u64);
    put_bytes(&mut status, 2, message.as_bytes());
    put_bytes(&mut status, 3, &any);
    status
}

/// Percent-encodes a `grpc-message` value (gRPC over HTTP/2 spec).
fn percent_encode_grpc(message: &str) -> String {
    message
        .bytes()
        .map(|b| {
            if (0x20..=0x7e).contains(&b) && b != b'%' {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

async fn machine_connect(
    state: State<Relay>,
    headers: HeaderMap,
    upgrade: Result<WebSocketUpgrade, axum::extract::ws::rejection::WebSocketUpgradeRejection>,
) -> Response {
    let mut response = match upgrade {
        Ok(upgrade) => machine_connect_inner(state, headers, upgrade).await,
        // Not a WebSocket handshake: refused before the relay looks at it.
        Err(rejection) => {
            let mut response = rejection.into_response();
            let status = response.status();
            response
                .extensions_mut()
                .insert(crate::conn_metrics::RelayStatus(status));
            response
        }
    };
    response
        .extensions_mut()
        .insert(crate::conn_metrics::Kind::Machine);
    response
}

async fn machine_connect_inner(
    State(relay): State<Relay>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    let Some(id) = headers
        .get(MACHINE_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .filter(|id| valid_machine_id(id))
    else {
        return text(StatusCode::BAD_REQUEST, "missing or invalid computer id");
    };
    // A reconnect storm (every machine back at once after a restart) is let
    // in at a steady rate; the rest are told when to come back.
    let admitted = relay
        .join_limiter
        .lock()
        .expect("join limiter")
        .admit(Instant::now());
    if let Err(wait) = admitted {
        let unit = {
            use rand::Rng as _;
            rand::thread_rng().gen::<f64>()
        };
        let secs = crate::join_limit::retry_after_secs(wait, Duration::from_secs(5), unit);
        let mut response = text(
            StatusCode::SERVICE_UNAVAILABLE,
            "the relay is busy; try again shortly",
        );
        if let Ok(v) = HeaderValue::from_str(&secs.to_string()) {
            response.headers_mut().insert(header::RETRY_AFTER, v);
        }
        return response;
    }
    let registrant = if let Some(index) = relay.token_index(&headers) {
        if relay.directory.get(&id).is_some() {
            return text(
                StatusCode::CONFLICT,
                "computer id is registered to an account",
            );
        }
        Registrant::Static(index)
    } else {
        let record = bearer(&headers, header::AUTHORIZATION.as_str())
            .filter(|t| t.starts_with(MACHINE_TOKEN_PREFIX))
            .and_then(|t| relay.directory.machine_for_token(t));
        match record {
            Some(record) if record.id == id => Registrant::Account(record.owner.id),
            Some(_) => {
                return text(
                    StatusCode::FORBIDDEN,
                    "computer token belongs to another computer id",
                )
            }
            None => {
                let reason = match bearer(&headers, header::AUTHORIZATION.as_str()) {
                    None => Reason::MissingToken,
                    Some(t) if t.starts_with(MACHINE_TOKEN_PREFIX) => Reason::UnknownMachineToken,
                    Some(_) => Reason::InvalidRelayToken,
                };
                auth_log::rejected(&headers, Route::Connect, reason, None);
                return text(StatusCode::UNAUTHORIZED, "invalid relay token");
            }
        }
    };
    let version = headers
        .get(VERSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .chars()
        .take(64)
        .collect::<String>();
    {
        let machines = relay.machines.lock().expect("machines");
        if let Registrant::Static(_) = registrant {
            let same_token = machines
                .values()
                .filter(|m| m.registrant == registrant && m.id != id)
                .count();
            if same_token >= relay.config.max_machines_per_token {
                return text(
                    StatusCode::TOO_MANY_REQUESTS,
                    "computer limit for this token reached",
                );
            }
        }
        if let Some(existing) = machines.get(&id) {
            if existing.registrant != registrant {
                return text(
                    StatusCode::CONFLICT,
                    "computer id registered with another token",
                );
            }
        }
    }
    let owner = match &registrant {
        Registrant::Account(owner) => Some(owner.clone()),
        Registrant::Static(_) => None,
    };
    let jwks = owner.as_ref().map(|_| {
        use base64::Engine as _;
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(relay.key.jwks().to_string())
    });
    let mut response = upgrade
        .max_message_size(64 * 1024 * 1024)
        .on_upgrade(move |socket| async move {
            let (inbound_tx, mut inbound_rx) = mpsc::channel(64);
            let (handle, driver) = mux::spawn(
                WsIo::new(socket),
                yamux::Mode::Client,
                inbound_tx,
                relay.config.machine_max_streams,
                relay.config.machine_window_bytes,
            );
            let generation = relay.generation.fetch_add(1, Ordering::SeqCst);
            let machine = Arc::new(Machine {
                id: id.clone(),
                registrant,
                version,
                connected_at: SystemTime::now(),
                generation,
                handle,
                counters: Arc::default(),
                active: AtomicU32::new(0),
                last_heartbeat: Mutex::new(Instant::now()),
                abort: driver.abort_handle(),
                clients_cut: Mutex::default(),
                user_cuts: Mutex::default(),
                clients: Mutex::default(),
            });
            let (replaced, over_limit) = relay.insert_machine(machine.clone());
            if over_limit {
                tracing::info!(machine = %id, "computer limit for this token reached");
                driver.abort();
                return;
            }
            if let Some(old) = replaced {
                tracing::info!(machine = %id, "replacing previous session");
                old.cut_clients();
                old.abort.abort();
            }
            tracing::info!(machine = %id, "machine connected");
            // Heartbeat streams opened by the machine: echo one byte.
            let echo_machine = machine.clone();
            let echo = tokio::spawn(async move {
                while let Some(mut stream) = inbound_rx.recv().await {
                    let machine = echo_machine.clone();
                    tokio::spawn(async move {
                        let mut buf = [0u8; 1];
                        if stream.read_exact(&mut buf).await.is_ok() && buf[0] == HEARTBEAT_BYTE {
                            *machine.last_heartbeat.lock().expect("heartbeat") = Instant::now();
                            let _ = stream.write_all(&buf).await;
                            let _ = stream.flush().await;
                        }
                        let _ = stream.close().await;
                    });
                }
            });
            let idle = relay.config.machine_idle_timeout;
            let watchdog_machine = machine.clone();
            let watchdog = tokio::spawn(async move {
                loop {
                    tokio::time::sleep(idle / 4).await;
                    if watchdog_machine.last_heartbeat.lock().expect("heartbeat").elapsed() > idle {
                        tracing::info!(machine = %watchdog_machine.id, "no heartbeat; dropping machine");
                        watchdog_machine.abort.abort();
                        return;
                    }
                }
            });
            let _ = driver.await;
            echo.abort();
            watchdog.abort();
            machine.cut_clients();
            let mut machines = relay.machines.lock().expect("machines");
            if machines.get(&id).is_some_and(|m| m.generation == generation) {
                machines.remove(&id);
            }
            tracing::info!(machine = %id, "machine disconnected");
        });
    if let (Some(owner), Some(jwks)) = (owner, jwks) {
        if let (Ok(owner), Ok(jwks)) = (HeaderValue::from_str(&owner), HeaderValue::from_str(&jwks))
        {
            response
                .headers_mut()
                .insert(assertion::OWNER_HEADER, owner);
            response.headers_mut().insert(assertion::JWKS_HEADER, jwks);
        }
    }
    response
}

async fn mesh_status(State(relay): State<Relay>, headers: HeaderMap) -> Response {
    if let Err(refused) = require_admin(&relay, &headers) {
        return refused;
    }
    axum::Json(crate::mesh::status(&relay)).into_response()
}

/// `GET /readyz`: writes, fsyncs and removes a small file in the state
/// directory and reports free space and each store's last write; 503 when
/// the probe fails or a store's last write failed. Liveness is `/healthz`.
async fn readyz(State(relay): State<Relay>) -> Response {
    let storage = relay.storage.clone();
    let (ready, mut body) = tokio::task::spawn_blocking(move || storage.report())
        .await
        .unwrap_or_else(|e| {
            (
                false,
                serde_json::json!({"ready": false, "error": e.to_string()}),
            )
        });
    body["mesh_nodes_connected"] = serde_json::json!(relay.mesh.connected());
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, axum::Json(body)).into_response()
}

/// `GET /metrics`: the state counters in Prometheus text format (no
/// account data).
async fn metrics(State(relay): State<Relay>) -> Response {
    let storage = relay.storage.clone();
    #[cfg_attr(not(feature = "hosted"), allow(unused_mut))]
    let mut text = tokio::task::spawn_blocking(move || storage.metrics())
        .await
        .unwrap_or_default();
    text.push_str(&relay.conns.render());
    text.push_str(&relay.mesh.metrics());
    #[cfg(feature = "hosted")]
    {
        text.push_str(&relay.org.metrics());
        if let Some(plans) = &relay.plans {
            text.push_str(&plans.metrics());
        }
    }
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        text,
    )
        .into_response()
}

/// Checks the admin token (`/relay/v1/*` admin views).
#[allow(clippy::result_large_err)]
pub(crate) fn require_admin(relay: &Relay, headers: &HeaderMap) -> Result<(), Response> {
    let Some(admin) = relay.config.admin_token.as_deref() else {
        return Err(text(StatusCode::NOT_FOUND, "admin endpoint disabled"));
    };
    let ok = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|t| {
            use subtle::ConstantTimeEq as _;
            t.len() == admin.len() && bool::from(t.as_bytes().ct_eq(admin.as_bytes()))
        });
    if !ok {
        let reason = if headers.contains_key(header::AUTHORIZATION) {
            Reason::InvalidAdminToken
        } else {
            Reason::MissingToken
        };
        auth_log::rejected(headers, Route::Admin, reason, None);
        return Err(text(StatusCode::UNAUTHORIZED, "invalid admin token"));
    }
    Ok(())
}

async fn machine_list(State(relay): State<Relay>, headers: HeaderMap) -> Response {
    if let Err(r) = require_admin(&relay, &headers) {
        return r;
    }
    axum::Json(relay.status()).into_response()
}

/// Resolves the machine and the path to forward for a client request. When
/// a host-mode domain is configured, path mode (`/m/<id>/…`) is refused:
/// every machine gets its own subdomain instead of sharing the relay's own
/// origin, so one machine's response can no longer be used against another
/// machine's viewer or against the relay's own pages (S6). Self-host a
/// relay without `--host-domain` to keep path mode (single registrable
/// origin, no wildcard certificate needed).
#[cfg(test)]
fn route(relay: &Relay, uri: &Uri, headers: &HeaderMap) -> Option<(String, String, String)> {
    route_ex(relay, uri, headers).map(|r| (r.id, r.forwarded, r.prefix))
}

/// A request for a `cua share` link, as the relay sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShareCtx {
    pub(crate) kind: crate::share_web::ShareKind,
    /// The share id (private) or token (public).
    pub(crate) sid: String,
    /// The request came in on the share's own origin
    /// (`s<id>-<machine>.<domain>`), not on the machine's origin or in
    /// path mode.
    pub(crate) isolated: bool,
    /// The `Host` the client used.
    pub(crate) host: String,
}

/// Where a client request goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Routed {
    pub(crate) id: String,
    pub(crate) forwarded: String,
    pub(crate) prefix: String,
    pub(crate) share: Option<ShareCtx>,
}

fn route_ex(relay: &Relay, uri: &Uri, headers: &HeaderMap) -> Option<Routed> {
    let path = uri.path();
    let query = uri.query().map(|q| format!("?{q}")).unwrap_or_default();
    let domain = relay.config.host_domain.as_deref();
    let host_header = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let share_of = |forwarded: &str| {
        crate::share_web::share_of_path(forwarded.split('?').next().unwrap_or(forwarded)).map(
            |(kind, sid)| ShareCtx {
                kind,
                sid,
                isolated: false,
                host: host_header.clone(),
            },
        )
    };
    if let Some(rest) = path.strip_prefix("/m/") {
        if domain.is_some() && !relay.config.keep_path_mode {
            return None;
        }
        let (id, tail) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        let forwarded = format!("{tail}{query}");
        let share = share_of(&forwarded);
        return Some(Routed {
            id: id.to_owned(),
            forwarded,
            prefix: format!("/m/{id}"),
            share,
        });
    }
    let domain = domain?;
    let host = headers.get(header::HOST)?.to_str().ok()?;
    let host = host.split(':').next()?;
    let label = host.strip_suffix(domain)?.strip_suffix('.')?;
    // A share's own origin: the path is the shared app's, rewritten for
    // the machine.
    if let Some(parsed) = crate::share_web::parse_label(label) {
        let (_, id) = relay.share_origin(host)?;
        return Some(Routed {
            id,
            forwarded: crate::share_web::machine_path(&parsed, path, uri.query()),
            prefix: String::new(),
            share: Some(ShareCtx {
                kind: parsed.kind,
                sid: parsed.sid,
                isolated: true,
                host: host_header,
            }),
        });
    }
    let forwarded = format!("{path}{query}");
    let share = share_of(&forwarded);
    Some(Routed {
        id: label.to_owned(),
        forwarded,
        prefix: String::new(),
        share,
    })
}

/// Where a browser that opened a path-mode share link
/// (`<relay>/m/<id>/share/<key>/…`, `…/share/pub/<token>/…`) on a relay that
/// now has a host domain and keeps path mode should go instead: the same
/// path on the share's own origin, `<s|p><key>-<id prefix>.<domain>`.
///
/// Only requests without an account credential header are redirected (a
/// browser never sends one; a tool that does keeps getting path mode), and
/// only when the label is well formed, so anything else is routed as before.
fn legacy_share_redirect(relay: &Relay, uri: &Uri, headers: &HeaderMap) -> Option<String> {
    let domain = relay.config.host_domain.as_deref()?;
    if !relay.config.keep_path_mode
        || headers.contains_key(header::AUTHORIZATION)
        || headers.contains_key("x-cua-env-authorization")
    {
        return None;
    }
    let rest = uri.path().strip_prefix("/m/")?;
    let (id, tail) = rest.split_once('/')?;
    let tail = format!("/{tail}");
    let (kind, sid) = crate::share_web::share_of_path(&tail)?;
    let (letter, base) = match kind {
        crate::share_web::ShareKind::Public => ('p', format!("/share/pub/{sid}")),
        crate::share_web::ShareKind::Private => ('s', format!("/share/{sid}")),
    };
    let id_prefix: String = id.chars().take(16).collect();
    let label = format!("{letter}{sid}-{id_prefix}");
    crate::share_web::parse_label(&label)?;
    let path = tail.strip_prefix(&base)?;
    let path = if path.is_empty() { "/" } else { path };
    if !path.starts_with('/') {
        return None;
    }
    let query = uri.query().map(|q| format!("?{q}")).unwrap_or_default();
    let scheme = relay.share_scheme(headers);
    Some(format!("{scheme}://{label}.{domain}{path}{query}"))
}

/// The address a request came from, for the public-share rate limit: the
/// socket peer, or, when the peer is a configured trusted proxy, the client
/// address it forwarded (see [`crate::proxies`]).
fn client_ip(trusted: &crate::proxies::TrustedProxies, request: &Request) -> String {
    let Some(peer) = request
        .extensions()
        .get::<axum::extract::ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip())
    else {
        return "unknown".into();
    };
    let xff = request
        .headers()
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|v| v.to_str().ok());
    trusted.client_ip(peer, xff).to_string()
}

/// Counts each connection route's outcome (see `conn_metrics`).
async fn count_connections(
    State(relay): State<Relay>,
    request: Request,
    next: axum::middleware::Next,
) -> Response {
    let response = next.run(request).await;
    if let Some((kind, outcome)) = crate::conn_metrics::classify(&response) {
        relay.conns.record(kind, outcome);
    }
    response
}

/// Share origins never fall through to the relay's own routes: every path
/// on `s<id>-<machine>.<domain>` belongs to the shared app.
async fn share_hosts(
    State(relay): State<Relay>,
    request: Request,
    next: axum::middleware::Next,
) -> Response {
    let on_share_origin = request
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .is_some_and(|host| relay.is_share_host(host));
    if on_share_origin {
        return proxy(State(relay), request).await;
    }
    next.run(request).await
}

fn has_client_credential(uri: &Uri, headers: &HeaderMap, forwarded_path: &str) -> bool {
    if headers.contains_key(header::AUTHORIZATION)
        || headers.contains_key("x-cua-env-authorization")
    {
        return true;
    }
    has_forwardable_credential(uri, headers, forwarded_path)
}

/// A credential that survives the relay stripping client credential
/// headers from account-machine requests: a machine-minted ticket or signed
/// URL (or the unauthenticated health probe). The machine validates it.
fn has_forwardable_credential(uri: &Uri, headers: &HeaderMap, forwarded_path: &str) -> bool {
    if forwarded_path == "/health" || forwarded_path.starts_with("/health?") {
        return true;
    }
    let query = uri.query().unwrap_or("");
    if query
        .split('&')
        .any(|p| p.starts_with("ticket=") || p.starts_with("sig="))
    {
        return true;
    }
    headers
        .get_all(header::SEC_WEBSOCKET_PROTOCOL)
        .iter()
        .filter_map(|v| v.to_str().ok())
        // `rcdp.v2.ticket.` is the legacy media ticket form.
        .any(|v| v.contains("cua.ticket.") || v.contains("rcdp.v2.ticket."))
}

const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-connection",
    "proxy-authenticate",
    "proxy-authorization",
    "transfer-encoding",
    "upgrade",
];

struct ActiveGuard(
    Arc<Machine>,
    Option<String>,
    Arc<AtomicU64>,
    // Held for its Drop: the metered stream closes with the guard.
    #[allow(dead_code)] Option<MeterStream>,
);

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
        self.2.fetch_sub(1, Ordering::SeqCst);
        if let Some(user) = &self.1 {
            self.0.client_leave(user);
        }
    }
}

/// Headers carrying client credentials the relay removes before forwarding
/// to an account machine (the assertion replaces them).
const CLIENT_CREDENTIAL_HEADERS: &[&str] = &[
    "authorization",
    RELAY_AUTHORIZATION_HEADER,
    "x-cua-env-authorization",
    DEVICE_SESSION_HEADER,
];

/// An account client let through: who, and how its device stands.
pub(crate) struct AccountClient {
    pub(crate) who: Identity,
    pub(crate) gate: crate::device_api::Gate,
}

/// Who a request let through to an account machine is.
#[derive(Default)]
pub(crate) struct Authorized {
    /// The signed-in account (an account token).
    pub(crate) client: Option<AccountClient>,
    /// The account a machine-minted ticket was issued to, when the request
    /// carries one instead: its streams are cut with that account's.
    pub(crate) ticket_user: Option<String>,
    /// When the request was authorized (ms).
    pub(crate) at: u64,
}

#[allow(clippy::result_large_err)]
/// Authorizes a client request for an account machine and rewrites its
/// credentials. Returns the account client to record in presence (none for
/// machine-minted tickets and signed URLs).
async fn authorize_account_client(
    relay: &Relay,
    id: &str,
    request: &mut Request,
    forwarded: &str,
    device_session: Option<&str>,
    share_session: Option<Identity>,
) -> Result<Authorized, Response> {
    // Taken before the role is computed: a revoke racing this request
    // counts as later (see `Grants::record`).
    let authorized_at = crate::grants::now_ms();
    let headers = request.headers();
    let Some(record) = relay.directory.get(id) else {
        return Err(refuse(
            headers,
            StatusCode::BAD_GATEWAY,
            GrpcCode::Unavailable,
            format!("computer {id:?} is not registered"),
        ));
    };
    let is_health = forwarded == "/health" || forwarded.starts_with("/health?");
    let identity = match (account_token(headers), &relay.config.oidc) {
        // A signed-in browser on a private share origin: the relay verified
        // its cookie already (machine, share and host bound).
        _ if share_session.is_some() => share_session.clone(),
        (Some(token), Some(oidc)) => match oidc.validate(token).await {
            Ok(identity) => Some(identity),
            Err(e) if !has_forwardable_credential(request.uri(), headers, forwarded) => {
                auth_log::rejected(headers, Route::Proxy, e.reason, Some(&e.facts));
                return Err(refuse(
                    headers,
                    StatusCode::UNAUTHORIZED,
                    GrpcCode::Unauthenticated,
                    format!("invalid account token: {e}"),
                ));
            }
            Err(_) => None,
        },
        _ => None,
    };
    if !record.sharing && !is_health {
        return Err(refuse(
            headers,
            StatusCode::FORBIDDEN,
            GrpcCode::PermissionDenied,
            "the host stopped sharing this computer",
        ));
    }
    let mut gate = None;
    let mut ticket_user = None;
    let assertion = match &identity {
        Some(who) => {
            use crate::org_access::{OrgDecision, TeamsSource};
            let source = relay.org.source();
            // Team-only sharing: a direct share is asked about before it is
            // used (the answer is cached; fail closed when the API is down
            // and the person was never asked about).
            let mut personal = record.role_of(who).is_some();
            if personal && relay.org.team_only_sharing() && record.owner.id != who.account {
                match relay
                    .org
                    .share_check(&record.owner.id, &crate::team_sharing::who_keys(who))
                    .await
                {
                    Ok(Ok(())) => {}
                    Ok(Err(_)) => personal = false,
                    Err(e) => {
                        tracing::warn!(machine = id, error = %e, "shared access refused: the Spaces API is unavailable");
                        return Err(refuse(
                            headers,
                            StatusCode::SERVICE_UNAVAILABLE,
                            GrpcCode::Unavailable,
                            "sharing cannot be checked right now; try again in a minute",
                        ));
                    }
                }
            }
            // The account token alone is not enough: the client device must
            // be enrolled (after the migration grace period).
            // A share session has no enrolled device behind it, by design
            // (see `share_web`): only that cookie, only on a share origin.
            let device_gate = if share_session.is_some() {
                Ok(crate::device_api::Gate::Off)
            } else {
                crate::device_api::gate(relay, device_session, who)
            };
            // The organization path (shadow / org): asked here, so the
            // synchronous re-checks find its answer cached.
            let org = if source.evaluates_orgs() && !personal {
                let device = device_gate.as_ref().ok().and_then(|g| g.device());
                let required = matches!(&device_gate, Ok(g) if *g != crate::device_api::Gate::Off);
                Some(
                    relay
                        .org
                        .decide(&who.user, &record.owner.id, &who.orgs, id, device, required)
                        .await,
                )
            } else {
                None
            };
            let shadow = |file: bool| {
                if let (TeamsSource::Shadow, Some(o)) = (source, &org) {
                    relay.org.shadow("open", &who.user, id, file, o);
                }
            };
            if source == TeamsSource::Org && !personal {
                match &org {
                    Some(OrgDecision::Granted { .. }) => {}
                    Some(OrgDecision::DeviceRefused { message, .. }) => {
                        return Err(refuse(
                            headers,
                            StatusCode::FORBIDDEN,
                            GrpcCode::PermissionDenied,
                            message.clone(),
                        ))
                    }
                    Some(OrgDecision::Unavailable(e)) => {
                        tracing::warn!(machine = id, error = %e, "team access refused: the Spaces API is unavailable");
                        return Err(refuse(
                            headers,
                            StatusCode::SERVICE_UNAVAILABLE,
                            GrpcCode::Unavailable,
                            "team access cannot be checked right now; try again in a minute",
                        ));
                    }
                    _ => {}
                }
            }
            // Not shared with them: a person a Space this computer provides
            // is shared with reaches that Space's thread here, and nothing
            // else ([`thread_grant`]).
            let (role, teams, thread_space) = match relay.machine_role(&record, who) {
                Some((role, teams)) => (role, teams, None),
                None => match thread_grant(relay, id, forwarded, headers, who) {
                    ThreadGrant::NotAsked => {
                        shadow(false);
                        return Err(refuse(
                            headers,
                            StatusCode::FORBIDDEN,
                            GrpcCode::PermissionDenied,
                            "this computer is not shared with your account",
                        ));
                    }
                    ThreadGrant::Refused(message) => {
                        shadow(false);
                        return Err(refuse_as(
                            headers,
                            StatusCode::FORBIDDEN,
                            GrpcCode::PermissionDenied,
                            message,
                            Some(THREAD_GRANT_REFUSED),
                        ));
                    }
                    ThreadGrant::Granted { space, role, teams } => (role, teams, Some(space)),
                },
            };
            match device_gate {
                Ok(g) => {
                    // Through a team, the device also needs the team's
                    // approval (a relay without device enrollment has no
                    // devices to approve). With organizations the API's
                    // answer already covered it.
                    if !teams.is_empty() && source != TeamsSource::Org {
                        let team = if g == crate::device_api::Gate::Off {
                            teams[0].clone()
                        } else {
                            match relay.teams.check_device_any(&teams, who, g.device(), id) {
                                (team, crate::teams::DeviceCheck::Approved) => team,
                                (_, crate::teams::DeviceCheck::Refused(message)) => {
                                    shadow(false);
                                    return Err(refuse(
                                        headers,
                                        StatusCode::FORBIDDEN,
                                        GrpcCode::PermissionDenied,
                                        message,
                                    ));
                                }
                            }
                        };
                        shadow(true);
                        relay.teams.record_open(&team, who, g.device(), id);
                    } else if !personal {
                        shadow(false);
                    }
                    crate::device_api::record_access(relay, &g, who, &record, role);
                    // A thread grant mints nothing on the machine that
                    // outlives it (its calls are limited there).
                    if thread_space.is_none() {
                        relay.grants.record(id, who, role, authorized_at);
                    }
                    gate = Some(g);
                }
                Err(message) => {
                    return Err(refuse(
                        headers,
                        StatusCode::FORBIDDEN,
                        GrpcCode::PermissionDenied,
                        message,
                    ))
                }
            }
            let now = assertion::now_secs();
            let claims = AssertionClaims {
                iss: relay.public_url(headers),
                aud: id.to_owned(),
                sub: who.user.clone(),
                acct: who.account.clone(),
                email: who.email.clone(),
                name: who.name.clone(),
                mid: id.to_owned(),
                // A thread grant's role is one no machine that predates it
                // accepts (it refuses the call); its share role rides in
                // `space_role`.
                role: if thread_space.is_some() {
                    assertion::THREAD_ROLE.to_owned()
                } else {
                    role.as_str().to_owned()
                },
                scope: "env".into(),
                iat: now,
                exp: now + assertion::MAX_ASSERTION_TTL_SECS,
                jti: uuid::Uuid::new_v4().simple().to_string(),
                teams: teams.clone(),
                space_role: thread_space.as_ref().map(|_| role.as_str().to_owned()),
                space: thread_space,
            };
            Some(relay.key.sign(&claims))
        }
        // Tickets and signed URLs were minted by the machine for an
        // authorized caller; the machine validates them itself.
        // (Credential headers are stripped below, so they do not count.)
        // A ticket names the account it was minted for: refused when that
        // account's access was revoked after it may have been minted
        // (CUA-1416).
        None if has_forwardable_credential(request.uri(), headers, forwarded) => {
            for ticket in crate::grants::tickets_in(request.uri(), headers)
                .iter()
                .filter_map(|t| crate::grants::parse_ticket(t))
            {
                if !relay.ticket_allowed(id, &record, &ticket) {
                    auth_log::rejected(headers, Route::Proxy, Reason::TicketRevoked, None);
                    return Err(refuse(
                        headers,
                        StatusCode::FORBIDDEN,
                        GrpcCode::PermissionDenied,
                        crate::grants::TICKET_REVOKED,
                    ));
                }
                if !ticket.user.is_empty() {
                    ticket_user = Some(ticket.user);
                }
            }
            None
        }
        None => {
            auth_log::rejected(headers, Route::Proxy, Reason::MissingToken, None);
            return Err(refuse(
                headers,
                StatusCode::UNAUTHORIZED,
                GrpcCode::Unauthenticated,
                "sign in: this computer accepts cua.ai account tokens",
            ));
        }
    };
    let headers = request.headers_mut();
    for name in CLIENT_CREDENTIAL_HEADERS {
        headers.remove(*name);
    }
    if let Some(assertion) = assertion {
        if let Ok(value) = HeaderValue::from_str(&assertion) {
            headers.insert(ASSERTION_HEADER, value);
        }
    }
    Ok(Authorized {
        client: identity
            .zip(gate)
            .map(|(who, gate)| AccountClient { who, gate }),
        ticket_user,
        at: authorized_at,
    })
}

/// Asks a machine for the thread of a Space it provides (its relay
/// machine id; `relay:` optional), on behalf of a person that Space is
/// shared with but the machine is not.
pub const THREAD_SPACE_HEADER: &str = "x-cua-thread-space";

/// What a thread grant reaches on the machine: the thread call and the
/// capability probe a client makes first.
pub const THREAD_GRANT_PATHS: &[&str] = &[
    "/cua.env.v1.HostThreadService/CallThreadTool",
    "/cua.env.v1.SystemService/GetCapabilities",
    "/cua.env.v1.SystemService/Health",
];

/// [`thread_grant`]'s answer.
enum ThreadGrant {
    /// No [`THREAD_SPACE_HEADER`]: not a thread call.
    NotAsked,
    /// Asked, and refused, saying why.
    Refused(String),
    /// The person's role on the Space (`Viewer` or `Shared`; owning the
    /// Space counts as `Shared`), and the teams it came through.
    Granted {
        space: String,
        role: crate::directory::Role,
        teams: Vec<String>,
    },
}

/// A person machine `host` is not shared with may reach the thread of a
/// Space it provides when that Space is shared with them: only
/// [`THREAD_GRANT_PATHS`], only that Space (the machine and its daemon
/// check both again), with the Space's role. Read from the directory on
/// every call, so revoking the Space's share ends it at once.
fn thread_grant(
    relay: &Relay,
    host: &str,
    forwarded: &str,
    headers: &HeaderMap,
    who: &crate::oidc::Identity,
) -> ThreadGrant {
    let Some(asked) = headers
        .get(THREAD_SPACE_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return ThreadGrant::NotAsked;
    };
    let space = asked.strip_prefix("relay:").unwrap_or(asked);
    let path = forwarded.split('?').next().unwrap_or(forwarded);
    if !THREAD_GRANT_PATHS.contains(&path) {
        return ThreadGrant::Refused(format!(
            "this computer is not shared with your account; relay:{space} being shared with \
             you reaches only its thread here, not {path}"
        ));
    }
    let not_shared = || {
        ThreadGrant::Refused(format!(
            "relay:{space} is not shared with your account, so its thread is not either"
        ))
    };
    let Some(record) = relay.directory.get(space) else {
        return not_shared();
    };
    if record.host.as_deref() != Some(host) {
        return ThreadGrant::Refused(format!(
            "relay:{space} is not a Space this computer provides"
        ));
    }
    if !record.sharing {
        return ThreadGrant::Refused(format!("relay:{space} stopped sharing"));
    }
    use crate::directory::Role;
    match relay.machine_role(&record, who) {
        Some((role, teams)) => ThreadGrant::Granted {
            space: space.to_owned(),
            role: if role == Role::Viewer {
                Role::Viewer
            } else {
                Role::Shared
            },
            teams,
        },
        None => not_shared(),
    }
}

async fn proxy(state: State<Relay>, request: Request) -> Response {
    let mut response = proxy_inner(state, request).await;
    response
        .extensions_mut()
        .insert(crate::conn_metrics::Kind::Client);
    response
}

async fn proxy_inner(State(relay): State<Relay>, mut request: Request) -> Response {
    if let Some(location) = legacy_share_redirect(&relay, request.uri(), request.headers()) {
        let mut response = Response::new(axum::body::Body::empty());
        *response.status_mut() = StatusCode::PERMANENT_REDIRECT;
        if let Ok(v) = HeaderValue::from_str(&location) {
            response.headers_mut().insert(header::LOCATION, v);
        }
        response.headers_mut().insert(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        );
        response
            .extensions_mut()
            .insert(crate::conn_metrics::Uncounted);
        return response;
    }
    let Some(Routed {
        id,
        forwarded,
        prefix,
        share,
    }) = route_ex(&relay, request.uri(), request.headers())
    else {
        let message = match &relay.config.host_domain {
            Some(domain) => format!("use <computer-id>.{domain}/…"),
            None => "use /m/<computer-id>/…".to_owned(),
        };
        // Not a computer route (stray paths, scanners): not a connection.
        let mut response = refuse(
            request.headers(),
            StatusCode::NOT_FOUND,
            GrpcCode::NotFound,
            message,
        );
        response
            .extensions_mut()
            .insert(crate::conn_metrics::Uncounted);
        return response;
    };
    use crate::share_web::{Cookie, ShareKind};
    // The session endpoint of a private share origin: trades a login code
    // for the share's cookie (never forwarded to the machine).
    if let Some(sh) = &share {
        if sh.isolated
            && sh.kind == ShareKind::Private
            && request.uri().path() == crate::share_web::SESSION_PATH
        {
            let mut response = crate::share_web::session(&relay, &request);
            response
                .extensions_mut()
                .insert(crate::conn_metrics::Uncounted);
            return response;
        }
    }
    // Only the relay mints assertions; device sessions are for the relay
    // alone and never reach a machine.
    request.headers_mut().remove(ASSERTION_HEADER);
    request.headers_mut().remove("x-cua-share-caller");
    let device_session = request.headers_mut().remove(DEVICE_SESSION_HEADER);
    let machine = relay.machines.lock().expect("machines").get(&id).cloned();
    let account_machine = matches!(
        machine.as_ref().map(|m| &m.registrant),
        Some(Registrant::Account(_))
    ) || (machine.is_none() && relay.directory.get(&id).is_some());
    // Share links (`/share/…` on an account machine): public ones carry
    // their own credential (the token), private ones are signed in with a
    // bearer token or, on their own origin, the share cookie.
    let mut session_identity = None;
    let mut public_share = false;
    if let Some(sh) = &share {
        if !account_machine {
            return refuse(
                request.headers(),
                StatusCode::NOT_FOUND,
                GrpcCode::NotFound,
                "not found",
            );
        }
        // What the app and the machine see of where they are served.
        let scheme = relay.share_scheme(request.headers());
        let cookie = if sh.isolated && sh.kind == ShareKind::Private {
            relay.share_cookie(request.headers(), &id, &sh.sid, &sh.host)
        } else {
            Cookie::Absent
        };
        let nav = crate::share_web::is_page_navigation(request.method(), request.headers());
        crate::share_web::strip_session_cookie(request.headers_mut());
        let headers = request.headers_mut();
        headers.remove("x-forwarded-host");
        headers.remove("x-forwarded-proto");
        if let Ok(v) = HeaderValue::from_str(&sh.host) {
            headers.insert("x-forwarded-host", v);
        }
        headers.insert("x-forwarded-proto", HeaderValue::from_static(scheme));
        match sh.kind {
            ShareKind::Public => {
                public_share = true;
                // The token is the credential: whatever else the client
                // sent is not for the app.
                for name in CLIENT_CREDENTIAL_HEADERS {
                    headers.remove(*name);
                }
                let record = relay.directory.get(&id);
                if !record.is_some_and(|r| r.sharing) {
                    return refuse(
                        request.headers(),
                        StatusCode::FORBIDDEN,
                        GrpcCode::PermissionDenied,
                        "the host stopped sharing this machine",
                    );
                }
                let client = client_ip(&relay.config.trusted_proxies, &request);
                if let Err(wait) = relay.share_rate_limit(&client, &id) {
                    auth_log::share_event("public_rate_limited", "429");
                    let mut response = refuse(
                        request.headers(),
                        StatusCode::TOO_MANY_REQUESTS,
                        GrpcCode::ResourceExhausted,
                        "too many requests to this link; slow down",
                    );
                    if let Ok(v) =
                        HeaderValue::from_str(&wait.as_secs().saturating_add(1).to_string())
                    {
                        response.headers_mut().insert(header::RETRY_AFTER, v);
                    }
                    return response;
                }
            }
            ShareKind::Private => match &cookie {
                Cookie::Valid(who) if account_token(request.headers()).is_none() => {
                    session_identity = Some(who.clone());
                }
                Cookie::Valid(_) | Cookie::Absent | Cookie::Invalid => {
                    let credential = account_token(request.headers()).is_some();
                    if !credential && sh.isolated && nav && relay.share_web_enabled() {
                        if matches!(cookie, Cookie::Invalid) {
                            auth_log::share_event("web_session_refused", "cookie_invalid");
                        }
                        let original = format!(
                            "{scheme}://{}{}",
                            sh.host,
                            request
                                .uri()
                                .path_and_query()
                                .map(|p| p.as_str())
                                .unwrap_or("/")
                        );
                        let mut response = relay.share_login_redirect(&original);
                        response
                            .extensions_mut()
                            .insert(crate::conn_metrics::Uncounted);
                        return response;
                    }
                }
            },
        }
    }
    let mut identity = None;
    let mut ticket_user = None;
    let mut authorized_at = 0;
    let mut grace = false;
    if public_share {
        // Anonymous by design: no identity, no assertion.
    } else if account_machine {
        let session = device_session.as_ref().and_then(|v| v.to_str().ok());
        match authorize_account_client(
            &relay,
            &id,
            &mut request,
            &forwarded,
            session,
            session_identity,
        )
        .await
        {
            Ok(authorized) => {
                if let Some(client) = authorized.client {
                    grace = matches!(client.gate, crate::device_api::Gate::Grace);
                    identity = Some(client.who);
                }
                ticket_user = authorized.ticket_user;
                authorized_at = authorized.at;
            }
            Err(response) => return response,
        }
    } else if relay.config.require_client_credentials
        && !has_client_credential(request.uri(), request.headers(), &forwarded)
    {
        auth_log::rejected(
            request.headers(),
            Route::Proxy,
            Reason::SpacesdCredentialMissing,
            None,
        );
        return refuse(
            request.headers(),
            StatusCode::UNAUTHORIZED,
            GrpcCode::Unauthenticated,
            "the relay forwards only requests carrying a spacesd credential",
        );
    }
    let machine = match machine {
        Some(machine) => Some(machine),
        None => relay.await_reconnect(&id).await,
    };
    let Some(machine) = machine else {
        let mut response = refuse(
            request.headers(),
            StatusCode::BAD_GATEWAY,
            GrpcCode::Unavailable,
            format!("computer {id:?} is not connected"),
        );
        response
            .extensions_mut()
            .insert(crate::conn_metrics::Offline);
        return response;
    };
    if machine.active.fetch_add(1, Ordering::SeqCst) >= relay.config.max_streams_per_machine {
        machine.active.fetch_sub(1, Ordering::SeqCst);
        return refuse(
            request.headers(),
            StatusCode::SERVICE_UNAVAILABLE,
            GrpcCode::ResourceExhausted,
            "too many concurrent streams for this computer",
        );
    }
    // The global memory budget (S7): checked in addition to the per-machine
    // cap above, so one account's worth of machines (or one machine with
    // many slow-reading clients) cannot by itself buffer enough to bring
    // down every other machine's connection. Backpressure, not buffering:
    // refused outright, never queued.
    if relay.global_streams.fetch_add(1, Ordering::SeqCst) >= relay.config.max_global_streams {
        relay.global_streams.fetch_sub(1, Ordering::SeqCst);
        machine.active.fetch_sub(1, Ordering::SeqCst);
        return refuse(
            request.headers(),
            StatusCode::SERVICE_UNAVAILABLE,
            GrpcCode::ResourceExhausted,
            "the relay is at its concurrent-stream budget; retry shortly",
        );
    }
    if let Some(who) = &identity {
        machine.client_enter(who);
    }
    // Measure only: past the allowance the stream still goes through, with
    // a header the client turns into an upgrade prompt (CUA-1360).
    // A ticketed WebSocket (a viewer's media stream) carries no account:
    // it is metered as the account that last used the machine.
    let metered_as = match &identity {
        Some(who) => Some(who.clone()),
        None if account_machine
            && !public_share
            && request.headers().contains_key(header::UPGRADE) =>
        {
            machine.recent_client()
        }
        None => None,
    };
    // An enforced plan (Pro) is refused at its live-session limit: a
    // WebSocket upgrade or a long gRPC stream that would open one session
    // too many gets 429 (ResourceExhausted for gRPC) here, whatever client
    // sent it. Decided at stream open from the method path; short unary
    // calls are never refused. In-memory, one lock, no network.
    let hold = request.headers().contains_key(header::UPGRADE)
        || holds_grpc_session(request.headers(), request.uri().path());
    let metered = match metered_as.as_ref() {
        Some(who) if hold => match relay.meter.enter_enforced(who, &id) {
            Ok(m) => m,
            Err(refusal) => {
                relay.global_streams.fetch_sub(1, Ordering::SeqCst);
                machine.active.fetch_sub(1, Ordering::SeqCst);
                if let Some(i) = &identity {
                    machine.client_leave(&i.user);
                }
                let mut response = refuse(
                    request.headers(),
                    StatusCode::TOO_MANY_REQUESTS,
                    GrpcCode::ResourceExhausted,
                    refusal.message(),
                );
                let h = response.headers_mut();
                for (k, v) in [
                    ("x-cua-relay-error", refusal.code.to_owned()),
                    ("x-cua-relay-plan", refusal.plan.clone()),
                    ("x-cua-relay-used", refusal.used.to_string()),
                    ("x-cua-relay-limit", refusal.limit.to_string()),
                ] {
                    if let Ok(v) = HeaderValue::from_str(&v) {
                        h.insert(k, v);
                    }
                }
                return response;
            }
        },
        Some(who) => relay.meter.enter(who, &id),
        None => None,
    };
    let allowance = metered.as_ref().and_then(|(_, over)| {
        crate::meter::allowance_header(*over, &relay.meter.config().upgrade_url)
    });
    let metered_user = metered_as.filter(|_| metered.is_some()).map(|w| w.user);
    let stream_user = identity.as_ref().map(|w| w.user.clone()).or(ticket_user);
    let cut = machine.cut_token(stream_user.as_deref());
    let guard = ActiveGuard(
        machine.clone(),
        identity.map(|w| w.user),
        relay.global_streams.clone(),
        metered.map(|(stream, _)| stream),
    );
    // A revoke that landed between authorizing this request and taking
    // its cut token: its cut missed this stream, so refuse it here (the
    // guard undoes the counts).
    if let Some(user) = &stream_user {
        if relay.grants.revoked_since(&id, user, authorized_at) {
            return refuse(
                request.headers(),
                StatusCode::FORBIDDEN,
                GrpcCode::PermissionDenied,
                crate::grants::TICKET_REVOKED,
            );
        }
    }
    let stream = match machine.handle.open().await {
        Ok(s) => s,
        Err(e) => {
            return refuse(
                request.headers(),
                StatusCode::BAD_GATEWAY,
                GrpcCode::Unavailable,
                format!("computer tunnel: {e}"),
            )
        }
    };
    let io = TokioIo::new(Counted::new(stream, machine.counters.clone()).compat());

    let is_upgrade =
        request.version() <= Version::HTTP_11 && request.headers().contains_key(header::UPGRADE);
    let client_upgrade = is_upgrade.then(|| hyper::upgrade::on(&mut request));
    if !prefix.is_empty() {
        if let Ok(value) = HeaderValue::from_str(&prefix) {
            request.headers_mut().insert("x-forwarded-prefix", value);
        }
    }
    if request.version() == Version::HTTP_2 {
        let (mut sender, connection) =
            match hyper::client::conn::http2::handshake(TokioExecutor::new(), io).await {
                Ok(pair) => pair,
                Err(e) => return text(StatusCode::BAD_GATEWAY, format!("h2 handshake: {e}")),
            };
        // hyper drives the h2 connection on its own task, so the response
        // body (not the connection future) carries the guard and the cut.
        let guard = Arc::new(guard);
        let body_guard = guard.clone();
        let body_cut = cut.clone();
        tokio::spawn(async move {
            let _guard = guard;
            tokio::select! {
                _ = connection => {}
                _ = cut.cancelled() => {}
            }
        });
        let authority = request
            .headers()
            .get(header::HOST)
            .and_then(|h| h.to_str().ok())
            .map(str::to_owned)
            .or_else(|| request.uri().authority().map(|a| a.to_string()))
            .unwrap_or_else(|| "spacesd".into());
        match format!("http://{authority}{forwarded}").parse() {
            Ok(uri) => *request.uri_mut() = uri,
            Err(_) => {
                return refuse(
                    request.headers(),
                    StatusCode::BAD_REQUEST,
                    GrpcCode::InvalidArgument,
                    "invalid path",
                )
            }
        }
        for name in HOP_BY_HOP {
            request.headers_mut().remove(*name);
        }
        request.headers_mut().remove(header::HOST);
        return match sender.send_request(request).await {
            Ok(mut response) => {
                harden_response(response.headers_mut(), &forwarded, share.as_ref());
                if grace {
                    crate::device_api::flag_grace(&relay, response.headers_mut());
                }
                if let Some(v) = allowance {
                    response
                        .headers_mut()
                        .insert(crate::meter::ALLOWANCE_HEADER, v);
                }
                response.map(|b| Body::new(GuardedBody::new(b, body_guard, body_cut)))
            }
            Err(e) => text(StatusCode::BAD_GATEWAY, format!("forward: {e}")),
        };
    }

    let (mut sender, connection) = match hyper::client::conn::http1::handshake(io).await {
        Ok(pair) => pair,
        Err(e) => return text(StatusCode::BAD_GATEWAY, format!("h1 handshake: {e}")),
    };
    let upgrade_cut = cut.clone();
    // The upgraded socket outlives the HTTP connection task; both hold the
    // guard so presence and stream limits cover WebSockets.
    let guard = Arc::new(guard);
    let upgrade_guard = guard.clone();
    let body_guard = guard.clone();
    let body_cut = cut.clone();
    tokio::spawn(async move {
        let _guard = guard;
        tokio::select! {
            _ = connection.with_upgrades() => {}
            _ = cut.cancelled() => {}
        }
    });
    match forwarded.parse() {
        Ok(uri) => *request.uri_mut() = uri,
        Err(_) => return text(StatusCode::BAD_REQUEST, "invalid path"),
    }
    if !is_upgrade {
        for name in HOP_BY_HOP {
            request.headers_mut().remove(*name);
        }
    }
    let mut response = match sender.send_request(request).await {
        Ok(r) => r,
        Err(e) => return text(StatusCode::BAD_GATEWAY, format!("forward: {e}")),
    };
    harden_response(response.headers_mut(), &forwarded, share.as_ref());
    if grace {
        crate::device_api::flag_grace(&relay, response.headers_mut());
    }
    if let Some(v) = allowance {
        response
            .headers_mut()
            .insert(crate::meter::ALLOWANCE_HEADER, v);
    }
    if response.status() == StatusCode::SWITCHING_PROTOCOLS {
        if let Some(user) = &metered_user {
            relay.meter.mark_live(user, &id);
            // The upgrade that opens one session too many gets the prompt.
            if let Some(v) = crate::meter::allowance_header(
                relay.meter.over_now(user),
                &relay.meter.config().upgrade_url,
            ) {
                response
                    .headers_mut()
                    .insert(crate::meter::ALLOWANCE_HEADER, v);
            }
        }
        if let Some(client_upgrade) = client_upgrade {
            let machine_upgrade = hyper::upgrade::on(&mut response);
            tokio::spawn(async move {
                let _guard = upgrade_guard;
                match tokio::try_join!(client_upgrade, machine_upgrade) {
                    Ok((client, machine)) => {
                        let mut client = TokioIo::new(client);
                        let mut machine = TokioIo::new(machine);
                        tokio::select! {
                            _ = tokio::io::copy_bidirectional(&mut client, &mut machine) => {}
                            _ = upgrade_cut.cancelled() => {}
                        }
                    }
                    Err(e) => tracing::debug!(%e, "upgrade failed"),
                }
            });
        }
    }
    response.map(|b| Body::new(GuardedBody::new(b, body_guard, body_cut)))
}

/// Path prefix of the HTML5 viewer spacesd serves at a fixed location
/// (`cua_spacesd_html5::VIEWER_PATH`; not depended on directly, since
/// `cua-spacesd` is server-only and the viewer is client code). It is the
/// only machine-served page allowed to render as HTML on the relay's own
/// origin (S6): everything else a machine sends back is data a client
/// library parses (gRPC, gRPC-Web, `/files`), never a page a browser should
/// navigate to.
const KNOWN_VIEWER_PATH: &str = "/viewer";

/// Hardens a machine's response before it reaches the client (S6). Every
/// account machine shares the relay's own origin (path mode) and, in host
/// mode, its parent registrable domain (until machines move to a separate
/// origin entirely; see the crate README). A machine an attacker registers
/// (S5 lowers the odds, but does not make it impossible) must not be able
/// to use that shared origin against other users: to set a cookie there, to
/// claim a broader Service Worker scope than its own page and intercept
/// later requests (including another machine's viewer page), or to get a
/// browser to render its response as a page at all outside the one known
/// viewer path.
/// Hardens a response for the kind of origin it is served on. A share's
/// own origin may render the shared app's HTML (nothing else lives there);
/// everywhere else the rules of [`harden_machine_response`] stand, with the
/// share headers added for `/share/` paths.
fn harden_response(headers: &mut HeaderMap, forwarded_path: &str, share: Option<&ShareCtx>) {
    match share {
        Some(sh) if sh.isolated => {
            headers.remove("service-worker-allowed");
            headers.remove("service-worker");
            // Cookies stay host-only on this one share's origin.
            crate::share_web::sanitize_set_cookies(headers);
            headers.insert(
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            );
            // Share links are not for search engines or referrer leaks.
            headers.insert(
                header::REFERRER_POLICY,
                HeaderValue::from_static("no-referrer"),
            );
            headers.insert(
                "x-robots-tag",
                HeaderValue::from_static("noindex, nofollow"),
            );
            if !headers.contains_key(header::CONTENT_SECURITY_POLICY) {
                headers.insert(
                    header::CONTENT_SECURITY_POLICY,
                    HeaderValue::from_static("frame-ancestors 'self'"),
                );
            }
        }
        Some(_) => {
            harden_machine_response(headers, forwarded_path);
            headers.insert(
                header::REFERRER_POLICY,
                HeaderValue::from_static("no-referrer"),
            );
            headers.insert(
                "x-robots-tag",
                HeaderValue::from_static("noindex, nofollow"),
            );
        }
        None => harden_machine_response(headers, forwarded_path),
    }
}

fn harden_machine_response(headers: &mut HeaderMap, forwarded_path: &str) {
    // spacesd sets no cookies (bearer tokens and tickets only); refuse one
    // that would ride the relay's own domain.
    headers.remove(header::SET_COOKIE);
    // `Service-Worker-Allowed` widens a worker's scope past its own script's
    // directory; without it a worker can only control paths at or below
    // where it was fetched from, but every machine's own script still sits
    // under the *same* origin as every other machine (path mode) or the
    // same parent domain (host mode), so strip it outright rather than try
    // to validate a scope.
    headers.remove("service-worker-allowed");
    headers.remove("service-worker");
    let path = forwarded_path.split('?').next().unwrap_or(forwarded_path);
    let is_known_viewer = path == KNOWN_VIEWER_PATH || path.starts_with("/viewer/");
    if !is_known_viewer {
        let is_html = headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.trim_start().to_ascii_lowercase().starts_with("text/html"));
        if is_html {
            // Never let a browser render an arbitrary machine response as a
            // page outside the one path meant to serve one.
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/octet-stream"),
            );
            headers.insert(
                header::CONTENT_DISPOSITION,
                HeaderValue::from_static("attachment"),
            );
        }
        if !headers.contains_key(header::CONTENT_SECURITY_POLICY) {
            headers.insert(
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_static("default-src 'none'; frame-ancestors 'none'"),
            );
        }
    }
    // Belt and suspenders against MIME-sniffing a crafted response into
    // something a browser executes or renders. The known viewer already
    // sets this itself (cua_spacesd_html5::serve); inserting again is a
    // harmless no-op for it.
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
}

/// A forwarded response body that keeps the stream's [`ActiveGuard`] alive
/// until it ends and fails as soon as the machine's clients are cut.
struct GuardedBody {
    inner: hyper::body::Incoming,
    _guard: Arc<ActiveGuard>,
    cut: std::pin::Pin<Box<tokio_util::sync::WaitForCancellationFutureOwned>>,
}

impl GuardedBody {
    fn new(inner: hyper::body::Incoming, guard: Arc<ActiveGuard>, cut: CancellationToken) -> Self {
        Self {
            inner,
            _guard: guard,
            cut: Box::pin(cut.cancelled_owned()),
        }
    }
}

impl http_body::Body for GuardedBody {
    type Data = bytes::Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        if std::future::Future::poll(self.cut.as_mut(), cx).is_ready() {
            return std::task::Poll::Ready(Some(Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionAborted,
                "the host stopped sharing",
            ))));
        }
        std::pin::Pin::new(&mut self.inner)
            .poll_frame(cx)
            .map_err(std::io::Error::other)
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> http_body::SizeHint {
        self.inner.size_hint()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending_machine(id: &str, generation: u64) -> (Arc<Machine>, tokio::io::DuplexStream) {
        use tokio_util::compat::TokioAsyncReadCompatExt as _;

        let (io, peer) = tokio::io::duplex(1024);
        let (inbound, _received) = mpsc::channel(1);
        let (handle, driver) = mux::spawn(
            io.compat(),
            yamux::Mode::Client,
            inbound,
            mux::MAX_STREAMS,
            mux::DEFAULT_WINDOW_BYTES,
        );
        (
            Arc::new(Machine {
                id: id.into(),
                registrant: Registrant::Static(0),
                version: String::new(),
                connected_at: SystemTime::now(),
                generation,
                handle,
                counters: Arc::default(),
                active: AtomicU32::new(0),
                last_heartbeat: Mutex::new(Instant::now()),
                abort: driver.abort_handle(),
                clients_cut: Mutex::default(),
                user_cuts: Mutex::default(),
                clients: Mutex::default(),
            }),
            peer,
        )
    }

    #[tokio::test]
    async fn machine_limit_is_independent_of_generation_order() {
        let relay = Relay::new(RelayConfig {
            max_machines_per_token: 2,
            ..RelayConfig::default()
        });
        let (first, _first_peer) = pending_machine("aaaaaaaaaaaaaaaa", 2);
        let (second, _second_peer) = pending_machine("bbbbbbbbbbbbbbbb", 3);
        let (late, _late_peer) = pending_machine("cccccccccccccccc", 1);
        assert!(!relay.insert_machine(first).1);
        assert!(!relay.insert_machine(second).1);

        // Its generation was allocated first, but it reached the registry last.
        let (replaced, over_limit) = relay.insert_machine(late);
        assert!(
            over_limit,
            "an earlier generation must not add a third slot: {:?}",
            relay.machine_ids()
        );
        assert!(replaced.is_none());
        assert_eq!(
            relay.machine_ids(),
            ["aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb"]
        );
    }

    #[tokio::test]
    async fn reconnecting_machine_replaces_its_slot_at_capacity() {
        let relay = Relay::new(RelayConfig {
            max_machines_per_token: 2,
            ..RelayConfig::default()
        });
        let (first, _first_peer) = pending_machine("aaaaaaaaaaaaaaaa", 1);
        let (second, _second_peer) = pending_machine("bbbbbbbbbbbbbbbb", 2);
        let (replacement, _replacement_peer) = pending_machine("aaaaaaaaaaaaaaaa", 3);
        assert!(!relay.insert_machine(first.clone()).1);
        assert!(!relay.insert_machine(second).1);

        let (replaced, over_limit) = relay.insert_machine(replacement.clone());
        assert!(!over_limit);
        assert!(Arc::ptr_eq(&replaced.expect("previous session"), &first));
        assert!(Arc::ptr_eq(
            &relay.machine(&replacement.id).unwrap(),
            &replacement
        ));
        assert_eq!(
            relay.machine_ids(),
            ["aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb"]
        );
    }

    fn relay(domain: Option<&str>) -> Relay {
        Relay::new(RelayConfig {
            host_domain: domain.map(str::to_owned),
            ..RelayConfig::default()
        })
    }

    #[test]
    fn path_routing_without_a_host_domain() {
        let r = relay(None);
        let h = HeaderMap::new();
        assert_eq!(
            route(
                &r,
                &"/m/abc123/cua.env.v1.SystemService/Health?x=1"
                    .parse()
                    .unwrap(),
                &h
            ),
            Some((
                "abc123".into(),
                "/cua.env.v1.SystemService/Health?x=1".into(),
                "/m/abc123".into()
            ))
        );
        assert_eq!(route(&r, &"/m/abc123".parse().unwrap(), &h).unwrap().1, "/");
    }

    #[test]
    fn a_host_domain_routes_by_subdomain_and_refuses_path_mode() {
        // S6: once a machine has its own origin, the shared `/m/<id>` path
        // on the main origin no longer forwards anything, so a machine
        // response can no longer be used against that shared origin.
        let r = relay(Some("relay.example"));
        let h = HeaderMap::new();
        assert_eq!(
            route(
                &r,
                &"/m/abc123/cua.env.v1.SystemService/Health?x=1"
                    .parse()
                    .unwrap(),
                &h
            ),
            None
        );
        let mut host = HeaderMap::new();
        host.insert(header::HOST, "abc123.relay.example:443".parse().unwrap());
        assert_eq!(
            route(&r, &"/files?sig=x".parse().unwrap(), &host),
            Some(("abc123".into(), "/files?sig=x".into(), String::new()))
        );
        assert_eq!(route(&relay(None), &"/files".parse().unwrap(), &host), None);
    }

    fn kept(domain: &str) -> Relay {
        Relay::new(RelayConfig {
            host_domain: Some(domain.into()),
            keep_path_mode: true,
            public_url: Some("https://relay.example".into()),
            ..RelayConfig::default()
        })
    }

    #[test]
    fn keep_path_mode_forwards_both_modes() {
        let r = kept("m.share.example");
        let h = HeaderMap::new();
        assert_eq!(
            route(
                &r,
                &"/m/abc123/cua.env.v1.SystemService/Health".parse().unwrap(),
                &h
            ),
            Some((
                "abc123".into(),
                "/cua.env.v1.SystemService/Health".into(),
                "/m/abc123".into()
            ))
        );
        let mut host = HeaderMap::new();
        host.insert(header::HOST, "abc123.m.share.example".parse().unwrap());
        assert_eq!(
            route(&r, &"/files".parse().unwrap(), &host),
            Some(("abc123".into(), "/files".into(), String::new()))
        );
        assert_eq!(
            r.machine_url(&h, "abc123"),
            "https://abc123.m.share.example"
        );
    }

    #[test]
    fn browsers_on_old_share_links_go_to_the_share_origin() {
        const ID: &str = "0123456789abcdef0123456789abcdef";
        let r = kept("m.share.example");
        let h = HeaderMap::new();
        let token = "a".repeat(40);
        let go = |r: &Relay, path: &str, h: &HeaderMap| {
            legacy_share_redirect(r, &path.parse().unwrap(), h)
        };
        assert_eq!(
            go(&r, &format!("/m/{ID}/share/pub/{token}/app/x.js?v=1"), &h).as_deref(),
            Some(
                format!("https://p{token}-0123456789abcdef.m.share.example/app/x.js?v=1").as_str()
            )
        );
        assert_eq!(
            go(&r, &format!("/m/{ID}/share/pub/{token}"), &h).as_deref(),
            Some(format!("https://p{token}-0123456789abcdef.m.share.example/").as_str())
        );
        assert_eq!(
            go(&r, &format!("/m/{ID}/share/aaaa1111bbbb2222/"), &h).as_deref(),
            Some("https://saaaa1111bbbb2222-0123456789abcdef.m.share.example/")
        );
        // A tool with an account token keeps path mode.
        let mut bearer = HeaderMap::new();
        bearer.insert(header::AUTHORIZATION, "Bearer t".parse().unwrap());
        assert_eq!(
            go(&r, &format!("/m/{ID}/share/aaaa1111bbbb2222/"), &bearer),
            None
        );
        // Not a share, a malformed share id, a non-hex machine id: routed as before.
        assert_eq!(go(&r, &format!("/m/{ID}/files"), &h), None);
        assert_eq!(go(&r, &format!("/m/{ID}/share/nothex/"), &h), None);
        assert_eq!(
            go(&r, "/m/my-machine-name/share/aaaa1111bbbb2222/", &h),
            None
        );
        // Without keep_path_mode (or without a host domain) nothing redirects.
        let strict = relay(Some("m.share.example"));
        assert_eq!(
            go(&strict, &format!("/m/{ID}/share/aaaa1111bbbb2222/"), &h),
            None
        );
        assert_eq!(
            go(
                &relay(None),
                &format!("/m/{ID}/share/aaaa1111bbbb2222/"),
                &h
            ),
            None
        );
    }

    #[test]
    fn machine_url_prefers_the_host_domain() {
        let mut config = RelayConfig {
            public_url: Some("https://relay.example".into()),
            ..RelayConfig::default()
        };
        let no_domain = Relay::new(config.clone());
        let h = HeaderMap::new();
        assert_eq!(
            no_domain.machine_url(&h, "abc123"),
            "https://relay.example/m/abc123"
        );
        config.host_domain = Some("m.relay.example".into());
        let with_domain = Relay::new(config);
        assert_eq!(
            with_domain.machine_url(&h, "abc123"),
            "https://abc123.m.relay.example"
        );
    }

    #[test]
    fn anonymous_requests_are_refused_but_credentials_pass() {
        let h = HeaderMap::new();
        assert!(!has_client_credential(&"/m/a/x".parse().unwrap(), &h, "/x"));
        assert!(has_client_credential(
            &"/m/a/health".parse().unwrap(),
            &h,
            "/health"
        ));
        assert!(has_client_credential(
            &"/m/a/tunnel?ticket=v1.x".parse().unwrap(),
            &h,
            "/tunnel"
        ));
        assert!(has_client_credential(
            &"/m/a/files?path=a&sig=b".parse().unwrap(),
            &h,
            "/files"
        ));
        let mut auth = HeaderMap::new();
        auth.insert("x-cua-env-authorization", "Bearer t".parse().unwrap());
        assert!(has_client_credential(
            &"/m/a/x".parse().unwrap(),
            &auth,
            "/x"
        ));
        // Headers are stripped for account machines, so they are not a
        // credential that lets an anonymous request through there.
        assert!(!has_forwardable_credential(
            &"/m/a/x".parse().unwrap(),
            &auth,
            "/x"
        ));
        assert!(has_forwardable_credential(
            &"/m/a/tunnel?ticket=v1.x".parse().unwrap(),
            &auth,
            "/tunnel"
        ));
    }

    #[test]
    fn machine_cookies_are_stripped() {
        let mut h = HeaderMap::new();
        h.append(
            header::SET_COOKIE,
            "a=1; Domain=example.com".parse().unwrap(),
        );
        h.append(header::SET_COOKIE, "b=2".parse().unwrap());
        h.insert(header::CONTENT_TYPE, "application/grpc".parse().unwrap());
        harden_machine_response(&mut h, "/tunnel");
        assert!(h.get(header::SET_COOKIE).is_none());
        assert_eq!(h.get(header::CONTENT_TYPE).unwrap(), "application/grpc");
    }

    #[test]
    fn a_machine_cannot_widen_a_service_worker_scope() {
        let mut h = HeaderMap::new();
        h.insert("service-worker-allowed", "/".parse().unwrap());
        h.insert("service-worker", "active".parse().unwrap());
        harden_machine_response(&mut h, "/anything");
        assert!(h.get("service-worker-allowed").is_none());
        assert!(h.get("service-worker").is_none());
    }

    #[test]
    fn only_the_known_viewer_may_answer_with_html() {
        let mut h = HeaderMap::new();
        h.insert(
            header::CONTENT_TYPE,
            "text/html; charset=utf-8".parse().unwrap(),
        );
        harden_machine_response(&mut h, "/phish?x=1");
        assert_eq!(
            h.get(header::CONTENT_TYPE).unwrap(),
            "application/octet-stream"
        );
        assert_eq!(h.get(header::CONTENT_DISPOSITION).unwrap(), "attachment");
        assert!(h.get(header::CONTENT_SECURITY_POLICY).is_some());
        assert_eq!(h.get(header::X_CONTENT_TYPE_OPTIONS).unwrap(), "nosniff");

        // The known viewer keeps its HTML and its own CSP (not overwritten).
        let mut viewer = HeaderMap::new();
        viewer.insert(
            header::CONTENT_TYPE,
            "text/html; charset=utf-8".parse().unwrap(),
        );
        viewer.insert(
            header::CONTENT_SECURITY_POLICY,
            "default-src 'self'".parse().unwrap(),
        );
        harden_machine_response(&mut viewer, "/viewer/index.html");
        assert_eq!(
            viewer.get(header::CONTENT_TYPE).unwrap(),
            "text/html; charset=utf-8"
        );
        assert_eq!(
            viewer.get(header::CONTENT_SECURITY_POLICY).unwrap(),
            "default-src 'self'"
        );
        // Bare /viewer (no trailing slash) counts too.
        let mut bare = HeaderMap::new();
        bare.insert(header::CONTENT_TYPE, "text/html".parse().unwrap());
        harden_machine_response(&mut bare, "/viewer");
        assert_eq!(bare.get(header::CONTENT_TYPE).unwrap(), "text/html");
        // A path that merely starts with "/viewer" as a prefix of another
        // word does not count.
        let mut lookalike = HeaderMap::new();
        lookalike.insert(header::CONTENT_TYPE, "text/html".parse().unwrap());
        harden_machine_response(&mut lookalike, "/viewership");
        assert_eq!(
            lookalike.get(header::CONTENT_TYPE).unwrap(),
            "application/octet-stream"
        );
    }
}
