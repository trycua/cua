// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `time`: the guest clock agrees with the caller's (evaluators stamp and
//! compare file times), VMs keep it synchronised, and the time zone is the
//! one the image claims.

use std::path::Path;
use std::time::{Duration, SystemTime};

use cua_spacesd_client::diagnose::{Check, Status};

use crate::sys;
use crate::{Ctx, Recorder};

/// Skew that warns.
pub const WARN_MS: i64 = 100;
/// Skew that fails.
pub const FAIL_MS: i64 = 500;

/// Guest minus host clock in ms.
pub fn skew_ms(guest: SystemTime, host: SystemTime) -> i64 {
    match guest.duration_since(host) {
        Ok(ahead) => ahead.as_millis() as i64,
        Err(behind) => -(behind.duration().as_millis() as i64),
    }
}

/// Verdict for a skew.
pub fn skew_status(ms: i64) -> Status {
    match ms.abs() {
        m if m > FAIL_MS => Status::Fail,
        m if m > WARN_MS => Status::Warn,
        _ => Status::Pass,
    }
}

/// Normalises a zone name ("Etc/UTC", "UTC", "Universal" are the same).
pub fn normalize_tz(tz: &str) -> String {
    let tz = tz.trim().trim_start_matches(':');
    let tz = tz.strip_prefix("Etc/").unwrap_or(tz);
    match tz {
        "UTC" | "Universal" | "Zulu" | "UCT" | "GMT" | "Greenwich" | "" => "UTC".into(),
        other => other.into(),
    }
}

/// The guest's zone: `TZ`, then `/etc/localtime`'s zoneinfo link, then
/// `/etc/timezone`, else UTC (glibc's default).
pub fn guest_tz() -> String {
    if let Ok(tz) = std::env::var("TZ") {
        if !tz.is_empty() {
            return tz;
        }
    }
    if let Ok(target) = std::fs::read_link("/etc/localtime") {
        let text = target.to_string_lossy();
        if let Some((_, zone)) = text.split_once("zoneinfo/") {
            return zone.to_owned();
        }
    }
    sys::read_capped(Path::new("/etc/timezone"))
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "UTC".into())
}

pub async fn run(ctx: &Ctx, rec: &mut Recorder<'_>) {
    if !rec.wants_group("time") {
        return;
    }
    match ctx.options.host_time {
        Some(host) => {
            // The caller stamped host_time when it sent the request; the
            // run started when it arrived, so compare those two instants
            // (checks before this one must not count as skew).
            let ms = skew_ms(ctx.started_wall, host);
            ctx.fidelity.lock().await.clock_skew_ms = ms;
            rec.push(
                Check::new(
                    "time.skew",
                    skew_status(ms),
                    format!("guest clock is {ms:+} ms from the caller's (warn > {WARN_MS}, fail > {FAIL_MS})"),
                )
                .fact("skew_ms", ms)
                .fix("sync the guest clock (Init with `now`, or NTP); VMs resumed from snapshots drift"),
                &["manifest:time"],
            )
            .await;
        }
        None => {
            rec.skip(
                "time.skew",
                &["manifest:time"],
                "not_applicable",
                "no caller clock in the request (run through `cua doctor` to compare)".into(),
            )
            .await;
        }
    }

    let vm = matches!(ctx.runtime.as_str(), "qemu" | "kubevirt" | "hyperv");
    if vm && ctx.init == "systemd" {
        let skew = ctx
            .options
            .host_time
            .map(|host| skew_ms(ctx.started_wall, host));
        rec.run("time.sync", &[], Duration::from_secs(45), async {
            // A VM that just booted may not have synced yet: give
            // systemd-timesyncd up to 30 s (bounded polls).
            let mut state = String::new();
            for _ in 0..15 {
                match sys::run_local("timedatectl", &["show", "-p", "NTPSynchronized", "--value"], &[], Duration::from_secs(10)).await {
                    Ok(out) => state = out.stdout.trim().to_owned(),
                    Err(error) => return Check::new("time.sync", Status::Warn, format!("timedatectl: {error}")),
                }
                if state == "yes" {
                    return Check::new("time.sync", Status::Pass, "NTP synchronized");
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            match skew {
                // Not synced, but the clock agrees with the caller's: fine.
                Some(ms) if ms.abs() <= WARN_MS => Check::new(
                    "time.sync",
                    Status::Pass,
                    format!("NTPSynchronized={state}, but the clock is within {ms:+} ms of the caller's"),
                ),
                _ => Check::new("time.sync", Status::Warn, format!("NTPSynchronized={state} after 30 s"))
                    .fix("systemd-timesyncd cannot reach an NTP server (no egress?); run through `cua doctor` to compare clocks"),
            }
        })
        .await;
    }

    let want = ctx.manifest.manifest.tz.clone();
    let tz = guest_tz();
    ctx.fidelity.lock().await.tz = tz.clone();
    if !want.is_empty() {
        rec.push(
            Check::new(
                "time.tz",
                super::verdict(normalize_tz(&tz) == normalize_tz(&want)),
                format!("time zone {tz} (image claims {want})"),
            ),
            &["manifest:tz"],
        )
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skew_thresholds() {
        let host = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        assert_eq!(skew_ms(host + Duration::from_millis(40), host), 40);
        assert_eq!(skew_ms(host - Duration::from_millis(40), host), -40);
        assert_eq!(skew_status(40), Status::Pass);
        assert_eq!(skew_status(-250), Status::Warn);
        assert_eq!(skew_status(900), Status::Fail);
    }

    #[test]
    fn zone_names_normalise() {
        assert_eq!(normalize_tz("Etc/UTC"), "UTC");
        assert_eq!(normalize_tz(":UTC"), "UTC");
        assert_eq!(normalize_tz("Universal"), "UTC");
        assert_eq!(normalize_tz("Europe/Paris"), "Europe/Paris");
    }
}
