//! Canonical browser-target mutation gates.
//!
//! Public session ids are capability namespaces, not browser identity. Gates
//! therefore key on process identity plus the real CDP target so two sessions
//! addressing one tab serialize while independently proven tabs can proceed.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

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
}
