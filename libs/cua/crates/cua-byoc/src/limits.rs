// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A connection's own limits, enforced on this device before any cloud
//! call: the largest machine ([`crate::MaxSize`]) and the monthly budget
//! (`budget_usd_month`). They come on top of the elastic pool's budget
//! (`cua cloud budget`), which caps the pool across every cloud; these cap
//! one cloud whatever creates there (a Space, a sandbox, a pool host).
//!
//! The month's spend is an estimate from this home's records: list price
//! times runtime of every machine Cua created there this calendar month
//! (UTC), the ones still recorded counted to now (or their expiry), plus
//! what the ones already gone cost ([`crate::Store::spent_month`]). With an
//! OIDC connection the issuer enforces the account-wide budget as well.

use cua_sandbox_core::Error;

use crate::api::Result;
use crate::model::Connection;

/// Refuses a new machine on `conn` (`title`: how people name the cloud)
/// that is larger than its `max_size` (`vcpus`, `memory_mb`, when known),
/// or when `spent` this month plus one hour of `usd_per_hour` would pass
/// its budget.
pub fn check_new_machine(
    conn: &Connection,
    title: &str,
    vcpus: Option<u32>,
    memory_mb: Option<u64>,
    usd_per_hour: f64,
    spent: f64,
) -> Result<()> {
    if let Some(max) = conn.max_size {
        let too_big_cpu = vcpus.is_some_and(|c| c > max.vcpu);
        let too_big_mem = memory_mb.is_some_and(|m| m > u64::from(max.mem_gib) * 1024);
        if too_big_cpu || too_big_mem {
            return Err(Error::InvalidArgument(format!(
                "refused: {title} allows machines up to {} vCPU and {} GB, and this one needs \
                 {} vCPU and {} GB. Ask for a smaller machine, or raise the limit with \
                 `cua cloud connect {} --max-size <vcpu>x<gb>`.",
                max.vcpu,
                max.mem_gib,
                vcpus.map_or("?".into(), |c| c.to_string()),
                memory_mb.map_or("?".into(), |m| format!("{}", m.div_ceil(1024))),
                conn.provider,
            )));
        }
    }
    if let Some(budget) = conn.budget_usd_month {
        if spent >= budget {
            return Err(Error::Cloud(format!(
                "refused: {title}'s budget of ${budget:.2} this month is reached (about \
                 ${spent:.2} spent). New machines are refused until next month; stop and \
                 delete still work. Raise it with `cua cloud connect {} --budget <usd>`.",
                conn.provider
            )));
        }
        if spent + usd_per_hour > budget {
            return Err(Error::Cloud(format!(
                "refused: one hour of this machine (${usd_per_hour:.2}) would pass {title}'s \
                 budget of ${budget:.2} this month (about ${spent:.2} spent). Pick a cheaper \
                 machine, or raise it with `cua cloud connect {} --budget <usd>`.",
                conn.provider
            )));
        }
    }
    Ok(())
}

/// Unix seconds of the first instant of the UTC month `t` falls in.
pub fn month_start(t: u64) -> u64 {
    let days = (t / 86_400) as i64;
    let (y, m, _) = civil_from_days(days);
    (days_from_civil(y, m, 1) as u64) * 86_400
}

/// `YYYY-MM` of `t` (UTC).
pub fn month_key(t: u64) -> String {
    let (y, m, _) = civil_from_days((t / 86_400) as i64);
    format!("{y:04}-{m:02}")
}

// Howard Hinnant's civil calendar conversions.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub(crate) fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = i64::from(if m > 2 { m - 3 } else { m + 9 });
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// What `r` cost this month up to `now`: its hourly price times how long
/// it ran since the month started (to its expiry, when that came first).
pub fn cost_this_month(r: &crate::Resource, now: u64) -> f64 {
    if !matches!(r.resource_type.as_str(), "instance" | "sandbox") {
        return 0.0;
    }
    let usd: f64 = r
        .extra
        .get("usd_per_hour")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.0);
    let from = r.created.max(month_start(now));
    let to = if r.expires > 0 {
        now.min(r.expires)
    } else {
        now
    };
    usd * to.saturating_sub(from) as f64 / 3600.0
}

