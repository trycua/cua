// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Input leases: at most one principal drives a given window at a time.
//!
//! Two people can drive two different windows concurrently, and an agent can
//! work in one window while a human works in another. A lease expires after a
//! period without input from its holder, after which anyone may take it. When
//! ownership moves, the caller releases every key and button the previous
//! holder left pressed.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Idle time after which a window's input lease is free again.
pub const DEFAULT_LEASE_TTL: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaseGrant {
    /// The caller already held the lease (or took a free one).
    Held,
    /// The caller took over an expired lease from `previous`.
    Transferred { previous: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaseConflict {
    pub holder_id: String,
    pub holder_name: String,
    pub retry_after: Duration,
}

#[derive(Debug)]
struct Lease {
    principal_id: String,
    principal_name: String,
    last_input: Instant,
}

#[derive(Debug)]
pub struct InputLeases {
    ttl: Duration,
    leases: Mutex<HashMap<String, Lease>>,
}

impl Default for InputLeases {
    fn default() -> Self {
        Self::new(DEFAULT_LEASE_TTL)
    }
}

impl InputLeases {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            leases: Mutex::new(HashMap::new()),
        }
    }

    /// Take or refresh the lease on `window_key` for a principal.
    pub fn acquire(
        &self,
        window_key: &str,
        principal_id: &str,
        principal_name: &str,
    ) -> Result<LeaseGrant, LeaseConflict> {
        self.acquire_at(window_key, principal_id, principal_name, Instant::now())
    }

    pub fn acquire_at(
        &self,
        window_key: &str,
        principal_id: &str,
        principal_name: &str,
        now: Instant,
    ) -> Result<LeaseGrant, LeaseConflict> {
        let mut leases = self
            .leases
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let grant = match leases.get(window_key) {
            Some(lease) if lease.principal_id == principal_id => LeaseGrant::Held,
            Some(lease) => {
                let idle = now.saturating_duration_since(lease.last_input);
                if idle < self.ttl {
                    return Err(LeaseConflict {
                        holder_id: lease.principal_id.clone(),
                        holder_name: lease.principal_name.clone(),
                        retry_after: self.ttl - idle,
                    });
                }
                LeaseGrant::Transferred {
                    previous: lease.principal_id.clone(),
                }
            }
            None => LeaseGrant::Held,
        };
        leases.insert(
            window_key.to_owned(),
            Lease {
                principal_id: principal_id.to_owned(),
                principal_name: principal_name.to_owned(),
                last_input: now,
            },
        );
        Ok(grant)
    }

    /// Drop every lease held by a principal (it left or disconnected).
    pub fn release_principal(&self, principal_id: &str) {
        self.leases
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|_, lease| lease.principal_id != principal_id);
    }

    /// Current holder of a window's lease, if it has not expired.
    pub fn holder(&self, window_key: &str) -> Option<String> {
        let leases = self
            .leases
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        leases
            .get(window_key)
            .filter(|lease| lease.last_input.elapsed() < self.ttl)
            .map(|lease| lease.principal_id.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_principals_can_drive_two_windows() {
        let leases = InputLeases::default();
        assert_eq!(leases.acquire("w1", "alice", "Alice"), Ok(LeaseGrant::Held));
        assert_eq!(leases.acquire("w2", "bob", "Bob"), Ok(LeaseGrant::Held));
        assert_eq!(leases.acquire("w1", "alice", "Alice"), Ok(LeaseGrant::Held));
    }

    #[test]
    fn a_held_window_refuses_another_principal_until_idle() {
        let leases = InputLeases::new(Duration::from_secs(5));
        let start = Instant::now();
        leases.acquire_at("w1", "alice", "Alice", start).unwrap();
        let conflict = leases
            .acquire_at("w1", "bob", "Bob", start + Duration::from_secs(1))
            .unwrap_err();
        assert_eq!(conflict.holder_id, "alice");
        assert_eq!(conflict.retry_after, Duration::from_secs(4));
        assert_eq!(
            leases.acquire_at("w1", "bob", "Bob", start + Duration::from_secs(6)),
            Ok(LeaseGrant::Transferred {
                previous: "alice".into()
            })
        );
    }

    #[test]
    fn leaving_releases_leases() {
        let leases = InputLeases::default();
        leases.acquire("w1", "alice", "Alice").unwrap();
        leases.release_principal("alice");
        assert_eq!(leases.holder("w1"), None);
        assert_eq!(leases.acquire("w1", "bob", "Bob"), Ok(LeaseGrant::Held));
    }
}
