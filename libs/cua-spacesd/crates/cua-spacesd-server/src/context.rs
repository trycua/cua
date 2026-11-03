// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! [`ServerContext`]: state shared by every service and by extension
//! providers (ticket minting and validation, principal, config, `Init`
//! defaults, audio uplink permission, shutdown).

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime};

use cua_proto::env::v1::{AudioUplinkAccess, AudioUplinkMode, Principal};
use tokio_util::sync::CancellationToken;

use crate::auth::{
    ticket_from_parts, AccessMode, Auth, PresentedTicket, TicketClaims, TicketError, TicketScope,
};
use crate::config::ServerConfig;

/// Who may open an audio uplink (`InitRequest.audio_uplink`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum AudioUplinkPolicy {
    /// No uplinks (initial).
    #[default]
    Disabled,
    /// Only these `Principal.id`s.
    Allowlist(BTreeSet<String>),
    /// Any authenticated caller.
    Any,
}

/// Defaults installed by `SystemService.Init`.
#[derive(Debug, Clone, Default)]
pub struct InitState {
    /// True once `Init` succeeded.
    pub initialized: bool,
    /// Environment added to every new process.
    pub env: BTreeMap<String, String>,
    /// Default OS user for processes and file operations.
    pub default_user: Option<String>,
    /// Default working directory.
    pub default_workdir: Option<PathBuf>,
    /// Diagnostic labels.
    pub labels: BTreeMap<String, String>,
    /// Audio uplink permission.
    pub audio_uplink: AudioUplinkPolicy,
    /// Environment set by the hotspot (`set_system_proxy`), layered below
    /// `env`.
    pub proxy_env: BTreeMap<String, String>,
    /// Environment the spawn channel adds to every new process
    /// (`CUA_SPACE`, `CUA_SPAWN`, `CUA_SPAWN_TOKEN_FILE`); `Init` keeps it.
    pub spawn_env: BTreeMap<String, String>,
}

struct Inner {
    config: Arc<ServerConfig>,
    auth: Arc<Auth>,
    mode: AccessMode,
    init: RwLock<InitState>,
    started: Instant,
    started_wall: SystemTime,
    shutdown: CancellationToken,
    restart: AtomicBool,
    cursor_probe: AtomicBool,
    media_sessions: AtomicU32,
    managed_processes: AtomicU32,
    local_addr: std::sync::OnceLock<std::net::SocketAddr>,
    /// A relay joined at start (`cua-spacesd join`): `AttachRelay` refuses.
    joined_at_start: AtomicBool,
    /// The relay attached with `SystemService.AttachRelay`, if any.
    relay: std::sync::Mutex<Option<RelayAttachment>>,
    token_rotations: tokio::sync::watch::Sender<u64>,
    session_revocations: tokio::sync::watch::Sender<u64>,
    connection_revocations: tokio::sync::watch::Sender<u64>,
    machine_renewals: tokio::sync::watch::Sender<u64>,
    /// This guest's sealed-delivery keypair (S1), at
    /// `<data_dir>/machine-seal.key` (0600), loaded or generated on first
    /// use ([`ServerContext::machine_seal`]). `None` until then, and for
    /// good if loading or generating it failed (reported to callers
    /// exactly like an older image that never had one, rather than
    /// crashing the server over it).
    machine_seal: std::sync::Mutex<Option<Arc<cua_machine_seal::MachineKeypair>>>,
    /// Rejects a replayed sealed envelope on unseal.
    seal_replay_guard: RwLock<Arc<cua_machine_seal::ReplayGuard>>,
}

/// A relay this running driver attached to (see
/// [`ServerContext::attach_relay`]).
pub struct RelayAttachment {
    /// The machine id it joined as.
    pub machine_id: String,
    stop: CancellationToken,
}

/// Shared server state. Cheap to clone.
#[derive(Clone)]
pub struct ServerContext {
    inner: Arc<Inner>,
}

