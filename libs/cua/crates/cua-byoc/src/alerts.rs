// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Budget alerts: when a cloud's estimated spend this month passes 50, 80
//! and 100% of its `budget_usd_month`, Cua says so once per threshold per
//! month. Each alert goes to the cloud audit log (`budget_alert`) and to
//! the app's notifications feed through the sink the Spaces runtime
//! registers ([`cua_sandbox_core::byoc::set_budget_alert_sink`]).
//!
//! What already fired is kept in `<cua home>/cloud/alerts.json` (0600), so
//! a restart or a second process never says it twice. A new month, or a
//! different budget, starts over. When spend jumps past several thresholds
//! at once, only the highest is said.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cua_sandbox_core::byoc::{CloudBudgetAlert, post_budget_alert};
use serde::{Deserialize, Serialize};

use crate::model::Connection;

/// The shares of the budget that are said, in percent.
pub const THRESHOLDS: [u32; 3] = [50, 80, 100];

/// What fired for one cloud.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Fired {
    /// `YYYY-MM` (UTC).
    month: String,
    /// The budget they were measured against.
    budget_usd: f64,
    /// The thresholds said (or passed over by a higher one).
    percents: Vec<u32>,
}

fn path(home: &Path) -> PathBuf {
    home.join("cloud").join("alerts.json")
}

/// How people name the cloud `provider`.
pub fn title_of(provider: &str) -> &str {
    match provider {
        "aws" => "AWS",
        "gcp" => "Google Cloud",
        "azure" => "Microsoft Azure",
        "hetzner" => "Hetzner",
        "modal" => "Modal",
        other => other,
    }
}

/// The alerts `conn` is due now that `spent_usd` is spent this month (at
/// `now`), recorded as fired. Nothing without a budget.
pub fn due(
    home: &Path,
    conn: &Connection,
    spent_usd: f64,
    now: u64,
) -> std::io::Result<Vec<CloudBudgetAlert>> {
    let Some(budget) = conn.budget_usd_month.filter(|b| *b > 0.0 && b.is_finite()) else {
        return Ok(vec![]);
    };
    if !spent_usd.is_finite() {
        return Ok(vec![]);
    }
    let month = crate::limits::month_key(now);
    let file = path(home);
    let dir = file.parent().expect("alerts dir");
    std::fs::create_dir_all(dir)?;
    // One process at a time across the daemon and CLI runs.
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(file.with_extension("lock"))?;
    lock.lock()?;
    let mut all: BTreeMap<String, Fired> = match std::fs::read(&file) {
        Ok(b) => serde_json::from_slice(&b).unwrap_or_default(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
        Err(e) => return Err(e),
    };
    let fired = all.entry(conn.provider.clone()).or_default();
    if fired.month != month || (fired.budget_usd - budget).abs() > f64::EPSILON {
        *fired = Fired {
            month: month.clone(),
            budget_usd: budget,
            percents: vec![],
        };
    }
    let share = spent_usd / budget * 100.0;
    let passed: Vec<u32> = THRESHOLDS
        .iter()
        .copied()
        .filter(|p| share >= f64::from(*p) && !fired.percents.contains(p))
        .collect();
    let Some(&top) = passed.iter().max() else {
        return Ok(vec![]);
    };
    fired.percents.extend(passed);
    fired.percents.sort_unstable();
    let bytes = serde_json::to_vec_pretty(&all).expect("alerts serialize");
    let tmp = dir.join(format!(".alerts.{}.tmp", std::process::id()));
    {
        use std::io::Write as _;
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            o.mode(0o600);
        }
        let mut f = o.open(&tmp)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, &file)?;
    Ok(vec![CloudBudgetAlert {
        provider: conn.provider.clone(),
        title: title_of(&conn.provider).to_string(),
        percent: top,
        month,
        spent_usd: (spent_usd * 100.0).round() / 100.0,
        budget_usd: budget,
    }])
}

