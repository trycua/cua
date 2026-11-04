// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Storage: the connection registry, the append-only audit log, the
//! device-proof replay cache ([`Registry`]) and the spend ledger
//! ([`Ledger`]). [`MemoryRegistry`] is for tests and a single replica;
//! [`crate::pg::PgRegistry`] is the deployed store.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;

use crate::model::{AuditRecord, Connection, Status};

/// A storage failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// A live connection of the same cloud and identifier exists (its id).
    Exists(String),
    /// The record changed since it was read (retry).
    Conflict,
    /// No such record.
    NotFound,
    /// The store failed.
    Unavailable(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Exists(id) => write!(f, "connection {id} already links this cloud"),
            Self::Conflict => f.write_str("the connection changed; retry"),
            Self::NotFound => f.write_str("no such connection"),
            Self::Unavailable(e) => write!(f, "storage: {e}"),
        }
    }
}

impl std::error::Error for StoreError {}

/// The issuer's durable state.
#[async_trait]
pub trait Registry: Send + Sync {
    /// Stores a new connection. [`StoreError::Exists`] when the account
    /// already has a live (not revoked) connection to the same cloud with
    /// the same naming identifier.
    async fn create(&self, connection: &Connection) -> Result<(), StoreError>;
    /// The account's connections, oldest first.
    async fn list(&self, account: &str) -> Result<Vec<Connection>, StoreError>;
    /// One connection of the account.
    async fn get(&self, account: &str, id: &str) -> Result<Option<Connection>, StoreError>;
    /// Replaces a connection if its stored version is still
    /// `connection.version - 1` ([`StoreError::Conflict`] otherwise).
    async fn update(&self, connection: &Connection) -> Result<(), StoreError>;
    /// Deletes a connection; whether it existed.
    async fn delete(&self, account: &str, id: &str) -> Result<bool, StoreError>;
    /// Appends an audit row (rows are never changed or removed).
    async fn append_audit(&self, record: &AuditRecord) -> Result<(), StoreError>;
    /// The account's newest audit rows (of one connection when given).
    async fn audit(
        &self,
        account: &str,
        connection: Option<&str>,
        limit: usize,
    ) -> Result<Vec<AuditRecord>, StoreError>;
    /// Records a device proof's `jti` until `expires_at`: `false` when it
    /// was already seen (a replay). Shared by every replica.
    async fn claim_proof(&self, key: &str, expires_at: u64) -> Result<bool, StoreError>;
    /// Whether the store answers (readiness).
    async fn ready(&self) -> Result<(), StoreError> {
        Ok(())
    }
}

/// This month's spend per connection, as the account's devices report it.
#[async_trait]
pub trait Ledger: Send + Sync {
    /// US dollars spent this calendar month (UTC) on `connection`: the sum
    /// of what each device reported for the month.
    async fn spent_month(&self, account: &str, connection: &str) -> Result<f64, StoreError>;
    /// Records what `device` spent on `connection` in `month` (`YYYY-MM`),
    /// replacing its earlier report for that month (a device reports its
    /// running total, so a repeat is harmless).
    async fn record_spend(
        &self,
        account: &str,
        connection: &str,
        device: &str,
        month: &str,
        usd: f64,
    ) -> Result<(), StoreError>;
}

/// Most replay-cache entries kept in memory.
const MAX_PROOFS: usize = 100_000;

/// An in-memory registry (tests, a single replica).
#[derive(Default)]
pub struct MemoryRegistry {
    connections: Mutex<Vec<Connection>>,
    audit: Mutex<Vec<AuditRecord>>,
    proofs: Mutex<HashMap<String, u64>>,
}

impl MemoryRegistry {
    /// Empty.
    pub fn new() -> Self {
        Self::default()
    }

    /// Every audit row (tests).
    pub fn all_audit(&self) -> Vec<AuditRecord> {
        self.audit.lock().expect("audit").clone()
    }
}

#[async_trait]
impl Registry for MemoryRegistry {
    async fn create(&self, connection: &Connection) -> Result<(), StoreError> {
        let mut all = self.connections.lock().expect("connections");
        if let Some(existing) = all.iter().find(|c| {
            c.account == connection.account
                && c.cloud == connection.cloud
                && c.status != Status::Revoked
                && c.primary_id() == connection.primary_id()
        }) {
            return Err(StoreError::Exists(existing.id.clone()));
        }
        if all.iter().any(|c| c.id == connection.id) {
            return Err(StoreError::Conflict);
        }
        all.push(connection.clone());
        Ok(())
    }

