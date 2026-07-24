//! Immutable authorization contexts for shared-daemon sessions.
//!
//! Public session strings are lifecycle labels, not credentials. This module
//! therefore binds delegated authority to an opaque, already-authenticated
//! action connection. The current daemon has no such action transport, so its
//! live calls resolve to a process-owned legacy context and delegated session
//! creation remains disabled.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use uuid::Uuid;

use crate::authorization::PermissionMode;
use crate::session_manifest::SessionManifest;

#[cfg(test)]
const DEFAULT_MAX_SESSION_TTL: Duration = Duration::from_secs(24 * 60 * 60);
#[cfg(test)]
const DEFAULT_MAX_IDLE_TTL: Duration = Duration::from_secs(60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorizationContextSource {
    Process,
    TrustedHost,
}

impl AuthorizationContextSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Process => "process",
            Self::TrustedHost => "trusted_host",
        }
    }
}

/// Immutable modes and policy bindings a daemon instance may host.
///
/// Modes are an explicit set rather than an ordering: `standard`, `bounded`,
/// and `unrestricted` have different semantics, so none implicitly contains
/// another.
#[derive(Debug, Clone)]
pub struct SessionModeCeiling {
    allowed_modes: HashSet<PermissionMode>,
    unrestricted_acknowledged: bool,
    delegation_enabled: bool,
    max_session_ttl: Option<Duration>,
    max_idle_ttl: Option<Duration>,
    user_policy_sha256: Option<String>,
    managed_policy_sha256: Option<String>,
}

impl SessionModeCeiling {
    fn process(mode: PermissionMode) -> Result<Self, String> {
        Ok(Self {
            allowed_modes: HashSet::from([mode]),
            unrestricted_acknowledged: mode == PermissionMode::Unrestricted,
            delegation_enabled: false,
            max_session_ttl: None,
            max_idle_ttl: None,
            user_policy_sha256: crate::policy::user_policy_sha256()?,
            managed_policy_sha256: crate::policy::managed_policy_sha256()?,
        })
    }

    #[cfg(test)]
    fn delegated(
        allowed_modes: impl IntoIterator<Item = PermissionMode>,
        unrestricted_acknowledged: bool,
    ) -> Self {
        Self {
            allowed_modes: allowed_modes.into_iter().collect(),
            unrestricted_acknowledged,
            delegation_enabled: true,
            max_session_ttl: Some(DEFAULT_MAX_SESSION_TTL),
            max_idle_ttl: Some(DEFAULT_MAX_IDLE_TTL),
            user_policy_sha256: None,
            managed_policy_sha256: None,
        }
    }

    /// Build an immutable ceiling for sessions created by trusted host code.
    ///
    /// This constructor is intentionally absent from the agent tool registry.
    /// A host may call it only before its runtime begins accepting actions.
    pub fn for_trusted_sessions(
        allowed_modes: impl IntoIterator<Item = PermissionMode>,
        unrestricted_acknowledged: bool,
        max_session_ttl: Duration,
        max_idle_ttl: Duration,
    ) -> Result<Self, String> {
        let allowed_modes = allowed_modes.into_iter().collect::<HashSet<_>>();
        if allowed_modes.is_empty() {
            return Err("runtime authorization ceiling must allow at least one mode".to_owned());
        }
        if max_session_ttl.is_zero() || max_idle_ttl.is_zero() || max_idle_ttl > max_session_ttl {
            return Err(
                "runtime authorization ceiling TTLs must be non-zero and idle TTL cannot exceed session TTL"
                    .to_owned(),
            );
        }
        if allowed_modes.contains(&PermissionMode::Unrestricted) && !unrestricted_acknowledged {
            return Err(
                "unrestricted runtime ceiling requires trusted launch-time acknowledgement"
                    .to_owned(),
            );
        }
        Ok(Self {
            allowed_modes,
            unrestricted_acknowledged,
            delegation_enabled: true,
            max_session_ttl: Some(max_session_ttl),
            max_idle_ttl: Some(max_idle_ttl),
            user_policy_sha256: crate::policy::user_policy_sha256()?,
            managed_policy_sha256: crate::policy::managed_policy_sha256()?,
        })
    }

    pub fn allows(&self, mode: PermissionMode) -> bool {
        self.allowed_modes.contains(&mode)
            && (mode != PermissionMode::Unrestricted || self.unrestricted_acknowledged)
    }

    pub fn delegation_enabled(&self) -> bool {
        self.delegation_enabled
    }

