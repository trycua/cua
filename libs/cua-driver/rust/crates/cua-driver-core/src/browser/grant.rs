// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Daemon-memory grants for user-owned browser profile attachment.
//!
//! Grant identifiers never cross the public tool boundary. The store itself is
//! the daemon-instance boundary: dropping/restarting the engine drops every
//! grant, connection generation, and reconnect budget.
//!
//! Each grant record carries its [`Attachment`], the one owner of whether it
//! holds the shared browser socket, keeps it parked for the next session,
//! needs `browser_prepare` again, or has ended with a cleanup still owed.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::platform::ExistingProfileSetupRequest;
use super::refusal::{BrowserRefusal, BrowserRefusalCode};
use super::types::{BrowserProduct, ProcessFingerprint};

/// Idle bound for a live grant, and for a parked socket after its session
/// ended.
pub(crate) const GRANT_IDLE_TTL: Duration = Duration::from_secs(30 * 60);
const GRANT_ABSOLUTE_TTL: Duration = Duration::from_secs(8 * 60 * 60);
pub const MAX_RECONNECT_ATTEMPTS: u8 = 3;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct GrantKey {
    public_session: String,
    transport_session: String,
    pid: i64,
}

/// What one grant holds of the shared browser socket.
#[derive(Debug, Clone)]
pub(crate) enum Attachment {
    /// The grant's generation holds the shared browser socket.
    Attached,
    /// The grant's session ended normally. The grant authorizes nothing, but
    /// its generation keeps the already-consented socket open, so the next
    /// approved session joins it without a new browser prompt, until the
    /// idle bound passes from `since`.
    Parked { since: Instant },
    /// Reconnect could not prove the approved endpoint: it disappeared or
    /// another process owns it. The grant and its consent stay, no socket is
    /// held, and only `browser_prepare` attaches again. Every call reports
    /// `refusal` until then.
    NeedsSetup { refusal: BrowserRefusal },
    /// The grant ended and turning remote debugging back off failed. The
    /// record holds nothing and stays only so its session end retries it.
    Expired,
}

#[derive(Debug, Clone)]
pub(crate) struct ExistingProfileGrant {
    pub public_session: String,
    pub transport_session: String,
    pub pid: i64,
    pub window_id: u64,
    pub fingerprint: ProcessFingerprint,
    pub browser: String,
    pub browser_product: BrowserProduct,
    pub endpoint_ws_url: String,
    pub generation: u64,
    pub cleanup_remote_debugging: bool,
    pub reconnect_attempts_remaining: u8,
    pub protected_consent: Option<crate::consent::ProtectedGrant>,
    pub permission_mode: crate::authorization::PermissionMode,
    pub managed_policy_sha256: Option<String>,
    pub user_policy_sha256: Option<String>,
    pub attachment: Attachment,
    created_at: Instant,
    last_used_at: Instant,
}

pub(crate) enum GrantLookup {
    Missing,
    Live(ExistingProfileGrant),
    Expired(ExistingProfileGrant),
}

impl ExistingProfileGrant {
    /// The remote-debugging cleanup this grant owes when it ends, if the
    /// driver turned remote debugging on for it.
    pub fn setup_cleanup(&self) -> Option<ExistingProfileSetupRequest> {
        self.cleanup_remote_debugging
            .then_some(ExistingProfileSetupRequest {
                pid: self.pid,
                window_id: self.window_id,
                browser: self.browser_product,
            })
    }

    /// Whether this grant's generation is a holder of the pooled socket.
    pub fn holds_socket(&self) -> bool {
        matches!(
            self.attachment,
            Attachment::Attached | Attachment::Parked { .. }
        )
    }

    fn belongs_to(&self, session: &str) -> bool {
        self.public_session == session || self.transport_session == session
    }

    fn expired(&self, now: Instant) -> bool {
        now.duration_since(self.last_used_at) > GRANT_IDLE_TTL
            || now.duration_since(self.created_at) > GRANT_ABSOLUTE_TTL
            || self
                .protected_consent
                .as_ref()
                .is_some_and(|grant| !grant.is_live())
    }

