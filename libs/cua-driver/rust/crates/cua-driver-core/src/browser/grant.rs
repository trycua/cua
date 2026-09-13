//! Daemon-memory grants for user-owned browser profile attachment.
//!
//! Grant identifiers never cross the public tool boundary. The store itself is
//! the daemon-instance boundary: dropping/restarting the engine drops every
//! grant, connection generation, and reconnect budget.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::refusal::{BrowserRefusal, BrowserRefusalCode};
use super::types::{BrowserProduct, ProcessFingerprint};

const GRANT_IDLE_TTL: Duration = Duration::from_secs(30 * 60);
const GRANT_ABSOLUTE_TTL: Duration = Duration::from_secs(8 * 60 * 60);
pub const MAX_RECONNECT_ATTEMPTS: u8 = 3;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct GrantKey {
    public_session: String,
    transport_session: String,
    pid: i64,
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
    created_at: Instant,
    last_used_at: Instant,
}

pub(crate) enum GrantLookup {
    Missing,
    Live(ExistingProfileGrant),
    Expired(ExistingProfileGrant),
}

impl ExistingProfileGrant {
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
pub(crate) struct ExistingProfileGrants {
    inner: Mutex<HashMap<GrantKey, ExistingProfileGrant>>,
}

impl ExistingProfileGrants {
    pub fn new() -> Self {
        Self::default()
    }

    fn key(public_session: &str, transport_session: Option<&str>, pid: i64) -> GrantKey {
        GrantKey {
            public_session: public_session.to_owned(),
            transport_session: transport_session.unwrap_or(public_session).to_owned(),
            pid,
        }
    }

    // Minting deliberately captures the complete immutable browser grant in
    // one call so partially initialized grants cannot enter the registry.
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
        let generation = self
            .inner
            .lock()
            .unwrap()
            .get(&key)
            .map_or(1, |grant| grant.generation.saturating_add(1));
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
            created_at: now,
            last_used_at: now,
        };
        self.inner.lock().unwrap().insert(key, grant.clone());
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
        let mut grants = self.inner.lock().unwrap();
        let Some(grant) = grants.get_mut(&key) else {
            return GrantLookup::Missing;
        };
        if grant.expired(now) || !grant.authorization_context_matches() {
            let mut expired = grants.remove(&key).expect("expired grant exists");
            transfer_cleanup_ownership(&mut grants, &mut expired);
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
        let mut grants = self.inner.lock().unwrap();
        let mut removed = grants.remove(&Self::key(public_session, transport_session, pid))?;
        transfer_cleanup_ownership(&mut grants, &mut removed);
        Some(removed)
    }

    pub fn remove_session(&self, session: &str) -> Vec<ExistingProfileGrant> {
        let mut removed = Vec::new();
        let mut grants = self.inner.lock().unwrap();
        grants.retain(|_, grant| {
            let keep = grant.public_session != session && grant.transport_session != session;
            if !keep {
                removed.push(grant.clone());
            }
            keep
        });
        for grant in &mut removed {
            transfer_cleanup_ownership(&mut grants, grant);
        }
        removed
    }

    pub fn bump_generation(
        &self,
        public_session: &str,
        transport_session: Option<&str>,
        pid: i64,
    ) -> Result<u64, BrowserRefusal> {
        let key = Self::key(public_session, transport_session, pid);
        let mut grants = self.inner.lock().unwrap();
        let grant = grants.get_mut(&key).ok_or_else(|| {
            BrowserRefusal::new(
                BrowserRefusalCode::BrowserConsentRequired,
                "no live existing-profile grant remains for reconnect",
            )
        })?;
        if grant.reconnect_attempts_remaining == 0 {
            return Err(BrowserRefusal::new(
                BrowserRefusalCode::BrowserReconnectExhausted,
                "the bounded existing-profile reconnect budget is exhausted",
            ));
        }
        grant.reconnect_attempts_remaining -= 1;
        grant.generation = grant.generation.saturating_add(1);
        grant.last_used_at = Instant::now();
        Ok(grant.generation)
    }
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
        assert_eq!(grants.remove_session("transport-a").len(), 1);
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

        let removed = grants.remove_session("transport-a");
        assert_eq!(removed.len(), 1);
        assert!(!removed[0].cleanup_remote_debugging);
        let GrantLookup::Live(successor) = grants.lookup("public-b", Some("transport-b"), 42)
        else {
            panic!("successor grant must remain live");
        };
        assert!(successor.cleanup_remote_debugging);

        let removed = grants.remove_session("transport-b");
        assert_eq!(removed.len(), 1);
        assert!(removed[0].cleanup_remote_debugging);
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

        let removed = grants.remove_session("transport-a");
        assert_eq!(removed.len(), 1);
        assert!(removed[0].cleanup_remote_debugging);
        let GrantLookup::Live(reused_process) = grants.lookup("public-b", Some("transport-b"), 42)
        else {
            panic!("reused-pid grant must remain live");
        };
        assert!(!reused_process.cleanup_remote_debugging);
    }
}
