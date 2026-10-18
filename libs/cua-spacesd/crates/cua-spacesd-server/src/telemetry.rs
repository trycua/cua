// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Aggregate, opt-in health telemetry from inside the sandbox.
//!
//! Off unless the host enabled it for this sandbox: `CUA_SPACESD_TELEMETRY=1`
//! in cua-spacesd's environment (the cua SDK sets it on local sandboxes it
//! creates while telemetry is on on the host). `DO_NOT_TRACK=1` or
//! `CUA_TELEMETRY=0` in the sandbox still turn it off.
//!
//! What is counted, and nothing else: session deliveries that arrive with
//! and without a Keyvault broker grant (`delivery_grant: present|absent`),
//! failed imports, and uptime. Hourly, as bucketed counts. Never app names,
//! import ids, file names, paths, sites or anything imported. The receive
//! side is where these counts are reliable: a modified client can strip
//! its own telemetry, but not what the guest sees arrive.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use cua_telemetry::events;

/// The per-sandbox switch.
pub const ENV_ENABLE: &str = "CUA_SPACESD_TELEMETRY";
/// Report interval.
pub const INTERVAL: Duration = Duration::from_secs(3600);

/// The counters (process-wide).
#[derive(Debug, Default)]
pub struct Counters {
    grant_present: AtomicU64,
    grant_absent: AtomicU64,
    imports_failed: AtomicU64,
}

/// One window's counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub grant_present: u64,
    pub grant_absent: u64,
    pub imports_failed: u64,
}

impl Counters {
    /// A session delivery arrived (its final chunk), with or without a
    /// broker grant.
    pub fn delivery(&self, broker_grant: &str) {
        if broker_grant.trim().is_empty() {
            self.grant_absent.fetch_add(1, Ordering::Relaxed);
        } else {
            self.grant_present.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// An import failed.
    pub fn import_failed(&self) {
        self.imports_failed.fetch_add(1, Ordering::Relaxed);
    }

    /// Reads the counts without resetting them.
    pub fn peek(&self) -> Snapshot {
        Snapshot {
            grant_present: self.grant_present.load(Ordering::Relaxed),
            grant_absent: self.grant_absent.load(Ordering::Relaxed),
            imports_failed: self.imports_failed.load(Ordering::Relaxed),
        }
    }

    /// Reads and resets the counts.
    pub fn take(&self) -> Snapshot {
        Snapshot {
            grant_present: self.grant_present.swap(0, Ordering::Relaxed),
            grant_absent: self.grant_absent.swap(0, Ordering::Relaxed),
            imports_failed: self.imports_failed.swap(0, Ordering::Relaxed),
        }
    }
}

/// The process-wide counters.
pub fn counters() -> &'static Counters {
    static C: std::sync::OnceLock<Counters> = std::sync::OnceLock::new();
    C.get_or_init(Counters::default)
}

/// Whether the host enabled telemetry for this sandbox (and nothing in the
/// sandbox turned it off).
pub fn enabled(env: &dyn Fn(&str) -> Option<String>) -> bool {
    let on = env(ENV_ENABLE)
        .as_deref()
        .and_then(cua_telemetry::config::parse_bool)
        .unwrap_or(false);
    // The shared switches still apply (DO_NOT_TRACK, CUA_TELEMETRY=0); CI
    // does not matter here: the host decided.
    let blocked = env(cua_telemetry::config::ENV_DO_NOT_TRACK)
        .is_some_and(|v| !v.trim().is_empty() && v.trim() != "0")
        || env(cua_telemetry::config::ENV_TELEMETRY)
            .as_deref()
            .and_then(cua_telemetry::config::parse_bool)
            == Some(false);
    on && !blocked
}

/// The events for one window (empty counts are not sent).
pub fn window_events(s: Snapshot, uptime: Duration) -> Vec<events::Event> {
    let mut out = vec![events::spacesd_health(uptime, s.imports_failed)];
    if s.grant_present > 0 {
        out.push(events::spacesd_session_deliveries(true, s.grant_present));
    }
    if s.grant_absent > 0 {
        out.push(events::spacesd_session_deliveries(false, s.grant_absent));
    }
    out
}

/// A client for this sandbox: its own random id under `data_dir`, product
/// `spacesd`. The host's notice covers it (the host opted this sandbox in).
pub fn client(data_dir: &Path) -> cua_telemetry::Telemetry {
    let home = data_dir.join("telemetry-home");
    let t = cua_telemetry::Telemetry::builder()
        .env(|k| {
            // The host decided for this sandbox; the sandbox's own CI
            // markers do not apply. Everything else reads the environment.
            if cua_telemetry::config::CI_VARIABLES.contains(&k) {
                None
            } else {
                std::env::var(k).ok()
            }
        })
        .home(&home)
        .product("spacesd", env!("CARGO_PKG_VERSION"))
        .build();
    t.acknowledge_notice();
    t
}

/// Starts the hourly reporter when enabled. Returns whether it started.
pub fn spawn_reporter(data_dir: &Path) -> bool {
    if !enabled(&|k| std::env::var(k).ok()) {
        return false;
    }
    let t = client(data_dir);
    let started = Instant::now();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(INTERVAL);
        tick.tick().await;
        loop {
            tick.tick().await;
            for e in window_events(counters().take(), started.elapsed()) {
                t.capture(e);
            }
            t.flush(Duration::from_secs(3));
        }
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| m.get(k).cloned()
    }

