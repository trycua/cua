//! Per-browser reconnect leadership.
//!
//! A public session is not browser identity. Reconnect callers therefore
//! single-flight on the approved process fingerprint and endpoint while
//! unrelated browsers remain independent.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

use super::types::ProcessFingerprint;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ReconnectKey {
    pid: i64,
    start_time: Option<u64>,
    executable: Option<String>,
    endpoint_ws_url: String,
}

impl ReconnectKey {
    fn new(fingerprint: &ProcessFingerprint, endpoint_ws_url: &str) -> Self {
        Self {
            pid: fingerprint.pid,
            start_time: fingerprint.start_time,
            executable: fingerprint.executable.clone(),
            endpoint_ws_url: endpoint_ws_url.to_owned(),
        }
    }
}

#[derive(Default)]
pub(crate) struct ReconnectGates {
    gates: Mutex<HashMap<ReconnectKey, Arc<AsyncMutex<()>>>>,
}

impl ReconnectGates {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn lock(
        &self,
        fingerprint: &ProcessFingerprint,
        endpoint_ws_url: &str,
    ) -> OwnedMutexGuard<()> {
        let gate = self
            .gates
            .lock()
            .unwrap()
            .entry(ReconnectKey::new(fingerprint, endpoint_ws_url))
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone();
        gate.lock_owned().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fingerprint(pid: i64) -> ProcessFingerprint {
        ProcessFingerprint {
            pid,
            start_time: Some(11),
            executable: Some("chrome".to_owned()),
        }
    }

    #[tokio::test]
    async fn same_browser_has_one_reconnect_leader() {
        let gates = ReconnectGates::new();
        let identity = fingerprint(42);
        let endpoint = "ws://127.0.0.1:1/devtools/browser/a";
        let first = gates.lock(&identity, endpoint).await;
        assert!(tokio::time::timeout(
            std::time::Duration::from_millis(20),
            gates.lock(&identity, endpoint),
        )
        .await
        .is_err());
        drop(first);
        let _next = gates.lock(&identity, endpoint).await;
    }

    #[tokio::test]
    async fn unrelated_browsers_reconnect_independently() {
        let gates = ReconnectGates::new();
        let _first = gates
            .lock(&fingerprint(42), "ws://127.0.0.1:1/devtools/browser/a")
            .await;
        tokio::time::timeout(
            std::time::Duration::from_millis(20),
            gates.lock(&fingerprint(43), "ws://127.0.0.1:2/devtools/browser/b"),
        )
        .await
        .expect("an unrelated browser must not wait on another prompt");
    }
}