/// Checks every connected cloud of `store` with a budget and says what is
/// due: an audit entry and the notifications feed. Never fails the caller.
pub fn check_all(store: &crate::Store, now: u64) {
    let Some(home) = store.home() else { return };
    let Ok(conns) = store.connections() else {
        return;
    };
    for c in conns.iter().filter(|c| c.budget_usd_month.is_some()) {
        let spent = match store.spent_month(&c.provider, now) {
            Ok(s) => s,
            Err(e) => {
                tracing::debug!(cloud = %c.provider, "budget alerts: {e}");
                continue;
            }
        };
        match due(&home, c, spent, now) {
            Ok(alerts) => {
                for a in alerts {
                    let mut e = crate::audit::entry("budget_alert", &a.provider, "cua");
                    e.detail = format!(
                        "{}% of ${:.2} budget (about ${:.2} spent in {})",
                        a.percent, a.budget_usd, a.spent_usd, a.month
                    );
                    store.audit(e);
                    if !post_budget_alert(&home, &a) {
                        tracing::info!(cloud = %a.provider, "{}: {}", a.title_line(), a.body_line());
                    }
                }
            }
            Err(e) => tracing::warn!(cloud = %c.provider, "budget alerts not checked: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-10-24T12:00:00Z and 2026-11-02T12:00:00Z.
    const OCT: u64 = 1_792_843_200;
    const NOV: u64 = 1_793_620_800;

    fn conn(budget: Option<f64>) -> Connection {
        Connection {
            provider: "aws".into(),
            budget_usd_month: budget,
            ..Default::default()
        }
    }

    fn percents(v: Vec<CloudBudgetAlert>) -> Vec<u32> {
        v.into_iter().map(|a| a.percent).collect()
    }

    #[test]
    fn each_threshold_fires_once_a_month() {
        let dir = tempfile::tempdir().unwrap();
        let c = conn(Some(100.0));
        let d = |spent, now| percents(due(dir.path(), &c, spent, now).unwrap());
        assert!(d(10.0, OCT).is_empty());
        assert_eq!(d(50.0, OCT), vec![50]);
        assert!(d(60.0, OCT).is_empty(), "said once");
        assert_eq!(d(85.0, OCT), vec![80]);
        assert_eq!(d(101.0, OCT), vec![100]);
        assert!(d(150.0, OCT).is_empty());
        // A new month starts over.
        assert!(d(0.0, NOV).is_empty());
        assert_eq!(d(55.0, NOV), vec![50]);
    }

    #[test]
    fn a_jump_says_only_the_highest_and_a_restart_does_not_repeat() {
        let dir = tempfile::tempdir().unwrap();
        let c = conn(Some(20.0));
        assert_eq!(percents(due(dir.path(), &c, 19.0, OCT).unwrap()), vec![80]);
        // Another process (the same file) does not say 50 or 80 again.
        assert!(due(dir.path(), &c, 19.5, OCT).unwrap().is_empty());
        let a = due(dir.path(), &c, 20.0, OCT).unwrap();
        assert_eq!(a[0].title_line(), "AWS reached its monthly budget");
        assert!(a[0].body_line().contains("New machines there are refused"));
    }

    #[test]
    fn no_budget_no_alerts_and_a_new_budget_starts_over() {
        let dir = tempfile::tempdir().unwrap();
        assert!(due(dir.path(), &conn(None), 1e9, OCT).unwrap().is_empty());
        assert!(!path(dir.path()).exists());
        assert_eq!(
            percents(due(dir.path(), &conn(Some(100.0)), 90.0, OCT).unwrap()),
            vec![80]
        );
        // Raised to 200: 90 is 45%, nothing; 100 is 50%.
        assert!(
            due(dir.path(), &conn(Some(200.0)), 90.0, OCT)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            percents(due(dir.path(), &conn(Some(200.0)), 100.0, OCT).unwrap()),
            vec![50]
        );
    }
}
