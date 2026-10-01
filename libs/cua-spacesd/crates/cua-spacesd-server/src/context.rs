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
    /// This guest's sealed-delivery keypair (S1), generated at
    /// `<data_dir>/machine-seal.key` (0600) on first start. `None` only if
    /// loading or generating it failed (reported to callers exactly like
    /// an older image that never had one, rather than crashing the
    /// server over it).
    machine_seal: Option<Arc<cua_machine_seal::MachineKeypair>>,
    /// Rejects a replayed sealed envelope on unseal.
    seal_replay_guard: Arc<cua_machine_seal::ReplayGuard>,
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
        let machine_seal = match cua_machine_seal::MachineKeypair::load_or_create(
            &config.data_dir.join("machine-seal.key"),
        ) {
            Ok(k) => Some(Arc::new(k)),
            Err(e) => {
                tracing::warn!(error = %e, "sealed delivery unavailable: could not load or create this guest's keypair");
                None
            }
        };
        Self {
            inner: Arc::new(Inner {
                config: Arc::new(config),
                machine_seal,
                seal_replay_guard: Arc::new(cua_machine_seal::ReplayGuard::new()),
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

    /// This guest's sealed-delivery keypair (S1), if loading or generating
    /// one at start succeeded.
    pub fn machine_seal(&self) -> Option<&Arc<cua_machine_seal::MachineKeypair>> {
        self.inner.machine_seal.as_ref()
    }

    /// This guest's public sealed-delivery key, for
    /// `GetCapabilitiesResponse::machine_seal_public_key` (empty when
    /// unavailable).
    pub fn machine_seal_public_key(&self) -> Vec<u8> {
        self.inner
            .machine_seal
            .as_ref()
            .map(|k| k.public().0.to_vec())
            .unwrap_or_default()
    }

    /// The replay guard for unsealing a delivery (S1).
    pub fn seal_replay_guard(&self) -> &Arc<cua_machine_seal::ReplayGuard> {
        &self.inner.seal_replay_guard
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
        let principal_id = principal.map(|p| p.id.as_str()).unwrap_or("");
        self.inner
            .auth
            .mint_ticket(scope, resource, principal_id, ttl)
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
        self.notify_token_rotated();
        if had_token {
            self.revoke_sessions();
        }
        true
    }

    /// Drops every session authorized by the previous token (see
    /// [`Self::session_revocations`]).
    pub fn revoke_sessions(&self) {
        self.inner
            .session_revocations
            .send_modify(|count| *count += 1);
    }

    /// Changes whenever sessions must be revoked: the server drops open
    /// connections, the tunnel service stops forwards and the hotspot.
    pub fn session_revocations(&self) -> tokio::sync::watch::Receiver<u64> {
        self.inner.session_revocations.subscribe()
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
            return Err(format!("invalid machine id {machine_id:?}"));
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
        tracing::info!(relay = %relay_url, machine = %machine_id, "attached to relay");
        tokio::spawn(cua_relay::client::run(join, stop));
        Ok(())
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