    async fn list(&self, account: &str) -> Result<Vec<Connection>, StoreError> {
        Ok(self
            .connections
            .lock()
            .expect("connections")
            .iter()
            .filter(|c| c.account == account)
            .cloned()
            .collect())
    }

    async fn get(&self, account: &str, id: &str) -> Result<Option<Connection>, StoreError> {
        Ok(self
            .connections
            .lock()
            .expect("connections")
            .iter()
            .find(|c| c.account == account && c.id == id)
            .cloned())
    }

    async fn update(&self, connection: &Connection) -> Result<(), StoreError> {
        let mut all = self.connections.lock().expect("connections");
        let Some(slot) = all
            .iter_mut()
            .find(|c| c.account == connection.account && c.id == connection.id)
        else {
            return Err(StoreError::NotFound);
        };
        if slot.version + 1 != connection.version {
            return Err(StoreError::Conflict);
        }
        *slot = connection.clone();
        Ok(())
    }

    async fn delete(&self, account: &str, id: &str) -> Result<bool, StoreError> {
        let mut all = self.connections.lock().expect("connections");
        let before = all.len();
        all.retain(|c| !(c.account == account && c.id == id));
        Ok(all.len() != before)
    }

    async fn append_audit(&self, record: &AuditRecord) -> Result<(), StoreError> {
        self.audit.lock().expect("audit").push(record.clone());
        Ok(())
    }

    async fn audit(
        &self,
        account: &str,
        connection: Option<&str>,
        limit: usize,
    ) -> Result<Vec<AuditRecord>, StoreError> {
        Ok(self
            .audit
            .lock()
            .expect("audit")
            .iter()
            .rev()
            .filter(|r| r.account == account)
            .filter(|r| connection.is_none() || r.connection.as_deref() == connection)
            .take(limit)
            .cloned()
            .collect())
    }

    async fn claim_proof(&self, key: &str, expires_at: u64) -> Result<bool, StoreError> {
        let now = crate::now_secs();
        let mut proofs = self.proofs.lock().expect("proofs");
        if proofs.len() >= MAX_PROOFS {
            proofs.retain(|_, exp| *exp > now);
            if proofs.len() >= MAX_PROOFS {
                return Err(StoreError::Unavailable("replay cache is full".into()));
            }
        }
        match proofs.get(key) {
            Some(exp) if *exp > now => Ok(false),
            _ => {
                proofs.insert(key.to_owned(), expires_at);
                Ok(true)
            }
        }
    }
}

/// An in-memory ledger (tests, a single replica without a database): the
/// devices' reports, plus spend set by hand in tests.
#[derive(Default)]
pub struct MemoryLedger {
    spent: Mutex<HashMap<(String, String), f64>>,
    /// (account, connection, device, month) -> usd.
    reports: Mutex<HashMap<(String, String, String, String), f64>>,
}

impl MemoryLedger {
    /// Empty.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets this month's spend of a connection.
    pub fn set(&self, account: &str, connection: &str, usd: f64) {
        self.spent
            .lock()
            .expect("ledger")
            .insert((account.to_owned(), connection.to_owned()), usd);
    }
}

#[async_trait]
impl Ledger for MemoryLedger {
    async fn spent_month(&self, account: &str, connection: &str) -> Result<f64, StoreError> {
        let set = self
            .spent
            .lock()
            .expect("ledger")
            .get(&(account.to_owned(), connection.to_owned()))
            .copied()
            .unwrap_or(0.0);
        let month = crate::this_month();
        let reported: f64 = self
            .reports
            .lock()
            .expect("ledger")
            .iter()
            .filter(|((a, c, _, m), _)| a == account && c == connection && *m == month)
            .map(|(_, usd)| usd)
            .sum();
        Ok(set + reported)
    }

    async fn record_spend(
        &self,
        account: &str,
        connection: &str,
        device: &str,
        month: &str,
        usd: f64,
    ) -> Result<(), StoreError> {
        self.reports.lock().expect("ledger").insert(
            (
                account.to_owned(),
                connection.to_owned(),
                device.to_owned(),
                month.to_owned(),
            ),
            usd,
        );
        Ok(())
    }
}