impl ServerContext {
    /// Creates a context. `token` is the initial root token (if any).
    pub fn new(config: ServerConfig, token: Option<String>) -> Self {
        let mode = if config.await_token_file.is_some() {
            AccessMode::AwaitTokenFile
        } else if config.listen.ip().is_loopback() {
            AccessMode::OpenLoopback
        } else {
            AccessMode::Bootstrap
        };
        let cursor_probe = config.cursor_probe;
        let auth = Arc::new(Auth::new(token));
        if config.access_log {
            auth.set_access_log(Arc::new(crate::access_log::AccessLog::open(
                config.access_log_path(),
            )));
        }
        Self {
            inner: Arc::new(Inner {
                config: Arc::new(config),
                machine_seal: std::sync::Mutex::new(None),
                seal_replay_guard: RwLock::new(Arc::new(cua_machine_seal::ReplayGuard::new())),
                auth,
                mode,
                init: RwLock::new(InitState::default()),
                started: Instant::now(),
                started_wall: SystemTime::now(),
                shutdown: CancellationToken::new(),
                restart: AtomicBool::new(false),
                cursor_probe: AtomicBool::new(cursor_probe),
                media_sessions: AtomicU32::new(0),
                managed_processes: AtomicU32::new(0),
                local_addr: std::sync::OnceLock::new(),
                joined_at_start: AtomicBool::new(false),
                relay: std::sync::Mutex::new(None),
                token_rotations: tokio::sync::watch::channel(0).0,
                session_revocations: tokio::sync::watch::channel(0).0,
                connection_revocations: tokio::sync::watch::channel(0).0,
                machine_renewals: tokio::sync::watch::channel(0).0,
            }),
        }
    }

    /// Static configuration.
    pub fn config(&self) -> &ServerConfig {
        &self.inner.config
    }

    /// Root token store and HMAC key derivation.
    pub fn auth(&self) -> &Arc<Auth> {
        &self.inner.auth
    }

    /// Where this guest keeps its sealed-delivery key.
    pub fn machine_seal_path(&self) -> PathBuf {
        self.inner.config.data_dir.join("machine-seal.key")
    }

    /// Whether this guest may hold a machine-seal key yet. A driver that
    /// waits for its token (await-token-file, gateway bootstrap) creates
    /// none before the first token exists, so an image or golden snapshot
    /// taken before a Space owned the guest never contains one.
    fn machine_seal_allowed(&self) -> bool {
        self.inner.mode == AccessMode::OpenLoopback || self.inner.auth.has_token()
    }

    /// This guest's sealed-delivery keypair (S1), loaded from (or generated
    /// at) [`Self::machine_seal_path`] on first use. `None` while
    /// [`Self::machine_seal_allowed`] says no, or if loading or generating
    /// it failed.
    pub fn machine_seal(&self) -> Option<Arc<cua_machine_seal::MachineKeypair>> {
        let mut slot = self.inner.machine_seal.lock().expect("machine seal lock");
        if let Some(keypair) = slot.as_ref() {
            return Some(keypair.clone());
        }
        if !self.machine_seal_allowed() {
            return None;
        }
        match cua_machine_seal::MachineKeypair::load_or_create(&self.machine_seal_path()) {
            Ok(keypair) => Some(slot.insert(Arc::new(keypair)).clone()),
            Err(e) => {
                tracing::warn!(error = %e, "sealed delivery unavailable: could not load or create this guest's keypair");
                None
            }
        }
    }

    /// This guest's public sealed-delivery key, for
    /// `GetCapabilitiesResponse::machine_seal_public_key` (empty when
    /// unavailable).
    pub fn machine_seal_public_key(&self) -> Vec<u8> {
        self.machine_seal()
            .map(|k| k.public().0.to_vec())
            .unwrap_or_default()
    }

    /// The replay guard for unsealing a delivery (S1).
    pub fn seal_replay_guard(&self) -> Arc<cua_machine_seal::ReplayGuard> {
        self.inner
            .seal_replay_guard
            .read()
            .expect("replay guard lock")
            .clone()
    }

