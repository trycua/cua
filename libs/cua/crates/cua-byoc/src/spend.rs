// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A linked cloud's budget is the account's, whichever device spends it.
//! Each device reports what it spent on an OIDC or sealed link this month
//! (list price times runtime of the machines it created, its running
//! total) to Cua's issuer, which adds up the account's devices: the issuer
//! refuses `create` tokens at the budget, and the client checks the same
//! total before any machine ([`crate::Store::spent_for_limits`]).
//!
//! Reports go when a machine is recorded or forgotten, when a create is
//! checked and when `cloud_status` runs. They never block or fail what the
//! person or agent asked for: offline, the next one catches up. An issuer
//! without spend reports (older) is asked once per run and then left
//! alone, so this home's own figure decides, as before.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::federation::{IssuerClient, IssuerError};
use crate::model::Connection;
use crate::state::AccountSpend;

/// The same report again within this long is not sent.
const DEBOUNCE: Duration = Duration::from_secs(60);

#[derive(Clone)]
struct Seen {
    month: String,
    /// This home's figure last reported (cents).
    reported_cents: i64,
    at: Instant,
    /// The account's total as last answered.
    total: f64,
}

/// [`AccountSpend`] over Cua's issuer.
pub struct IssuerSpend {
    issuer: Arc<Mutex<Option<Arc<dyn IssuerClient>>>>,
    seen: Mutex<HashMap<String, Seen>>,
    /// The issuer has no spend reports (an older one).
    unsupported: std::sync::atomic::AtomicBool,
}

impl IssuerSpend {
    /// Reports through the issuer in `slot` (the one [`crate::Clouds`]
    /// uses; filled from the environment on first use).
    pub fn new(slot: Arc<Mutex<Option<Arc<dyn IssuerClient>>>>) -> Self {
        IssuerSpend {
            issuer: slot,
            seen: Mutex::default(),
            unsupported: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn issuer(&self) -> Option<Arc<dyn IssuerClient>> {
        crate::sealed::issuer_of(&self.issuer)
    }
}

#[async_trait::async_trait]
impl AccountSpend for IssuerSpend {
    async fn sync(&self, conn: &Connection, local_usd: f64, fresh: bool) -> Option<f64> {
        let id = conn.auth.conn_id();
        if id.is_empty() || self.unsupported.load(std::sync::atomic::Ordering::Relaxed) {
            return self.cached(conn);
        }
        let month = crate::limits::month_key(crate::model::now());
        let cents = (local_usd.max(0.0) * 100.0).round() as i64;
        if !fresh
            && let Some(s) = self.seen.lock().expect("spend").get(id)
            && s.month == month
            && s.reported_cents == cents
            && s.at.elapsed() < DEBOUNCE
        {
            return Some(s.total);
        }
        let issuer = self.issuer()?;
        match issuer.report_spend(id, &month, cents as f64 / 100.0).await {
            Ok(total) => {
                self.seen.lock().expect("spend").insert(
                    id.to_string(),
                    Seen {
                        month,
                        reported_cents: cents,
                        at: Instant::now(),
                        total,
                    },
                );
                Some(total)
            }
            Err(IssuerError::NotFound(_)) if conn_known_elsewhere(&*issuer, id).await => {
                // The connection is there, the route is not: an older issuer.
                self.unsupported
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                tracing::debug!(
                    "Cua's issuer takes no spend reports; this device's figure decides"
                );
                None
            }
            Err(e) => {
                tracing::debug!(connection = id, "spend not reported: {e}");
                self.cached(conn)
            }
        }
    }

    fn cached(&self, conn: &Connection) -> Option<f64> {
        let month = crate::limits::month_key(crate::model::now());
        self.seen
            .lock()
            .expect("spend")
            .get(conn.auth.conn_id())
            .filter(|s| s.month == month)
            .map(|s| s.total)
    }
}

impl IssuerSpend {
    /// Takes the account's total from a connection listing (`cloud_status`).
    pub fn heard(&self, id: &str, total: f64) {
        let month = crate::limits::month_key(crate::model::now());
        let mut seen = self.seen.lock().expect("spend");
        let e = seen.entry(id.to_string()).or_insert(Seen {
            month: month.clone(),
            reported_cents: -1,
            at: Instant::now() - DEBOUNCE,
            total,
        });
        if e.month != month {
            e.month = month;
            e.reported_cents = -1;
        }
        e.total = total;
    }
}

/// Whether the issuer lists connection `id` (so a 404 on its spend route
/// means the route is missing, not the connection).
async fn conn_known_elsewhere(issuer: &dyn IssuerClient, id: &str) -> bool {
    issuer
        .connections()
        .await
        .is_ok_and(|v| v.iter().any(|c| c.id == id))
}