    fn authorization_context_matches(&self) -> bool {
        crate::tool::current_dispatch_authorization_context()
            .map(|context| context.mode())
            .map(Ok)
            .unwrap_or_else(crate::authorization::configured_permission_mode)
            .is_ok_and(|mode| mode == self.permission_mode)
            && crate::policy::managed_policy_sha256().ok().flatten() == self.managed_policy_sha256
            && crate::policy::user_policy_sha256().ok().flatten() == self.user_policy_sha256
    }
}

#[derive(Default)]
struct Registry {
    /// Grants that authorize calls: `Attached` or `NeedsSetup`.
    live: HashMap<GrantKey, ExistingProfileGrant>,
    /// Ended grants that still own something: `Parked` ones hold the socket,
    /// `Expired` ones owe a remote-debugging cleanup. They authorize nothing.
    ended: Vec<ExistingProfileGrant>,
}

pub(crate) struct ExistingProfileGrants {
    inner: Mutex<Registry>,
    /// Connection generations are unique across every grant in this engine,
    /// not just per session. Sessions for the same browser share one socket,
    /// so a per-session counter would let one session's generation match,
    /// release, or invalidate another session's claim and targets.
    last_generation: AtomicU64,
    parked_idle_ttl: Duration,
}

impl Default for ExistingProfileGrants {
    fn default() -> Self {
        Self::with_parked_idle_ttl(GRANT_IDLE_TTL)
    }
}

impl ExistingProfileGrants {
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn with_parked_idle_ttl(parked_idle_ttl: Duration) -> Self {
        Self {
            inner: Default::default(),
            last_generation: Default::default(),
            parked_idle_ttl,
        }
    }