    /// This guest became a new machine (`InitRequest.new_machine`):
    /// regenerates the machine-seal keypair (saved over the old one, mixed
    /// with `entropy`; or, while no key may exist yet, removes any stale
    /// one), starts a fresh replay guard, renews the ticket secret used
    /// without a token, and tells providers to renew theirs
    /// ([`Self::machine_renewals`]: the media QUIC certificate). Returns
    /// false when the new key could not be saved (it is used from memory
    /// and the old file is removed, so a restart makes yet another one).
    pub fn renew_machine(&self, entropy: &[u8]) -> bool {
        let path = self.machine_seal_path();
        let mut saved = true;
        {
            let mut slot = self.inner.machine_seal.lock().expect("machine seal lock");
            if self.machine_seal_allowed() {
                let keypair = match cua_machine_seal::MachineKeypair::regenerate(&path, entropy) {
                    Ok(keypair) => keypair,
                    Err(e) => {
                        saved = false;
                        tracing::warn!(error = %e, "could not save the regenerated machine-seal key; using it from memory");
                        let _ = std::fs::remove_file(&path);
                        cua_machine_seal::MachineKeypair::generate_mixed(entropy)
                    }
                };
                *slot = Some(Arc::new(keypair));
            } else {
                *slot = None;
                match std::fs::remove_file(&path) {
                    Ok(()) => {
                        tracing::info!(path = %path.display(), "removed a machine-seal key that predates this guest's token")
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => {
                        saved = false;
                        tracing::warn!(path = %path.display(), error = %e, "could not remove a stale machine-seal key");
                    }
                }
            }
        }
        self.renew_process_secrets(entropy);
        saved
    }

    /// The in-memory part of [`Self::renew_machine`]: replay guard, ticket
    /// secret, providers' keys. Also run when the first token arrives in
    /// await-token-file and bootstrap modes, so a guest snapshotted while
    /// it waited for one (a warm-pool member, a microVM golden) never
    /// shares those with its clones.
    fn renew_process_secrets(&self, entropy: &[u8]) {
        *self
            .inner
            .seal_replay_guard
            .write()
            .expect("replay guard lock") = Arc::new(cua_machine_seal::ReplayGuard::new());
        self.inner.auth.renew_boot_secret(entropy);
        self.inner.machine_renewals.send_modify(|count| *count += 1);
    }

    /// The first token of a driver that waited for one (bootstrap Init,
    /// await-token-file) was installed: see [`Self::renew_process_secrets`].
    pub fn first_token_installed(&self) {
        self.renew_process_secrets(&[]);
    }

    /// Changes whenever this guest's machine secrets were renewed
    /// ([`Self::renew_machine`], [`Self::first_token_installed`]).
    /// Providers holding per-machine keys of their own regenerate them.
    pub fn machine_renewals(&self) -> tokio::sync::watch::Receiver<u64> {
        self.inner.machine_renewals.subscribe()
    }

    /// How callers are treated while no token is configured.
    pub fn access_mode(&self) -> AccessMode {
        self.inner.mode
    }

    /// Mints a ticket for `scope`, bound to `resource`.
    pub fn mint_ticket(
        &self,
        scope: TicketScope,
        resource: &str,
        principal: Option<&Principal>,
        ttl: Duration,
    ) -> (String, SystemTime) {
        self.mint_ticket_issued(scope, resource, principal, None, ttl)
    }

    /// [`Self::mint_ticket`] for a call the relay asserted at `issued`
    /// (`RelayCaller::issued_at`), recorded in the ticket.
    pub fn mint_ticket_issued(
        &self,
        scope: TicketScope,
        resource: &str,
        principal: Option<&Principal>,
        issued: Option<u64>,
        ttl: Duration,
    ) -> (String, SystemTime) {
        let principal_id = principal.map(|p| p.id.as_str()).unwrap_or("");
        self.inner
            .auth
            .mint_ticket_issued(scope, resource, principal_id, issued, ttl)
    }

    /// Validates a ticket string for `scope`.
    pub fn validate_ticket(
        &self,
        ticket: &str,
        scope: TicketScope,
    ) -> Result<TicketClaims, TicketError> {
        self.inner.auth.validate_ticket(ticket, scope)
    }

    /// Finds and validates the ticket on an HTTP/WebSocket request (query
    /// `ticket=` or subprotocol `cua.ticket.<ticket>`). On success returns
    /// the claims and the subprotocol to echo, if any.
    pub fn validate_request_ticket(
        &self,
        uri: &http::Uri,
        headers: &http::HeaderMap,
        scope: TicketScope,
    ) -> Result<(TicketClaims, Option<String>), TicketError> {
        let PresentedTicket {
            ticket,
            subprotocol,
        } = ticket_from_parts(uri, headers).ok_or(TicketError::Malformed)?;
        let claims = self.validate_ticket(&ticket, scope)?;
        Ok((claims, subprotocol))
    }

    /// Checks the root token on a plain HTTP request (for routes outside
    /// the gRPC surface, like `/mcp`), in `authorization` or
    /// `x-cua-env-authorization`. In bootstrap mode (no token yet on a
    /// non-loopback bind) and while awaiting the token file every such
    /// request is refused.
    pub fn check_bearer(&self, headers: &http::HeaderMap) -> bool {
        if !self.inner.auth.has_token() && self.inner.mode != AccessMode::OpenLoopback {
            return false;
        }
        self.inner.auth.check_headers(headers)
    }

    /// Snapshot of the `Init` defaults.
    pub fn init_state(&self) -> InitState {
        self.inner.init.read().expect("init lock").clone()
    }

    /// Mutates the `Init` defaults.
    pub fn update_init<R>(&self, f: impl FnOnce(&mut InitState) -> R) -> R {
        f(&mut self.inner.init.write().expect("init lock"))
    }

    /// Applies `InitRequest.audio_uplink` (UNSPECIFIED keeps the current
    /// policy).
    pub fn set_audio_uplink(&self, access: &AudioUplinkAccess) {
        let policy = match AudioUplinkMode::try_from(access.mode) {
            Ok(AudioUplinkMode::Disabled) => AudioUplinkPolicy::Disabled,
            Ok(AudioUplinkMode::Allowlist) => {
                AudioUplinkPolicy::Allowlist(access.principal_ids.iter().cloned().collect())
            }
            Ok(AudioUplinkMode::Any) => AudioUplinkPolicy::Any,
            _ => return,
        };
        self.update_init(|state| state.audio_uplink = policy);
    }

    /// Whether `principal` may open an audio uplink. The desktop provider
    /// calls this from `StreamService.OpenMedia`.
    pub fn audio_uplink_allowed(&self, principal: Option<&Principal>) -> bool {
        match &self.inner.init.read().expect("init lock").audio_uplink {
            AudioUplinkPolicy::Disabled => false,
            AudioUplinkPolicy::Any => true,
            AudioUplinkPolicy::Allowlist(ids) => principal.is_some_and(|p| ids.contains(&p.id)),
        }
    }

    /// `PresenceSettings.cursor_probe`: whether presence may probe the real
    /// cursor shape by briefly moving the idle guest pointer.
    pub fn cursor_probe(&self) -> bool {
        self.inner.cursor_probe.load(Ordering::SeqCst)
    }

    /// Applies `InitRequest.presence` (unset fields keep the current value).
    pub fn set_presence_settings(&self, settings: &cua_proto::env::v1::PresenceSettings) {
        if let Some(on) = settings.cursor_probe {
            self.inner.cursor_probe.store(on, Ordering::SeqCst);
        }
    }

    /// Current audio uplink policy.
    pub fn audio_uplink_policy(&self) -> AudioUplinkPolicy {
        self.inner
            .init
            .read()
            .expect("init lock")
            .audio_uplink
            .clone()
    }

    /// Time since the server started.
    pub fn uptime(&self) -> Duration {
        self.inner.started.elapsed()
    }

    /// Wall-clock start time.
    pub fn started_at(&self) -> SystemTime {
        self.inner.started_wall
    }

    /// Cancelled when the server begins shutting down.
    pub fn shutdown_token(&self) -> CancellationToken {
        self.inner.shutdown.clone()
    }

    /// Starts a graceful shutdown; with `restart`, the binary re-execs itself
    /// afterwards.
    pub fn request_shutdown(&self, restart: bool) {
        if restart {
            self.inner.restart.store(true, Ordering::SeqCst);
        }
        self.inner.shutdown.cancel();
    }

    /// True if a `DRIVER_RESTART` shutdown was requested.
    pub fn restart_requested(&self) -> bool {
        self.inner.restart.load(Ordering::SeqCst)
    }

    /// Records that `Init` installed or rotated the root token. Tickets are
    /// already invalid (their HMAC key derives from the token); providers
    /// subscribe to close sockets that attached with them (close code 4401).
    pub fn notify_token_rotated(&self) {
        self.inner.token_rotations.send_modify(|count| *count += 1);
    }

    /// Applies the token file's current content (await-token-file mode):
    /// `Some` installs or rotates the token, `None` revokes it. Replacing or
    /// removing a live token also revokes every session opened under it:
    /// media sockets close (4401), forwards and the hotspot stop, and every
    /// open HTTP/gRPC connection is dropped. A revocation also resets the
    /// `Init` defaults, since the next claim is a new tenant. Returns true if
    /// anything changed.
    pub fn apply_file_token(&self, token: Option<&str>) -> bool {
        let auth = &self.inner.auth;
        let had_token = auth.has_token();
        let changed = match token {
            Some(token) => auth.set_token(token),
            None => auth.clear_token(),
        };
        if !changed {
            return false;
        }
        match (had_token, token.is_some()) {
            (false, true) => tracing::info!("access token installed from the token file"),
            (true, true) => {
                tracing::info!("access token rotated by the token file; sessions revoked")
            }
            _ => tracing::info!("access token revoked (token file emptied); awaiting a new one"),
        }
        if token.is_none() {
            self.update_init(|state| *state = InitState::default());
        }
        if !had_token && token.is_some() {
            self.first_token_installed();
        }
        self.notify_token_rotated();
        if had_token {
            self.revoke_sessions();
        }
        true
    }

    /// Drops every session authorized by the previous token (see
    /// [`Self::session_revocations`]) and every open connection.
    pub fn revoke_sessions(&self) {
        self.revoke_sessions_keep_connections();
        self.inner
            .connection_revocations
            .send_modify(|count| *count += 1);
    }

    /// Stops forwards, the hotspot and the volume mount opened before
    /// ([`Self::session_revocations`]) but keeps connections open: every
    /// later call on them is checked against the current token anyway, and
    /// the caller that renewed the machine (`Init`) keeps its channel.
    pub fn revoke_sessions_keep_connections(&self) {
        self.inner
            .session_revocations
            .send_modify(|count| *count += 1);
    }

    /// Changes whenever sessions must be revoked: the tunnel service stops
    /// forwards and the hotspot, the volume unmounts.
    pub fn session_revocations(&self) -> tokio::sync::watch::Receiver<u64> {
        self.inner.session_revocations.subscribe()
    }

    /// Changes whenever open connections must be dropped (the token file
    /// rotated or revoked the token: no stream outlives the token that
    /// authorized it).
    pub fn connection_revocations(&self) -> tokio::sync::watch::Receiver<u64> {
        self.inner.connection_revocations.subscribe()
    }

    /// True while waiting for the token file to provide a token.
    pub fn awaiting_token(&self) -> bool {
        self.inner.mode == AccessMode::AwaitTokenFile && !self.inner.auth.has_token()
    }

    /// Changes whenever the root token is installed or rotated.
    pub fn token_rotations(&self) -> tokio::sync::watch::Receiver<u64> {
        self.inner.token_rotations.subscribe()
    }

    /// Open media sessions (maintained by the desktop provider).
    pub fn media_sessions(&self) -> &AtomicU32 {
        &self.inner.media_sessions
    }

    /// Managed processes (maintained by the process service).
    pub fn managed_processes(&self) -> &AtomicU32 {
        &self.inner.managed_processes
    }

    /// Marks this driver as joined to a relay at start (`join`).
    pub fn mark_joined_at_start(&self) {
        self.inner.joined_at_start.store(true, Ordering::SeqCst);
    }

    /// The policy file of a relay attached at run time.
    pub fn share_policy_path(&self) -> std::path::PathBuf {
        self.inner.config.data_dir.join("share-policy.json")
    }

    /// Joins a relay as machine `machine_id` of `owner` without a restart:
    /// pins the relay's keys, writes the share policy (owner, sharing on,
    /// the relay's allowlist trusted), installs relay-assertion auth and
    /// starts the tunnel. Replaces an earlier attachment.
    pub fn attach_relay(
        &self,
        relay_url: &str,
        machine_token: &str,
        machine_id: &str,
        jwks_json: &str,
        owner: &str,
        owner_email: &str,
    ) -> Result<(), String> {
        if self.inner.joined_at_start.load(Ordering::SeqCst) {
            return Err("this driver joined a relay at start (cua-spacesd join)".into());
        }
        if !cua_relay::valid_machine_id(machine_id) {
            return Err(format!("invalid computer id {machine_id:?}"));
        }
        if machine_token.is_empty() || relay_url.is_empty() || owner.is_empty() {
            return Err("relay_url, machine_token and owner are required".into());
        }
        let local = self
            .local_addr()
            .ok_or_else(|| "the driver is not listening yet".to_string())?;
        let policy = self.share_policy_path();
        if let Some(dir) = policy.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let body = serde_json::json!({
            "owner": owner,
            "owner_email": (!owner_email.is_empty()).then_some(owner_email),
            "sharing": true,
            "trust_relay_allowlist": true,
        });
        write_private(&policy, body.to_string().as_bytes())
            .map_err(|e| format!("{}: {e}", policy.display()))?;
        let mut join = cua_relay::client::JoinConfig::new(
            relay_url.to_owned(),
            machine_token.to_owned(),
            machine_id.to_owned(),
            local,
        );
        join.version = env!("CARGO_PKG_VERSION").into();
        join.heartbeat = std::time::Duration::from_secs(5);
        join.account.pin_jwks_json(jwks_json)?;
        *join.account.owner.write().expect("owner") = Some(owner.to_owned());
        self.inner
            .auth
            .set_external(Arc::new(crate::relay_account::RelayAssertionAuth::new(
                machine_id.to_owned(),
                join.account.clone(),
                Some(policy),
            )));
        let stop = self.inner.shutdown.child_token();
        let previous = self
            .inner
            .relay
            .lock()
            .expect("relay lock")
            .replace(RelayAttachment {
                machine_id: machine_id.to_owned(),
                stop: stop.clone(),
            });
        if let Some(p) = previous {
            p.stop.cancel();
        }
        tracing::info!(machine = %machine_id, "attached to relay");
        let mut mesh = self.mesh_host_config(
            relay_url,
            machine_token,
            None,
            machine_id,
            join.account.clone(),
            local,
        );
        // A machine attached at run time is a Space: it runs no gangs.
        mesh.gang_home = None;
        tokio::spawn(crate::mesh_host::run(mesh, stop.clone()));
        tokio::spawn(cua_relay::client::run(join, stop));
        Ok(())
    }

    /// The machine's mesh settings next to a relay join: its node lives in
    /// `<data dir>/mesh`, peers reach this listener and the QUIC media
    /// port, and static-mode pairing proofs are checked against the env
    /// token. The node runs only while [`cua_mesh::policy`] allows it:
    /// `CUA_MESH`, else the channel the Cua Spaces app recorded in this
    /// machine's Cua home (the data dir's parent). Gang jobs of that Cua
    /// home reach the account's other machines through it
    /// ([`crate::mesh_host::MeshHostConfig::gang_home`]).
    pub fn mesh_host_config(
        &self,
        relay_url: &str,
        relay_token: &str,
        relay_token_file: Option<std::path::PathBuf>,
        machine_id: &str,
        account: Arc<cua_relay::client::AccountLink>,
        local: std::net::SocketAddr,
    ) -> crate::mesh_host::MeshHostConfig {
        let ip = if local.ip().is_unspecified() {
            std::net::IpAddr::from([127, 0, 0, 1])
        } else {
            local.ip()
        };
        let quic_port = self.config().media_quic_port;
        let auth = self.inner.auth.clone();
        let home = self
            .config()
            .data_dir
            .parent()
            .map(std::path::Path::to_path_buf);
        crate::mesh_host::MeshHostConfig {
            relay_url: relay_url.to_owned(),
            relay_token: relay_token.to_owned(),
            relay_token_file,
            machine_id: machine_id.to_owned(),
            dir: self.config().data_dir.join("mesh"),
            local: std::net::SocketAddr::new(ip, local.port()),
            quic: (quic_port != 0).then(|| std::net::SocketAddr::new(ip, quic_port)),
            media_udp_port: self.config().mesh_udp_port,
            account,
            env_token: Arc::new(move || auth.token().map(|t| t.to_string())),
            name: cua_mesh::node::hostname(),
            policy: Arc::new(cua_mesh::Policy::current),
            recheck: crate::mesh_host::GATE_RECHECK,
            gang_home: home,
            gang_wait: crate::mesh_host::GANG_WAIT,
        }
    }

    /// Leaves the relay attached with [`Self::attach_relay`]. Returns
    /// false when none was attached.
    pub fn detach_relay(&self) -> bool {
        let Some(a) = self.inner.relay.lock().expect("relay lock").take() else {
            return false;
        };
        a.stop.cancel();
        self.inner.auth.clear_external();
        let _ = std::fs::remove_file(self.share_policy_path());
        tracing::info!(machine = %a.machine_id, "detached from relay");
        true
    }

    /// The machine id of the relay attached at run time, if any.
    pub fn attached_relay(&self) -> Option<String> {
        self.inner
            .relay
            .lock()
            .expect("relay lock")
            .as_ref()
            .map(|a| a.machine_id.clone())
    }

    /// Records the address the server is actually bound to (set once by
    /// [`crate::Server::serve`]; later calls are ignored).
    pub fn set_local_addr(&self, addr: std::net::SocketAddr) {
        let _ = self.inner.local_addr.set(addr);
    }

    /// The bound address, once serving.
    pub fn local_addr(&self) -> Option<std::net::SocketAddr> {
        self.inner.local_addr.get().copied()
    }
}

/// Writes `bytes` to `path` readable by the owner only.
fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let tmp = path.with_extension("tmp");
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    std::fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_probe_starts_from_config_and_follows_init() {
        let ctx = ServerContext::new(ServerConfig::default(), None);
        assert!(ctx.cursor_probe(), "on by default");
        ctx.set_presence_settings(&cua_proto::env::v1::PresenceSettings {
            cursor_probe: Some(false),
        });
        assert!(!ctx.cursor_probe());
        // Unset keeps the current setting.
        ctx.set_presence_settings(&cua_proto::env::v1::PresenceSettings::default());
        assert!(!ctx.cursor_probe());
        let off = ServerContext::new(
            ServerConfig {
                cursor_probe: false,
                ..ServerConfig::default()
            },
            None,
        );
        assert!(!off.cursor_probe());
    }

