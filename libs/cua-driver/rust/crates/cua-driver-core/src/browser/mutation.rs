//! Canonical browser-target mutation gates.
//!
//! Public session ids are capability namespaces, not browser identity. Gates
//! therefore key on process identity plus the real CDP target so two sessions
//! addressing one tab serialize while independently proven tabs can proceed.
//! Consequential paths take gates in this order: real tab, any browser-wide
//! download gate, session admission read, exact origin, then a short store
//! lock. Store locks never cross an await.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use tokio::sync::{
    Mutex as AsyncMutex, OwnedMutexGuard, OwnedRwLockReadGuard, OwnedRwLockWriteGuard,
    RwLock as AsyncRwLock,
};

use super::types::ProcessFingerprint;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct MutationKey {
    pid: i64,
    start_time: Option<u64>,
    executable: Option<String>,
    cdp_target_id: String,
}

impl MutationKey {
    pub fn new(fingerprint: &ProcessFingerprint, cdp_target_id: &str) -> Self {
        Self {
            pid: fingerprint.pid,
            start_time: fingerprint.start_time,
            executable: fingerprint.executable.clone(),
            cdp_target_id: cdp_target_id.to_owned(),
        }
    }
}

#[derive(Default)]
pub(crate) struct MutationGates {
    gates: Mutex<HashMap<MutationKey, Arc<AsyncMutex<()>>>>,
}

impl MutationGates {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn lock(&self, key: MutationKey) -> OwnedMutexGuard<()> {
        let gate = self
            .gates
            .lock()
            .unwrap()
            .entry(key)
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone();
        gate.lock_owned().await
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct OriginAdmissionKey {
    session: String,
    origin: String,
}

#[derive(Default)]
struct OriginAdmissionGateSet {
    sessions: HashMap<String, Weak<AsyncRwLock<()>>>,
    origins: HashMap<OriginAdmissionKey, Weak<AsyncMutex<()>>>,
}

/// Session- and origin-scoped admission gates for consequential browser work.
///
/// Ordinary actions take a session read lock and then the exact origin lock.
/// The read lock leaves unrelated origins concurrent while preserving a fixed
/// place in the lock order for session-wide safety transitions. Registry
/// entries are weak so idle gates do not retain runtime state; session cleanup
/// also removes their keys so ended session labels do not accumulate.
#[derive(Default)]
pub(crate) struct OriginAdmissionGates {
    gates: Mutex<OriginAdmissionGateSet>,
}

/// Keeps an origin admission decision stable through the caller's last
/// consequential dispatch.
pub(crate) struct OriginAdmissionGuard {
    // Fields drop in declaration order, so release in reverse lock order.
    _origin: OwnedMutexGuard<()>,
    _session: OwnedRwLockReadGuard<()>,
    session: String,
    origin: String,
}

/// Owns the session-wide write boundary used to install blocker state.
///
/// A transition waits for actions admitted before the observation, prevents
/// new actions from entering, and then takes the exact origin lock. This makes
/// a session-wide capacity latch visible only after earlier dispatches finish.
pub(crate) struct OriginTransitionGuard {
    // Fields drop in declaration order, so release in reverse lock order.
    _origin: OwnedMutexGuard<()>,
    _session: OwnedRwLockWriteGuard<()>,
    session: String,
    origin: String,
}

impl OriginAdmissionGuard {
    pub(crate) fn session(&self) -> &str {
        &self.session
    }

    pub(crate) fn origin(&self) -> &str {
        &self.origin
    }
}

impl OriginTransitionGuard {
    pub(crate) fn session(&self) -> &str {
        &self.session
    }

    pub(crate) fn origin(&self) -> &str {
        &self.origin
    }
}

impl OriginAdmissionGates {
    pub fn new() -> Self {
        Self::default()
    }

    fn gates_for(
        &self,
        session: &str,
        origin: &str,
    ) -> (
        Arc<AsyncRwLock<()>>,
        Arc<AsyncMutex<()>>,
        OriginAdmissionKey,
    ) {
        let key = OriginAdmissionKey {
            session: session.to_owned(),
            origin: origin.to_owned(),
        };
        let (session_gate, origin_gate) = {
            let mut gates = self.gates.lock().unwrap();
            gates.sessions.retain(|_, gate| gate.strong_count() != 0);
            gates.origins.retain(|_, gate| gate.strong_count() != 0);
            let session_gate = gates
                .sessions
                .get(session)
                .and_then(Weak::upgrade)
                .unwrap_or_else(|| {
                    let gate = Arc::new(AsyncRwLock::new(()));
                    gates
                        .sessions
                        .insert(session.to_owned(), Arc::downgrade(&gate));
                    gate
                });
            let origin_gate = gates
                .origins
                .get(&key)
                .and_then(Weak::upgrade)
                .unwrap_or_else(|| {
                    let gate = Arc::new(AsyncMutex::new(()));
                    gates.origins.insert(key.clone(), Arc::downgrade(&gate));
                    gate
                });
            (session_gate, origin_gate)
        };
        (session_gate, origin_gate, key)
    }

