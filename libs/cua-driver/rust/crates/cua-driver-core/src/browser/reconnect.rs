//! Per-browser reconnect leadership.
//!
//! A public session is not browser identity. Reconnect callers therefore
//! single-flight on the approved process fingerprint and endpoint while
//! unrelated browsers remain independent.

use super::keyed_gates::KeyedGates;
use super::types::ProcessFingerprint;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ReconnectKey {
    pid: i64,
    start_time: Option<u64>,
    executable: Option<String>,
    endpoint_ws_url: String,
}

impl ReconnectKey {
    pub fn new(fingerprint: &ProcessFingerprint, endpoint_ws_url: &str) -> Self {
        Self {
            pid: fingerprint.pid,
            start_time: fingerprint.start_time,
            executable: fingerprint.executable.clone(),
            endpoint_ws_url: endpoint_ws_url.to_owned(),
        }
    }
}

pub(crate) type ReconnectGates = KeyedGates<ReconnectKey>;