    #[test]
    fn the_mesh_media_port_follows_the_config() {
        let host = |ctx: &ServerContext| {
            ctx.mesh_host_config(
                "https://relay.example",
                "token",
                None,
                "machine-1",
                Arc::default(),
                "127.0.0.1:3211".parse().unwrap(),
            )
        };
        // Unset: the port every release has used.
        let ctx = ServerContext::new(ServerConfig::default(), None);
        assert_eq!(host(&ctx).media_udp_port, 3212);
        // An override (`--mesh-udp-port`) reaches the mesh node.
        let local = ServerContext::new(
            ServerConfig {
                mesh_udp_port: 3222,
                ..ServerConfig::default()
            },
            None,
        );
        assert_eq!(host(&local).media_udp_port, 3222);
    }

    #[test]
    fn audio_uplink_policy_follows_init() {
        let ctx = ServerContext::new(ServerConfig::default(), None);
        let alice = Principal {
            id: "alice".into(),
            ..Default::default()
        };
        assert!(!ctx.audio_uplink_allowed(Some(&alice)));
        ctx.set_audio_uplink(&AudioUplinkAccess {
            mode: AudioUplinkMode::Allowlist as i32,
            principal_ids: vec!["alice".into()],
        });
        assert!(ctx.audio_uplink_allowed(Some(&alice)));
        assert!(!ctx.audio_uplink_allowed(None));
        // UNSPECIFIED keeps the current policy.
        ctx.set_audio_uplink(&AudioUplinkAccess::default());
        assert!(ctx.audio_uplink_allowed(Some(&alice)));
        ctx.set_audio_uplink(&AudioUplinkAccess {
            mode: AudioUplinkMode::Any as i32,
            principal_ids: vec![],
        });
        assert!(ctx.audio_uplink_allowed(None));
    }