    pub async fn lock(&self, session: &str, origin: &str) -> OriginAdmissionGuard {
        let (session_gate, origin_gate, key) = self.gates_for(session, origin);

        // Lock ordering is part of the browser mutation contract: session
        // admission always precedes exact-origin admission.
        let session = session_gate.read_owned().await;
        let origin = origin_gate.lock_owned().await;
        OriginAdmissionGuard {
            _origin: origin,
            _session: session,
            session: key.session,
            origin: key.origin,
        }
    }

    pub async fn transition(&self, session: &str, origin: &str) -> OriginTransitionGuard {
        let (session_gate, origin_gate, key) = self.gates_for(session, origin);

        // Never upgrade a read guard. Callers release ordinary admission before
        // taking this write boundary, then follow the same session→origin order.
        let session = session_gate.write_owned().await;
        let origin = origin_gate.lock_owned().await;
        OriginTransitionGuard {
            _origin: origin,
            _session: session,
            session: key.session,
            origin: key.origin,
        }
    }

    pub fn remove_session(&self, session: &str) {
        let mut gates = self.gates.lock().unwrap();
        gates.sessions.remove(session);
        gates.origins.retain(|key, _| key.session != session);
    }

    #[cfg(test)]
    pub(crate) fn registry_counts(&self) -> (usize, usize) {
        let gates = self.gates.lock().unwrap();
        (gates.sessions.len(), gates.origins.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fingerprint() -> ProcessFingerprint {
        ProcessFingerprint {
            pid: 42,
            start_time: Some(1),
            executable: Some("chrome".to_owned()),
        }
    }

    #[tokio::test]
    async fn same_real_tab_serializes_across_callers() {
        let gates = MutationGates::new();
        let key = MutationKey::new(&fingerprint(), "target-a");
        let first = gates.lock(key.clone()).await;
        let second = tokio::time::timeout(
            std::time::Duration::from_millis(20),
            gates.lock(key.clone()),
        )
        .await;
        assert!(
            second.is_err(),
            "same target must wait for the live mutation"
        );
        drop(first);
        let _next = gates.lock(key).await;
    }

    #[tokio::test]
    async fn independent_tabs_do_not_block_each_other() {
        let gates = MutationGates::new();
        let _first = gates
            .lock(MutationKey::new(&fingerprint(), "target-a"))
            .await;
        tokio::time::timeout(
            std::time::Duration::from_millis(20),
            gates.lock(MutationKey::new(&fingerprint(), "target-b")),
        )
        .await
        .expect("independent target should not block");
    }

    #[tokio::test]
    async fn same_session_origin_serializes_admission() {
        let gates = OriginAdmissionGates::new();
        let first = gates.lock("session-a", "https://same.example").await;
        let second = tokio::time::timeout(
            std::time::Duration::from_millis(20),
            gates.lock("session-a", "https://same.example"),
        )
        .await;
        assert!(second.is_err(), "same origin must wait for admission");
        drop(first);
        let _next = gates.lock("session-a", "https://same.example").await;
    }

    #[tokio::test]
    async fn unrelated_origins_remain_concurrent() {
        let gates = OriginAdmissionGates::new();
        let _first = gates.lock("session-a", "https://one.example").await;
        tokio::time::timeout(
            std::time::Duration::from_millis(20),
            gates.lock("session-a", "https://two.example"),
        )
        .await
        .expect("different origins in one session should not block");
    }

    #[tokio::test]
    async fn session_transition_waits_for_existing_actions_and_blocks_new_admission() {
        let gates = Arc::new(OriginAdmissionGates::new());
        let existing = gates.lock("session-a", "https://one.example").await;
        let transition_gates = gates.clone();
        let transition = tokio::spawn(async move {
            transition_gates
                .transition("session-a", "https://blocker.example")
                .await
        });
        tokio::task::yield_now().await;
        assert!(!transition.is_finished());

        let later_gates = gates.clone();
        let later =
            tokio::spawn(async move { later_gates.lock("session-a", "https://two.example").await });
        tokio::task::yield_now().await;
        assert!(!later.is_finished());

        drop(existing);
        let transition = transition.await.unwrap();
        assert!(!later.is_finished());
        drop(transition);
        let _later = later.await.unwrap();
    }

    #[tokio::test]
    async fn same_origin_in_different_sessions_remains_concurrent() {
        let gates = OriginAdmissionGates::new();
        let _first = gates.lock("session-a", "https://same.example").await;
        tokio::time::timeout(
            std::time::Duration::from_millis(20),
            gates.lock("session-b", "https://same.example"),
        )
        .await
        .expect("separate runtime sessions should not share admission state");
    }

    #[tokio::test]
    async fn session_cleanup_prunes_registry_keys_without_revoking_live_guards() {
        let gates = OriginAdmissionGates::new();
        let guard = gates.lock("session-a", "https://same.example").await;
        assert_eq!(gates.registry_counts(), (1, 1));

        gates.remove_session("session-a");
        assert_eq!(gates.registry_counts(), (0, 0));

        // Removing registry keys cannot invalidate an operation that already
        // owns its admission guards.
        drop(guard);
    }

    #[tokio::test]
    async fn later_admission_prunes_idle_weak_entries() {
        let gates = OriginAdmissionGates::new();
        drop(gates.lock("session-a", "https://old.example").await);
        assert_eq!(gates.registry_counts(), (1, 1));

        let _live = gates.lock("session-b", "https://live.example").await;
        assert_eq!(gates.registry_counts(), (1, 1));
    }
}