/// What `r` cost since it was created, up to `now` (or its expiry, when
/// that came first): its hourly price times how long it ran. Rounded to
/// the cent.
pub fn cost_so_far(r: &crate::Resource, now: u64) -> f64 {
    if !matches!(r.resource_type.as_str(), "instance" | "sandbox") || r.created == 0 {
        return 0.0;
    }
    let usd: f64 = r
        .extra
        .get("usd_per_hour")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.0);
    let to = if r.expires > 0 {
        now.min(r.expires)
    } else {
        now
    };
    (usd * to.saturating_sub(r.created) as f64 / 3600.0 * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::MaxSize;

    fn conn() -> Connection {
        Connection {
            provider: "aws".into(),
            budget_usd_month: Some(10.0),
            max_size: Some(MaxSize {
                vcpu: 4,
                mem_gib: 16,
            }),
            ..Default::default()
        }
    }

    #[test]
    fn a_machine_costs_its_price_times_its_runtime_so_far() {
        let mut r = crate::Resource {
            provider: "aws".into(),
            resource_type: "instance".into(),
            created: 1_000_000,
            ..Default::default()
        };
        r.extra.insert("usd_per_hour".into(), "0.5".into());
        assert_eq!(cost_so_far(&r, 1_000_000 + 2 * 3600), 1.0);
        r.expires = 1_000_000 + 3600;
        assert_eq!(cost_so_far(&r, 1_000_000 + 2 * 3600), 0.5);
        r.resource_type = "security_group".into();
        assert_eq!(cost_so_far(&r, 1_000_000 + 2 * 3600), 0.0);
    }

    #[test]
    fn a_machine_over_the_max_size_is_refused() {
        let c = conn();
        assert!(check_new_machine(&c, "AWS", Some(4), Some(16 * 1024), 0.1, 0.0).is_ok());
        let e = check_new_machine(&c, "AWS", Some(8), Some(16 * 1024), 0.1, 0.0).unwrap_err();
        assert!(matches!(e, Error::InvalidArgument(_)), "{e}");
        assert!(e.to_string().contains("up to 4 vCPU and 16 GB"), "{e}");
        let e = check_new_machine(&c, "AWS", Some(2), Some(32 * 1024), 0.1, 0.0).unwrap_err();
        assert!(e.to_string().contains("32 GB"), "{e}");
        // Unknown size: not refused for size.
        assert!(check_new_machine(&c, "AWS", None, None, 0.1, 0.0).is_ok());
    }

    #[test]
    fn a_reached_budget_refuses_new_machines() {
        let c = conn();
        let e = check_new_machine(&c, "AWS", Some(2), Some(4096), 0.1, 10.0).unwrap_err();
        assert!(matches!(e, Error::Cloud(_)), "{e}");
        assert!(
            e.to_string()
                .contains("budget of $10.00 this month is reached"),
            "{e}"
        );
        let e = check_new_machine(&c, "AWS", Some(2), Some(4096), 1.5, 9.0).unwrap_err();
        assert!(e.to_string().contains("one hour of this machine"), "{e}");
        assert!(check_new_machine(&c, "AWS", Some(2), Some(4096), 0.5, 9.0).is_ok());
        // No limits: nothing refused.
        let open = Connection::default();
        assert!(check_new_machine(&open, "AWS", Some(192), Some(1 << 21), 55.0, 1e6).is_ok());
    }

    #[test]
    fn months_start_on_the_first_utc() {
        // 2026-10-24T12:00:00Z
        let t = 1_792_843_200;
        assert_eq!(month_key(t), "2026-10");
        assert_eq!(month_start(t), 1_790_812_800); // 2026-10-01T00:00:00Z
        assert_eq!(month_key(1_790_812_800 - 1), "2026-09");
        assert_eq!(month_start(951_782_400), 949_363_200); // 2000-02-29 -> 2000-02-01
    }

    #[test]
    fn a_machine_costs_its_runtime_this_month() {
        let start = month_start(1_792_843_200);
        let mut r = crate::Resource {
            resource_type: "instance".into(),
            created: start - 3600,
            ..Default::default()
        };
        r.extra.insert("usd_per_hour".into(), "2.0".into());
        assert!((cost_this_month(&r, start + 7200) - 4.0).abs() < 1e-9);
        r.expires = start + 3600;
        assert!((cost_this_month(&r, start + 7200) - 2.0).abs() < 1e-9);
        r.resource_type = "security_group".into();
        assert_eq!(cost_this_month(&r, start + 7200), 0.0);
    }
}