    #[tokio::test]
    async fn file_token_install_rotate_revoke() {
        let config = ServerConfig {
            listen: "0.0.0.0:0".parse().unwrap(),
            await_token_file: Some("/nonexistent/env-token".into()),
            ..ServerConfig::default()
        };
        let ctx = ServerContext::new(config, None);
        assert_eq!(ctx.access_mode(), AccessMode::AwaitTokenFile);
        assert!(ctx.awaiting_token());
        let mut rotations = ctx.token_rotations();
        let mut revocations = ctx.session_revocations();
        let mut headers = http::HeaderMap::new();
        // Awaiting: plain-HTTP bearer checks fail closed.
        assert!(!ctx.check_bearer(&headers));
        headers.insert(http::header::AUTHORIZATION, "Bearer aaaa".parse().unwrap());
        assert!(!ctx.check_bearer(&headers));

        assert!(ctx.apply_file_token(Some("aaaa")));
        assert!(!ctx.awaiting_token());
        assert!(ctx.check_bearer(&headers));
        assert!(rotations.has_changed().unwrap());
        rotations.mark_unchanged();
        // Installing into an empty slot revokes nothing.
        assert!(!revocations.has_changed().unwrap());
        assert!(!ctx.apply_file_token(Some("aaaa")), "same token: no change");

        ctx.update_init(|s| s.labels.insert("k".into(), "v".into()));
        assert!(ctx.apply_file_token(Some("bbbb")));
        assert!(!ctx.check_bearer(&headers));
        assert!(rotations.has_changed().unwrap());
        assert!(revocations.has_changed().unwrap());
        revocations.mark_unchanged();
        assert!(
            ctx.init_state().labels.contains_key("k"),
            "rotation keeps defaults"
        );

        assert!(ctx.apply_file_token(None));
        assert!(ctx.awaiting_token());
        assert!(revocations.has_changed().unwrap());
        assert!(
            ctx.init_state().labels.is_empty(),
            "revocation resets defaults"
        );
        assert!(!ctx.apply_file_token(None));
    }
}