    pub fn allowed_mode_names(&self) -> Vec<&'static str> {
        let mut modes = [
            PermissionMode::Standard,
            PermissionMode::Bounded,
            PermissionMode::Unrestricted,
        ]
        .into_iter()
        .filter(|mode| self.allows(*mode))
        .map(PermissionMode::as_str)
        .collect::<Vec<_>>();
        modes.sort_unstable();
        modes
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct DaemonGeneration(Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ConnectionAuthorityId(Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct HostLeaseId(Uuid);

/// Proof that an action arrived on a connection accepted from a trusted host.
///
/// This value is intentionally opaque, non-serializable, and has no public
/// constructor. The future authenticated action transport will own its
/// creation. A path-addressed socket or caller-supplied session string must
/// never manufacture one.
pub struct AuthenticatedActionConnection {
    id: ConnectionAuthorityId,
    daemon_generation: DaemonGeneration,
    host_lease: HostLeaseId,
}

/// Process-local proof that a certified host-control lease is live.
///
/// The current liveness-only embedded pipe cannot create this proof. It will
/// become constructible only from the versioned protected control channel.
pub struct TrustedHostLease {
    id: HostLeaseId,
    daemon_generation: DaemonGeneration,
}

#[derive(Clone)]
pub struct EffectiveAuthorizationContext {
    daemon_generation: DaemonGeneration,
    source: AuthorizationContextSource,
    mode: PermissionMode,
    public_session: Option<String>,
    transport_session: Option<String>,
    host_lease: Option<HostLeaseId>,
    bounded_manifest: Option<Arc<SessionManifest>>,
    user_policy_sha256: Option<String>,
    managed_policy_sha256: Option<String>,
    expires_unix_ms: Option<u128>,
    expires_at: Option<Instant>,
    idle_ttl: Option<Duration>,
    last_authorized_dispatch: Option<Arc<Mutex<Instant>>>,
    idle_expired: Option<Arc<AtomicBool>>,
    revoked: Arc<AtomicBool>,
}

impl EffectiveAuthorizationContext {
    pub fn mode(&self) -> PermissionMode {
        self.mode
    }

    pub fn source(&self) -> AuthorizationContextSource {
        self.source
    }

    pub fn bounded_manifest(&self) -> Option<&SessionManifest> {
        self.bounded_manifest.as_deref()
    }

    pub fn bounded_manifest_sha256(&self) -> Option<&str> {
        self.bounded_manifest
            .as_deref()
            .map(SessionManifest::sha256)
    }

    pub fn public_session(&self) -> Option<&str> {
        self.public_session.as_deref()
    }

    pub fn is_expired(&self) -> bool {
        if self.revoked.load(Ordering::Acquire) {
            return true;
        }
        if self
            .expires_at
            .is_some_and(|expires| Instant::now() >= expires)
        {
            return true;
        }
        let (Some(idle_ttl), Some(last), Some(expired)) = (
            self.idle_ttl,
            self.last_authorized_dispatch.as_ref(),
            self.idle_expired.as_ref(),
        ) else {
            return false;
        };
        if expired.load(Ordering::Acquire) {
            return true;
        }
        if Instant::now().duration_since(*last.lock().unwrap()) >= idle_ttl {
            expired.store(true, Ordering::Release);
            return true;
        }
        false
    }

    pub fn authorize_dispatch(&self) -> Result<(), String> {
        if self.revoked.load(Ordering::Acquire) {
            return Err("authorization context has been revoked".to_owned());
        }
        let (Some(idle_ttl), Some(last), Some(expired)) = (
            self.idle_ttl,
            self.last_authorized_dispatch.as_ref(),
            self.idle_expired.as_ref(),
        ) else {
            return Ok(());
        };
        if expired.load(Ordering::Acquire) {
            return Err("authorization context idle timeout exceeded".to_owned());
        }
        let now = Instant::now();
        let mut previous = last.lock().unwrap();
        if now.duration_since(*previous) > idle_ttl {
            expired.store(true, Ordering::Release);
            return Err("authorization context idle timeout exceeded".to_owned());
        }
        *previous = now;
        Ok(())
    }
}

pub struct DelegatedSessionRequest {
    pub public_session: String,
    pub transport_session: String,
    pub mode: PermissionMode,
    pub ttl: Duration,
    pub idle_ttl: Duration,
    pub bounded_manifest: Option<Arc<SessionManifest>>,
}

impl std::fmt::Debug for AuthenticatedActionConnection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AuthenticatedActionConnection")
            .field("authority", &"[redacted]")
            .finish()
    }
}

impl std::fmt::Debug for TrustedHostLease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TrustedHostLease")
            .field("authority", &"[redacted]")
            .finish()
    }
}

impl std::fmt::Debug for EffectiveAuthorizationContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EffectiveAuthorizationContext")
            .field("source", &self.source)
            .field("mode", &self.mode)
            .field("sessions", &"[redacted]")
            .field("host_lease", &self.host_lease.is_some())
            .field("bounded_manifest_sha256", &self.bounded_manifest_sha256())
            .field("user_policy_bound", &self.user_policy_sha256.is_some())
            .field(
                "managed_policy_bound",
                &self.managed_policy_sha256.is_some(),
            )
            .field("expires_unix_ms", &self.expires_unix_ms)
            .finish()
    }
}

impl std::fmt::Debug for DelegatedSessionRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DelegatedSessionRequest")
            .field("sessions", &"[redacted]")
            .field("mode", &self.mode)
            .field("ttl", &self.ttl)
            .field("idle_ttl", &self.idle_ttl)
            .field(
                "bounded_manifest_sha256",
                &self
                    .bounded_manifest
                    .as_deref()
                    .map(SessionManifest::sha256),
            )
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SessionAuthorizationError {
    #[error("the owning runtime is not accepting new sessions")]
    RuntimeUnavailable,
    #[error("trusted per-session mode delegation is not enabled for this daemon")]
    DelegationDisabled,
    #[error("the requested session mode is outside the daemon authorization ceiling")]
    ModeOutsideCeiling,
    #[error("unrestricted session mode lacks trusted launch-time risk acceptance")]
    UnrestrictedNotAcknowledged,
    #[error("the trusted host or action connection belongs to another daemon generation")]
    GenerationMismatch,
    #[error("the action connection is not owned by this trusted host lease")]
    HostLeaseMismatch,
    #[error("the action connection already has an authorization context")]
    ConnectionAlreadyBound,
    #[error("the public or transport session is already bound to a live authorization context")]
    SessionAlreadyBound,
    #[error("the action connection has no live authorization context")]
    UnknownConnection,
    #[error("the public or transport session does not match the bound authorization context")]
    SessionMismatch,
    #[error("the authorization context expired")]
    Expired,
    #[error("session and idle TTLs must be non-zero and within the daemon ceiling")]
    InvalidTtl,
    #[error("bounded mode requires an immutable per-session manifest")]
    MissingBoundedManifest,
    #[error("a bounded manifest is valid only for bounded mode")]
    UnexpectedBoundedManifest,
}

