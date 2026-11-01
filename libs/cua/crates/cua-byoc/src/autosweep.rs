// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The automatic sweep: what the daemon schedules so a forgotten Space
//! does not bill until someone remembers `cua cloud sweep`.
//!
//! One pass is `cua cloud sweep --delete` without `--all` over every
//! connected cloud: it deletes only Cua's own expired resources and
//! resources whose Space is gone (after the grace period a create under
//! way gets), never a live Space, and never anything untagged or another
//! owner's. A cloud that cannot be reached is reported and the others are
//! still swept. [`CloudManager::auto_sweep`] is the pass; this module is
//! its schedule and its log line.

use std::time::Duration;

use cua_sandbox_core::byoc::{CloudManager, CloudSweepReport};

/// `CUA_CLOUD_SWEEP_SECS`: seconds between passes (default 900; `0`
/// turns the automatic sweep off).
pub const ENV_SWEEP_SECS: &str = "CUA_CLOUD_SWEEP_SECS";

/// The default between passes.
pub const DEFAULT_SWEEP_SECS: u64 = 900;

/// How long after start the first pass runs.
pub const FIRST_RUN_DELAY: Duration = Duration::from_secs(60);

/// The time between passes for the value of [`ENV_SWEEP_SECS`]; `None`:
/// off. Unset, empty or not a number means the default.
pub fn interval(value: Option<&str>) -> Option<Duration> {
    let secs = value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_SWEEP_SECS);
    (secs > 0).then(|| Duration::from_secs(secs))
}

/// [`interval`] of the process environment.
pub fn interval_from_env() -> Option<Duration> {
    interval(std::env::var(ENV_SWEEP_SECS).ok().as_deref())
}

/// One automatic pass over `clouds`, and the one-line summary to log (none
/// when there was nothing to do).
pub async fn run_once(
    clouds: &dyn CloudManager,
) -> cua_sandbox_core::Result<(CloudSweepReport, Option<String>)> {
    let report = clouds.auto_sweep().await?;
    let line = summary(&report);
    Ok((report, line))
}

/// "cloud auto-sweep: deleted 2 (aws instance i-1: expired, ...); 1 failed (...)".
pub fn summary(report: &CloudSweepReport) -> Option<String> {
    let of = |action: &str| {
        report
            .resources
            .iter()
            .filter(move |i| i.action == action)
            .collect::<Vec<_>>()
    };
    let deleted = of("deleted");
    let forgotten = of("forgotten");
    let failed = of("failed");
    if deleted.is_empty() && forgotten.is_empty() && failed.is_empty() {
        return None;
    }
    let list = |rows: &[&cua_sandbox_core::byoc::CloudSweepItem]| {
        rows.iter()
            .take(5)
            .map(|i| {
                format!(
                    "{} {} {}: {}",
                    i.resource.provider, i.resource.resource_type, i.resource.id, i.reason
                )
            })
            .collect::<Vec<_>>()
            .join("; ")
    };
    let mut parts = Vec::new();
    if !deleted.is_empty() {
        parts.push(format!("deleted {} ({})", deleted.len(), list(&deleted)));
    }
    if !forgotten.is_empty() {
        parts.push(format!("forgot {} stale records", forgotten.len()));
    }
    if !failed.is_empty() {
        parts.push(format!("{} failed ({})", failed.len(), list(&failed)));
    }
    Some(format!("cloud auto-sweep: {}", parts.join(", ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_interval_defaults_and_zero_turns_it_off() {
        assert_eq!(interval(None), Some(Duration::from_secs(900)));
        assert_eq!(interval(Some("")), Some(Duration::from_secs(900)));
        assert_eq!(interval(Some("soon")), Some(Duration::from_secs(900)));
        assert_eq!(interval(Some(" 30 ")), Some(Duration::from_secs(30)));
        assert_eq!(interval(Some("0")), None);
    }

    #[test]
    fn nothing_to_do_says_nothing() {
        assert_eq!(summary(&CloudSweepReport::default()), None);
    }
}