    #[test]
    fn off_unless_the_host_enabled_it_and_not_overridden() {
        assert!(!enabled(&env(&[])));
        assert!(enabled(&env(&[(ENV_ENABLE, "1")])));
        assert!(!enabled(&env(&[(ENV_ENABLE, "1"), ("DO_NOT_TRACK", "1")])));
        assert!(!enabled(&env(&[(ENV_ENABLE, "1"), ("CUA_TELEMETRY", "0")])));
        assert!(!enabled(&env(&[(ENV_ENABLE, "maybe")])));
    }

    #[test]
    fn absent_and_present_grants_are_counted_separately_and_reset() {
        let c = Counters::default();
        c.delivery("");
        c.delivery("   ");
        c.delivery("keyvault");
        c.import_failed();
        assert_eq!(
            c.take(),
            Snapshot {
                grant_present: 1,
                grant_absent: 2,
                imports_failed: 1
            }
        );
        assert_eq!(c.take(), Snapshot::default());
    }

    #[test]
    fn window_events_are_counts_only_and_validate() {
        let h = tempfile::tempdir().unwrap();
        let sink = std::sync::Arc::new(cua_telemetry::sink::MemorySink::new());
        let t = cua_telemetry::Telemetry::builder()
            .env(|_| None)
            .home(h.path())
            .sink(sink.clone())
            .product("spacesd", "0.1.0")
            .foreground()
            .build();
        t.acknowledge_notice();
        let evs = window_events(
            Snapshot {
                grant_present: 3,
                grant_absent: 2,
                imports_failed: 0,
            },
            Duration::from_secs(7200),
        );
        for e in evs {
            assert_eq!(t.capture(e), cua_telemetry::Captured::Queued);
        }
        t.flush(Duration::from_secs(1));
        let got = sink.events();
        let grants: Vec<(String, u64)> = got
            .iter()
            .filter(|e| e["event"] == "cua_spacesd_session_deliveries")
            .map(|e| {
                (
                    e["properties"]["delivery_grant"]
                        .as_str()
                        .unwrap()
                        .to_string(),
                    e["properties"]["count"].as_u64().unwrap(),
                )
            })
            .collect();
        assert_eq!(grants, [("present".into(), 3), ("absent".into(), 2)]);
        assert!(window_events(Snapshot::default(), Duration::ZERO).len() == 1);
    }
}