#[derive(Debug, Default)]
struct RegistryState {
    by_connection: HashMap<ConnectionAuthorityId, Arc<EffectiveAuthorizationContext>>,
}

/// Process-owned registry for immutable, connection-bound authorization.
///
/// Released callers use [`Self::legacy_context`]. A trusted direct SDK host
/// may additionally mint opaque in-process bindings; public protocols cannot
/// obtain or reconstruct the host/connection proofs required by
/// [`Self::bind_delegated_session`].
#[derive(Debug)]
pub struct SessionAuthorizationRegistry {
    daemon_generation: DaemonGeneration,
    ceiling: SessionModeCeiling,
    state: Mutex<RegistryState>,
}

impl SessionAuthorizationRegistry {
    /// Construct the compatibility authorization registry for one runtime.
    ///
    /// The resulting ceiling and default context are frozen from trusted
    /// startup configuration. Runtime owners should retain this registry
    /// instead of repeatedly consulting the process-global compatibility
    /// accessor.
    pub fn process() -> Result<Self, String> {
        let mode = crate::authorization::configured_permission_mode()?;
        Ok(Self {
            daemon_generation: DaemonGeneration(Uuid::new_v4()),
            ceiling: SessionModeCeiling::process(mode)?,
            state: Mutex::new(RegistryState::default()),
        })
    }

    pub fn with_ceiling(ceiling: SessionModeCeiling) -> Self {
        Self {
            daemon_generation: DaemonGeneration(Uuid::new_v4()),
            ceiling,
            state: Mutex::new(RegistryState::default()),
        }
    }

    /// Mint process-local authority for a direct SDK host.
    ///
    /// The returned values are opaque and non-serializable. Service adapters
    /// must not use this shortcut; they first authenticate the accepted peer
    /// and then establish an equivalent connection-bound proof.
    pub fn trusted_in_process_binding(&self) -> (TrustedHostLease, AuthenticatedActionConnection) {
        let host = TrustedHostLease {
            id: HostLeaseId(Uuid::new_v4()),
            daemon_generation: self.daemon_generation,
        };
        let connection = AuthenticatedActionConnection {
            id: ConnectionAuthorityId(Uuid::new_v4()),
            daemon_generation: self.daemon_generation,
            host_lease: host.id,
        };
        (host, connection)
    }

    #[cfg(test)]
    fn delegated_for_test(ceiling: SessionModeCeiling) -> Self {
        Self {
            daemon_generation: DaemonGeneration(Uuid::new_v4()),
            ceiling,
            state: Mutex::new(RegistryState::default()),
        }
    }

    pub fn ceiling(&self) -> &SessionModeCeiling {
        &self.ceiling
    }

    pub fn legacy_context(&self) -> Result<Arc<EffectiveAuthorizationContext>, String> {
        let mode = crate::authorization::configured_permission_mode()?;
        let bounded_manifest = if mode == PermissionMode::Bounded {
            crate::session_manifest::configured_session_manifest()?
                .cloned()
                .map(Arc::new)
        } else {
            None
        };
        self.compatibility_context(mode, bounded_manifest)
    }

    /// Construct the compatibility action context from explicit trusted
    /// runtime options without consulting compatibility environment variables.
    pub fn compatibility_context(
        &self,
        mode: PermissionMode,
        bounded_manifest: Option<Arc<SessionManifest>>,
    ) -> Result<Arc<EffectiveAuthorizationContext>, String> {
        if !self.ceiling.allows(mode) {
            return Err("compatibility permission mode is outside the runtime ceiling".to_owned());
        }
        match (mode, bounded_manifest.as_ref()) {
            (PermissionMode::Bounded, None) => {
                return Err("bounded compatibility mode requires a session manifest".to_owned())
            }
            (PermissionMode::Standard | PermissionMode::Unrestricted, Some(_)) => {
                return Err(
                    "a compatibility session manifest is valid only for bounded mode".to_owned(),
                )
            }
            _ => {}
        }
        Ok(Arc::new(EffectiveAuthorizationContext {
            daemon_generation: self.daemon_generation,
            source: AuthorizationContextSource::Process,
            mode,
            public_session: None,
            transport_session: None,
            host_lease: None,
            bounded_manifest,
            user_policy_sha256: self.ceiling.user_policy_sha256.clone(),
            managed_policy_sha256: self.ceiling.managed_policy_sha256.clone(),
            expires_unix_ms: None,
            expires_at: None,
            idle_ttl: None,
            last_authorized_dispatch: None,
            idle_expired: None,
            revoked: Arc::new(AtomicBool::new(false)),
        }))
    }

