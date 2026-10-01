//! Canonical browser-target mutation gates.
//!
//! Public session ids are capability namespaces, not browser identity. Gates
//! therefore key on process identity plus the real CDP target so two sessions
//! addressing one tab serialize while independently proven tabs can proceed.

use super::keyed_gates::KeyedGates;
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

pub(crate) type MutationGates = KeyedGates<MutationKey>;
