// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! What the machines Cua created cost (`cloud_cost`, `cua cloud cost`).
//!
//! `state.json` holds only the machines that still exist, so when one is
//! forgotten its run is kept here, in `<cua home>/cloud/usage.jsonl` (0600),
//! one JSON object per line:
//!
//! ```json
//! {"provider":"aws","id":"i-0abc","kind":"instance","space":"research",
//!  "started_by":"user","start":1792000000,"end":1792007200,"usd_per_hour":0.04}
//! ```
//!
//! A report adds those runs to the machines still recorded, each one's list
//! price times how long it ran inside the window, and groups them by Space,
//! agent, UTC day or cloud. It is an estimate: the cloud's own bill is the
//! final word. At more than [`ROTATE_BYTES`] the file moves to
//! `usage.jsonl.1` (replacing the previous one).

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use cua_sandbox_core::byoc::{CloudCostReport, CloudCostRow, cost_by};
use serde::{Deserialize, Serialize};

use crate::model::{self, Resource};

/// The log rotates when it is larger than this.
pub const ROTATE_BYTES: u64 = 5 * 1024 * 1024;

/// The `key` of a machine that belongs to no Space: a pool host.
pub const POOL_HOSTS: &str = "pool hosts";
/// The `key` of a machine with no Space and no pool role.
pub const NO_SPACE: &str = "no Space";
/// The `key` of a machine recorded before who started it was kept.
pub const UNKNOWN_AGENT: &str = "unknown";

/// One machine's run.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub provider: String,
    /// The cloud's id.
    pub id: String,
    /// `instance` or `sandbox`.
    pub kind: String,
    /// The Space, [`POOL_HOSTS`] or [`NO_SPACE`].
    #[serde(default)]
    pub space: String,
    /// An agent's name, `user`, or empty (recorded before it was kept).
    #[serde(default)]
    pub started_by: String,
    /// Unix seconds.
    pub start: u64,
    /// Unix seconds.
    pub end: u64,
    /// List price.
    pub usd_per_hour: f64,
}

impl Usage {
    /// The run of `r` up to `now` (or its expiry, when that came first);
    /// `None` for what is not a machine, has no price, or never started.
    pub fn of(r: &Resource, now: u64) -> Option<Usage> {
        if !matches!(r.resource_type.as_str(), "instance" | "sandbox")
            || r.created == 0
            || r.pending()
        {
            return None;
        }
        let usd_per_hour: f64 = r
            .extra
            .get("usd_per_hour")
            .and_then(|v| v.parse().ok())
            .filter(|v: &f64| v.is_finite() && *v > 0.0)?;
        let end = if r.expires > 0 {
            now.min(r.expires)
        } else {
            now
        };
        Some(Usage {
            provider: r.provider.clone(),
            id: r.id.clone(),
            kind: r.resource_type.clone(),
            space: space_of(r),
            started_by: r.extra.get(model::STARTED_BY).cloned().unwrap_or_default(),
            start: r.created,
            end: end.max(r.created),
            usd_per_hour,
        })
    }
}

/// The Space `r` belongs to, as people name it.
fn space_of(r: &Resource) -> String {
    let s = match r.sandbox.split_once(':') {
        Some((_, s)) => s.to_string(),
        None => r
            .tags
            .get(model::tag::SPACE)
            .cloned()
            .unwrap_or_else(|| r.sandbox.clone()),
    };
    if !s.is_empty() {
        s
    } else if crate::hosts::is_host_record(r) {
        POOL_HOSTS.into()
    } else {
        NO_SPACE.into()
    }
}

/// The usage log of one cua home.
pub struct UsageLog {
    path: PathBuf,
    rotate_at: u64,
    lock: Mutex<()>,
}

impl UsageLog {
    /// The log under `home` (`<home>/cloud/usage.jsonl`).
    pub fn new(home: &Path) -> Self {
        UsageLog {
            path: home.join("cloud").join("usage.jsonl"),
            rotate_at: ROTATE_BYTES,
            lock: Mutex::new(()),
        }
    }

    /// Rotates at `bytes` instead of [`ROTATE_BYTES`] (tests).
    pub fn rotating_at(mut self, bytes: u64) -> Self {
        self.rotate_at = bytes;
        self
    }