    pub fn bind_delegated_session(
        &self,
        host: &TrustedHostLease,
        connection: &AuthenticatedActionConnection,
        request: DelegatedSessionRequest,
    ) -> Result<(), SessionAuthorizationError> {
        if !self.ceiling.delegation_enabled {
            return Err(SessionAuthorizationError::DelegationDisabled);
        }
        if host.daemon_generation != self.daemon_generation
            || connection.daemon_generation != self.daemon_generation
        {
            return Err(SessionAuthorizationError::GenerationMismatch);
        }
        if connection.host_lease != host.id {
            return Err(SessionAuthorizationError::HostLeaseMismatch);
        }
        if !self.ceiling.allowed_modes.contains(&request.mode) {
            return Err(SessionAuthorizationError::ModeOutsideCeiling);
        }
        if request.mode == PermissionMode::Unrestricted && !self.ceiling.unrestricted_acknowledged {
            return Err(SessionAuthorizationError::UnrestrictedNotAcknowledged);
        }
        if request.ttl.is_zero()
            || request.idle_ttl.is_zero()
            || self
                .ceiling
                .max_session_ttl
                .is_none_or(|maximum| request.ttl > maximum)
            || self
                .ceiling
                .max_idle_ttl
                .is_none_or(|maximum| request.idle_ttl > maximum)
            || request.idle_ttl > request.ttl
        {
            return Err(SessionAuthorizationError::InvalidTtl);
        }
        match (request.mode, request.bounded_manifest.as_ref()) {
            (PermissionMode::Bounded, None) => {
                return Err(SessionAuthorizationError::MissingBoundedManifest)
            }
            (PermissionMode::Standard | PermissionMode::Unrestricted, Some(_)) => {
                return Err(SessionAuthorizationError::UnexpectedBoundedManifest)
            }
            _ => {}
        }
        let mut state = self.state.lock().unwrap();
        state.by_connection.retain(|_, context| {
            let retain = !context.is_expired();
            if !retain {
                context.revoked.store(true, Ordering::Release);
            }
            retain
        });
        if state.by_connection.contains_key(&connection.id) {
            return Err(SessionAuthorizationError::ConnectionAlreadyBound);
        }
        if state.by_connection.values().any(|context| {
            context.public_session.as_deref() == Some(request.public_session.as_str())
                || context.transport_session.as_deref() == Some(request.transport_session.as_str())
        }) {
            return Err(SessionAuthorizationError::SessionAlreadyBound);
        }
        let context = Arc::new(EffectiveAuthorizationContext {
            daemon_generation: self.daemon_generation,
            source: AuthorizationContextSource::TrustedHost,
            mode: request.mode,
            public_session: Some(request.public_session),
            transport_session: Some(request.transport_session),
            host_lease: Some(host.id),
            bounded_manifest: request.bounded_manifest,
            user_policy_sha256: self.ceiling.user_policy_sha256.clone(),
            managed_policy_sha256: self.ceiling.managed_policy_sha256.clone(),
            expires_unix_ms: Some(now_unix_ms() + request.ttl.as_millis()),
            expires_at: Some(Instant::now() + request.ttl),
            idle_ttl: Some(request.idle_ttl),
            last_authorized_dispatch: Some(Arc::new(Mutex::new(Instant::now()))),
            idle_expired: Some(Arc::new(AtomicBool::new(false))),
            revoked: Arc::new(AtomicBool::new(false)),
        });
        state.by_connection.insert(connection.id, context);
        Ok(())
    }

    pub fn resolve_delegated(
        &self,
        connection: &AuthenticatedActionConnection,
        public_session: &str,
        transport_session: &str,
    ) -> Result<Arc<EffectiveAuthorizationContext>, SessionAuthorizationError> {
        if connection.daemon_generation != self.daemon_generation {
            return Err(SessionAuthorizationError::GenerationMismatch);
        }
        let mut state = self.state.lock().unwrap();
        let context = state
            .by_connection
            .get(&connection.id)
            .cloned()
            .ok_or(SessionAuthorizationError::UnknownConnection)?;
        if context.daemon_generation != self.daemon_generation {
            return Err(SessionAuthorizationError::GenerationMismatch);
        }
        if context.is_expired() {
            state.by_connection.remove(&connection.id);
            return Err(SessionAuthorizationError::Expired);
        }
        if context.public_session.as_deref() != Some(public_session)
            || context.transport_session.as_deref() != Some(transport_session)
        {
            return Err(SessionAuthorizationError::SessionMismatch);
        }
        Ok(context)
    }

    pub fn revoke_connection(&self, connection: &AuthenticatedActionConnection) -> bool {
        if connection.daemon_generation != self.daemon_generation {
            return false;
        }
        self.state
            .lock()
            .unwrap()
            .by_connection
            .remove(&connection.id)
            .is_some_and(|context| {
                context.revoked.store(true, Ordering::Release);
                true
            })
    }

    pub fn revoke_host(&self, host: &TrustedHostLease) -> usize {
        if host.daemon_generation != self.daemon_generation {
            return 0;
        }
        let mut state = self.state.lock().unwrap();
        let before = state.by_connection.len();
        state.by_connection.retain(|_, context| {
            let retain = context.host_lease != Some(host.id);
            if !retain {
                context.revoked.store(true, Ordering::Release);
            }
            retain
        });
        before - state.by_connection.len()
    }

    /// Revoke every delegated context owned by this runtime generation.
    ///
    /// Runtime shutdown calls this before releasing process ownership so any
    /// outstanding bound action surface fails closed even if a language
    /// binding retains it after the runtime object is shut down.
    pub fn revoke_all(&self) -> usize {
        let mut state = self.state.lock().unwrap();
        let count = state.by_connection.len();
        for context in state.by_connection.values() {
            context.revoked.store(true, Ordering::Release);
        }
        state.by_connection.clear();
        count
    }