    fn next_generation(&self) -> u64 {
        self.last_generation.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn key(public_session: &str, transport_session: Option<&str>, pid: i64) -> GrantKey {
        GrantKey {
            public_session: public_session.to_owned(),
            transport_session: transport_session.unwrap_or(public_session).to_owned(),
            pid,
        }
    }

    /// How long a parked socket outlives its session.
    pub fn parked_idle_ttl(&self) -> Duration {
        self.parked_idle_ttl
    }

    // Minting deliberately captures the complete immutable browser grant in
    // one call so partially initialized grants cannot enter the registry.
    // An attached grant it replaces is parked, so the new generation joins
    // the socket it held instead of dialing again.
    #[allow(clippy::too_many_arguments)]
    pub fn mint(
        &self,
        public_session: &str,
        transport_session: Option<&str>,
        pid: i64,
        window_id: u64,
        fingerprint: ProcessFingerprint,
        browser: String,
        browser_product: BrowserProduct,
        endpoint_ws_url: String,
        cleanup_remote_debugging: bool,
        protected_consent: Option<crate::consent::ProtectedGrant>,
    ) -> ExistingProfileGrant {
        let now = Instant::now();
        let key = Self::key(public_session, transport_session, pid);
        let generation = self.next_generation();
        let grant = ExistingProfileGrant {
            public_session: public_session.to_owned(),
            transport_session: transport_session.unwrap_or(public_session).to_owned(),
            pid,
            window_id,
            fingerprint,
            browser,
            browser_product,
            endpoint_ws_url,
            generation,
            cleanup_remote_debugging,
            reconnect_attempts_remaining: MAX_RECONNECT_ATTEMPTS,
            protected_consent,
            permission_mode: crate::tool::current_dispatch_authorization_context()
                .map(|context| context.mode())
                .map(Ok)
                .unwrap_or_else(crate::authorization::configured_permission_mode)
                .unwrap_or(crate::authorization::PermissionMode::Standard),
            managed_policy_sha256: crate::policy::managed_policy_sha256().ok().flatten(),
            user_policy_sha256: crate::policy::user_policy_sha256().ok().flatten(),
            attachment: Attachment::Attached,
            created_at: now,
            last_used_at: now,
        };
        let mut registry = self.inner.lock().unwrap();
        if let Some(mut replaced) = registry.live.insert(key, grant.clone()) {
            if matches!(replaced.attachment, Attachment::Attached) {
                replaced.attachment = Attachment::Parked { since: now };
                registry.ended.push(replaced);
            }
        }
        grant
    }

    pub fn lookup(
        &self,
        public_session: &str,
        transport_session: Option<&str>,
        pid: i64,
    ) -> GrantLookup {
        let key = Self::key(public_session, transport_session, pid);
        let now = Instant::now();
        let mut registry = self.inner.lock().unwrap();
        let Some(grant) = registry.live.get_mut(&key) else {
            return GrantLookup::Missing;
        };
        if grant.expired(now) || !grant.authorization_context_matches() {
            let mut expired = registry.live.remove(&key).expect("expired grant exists");
            transfer_cleanup_ownership(&mut registry.live, &mut expired);
            return GrantLookup::Expired(expired);
        }
        grant.last_used_at = now;
        GrantLookup::Live(grant.clone())
    }

    pub fn revoke(
        &self,
        public_session: &str,
        transport_session: Option<&str>,
        pid: i64,
    ) -> Option<ExistingProfileGrant> {
        let mut registry = self.inner.lock().unwrap();
        let mut removed =
            registry
                .live
                .remove(&Self::key(public_session, transport_session, pid))?;
        transfer_cleanup_ownership(&mut registry.live, &mut removed);
        Some(removed)
    }

    /// End every grant of `session`. An attached grant with no cleanup owed
    /// is parked and keeps its socket. The rest are returned for the caller to
    /// release and clean up, together with the session's ended grants whose
    /// cleanup failed before. Every returned or parked grant loses its
    /// authority; the caller revokes their consent.
    pub fn end_session(&self, session: &str) -> SessionEnd {
        let now = Instant::now();
        let mut registry = self.inner.lock().unwrap();
        let mut ended = Vec::new();
        registry.live.retain(|_, grant| {
            let keep = !grant.belongs_to(session);
            if !keep {
                ended.push(grant.clone());
            }
            keep
        });
        let mut released = Vec::new();
        let mut parked = Vec::new();
        for mut grant in ended {
            transfer_cleanup_ownership(&mut registry.live, &mut grant);
            if !grant.cleanup_remote_debugging && matches!(grant.attachment, Attachment::Attached) {
                parked.push(grant.clone());
                grant.attachment = Attachment::Parked { since: now };
                registry.ended.push(grant);
            } else {
                released.push(grant);
            }
        }
        let mut owed_cleanups = Vec::new();
        registry.ended.retain(|grant| {
            let keep =
                !(matches!(grant.attachment, Attachment::Expired) && grant.belongs_to(session));
            if !keep {
                owed_cleanups.push(grant.clone());
            }
            keep
        });
        SessionEnd {
            parked,
            released,
            owed_cleanups,
        }
    }

    /// Keep an ended grant whose remote-debugging cleanup failed, so its
    /// session end retries it.
    pub fn owe_cleanup(&self, mut grant: ExistingProfileGrant) {
        grant.attachment = Attachment::Expired;
        self.inner.lock().unwrap().ended.push(grant);
    }

    /// Remove parked grants idle for longer than the parking bound, for the
    /// caller to release their socket.
    pub fn take_idle_parked(&self, now: Instant) -> Vec<ExistingProfileGrant> {
        let ttl = self.parked_idle_ttl;
        self.take_ended(|grant| {
            matches!(grant.attachment, Attachment::Parked { since } if now.duration_since(since) > ttl)
        })
    }

    /// Remove the parked grants on the socket for `endpoint_ws_url`, for the
    /// caller to release. A grant that closes its socket ends them too.
    pub fn take_parked_on(&self, endpoint_ws_url: &str) -> Vec<ExistingProfileGrant> {
        self.take_ended(|grant| {
            matches!(grant.attachment, Attachment::Parked { .. })
                && grant.endpoint_ws_url == endpoint_ws_url
        })
    }

    #[cfg(test)]
    pub fn parked_generations(&self) -> Vec<u64> {
        self.inner
            .lock()
            .unwrap()
            .ended
            .iter()
            .filter(|grant| matches!(grant.attachment, Attachment::Parked { .. }))
            .map(|grant| grant.generation)
            .collect()
    }

    /// Remove every parked grant, for the caller to release its socket.
    pub fn take_parked(&self) -> Vec<ExistingProfileGrant> {
        self.take_ended(|grant| matches!(grant.attachment, Attachment::Parked { .. }))
    }

    fn take_ended(
        &self,
        take: impl Fn(&ExistingProfileGrant) -> bool,
    ) -> Vec<ExistingProfileGrant> {
        let mut taken = Vec::new();
        self.inner.lock().unwrap().ended.retain(|grant| {
            let keep = !take(grant);
            if !keep {
                taken.push(grant.clone());
            }
            keep
        });
        taken
    }

    /// Move the attached grant at `generation` to `NeedsSetup`. Returns the
    /// grant as it was, for the caller to release its socket, or `None` when
    /// the grant is gone or already moved on.
    pub fn needs_setup(
        &self,
        public_session: &str,
        transport_session: Option<&str>,
        pid: i64,
        generation: u64,
        refusal: BrowserRefusal,
    ) -> Option<ExistingProfileGrant> {
        let key = Self::key(public_session, transport_session, pid);
        let mut registry = self.inner.lock().unwrap();
        let grant = registry.live.get_mut(&key)?;
        if grant.generation != generation || !matches!(grant.attachment, Attachment::Attached) {
            return None;
        }
        let attached = grant.clone();
        grant.attachment = Attachment::NeedsSetup { refusal };
        Some(attached)
    }

    /// Start a reconnect attempt for the grant attached at `expected`: spend
    /// one reconnect and return the generation the new socket is claimed for.
    /// A grant that moved on meanwhile (to `NeedsSetup`, or to another
    /// generation) refuses, so no attempt claims a socket for it.
    pub fn bump_generation(
        &self,
        public_session: &str,
        transport_session: Option<&str>,
        pid: i64,
        expected: u64,
    ) -> Result<u64, BrowserRefusal> {
        let key = Self::key(public_session, transport_session, pid);
        let mut registry = self.inner.lock().unwrap();
        let grant = registry.live.get_mut(&key).ok_or_else(|| {
            BrowserRefusal::new(
                BrowserRefusalCode::BrowserConsentRequired,
                "no live existing-profile grant remains for reconnect",
            )
        })?;
        if let Attachment::NeedsSetup { refusal } = &grant.attachment {
            return Err(refusal.clone());
        }
        if grant.generation != expected {
            return Err(BrowserRefusal::new(
                BrowserRefusalCode::BrowserBindingStale,
                "the browser attachment changed while reconnecting; re-run get_browser_state",
            ));
        }
        if grant.reconnect_attempts_remaining == 0 {
            return Err(BrowserRefusal::new(
                BrowserRefusalCode::BrowserReconnectExhausted,
                "the bounded existing-profile reconnect budget is exhausted",
            ));
        }
        grant.reconnect_attempts_remaining -= 1;
        grant.generation = self.next_generation();
        grant.last_used_at = Instant::now();
        Ok(grant.generation)
    }
}

impl super::cdp_ws::SharedSocketHolders for ExistingProfileGrants {
    fn keep_in_use(&self, holders: &std::collections::HashSet<u64>, now: Instant) -> bool {
        let registry = self.inner.lock().unwrap();
        holders.iter().any(|generation| {
            !registry.ended.iter().any(|grant| {
                grant.generation == *generation
                    && matches!(grant.attachment, Attachment::Parked { since }
                        if now.duration_since(since) > self.parked_idle_ttl)
            })
        })
    }
}

/// What ending a session hands back to the caller.
pub(crate) struct SessionEnd {
    /// Grants now parked on their socket, as they were before parking.
    pub parked: Vec<ExistingProfileGrant>,
    /// Grants to release: their socket closes once no one else holds it.
    pub released: Vec<ExistingProfileGrant>,
    /// Earlier ended grants of this session whose cleanup failed.
    pub owed_cleanups: Vec<ExistingProfileGrant>,
}

fn transfer_cleanup_ownership(
    grants: &mut HashMap<GrantKey, ExistingProfileGrant>,
    removed: &mut ExistingProfileGrant,
) {
    if !removed.cleanup_remote_debugging {
        return;
    }
    if let Some(successor) = grants.values_mut().find(|grant| {
        grant.pid == removed.pid
            && grant.fingerprint.matches(&removed.fingerprint)
            && grant.browser_product == removed.browser_product
    }) {
        successor.cleanup_remote_debugging = true;
        removed.cleanup_remote_debugging = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fingerprint(pid: i64) -> ProcessFingerprint {
        ProcessFingerprint {
            pid,
            start_time: Some(7),
            executable: Some("chrome".to_owned()),
        }
    }

    #[test]
    fn grants_are_scoped_to_public_and_transport_session() {
        let grants = ExistingProfileGrants::new();
        grants.mint(
            "public-a",
            Some("transport-a"),
            42,
            9,
            fingerprint(42),
            "chromium".to_owned(),
            BrowserProduct::GoogleChrome,
            "ws://127.0.0.1:1/devtools/browser/x".to_owned(),
            false,
            None,
        );
        assert!(matches!(
            grants.lookup("public-a", Some("transport-a"), 42),
            GrantLookup::Live(_)
        ));
        assert!(matches!(
            grants.lookup("public-b", Some("transport-a"), 42),
            GrantLookup::Missing
        ));
        assert!(matches!(
            grants.lookup("public-a", Some("transport-b"), 42),
            GrantLookup::Missing
        ));
    }

    #[test]
    fn session_cleanup_revokes_both_owners() {
        let grants = ExistingProfileGrants::new();
        grants.mint(
            "public-a",
            Some("transport-a"),
            42,
            9,
            fingerprint(42),
            "chromium".to_owned(),
            BrowserProduct::GoogleChrome,
            "ws://127.0.0.1:1/devtools/browser/x".to_owned(),
            false,
            None,
        );
        assert_eq!(grants.end_session("transport-a").parked.len(), 1);
        assert!(matches!(
            grants.lookup("public-a", Some("transport-a"), 42),
            GrantLookup::Missing
        ));
    }

    #[test]
    fn expired_lookup_returns_the_connection_owner_for_cleanup() {
        let grants = ExistingProfileGrants::new();
        grants.mint(
            "public-a",
            Some("transport-a"),
            42,
            9,
            fingerprint(42),
            "chromium".to_owned(),
            BrowserProduct::GoogleChrome,
            "ws://127.0.0.1:1/devtools/browser/x".to_owned(),
            false,
            None,
        );
        let key = ExistingProfileGrants::key("public-a", Some("transport-a"), 42);
        grants
            .inner
            .lock()
            .unwrap()
            .live
            .get_mut(&key)
            .unwrap()
            .last_used_at = Instant::now() - GRANT_IDLE_TTL - Duration::from_secs(1);

        let GrantLookup::Expired(expired) = grants.lookup("public-a", Some("transport-a"), 42)
        else {
            panic!("expired grant must be returned for socket cleanup");
        };
        assert_eq!(expired.generation, 1);
        assert_eq!(
            expired.endpoint_ws_url,
            "ws://127.0.0.1:1/devtools/browser/x"
        );
        assert!(matches!(
            grants.lookup("public-a", Some("transport-a"), 42),
            GrantLookup::Missing
        ));
    }

    #[test]
    fn cleanup_ownership_moves_to_another_live_grant_for_the_same_process() {
        let grants = ExistingProfileGrants::new();
        grants.mint(
            "public-a",
            Some("transport-a"),
            42,
            9,
            fingerprint(42),
            "chromium".to_owned(),
            BrowserProduct::GoogleChrome,
            "ws://127.0.0.1:1/devtools/browser/x".to_owned(),
            true,
            None,
        );
        grants.mint(
            "public-b",
            Some("transport-b"),
            42,
            9,
            fingerprint(42),
            "chromium".to_owned(),
            BrowserProduct::GoogleChrome,
            "ws://127.0.0.1:1/devtools/browser/x".to_owned(),
            false,
            None,
        );

        let ended = grants.end_session("transport-a");
        assert_eq!(ended.parked.len(), 1);
        assert!(ended.released.is_empty());
        assert!(!ended.parked[0].cleanup_remote_debugging);
        let GrantLookup::Live(successor) = grants.lookup("public-b", Some("transport-b"), 42)
        else {
            panic!("successor grant must remain live");
        };
        assert!(successor.cleanup_remote_debugging);

        let ended = grants.end_session("transport-b");
        assert!(ended.parked.is_empty());
        assert_eq!(ended.released.len(), 1);
        assert!(ended.released[0].cleanup_remote_debugging);
    }

    #[test]
    fn cleanup_ownership_does_not_move_to_a_reused_pid() {
        let grants = ExistingProfileGrants::new();
        grants.mint(
            "public-a",
            Some("transport-a"),
            42,
            9,
            fingerprint(42),
            "chromium".to_owned(),
            BrowserProduct::GoogleChrome,
            "ws://127.0.0.1:1/devtools/browser/x".to_owned(),
            true,
            None,
        );
        let mut reused = fingerprint(42);
        reused.start_time = Some(8);
        grants.mint(
            "public-b",
            Some("transport-b"),
            42,
            10,
            reused,
            "chromium".to_owned(),
            BrowserProduct::GoogleChrome,
            "ws://127.0.0.1:2/devtools/browser/y".to_owned(),
            false,
            None,
        );

        let ended = grants.end_session("transport-a");
        assert_eq!(ended.released.len(), 1);
        assert!(ended.released[0].cleanup_remote_debugging);
        let GrantLookup::Live(reused_process) = grants.lookup("public-b", Some("transport-b"), 42)
        else {
            panic!("reused-pid grant must remain live");
        };
        assert!(!reused_process.cleanup_remote_debugging);
    }

    #[test]
    fn generations_are_unique_across_sessions_for_the_same_browser() {
        let grants = ExistingProfileGrants::new();
        let mint = |session: &str| {
            grants
                .mint(
                    session,
                    None,
                    42,
                    9,
                    fingerprint(42),
                    "chromium".to_owned(),
                    BrowserProduct::GoogleChrome,
                    "ws://127.0.0.1:1/devtools/browser/x".to_owned(),
                    false,
                    None,
                )
                .generation
        };
        let a = mint("session-a");
        let b = mint("session-b");
        assert_ne!(a, b, "sessions sharing one socket need distinct holders");
        let bumped = grants.bump_generation("session-a", None, 42, a).unwrap();
        assert!(
            bumped > b,
            "a reconnect generation never collides with another session"
        );
    }

    fn mint(grants: &ExistingProfileGrants, session: &str, cleanup: bool) -> ExistingProfileGrant {
        grants.mint(
            session,
            None,
            42,
            9,
            fingerprint(42),
            "chromium".to_owned(),
            BrowserProduct::GoogleChrome,
            "ws://127.0.0.1:1/devtools/browser/x".to_owned(),
            cleanup,
            None,
        )
    }

    fn lost_endpoint() -> BrowserRefusal {
        BrowserRefusal::new(
            BrowserRefusalCode::BrowserRequiresSetup,
            "the approved browser endpoint disappeared during reconnect",
        )
    }

    fn live(grants: &ExistingProfileGrants, session: &str) -> ExistingProfileGrant {
        match grants.lookup(session, None, 42) {
            GrantLookup::Live(grant) => grant,
            _ => panic!("grant for {session} must be live"),
        }
    }

    #[test]
    fn an_ended_session_parks_its_attached_grant_for_the_idle_bound() {
        let grants = ExistingProfileGrants::new();
        let attached = mint(&grants, "a", false);
        assert!(matches!(attached.attachment, Attachment::Attached));

        let ended = grants.end_session("a");
        assert_eq!(ended.parked.len(), 1);
        assert!(ended.released.is_empty());
        assert!(
            matches!(grants.lookup("a", None, 42), GrantLookup::Missing),
            "a parked grant authorizes nothing"
        );

        let parked_at = Instant::now();
        assert!(grants.take_idle_parked(parked_at).is_empty());
        let reaped = grants.take_idle_parked(parked_at + GRANT_IDLE_TTL + Duration::from_secs(1));
        assert_eq!(reaped.len(), 1);
        assert_eq!(reaped[0].generation, attached.generation);
        assert!(matches!(reaped[0].attachment, Attachment::Parked { .. }));
        assert!(grants.take_parked().is_empty());
    }

    #[test]
    fn a_grant_owing_cleanup_is_released_not_parked() {
        let grants = ExistingProfileGrants::new();
        mint(&grants, "a", true);
        let ended = grants.end_session("a");
        assert!(ended.parked.is_empty());
        assert_eq!(ended.released.len(), 1);
        assert!(grants.take_parked().is_empty());
    }

    #[test]
    fn closing_a_socket_ends_only_the_grants_parked_on_it() {
        let grants = ExistingProfileGrants::new();
        mint(&grants, "a", false);
        grants.mint(
            "b",
            None,
            43,
            9,
            fingerprint(43),
            "chromium".to_owned(),
            BrowserProduct::GoogleChrome,
            "ws://127.0.0.1:2/devtools/browser/y".to_owned(),
            false,
            None,
        );
        grants.end_session("a");
        grants.end_session("b");
        let taken = grants.take_parked_on("ws://127.0.0.1:1/devtools/browser/x");
        assert_eq!(taken.len(), 1);
        assert_eq!(taken[0].pid, 42);
        assert_eq!(grants.take_parked().len(), 1);
    }

    #[test]
    fn reminting_parks_the_attached_grant_it_replaces() {
        let grants = ExistingProfileGrants::new();
        let first = mint(&grants, "a", false);
        let second = mint(&grants, "a", false);
        assert_ne!(first.generation, second.generation);
        let parked = grants.take_parked();
        assert_eq!(parked.len(), 1);
        assert_eq!(
            parked[0].generation, first.generation,
            "the replaced generation keeps the socket for the new one to join"
        );
    }

    #[test]
    fn a_lost_endpoint_moves_the_attached_grant_to_needs_setup() {
        let grants = ExistingProfileGrants::new();
        let attached = mint(&grants, "a", false);

        let stale = grants.needs_setup("a", None, 42, attached.generation + 1, lost_endpoint());
        assert!(stale.is_none(), "only the current generation moves");

        let released = grants
            .needs_setup("a", None, 42, attached.generation, lost_endpoint())
            .expect("the attached grant moves to NeedsSetup");
        assert!(released.holds_socket(), "the caller releases what it held");

        let grant = live(&grants, "a");
        let Attachment::NeedsSetup { refusal } = &grant.attachment else {
            panic!("the grant must report NeedsSetup");
        };
        assert_eq!(refusal.code, BrowserRefusalCode::BrowserRequiresSetup);
        assert!(!grant.holds_socket());
        assert_eq!(grant.reconnect_attempts_remaining, MAX_RECONNECT_ATTEMPTS);
        assert!(grants
            .needs_setup("a", None, 42, attached.generation, lost_endpoint())
            .is_none());

        let ended = grants.end_session("a");
        assert!(
            ended.parked.is_empty(),
            "a grant without a socket never parks"
        );
        assert_eq!(ended.released.len(), 1);
    }

    #[test]
    fn a_grant_that_needs_setup_still_expires() {
        let grants = ExistingProfileGrants::new();
        let attached = mint(&grants, "a", true);
        grants.needs_setup("a", None, 42, attached.generation, lost_endpoint());
        grants
            .inner
            .lock()
            .unwrap()
            .live
            .get_mut(&ExistingProfileGrants::key("a", None, 42))
            .unwrap()
            .last_used_at = Instant::now() - GRANT_IDLE_TTL - Duration::from_secs(1);

        let GrantLookup::Expired(expired) = grants.lookup("a", None, 42) else {
            panic!("a NeedsSetup grant expires like any other");
        };
        assert!(matches!(expired.attachment, Attachment::NeedsSetup { .. }));
        assert!(matches!(grants.lookup("a", None, 42), GrantLookup::Missing));

        // Its remote-debugging cleanup failed: the ended grant keeps it.
        grants.owe_cleanup(expired);
        let ended = grants.end_session("a");
        assert_eq!(ended.owed_cleanups.len(), 1);
        assert!(matches!(
            ended.owed_cleanups[0].attachment,
            Attachment::Expired
        ));
    }

    #[test]
    fn prepare_attaches_a_grant_that_needs_setup_again() {
        let grants = ExistingProfileGrants::new();
        let attached = mint(&grants, "a", false);
        grants.needs_setup("a", None, 42, attached.generation, lost_endpoint());

        let reattached = mint(&grants, "a", false);
        assert!(matches!(reattached.attachment, Attachment::Attached));
        assert!(matches!(
            live(&grants, "a").attachment,
            Attachment::Attached
        ));
        assert!(
            grants.take_parked().is_empty(),
            "the replaced grant held no socket, so nothing parks"
        );
    }

    #[test]
    fn a_failed_cleanup_waits_on_its_grant_for_the_session_end() {
        let grants = ExistingProfileGrants::new();
        let grant = mint(&grants, "a", true);
        let GrantLookup::Live(_) = grants.lookup("a", None, 42) else {
            panic!("grant must be live");
        };
        let revoked = grants.revoke("a", None, 42).unwrap();
        grants.owe_cleanup(revoked);
        assert!(matches!(grants.lookup("a", None, 42), GrantLookup::Missing));
        assert!(grants.end_session("b").owed_cleanups.is_empty());

        let ended = grants.end_session("a");
        assert_eq!(ended.owed_cleanups.len(), 1);
        assert_eq!(ended.owed_cleanups[0].generation, grant.generation);
        assert!(matches!(
            ended.owed_cleanups[0].attachment,
            Attachment::Expired
        ));
        assert!(grants.end_session("a").owed_cleanups.is_empty());
    }

    #[test]
    fn a_reconnect_attempt_needs_the_grant_still_attached_at_its_generation() {
        let grants = ExistingProfileGrants::new();
        let attached = mint(&grants, "a", false);
        let stale = grants
            .bump_generation("a", None, 42, attached.generation + 1)
            .unwrap_err();
        assert_eq!(stale.code, BrowserRefusalCode::BrowserBindingStale);

        grants.needs_setup("a", None, 42, attached.generation, lost_endpoint());
        let refused = grants
            .bump_generation("a", None, 42, attached.generation)
            .unwrap_err();
        assert_eq!(refused.code, BrowserRefusalCode::BrowserRequiresSetup);
        assert_eq!(
            live(&grants, "a").reconnect_attempts_remaining,
            MAX_RECONNECT_ATTEMPTS,
            "a refused attempt spends no reconnect"
        );
    }

    #[test]
    fn a_grant_parked_past_its_idle_bound_keeps_no_socket_in_use() {
        use super::super::cdp_ws::SharedSocketHolders;
        let grants = ExistingProfileGrants::new();
        let parked = mint(&grants, "a", false);
        grants.end_session("a");
        let attached = mint(&grants, "b", false);
        let now = Instant::now();
        let later = now + GRANT_IDLE_TTL + Duration::from_secs(1);
        let only_parked = std::collections::HashSet::from([parked.generation]);
        assert!(grants.keep_in_use(&only_parked, now));
        assert!(!grants.keep_in_use(&only_parked, later));
        let both = std::collections::HashSet::from([parked.generation, attached.generation]);
        assert!(
            grants.keep_in_use(&both, later),
            "an attached holder keeps it"
        );
    }
}