    /// Where it lives.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn rotated(&self) -> PathBuf {
        let mut p = self.path.clone().into_os_string();
        p.push(".1");
        PathBuf::from(p)
    }

    /// Appends `u`.
    pub fn append(&self, u: &Usage) -> std::io::Result<()> {
        let mut line = serde_json::to_vec(u).expect("a usage line serializes");
        line.push(b'\n');
        let _g = self.lock.lock().expect("cloud usage lock");
        std::fs::create_dir_all(self.path.parent().expect("usage dir"))?;
        if std::fs::metadata(&self.path).is_ok_and(|m| m.len() > self.rotate_at) {
            std::fs::rename(&self.path, self.rotated())?;
        }
        let mut o = std::fs::OpenOptions::new();
        o.append(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            o.mode(0o600);
        }
        // One write per line keeps lines whole across processes.
        o.open(&self.path)?.write_all(&line)
    }

    /// Every run kept (the rotated file first). Lines that do not parse
    /// are skipped.
    pub fn read(&self) -> std::io::Result<Vec<Usage>> {
        let _g = self.lock.lock().expect("cloud usage lock");
        let mut all = Vec::new();
        for p in [self.rotated(), self.path.clone()] {
            match std::fs::read_to_string(&p) {
                Ok(t) => all.extend(t.lines().filter_map(|l| serde_json::from_str(l).ok())),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        Ok(all)
    }
}

/// Unix seconds of `YYYY-MM-DD` at 00:00 UTC.
pub fn parse_day(s: &str) -> Option<u64> {
    let mut it = s.trim().splitn(3, '-');
    let y: i64 = it.next()?.parse().ok()?;
    let m: u32 = it.next()?.parse().ok()?;
    let d: u32 = it.next()?.parse().ok()?;
    if !(1970..=9999).contains(&y) || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let days = crate::limits::days_from_civil(y, m, d);
    // Reject days a month does not have (2026-02-30).
    let t = (days as u64) * 86_400;
    (day_key(t) == format!("{y:04}-{m:02}-{d:02}")).then_some(t)
}

/// `YYYY-MM-DD` of `t` (UTC).
pub fn day_key(t: u64) -> String {
    model::rfc3339(t)[..10].to_string()
}

/// One row while it adds up: dollars, seconds and the machines (provider,
/// id) seen.
type Group = (f64, u64, BTreeSet<(String, String)>);

/// The report of `runs` (closed and open alike) inside `[since, now]`,
/// for `provider` when given, grouped `by`.
pub fn report(
    runs: &[Usage],
    provider: Option<&str>,
    by: &str,
    since: u64,
    now: u64,
) -> CloudCostReport {
    // key -> (usd, seconds, machines)
    let mut acc: BTreeMap<String, Group> = BTreeMap::new();
    let mut add = |key: String, u: &Usage, from: u64, to: u64| {
        let e = acc.entry(key).or_default();
        let secs = to - from;
        e.0 += u.usd_per_hour * secs as f64 / 3600.0;
        e.1 += secs;
        e.2.insert((u.provider.clone(), u.id.clone()));
    };
    for u in runs
        .iter()
        .filter(|u| provider.is_none_or(|p| u.provider.eq_ignore_ascii_case(p)))
    {
        let from = u.start.max(since);
        let to = u.end.min(now);
        if to <= from {
            continue;
        }
        match by {
            cost_by::DAY => {
                let mut t = from;
                while t < to {
                    let next = ((t / 86_400) + 1) * 86_400;
                    let end = next.min(to);
                    add(day_key(t), u, t, end);
                    t = end;
                }
            }
            cost_by::AGENT => {
                let who = if u.started_by.is_empty() {
                    UNKNOWN_AGENT.to_string()
                } else {
                    u.started_by.clone()
                };
                add(who, u, from, to);
            }
            cost_by::CLOUD => add(u.provider.clone(), u, from, to),
            _ => add(u.space.clone(), u, from, to),
        }
    }
    let total: f64 = acc.values().map(|v| v.0).sum();
    let mut rows: Vec<CloudCostRow> = acc
        .into_iter()
        .map(|(key, (usd, secs, m))| CloudCostRow {
            key,
            usd: cents(usd),
            hours: (secs as f64 / 3600.0 * 100.0).round() / 100.0,
            machines: m.len() as u32,
        })
        .collect();
    if by != cost_by::DAY {
        rows.sort_by(|a, b| {
            b.usd
                .partial_cmp(&a.usd)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.key.cmp(&b.key))
        });
    }
    CloudCostReport {
        by: by.to_string(),
        since: day_key(since),
        until: model::rfc3339(now),
        rows,
        total_usd: cents(total),
    }
}