    pub fn status_json(&self) -> serde_json::Value {
        let state = self.state.lock().unwrap();
        let mut standard = 0_u64;
        let mut bounded = 0_u64;
        let mut unrestricted = 0_u64;
        for context in state.by_connection.values() {
            if context.is_expired() {
                continue;
            }
            match context.mode {
                PermissionMode::Standard => standard += 1,
                PermissionMode::Bounded => bounded += 1,
                PermissionMode::Unrestricted => unrestricted += 1,
            }
        }
        serde_json::json!({
            "daemon_authorization_ceiling": {
                "allowed_modes": self.ceiling.allowed_mode_names(),
                "unrestricted_acknowledged": self.ceiling.unrestricted_acknowledged,
                "max_session_ttl_seconds": self.ceiling.max_session_ttl.map(|ttl| ttl.as_secs()),
                "max_idle_ttl_seconds": self.ceiling.max_idle_ttl.map(|ttl| ttl.as_secs()),
                "policy_hashes_bound": {
                    "user": self.ceiling.user_policy_sha256.is_some(),
                    "managed": self.ceiling.managed_policy_sha256.is_some(),
                },
            },
            "effective_session_mode_source": AuthorizationContextSource::Process.as_str(),
            "session_mode_delegation_enabled": self.ceiling.delegation_enabled,
            "session_mode_delegation_ready": false,
            "session_mode_delegation_blocker": "authenticated_action_connection_required",
            "delegated_session_counts": {
                "standard": standard,
                "bounded": bounded,
                "unrestricted": unrestricted,
            },
        })
    }

    #[cfg(test)]
    fn trusted_pair_for_test(
        &self,
        host: Option<&TrustedHostLease>,
    ) -> (TrustedHostLease, AuthenticatedActionConnection) {
        let lease = host.map_or(
            TrustedHostLease {
                id: HostLeaseId(Uuid::new_v4()),
                daemon_generation: self.daemon_generation,
            },
            |host| TrustedHostLease {
                id: host.id,
                daemon_generation: host.daemon_generation,
            },
        );
        let connection = AuthenticatedActionConnection {
            id: ConnectionAuthorityId(Uuid::new_v4()),
            daemon_generation: self.daemon_generation,
            host_lease: lease.id,
        };
        (lease, connection)
    }
}

static CONFIGURED_REGISTRY: OnceLock<Result<SessionAuthorizationRegistry, String>> =
    OnceLock::new();

pub fn configured_registry() -> Result<&'static SessionAuthorizationRegistry, String> {
    match CONFIGURED_REGISTRY.get_or_init(SessionAuthorizationRegistry::process) {
        Ok(registry) => Ok(registry),
        Err(error) => Err(error.clone()),
    }
}

pub fn status_json() -> serde_json::Value {
    match configured_registry() {
        Ok(registry) => registry.status_json(),
        Err(error) => serde_json::json!({
            "daemon_authorization_ceiling": null,
            "effective_session_mode_source": null,
            "session_mode_delegation_enabled": false,
            "session_mode_delegation_ready": false,
            "session_mode_delegation_blocker": "invalid_startup_authorization",
            "session_authorization_error": error,
        }),
    }
}

fn now_unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn request(mode: PermissionMode) -> DelegatedSessionRequest {
        DelegatedSessionRequest {
            public_session: "public-a".to_owned(),
            transport_session: "transport-a".to_owned(),
            mode,
            ttl: Duration::from_secs(60),
            idle_ttl: Duration::from_secs(30),
            bounded_manifest: None,
        }
    }

    fn manifest_with_allowed_tool(tool: &str) -> Arc<SessionManifest> {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            "version: 1\nmode: bounded\nexpires_after: 1h\nidle_timeout: 30m\nallow:\n  tools:\n    - {tool}"
        )
        .unwrap();
        Arc::new(crate::session_manifest::load_manifest(file.path()).unwrap())
    }

    #[test]
    fn process_ceiling_disables_delegation() {
        let registry = SessionAuthorizationRegistry::delegated_for_test(
            SessionModeCeiling::process(PermissionMode::Standard).unwrap(),
        );
        let (host, connection) = registry.trusted_pair_for_test(None);
        assert_eq!(
            registry
                .bind_delegated_session(&host, &connection, request(PermissionMode::Standard))
                .unwrap_err(),
            SessionAuthorizationError::DelegationDisabled
        );
    }

    #[test]
    fn mode_outside_explicit_ceiling_is_denied() {
        let registry = SessionAuthorizationRegistry::delegated_for_test(
            SessionModeCeiling::delegated([PermissionMode::Standard], false),
        );
        let (host, connection) = registry.trusted_pair_for_test(None);
        assert_eq!(
            registry
                .bind_delegated_session(&host, &connection, request(PermissionMode::Unrestricted))
                .unwrap_err(),
            SessionAuthorizationError::ModeOutsideCeiling
        );
    }

    #[test]
    fn daemon_generation_and_host_lease_must_match() {
        let ceiling = SessionModeCeiling::delegated([PermissionMode::Standard], false);
        let registry_a = SessionAuthorizationRegistry::delegated_for_test(ceiling.clone());
        let registry_b = SessionAuthorizationRegistry::delegated_for_test(ceiling);
        let (host_a, connection_a) = registry_a.trusted_pair_for_test(None);
        let (host_b, connection_b) = registry_b.trusted_pair_for_test(None);

        assert_eq!(
            registry_a
                .bind_delegated_session(&host_b, &connection_b, request(PermissionMode::Standard))
                .unwrap_err(),
            SessionAuthorizationError::GenerationMismatch
        );
        assert_eq!(
            registry_a
                .resolve_delegated(&connection_b, "public-a", "transport-a")
                .unwrap_err(),
            SessionAuthorizationError::GenerationMismatch
        );

        let (_, foreign_host_connection) = registry_a.trusted_pair_for_test(None);
        assert_eq!(
            registry_a
                .bind_delegated_session(
                    &host_a,
                    &foreign_host_connection,
                    request(PermissionMode::Standard)
                )
                .unwrap_err(),
            SessionAuthorizationError::HostLeaseMismatch
        );
        assert!(!registry_a.revoke_connection(&connection_b));
        assert_eq!(registry_a.revoke_host(&host_b), 0);

        registry_a
            .bind_delegated_session(&host_a, &connection_a, request(PermissionMode::Standard))
            .unwrap();
    }

    #[test]
    fn unrestricted_requires_separate_launch_acknowledgement() {
        let registry = SessionAuthorizationRegistry::delegated_for_test(
            SessionModeCeiling::delegated([PermissionMode::Unrestricted], false),
        );
        let (host, connection) = registry.trusted_pair_for_test(None);
        assert_eq!(
            registry
                .bind_delegated_session(&host, &connection, request(PermissionMode::Unrestricted))
                .unwrap_err(),
            SessionAuthorizationError::UnrestrictedNotAcknowledged
        );
    }

    #[test]
    fn public_and_transport_session_substitution_is_denied() {
        let registry = SessionAuthorizationRegistry::delegated_for_test(
            SessionModeCeiling::delegated([PermissionMode::Unrestricted], true),
        );
        let (host, connection) = registry.trusted_pair_for_test(None);
        registry
            .bind_delegated_session(&host, &connection, request(PermissionMode::Unrestricted))
            .unwrap();
        assert_eq!(
            registry
                .resolve_delegated(&connection, "public-b", "transport-a")
                .unwrap_err(),
            SessionAuthorizationError::SessionMismatch
        );
        assert_eq!(
            registry
                .resolve_delegated(&connection, "public-a", "transport-b")
                .unwrap_err(),
            SessionAuthorizationError::SessionMismatch
        );
        assert_eq!(
            registry
                .resolve_delegated(&connection, "public-a", "transport-a")
                .unwrap()
                .mode(),
            PermissionMode::Unrestricted
        );
    }

    #[test]
    fn connection_binding_is_single_use() {
        let registry = SessionAuthorizationRegistry::delegated_for_test(
            SessionModeCeiling::delegated([PermissionMode::Standard], false),
        );
        let (host, connection) = registry.trusted_pair_for_test(None);
        registry
            .bind_delegated_session(&host, &connection, request(PermissionMode::Standard))
            .unwrap();
        assert_eq!(
            registry
                .bind_delegated_session(&host, &connection, request(PermissionMode::Standard))
                .unwrap_err(),
            SessionAuthorizationError::ConnectionAlreadyBound
        );
    }

    #[test]
    fn live_public_and_transport_session_labels_are_unique() {
        let registry = SessionAuthorizationRegistry::delegated_for_test(
            SessionModeCeiling::delegated([PermissionMode::Standard], false),
        );
        let (host, connection_a) = registry.trusted_pair_for_test(None);
        let (_, connection_b) = registry.trusted_pair_for_test(Some(&host));
        registry
            .bind_delegated_session(&host, &connection_a, request(PermissionMode::Standard))
            .unwrap();

        let mut duplicate_public = request(PermissionMode::Standard);
        duplicate_public.transport_session = "transport-b".to_owned();
        assert_eq!(
            registry
                .bind_delegated_session(&host, &connection_b, duplicate_public)
                .unwrap_err(),
            SessionAuthorizationError::SessionAlreadyBound
        );

        let (_, connection_c) = registry.trusted_pair_for_test(Some(&host));
        let mut duplicate_transport = request(PermissionMode::Standard);
        duplicate_transport.public_session = "public-b".to_owned();
        assert_eq!(
            registry
                .bind_delegated_session(&host, &connection_c, duplicate_transport)
                .unwrap_err(),
            SessionAuthorizationError::SessionAlreadyBound
        );
    }

    #[test]
    fn expired_session_labels_can_be_rebound_on_a_new_connection() {
        let registry = SessionAuthorizationRegistry::delegated_for_test(
            SessionModeCeiling::delegated([PermissionMode::Standard], false),
        );
        let (host, expired_connection) = registry.trusted_pair_for_test(None);
        let mut short = request(PermissionMode::Standard);
        short.ttl = Duration::from_millis(1);
        short.idle_ttl = Duration::from_millis(1);
        registry
            .bind_delegated_session(&host, &expired_connection, short)
            .unwrap();
        std::thread::sleep(Duration::from_millis(3));

        let (_, replacement_connection) = registry.trusted_pair_for_test(Some(&host));
        registry
            .bind_delegated_session(
                &host,
                &replacement_connection,
                request(PermissionMode::Standard),
            )
            .expect("expired labels must not remain reserved");
        assert_eq!(
            registry
                .resolve_delegated(&replacement_connection, "public-a", "transport-a")
                .unwrap()
                .mode(),
            PermissionMode::Standard
        );
    }

    #[test]
    fn ttl_constraints_fail_closed() {
        let registry = SessionAuthorizationRegistry::delegated_for_test(
            SessionModeCeiling::delegated([PermissionMode::Standard], false),
        );
        let invalid = [
            (Duration::ZERO, Duration::from_secs(1)),
            (Duration::from_secs(1), Duration::ZERO),
            (
                DEFAULT_MAX_SESSION_TTL + Duration::from_secs(1),
                Duration::from_secs(1),
            ),
            (
                Duration::from_secs(60),
                DEFAULT_MAX_IDLE_TTL + Duration::from_secs(1),
            ),
            (Duration::from_secs(1), Duration::from_secs(2)),
        ];
        for (ttl, idle_ttl) in invalid {
            let (host, connection) = registry.trusted_pair_for_test(None);
            let mut request = request(PermissionMode::Standard);
            request.ttl = ttl;
            request.idle_ttl = idle_ttl;
            assert_eq!(
                registry
                    .bind_delegated_session(&host, &connection, request)
                    .unwrap_err(),
                SessionAuthorizationError::InvalidTtl
            );
        }
    }

    #[test]
    fn absolute_and_idle_expiry_are_terminal() {
        let registry = SessionAuthorizationRegistry::delegated_for_test(
            SessionModeCeiling::delegated([PermissionMode::Standard], false),
        );
        let (host, absolute_connection) = registry.trusted_pair_for_test(None);
        let mut absolute = request(PermissionMode::Standard);
        absolute.ttl = Duration::from_millis(1);
        absolute.idle_ttl = Duration::from_millis(1);
        registry
            .bind_delegated_session(&host, &absolute_connection, absolute)
            .unwrap();
        std::thread::sleep(Duration::from_millis(3));
        assert_eq!(
            registry
                .resolve_delegated(&absolute_connection, "public-a", "transport-a")
                .unwrap_err(),
            SessionAuthorizationError::Expired
        );

        let (_, idle_connection) = registry.trusted_pair_for_test(Some(&host));
        let mut idle = request(PermissionMode::Standard);
        idle.public_session = "public-idle".to_owned();
        idle.transport_session = "transport-idle".to_owned();
        idle.ttl = Duration::from_secs(1);
        idle.idle_ttl = Duration::from_millis(1);
        registry
            .bind_delegated_session(&host, &idle_connection, idle)
            .unwrap();
        let context = registry
            .resolve_delegated(&idle_connection, "public-idle", "transport-idle")
            .unwrap();
        std::thread::sleep(Duration::from_millis(3));
        assert!(context.is_expired());
        assert!(context.authorize_dispatch().is_err());
        assert_eq!(
            registry.status_json()["delegated_session_counts"]["standard"],
            0
        );
    }

    #[test]
    fn connection_authority_cannot_be_replayed_on_another_connection() {
        let registry = SessionAuthorizationRegistry::delegated_for_test(
            SessionModeCeiling::delegated([PermissionMode::Unrestricted], true),
        );
        let (host, connection) = registry.trusted_pair_for_test(None);
        let (_, other_connection) = registry.trusted_pair_for_test(Some(&host));
        registry
            .bind_delegated_session(&host, &connection, request(PermissionMode::Unrestricted))
            .unwrap();
        assert_eq!(
            registry
                .resolve_delegated(&other_connection, "public-a", "transport-a")
                .unwrap_err(),
            SessionAuthorizationError::UnknownConnection
        );
    }

    #[test]
    fn bounded_manifests_are_stored_per_connection_context() {
        let registry = SessionAuthorizationRegistry::delegated_for_test(
            SessionModeCeiling::delegated([PermissionMode::Bounded], false),
        );
        let (host, connection_a) = registry.trusted_pair_for_test(None);
        let (_, connection_b) = registry.trusted_pair_for_test(Some(&host));
        let manifest_a = manifest_with_allowed_tool("click");
        let manifest_b = manifest_with_allowed_tool("scroll");
        let hash_a = manifest_a.sha256().to_owned();
        let hash_b = manifest_b.sha256().to_owned();

        let mut request_a = request(PermissionMode::Bounded);
        request_a.bounded_manifest = Some(manifest_a);
        registry
            .bind_delegated_session(&host, &connection_a, request_a)
            .unwrap();

        let mut request_b = request(PermissionMode::Bounded);
        request_b.public_session = "public-b".to_owned();
        request_b.transport_session = "transport-b".to_owned();
        request_b.bounded_manifest = Some(manifest_b);
        registry
            .bind_delegated_session(&host, &connection_b, request_b)
            .unwrap();

        assert_ne!(hash_a, hash_b);
        assert_eq!(
            registry
                .resolve_delegated(&connection_a, "public-a", "transport-a")
                .unwrap()
                .bounded_manifest_sha256(),
            Some(hash_a.as_str())
        );
        assert_eq!(
            registry
                .resolve_delegated(&connection_b, "public-b", "transport-b")
                .unwrap()
                .bounded_manifest_sha256(),
            Some(hash_b.as_str())
        );
    }

    #[test]
    fn manifest_presence_matches_the_exact_mode() {
        let registry =
            SessionAuthorizationRegistry::delegated_for_test(SessionModeCeiling::delegated(
                [PermissionMode::Standard, PermissionMode::Bounded],
                false,
            ));
        let (host, bounded_connection) = registry.trusted_pair_for_test(None);
        assert_eq!(
            registry
                .bind_delegated_session(
                    &host,
                    &bounded_connection,
                    request(PermissionMode::Bounded)
                )
                .unwrap_err(),
            SessionAuthorizationError::MissingBoundedManifest
        );

        let (_, standard_connection) = registry.trusted_pair_for_test(Some(&host));
        let mut standard = request(PermissionMode::Standard);
        standard.bounded_manifest = Some(manifest_with_allowed_tool("click"));
        assert_eq!(
            registry
                .bind_delegated_session(&host, &standard_connection, standard)
                .unwrap_err(),
            SessionAuthorizationError::UnexpectedBoundedManifest
        );
    }

    #[test]
    fn host_revocation_is_scoped_to_that_host() {
        let registry = SessionAuthorizationRegistry::delegated_for_test(
            SessionModeCeiling::delegated([PermissionMode::Standard], false),
        );
        let (host_a, connection_a) = registry.trusted_pair_for_test(None);
        let (host_b, connection_b) = registry.trusted_pair_for_test(None);
        registry
            .bind_delegated_session(&host_a, &connection_a, request(PermissionMode::Standard))
            .unwrap();
        let mut request_b = request(PermissionMode::Standard);
        request_b.public_session = "public-b".to_owned();
        request_b.transport_session = "transport-b".to_owned();
        registry
            .bind_delegated_session(&host_b, &connection_b, request_b)
            .unwrap();

        assert_eq!(registry.revoke_host(&host_a), 1);
        assert_eq!(
            registry
                .resolve_delegated(&connection_a, "public-a", "transport-a")
                .unwrap_err(),
            SessionAuthorizationError::UnknownConnection
        );
        assert!(registry
            .resolve_delegated(&connection_b, "public-b", "transport-b")
            .is_ok());
    }

    #[test]
    fn revocation_invalidates_already_resolved_contexts() {
        let registry = SessionAuthorizationRegistry::delegated_for_test(
            SessionModeCeiling::delegated([PermissionMode::Standard], false),
        );
        let (host, connection) = registry.trusted_pair_for_test(None);
        registry
            .bind_delegated_session(&host, &connection, request(PermissionMode::Standard))
            .unwrap();
        let context = registry
            .resolve_delegated(&connection, "public-a", "transport-a")
            .unwrap();

        assert!(registry.revoke_connection(&connection));
        assert!(context.authorize_dispatch().is_err());
        assert!(context.is_expired());
    }

    #[test]
    fn runtime_revocation_invalidates_every_live_context() {
        let registry = SessionAuthorizationRegistry::delegated_for_test(
            SessionModeCeiling::delegated([PermissionMode::Standard], false),
        );
        let (host, connection_a) = registry.trusted_pair_for_test(None);
        let (_, connection_b) = registry.trusted_pair_for_test(Some(&host));
        registry
            .bind_delegated_session(&host, &connection_a, request(PermissionMode::Standard))
            .unwrap();
        let mut second = request(PermissionMode::Standard);
        second.public_session = "public-b".to_owned();
        second.transport_session = "transport-b".to_owned();
        registry
            .bind_delegated_session(&host, &connection_b, second)
            .unwrap();
        let context_a = registry
            .resolve_delegated(&connection_a, "public-a", "transport-a")
            .unwrap();
        let context_b = registry
            .resolve_delegated(&connection_b, "public-b", "transport-b")
            .unwrap();

        assert_eq!(registry.revoke_all(), 2);
        assert!(context_a.authorize_dispatch().is_err());
        assert!(context_b.authorize_dispatch().is_err());
        assert_eq!(registry.revoke_all(), 0);
    }

    #[test]
    fn status_is_content_free_and_truthful_about_unavailable_binding() {
        let registry =
            SessionAuthorizationRegistry::delegated_for_test(SessionModeCeiling::delegated(
                [PermissionMode::Standard, PermissionMode::Unrestricted],
                true,
            ));
        let status = registry.status_json();
        assert_eq!(status["session_mode_delegation_enabled"], true);
        assert_eq!(status["session_mode_delegation_ready"], false);
        assert_eq!(
            status["session_mode_delegation_blocker"],
            "authenticated_action_connection_required"
        );
        let serialized = serde_json::to_string(&status).unwrap();
        assert!(!serialized.contains("public-a"));
        assert!(!serialized.contains("transport-a"));
    }

    #[test]
    fn debug_output_redacts_authority_and_session_labels() {
        let registry = SessionAuthorizationRegistry::delegated_for_test(
            SessionModeCeiling::delegated([PermissionMode::Standard], false),
        );
        let (host, connection) = registry.trusted_pair_for_test(None);
        let request = request(PermissionMode::Standard);
        let request_debug = format!("{request:?}");
        assert!(!request_debug.contains("public-a"));
        assert!(!request_debug.contains("transport-a"));
        registry
            .bind_delegated_session(&host, &connection, request)
            .unwrap();
        let context = registry
            .resolve_delegated(&connection, "public-a", "transport-a")
            .unwrap();
        let combined = format!("{host:?} {connection:?} {context:?}");
        assert!(!combined.contains("public-a"));
        assert!(!combined.contains("transport-a"));
        assert!(!combined.contains(&connection.id.0.to_string()));
        assert!(!combined.contains(&host.id.0.to_string()));
    }
}