fn cents(usd: f64) -> f64 {
    (usd * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(id: &str, space: &str, who: &str, start: u64, end: u64, usd: f64) -> Usage {
        Usage {
            provider: "aws".into(),
            id: id.into(),
            kind: "instance".into(),
            space: space.into(),
            started_by: who.into(),
            start,
            end,
            usd_per_hour: usd,
        }
    }

    // 2026-10-01T00:00:00Z
    const OCT1: u64 = 1_790_812_800;

    #[test]
    fn days_parse_and_print_in_utc() {
        assert_eq!(parse_day("2026-10-01"), Some(OCT1));
        assert_eq!(day_key(OCT1 + 86_399), "2026-10-01");
        assert_eq!(parse_day("2026-02-30"), None);
        assert_eq!(parse_day("2026-13-01"), None);
        assert_eq!(parse_day("yesterday"), None);
    }

    #[test]
    fn a_run_is_split_at_midnight_and_clipped_to_the_window() {
        // 22:00 on the 1st to 02:00 on the 2nd at $1/h.
        let r = run("i-1", "a", "user", OCT1 + 22 * 3600, OCT1 + 26 * 3600, 1.0);
        let rep = report(
            std::slice::from_ref(&r),
            None,
            cost_by::DAY,
            OCT1,
            OCT1 + 30 * 3600,
        );
        assert_eq!(rep.rows.len(), 2);
        assert_eq!(rep.rows[0].key, "2026-10-01");
        assert_eq!(rep.rows[0].usd, 2.0);
        assert_eq!(rep.rows[1].key, "2026-10-02");
        assert_eq!(rep.rows[1].hours, 2.0);
        assert_eq!(rep.total_usd, 4.0);
        // A window that starts on the 2nd counts only that day.
        let rep = report(&[r], None, cost_by::DAY, OCT1 + 86_400, OCT1 + 30 * 3600);
        assert_eq!(rep.rows.len(), 1);
        assert_eq!(rep.total_usd, 2.0);
        assert_eq!(rep.since, "2026-10-02");
    }

    #[test]
    fn groups_by_space_agent_and_cloud_costliest_first() {
        let runs = vec![
            run("i-1", "research", "user", OCT1, OCT1 + 3600, 1.0),
            run("i-2", "research", "planner", OCT1, OCT1 + 3600, 1.0),
            run("i-3", "build", "planner", OCT1, OCT1 + 7200, 2.0),
            run("i-4", "old", "", OCT1, OCT1 + 3600, 0.5),
        ];
        let now = OCT1 + 10 * 3600;
        let by_space = report(&runs, None, cost_by::SPACE, OCT1, now);
        assert_eq!(
            by_space
                .rows
                .iter()
                .map(|r| (r.key.as_str(), r.usd, r.machines))
                .collect::<Vec<_>>(),
            vec![("build", 4.0, 1), ("research", 2.0, 2), ("old", 0.5, 1)]
        );
        let by_agent = report(&runs, None, cost_by::AGENT, OCT1, now);
        assert_eq!(by_agent.rows[0].key, "planner");
        assert_eq!(by_agent.rows[0].usd, 5.0);
        assert!(by_agent.rows.iter().any(|r| r.key == UNKNOWN_AGENT));
        let by_cloud = report(&runs, None, cost_by::CLOUD, OCT1, now);
        assert_eq!(by_cloud.rows.len(), 1);
        assert_eq!(by_cloud.rows[0].machines, 4);
        assert_eq!(by_cloud.total_usd, 6.5);
        assert!(
            report(&runs, Some("gcp"), cost_by::CLOUD, OCT1, now)
                .rows
                .is_empty()
        );
    }

    #[test]
    fn the_log_keeps_runs_across_rotation() {
        let dir = tempfile::tempdir().unwrap();
        let log = UsageLog::new(dir.path()).rotating_at(10);
        let a = run("i-1", "a", "user", 1, 2, 1.0);
        let b = run("i-2", "b", "user", 3, 4, 1.0);
        log.append(&a).unwrap();
        log.append(&b).unwrap();
        assert_eq!(log.read().unwrap(), vec![a, b]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(log.path()).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn only_priced_started_machines_count() {
        let mut r = Resource {
            provider: "aws".into(),
            id: "i-1".into(),
            resource_type: "instance".into(),
            sandbox: "aws:research".into(),
            created: 100,
            expires: 200,
            ..Default::default()
        };
        assert!(Usage::of(&r, 300).is_none(), "no price");
        r.extra.insert("usd_per_hour".into(), "0.5".into());
        let u = Usage::of(&r, 300).unwrap();
        assert_eq!((u.space.as_str(), u.end), ("research", 200));
        r.resource_type = "security_group".into();
        assert!(Usage::of(&r, 300).is_none());
    }
}
