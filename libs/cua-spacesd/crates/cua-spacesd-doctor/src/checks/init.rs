// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `init`: the image's services run under the init system its variant
//! boots with (supervisord in containers, systemd in VMs, launchd on macOS,
//! the SCM on Windows), none failed, and cua-spacesd restarts on exit.

use std::collections::BTreeMap;
use std::time::Duration;

use cua_spacesd_client::diagnose::{Check, Status};

use crate::sys;
use crate::{Ctx, Recorder};

/// Parses `supervisorctl status` into name -> state.
pub fn parse_supervisorctl(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some((fields.next()?.to_owned(), fields.next()?.to_owned()))
        })
        .collect()
}

/// Units whose process is missing, by command-line substring.
fn missing_by_process(units: &[String], patterns: &BTreeMap<String, String>) -> Vec<String> {
    let cmdlines: Vec<String> = sys::pids()
        .into_iter()
        .filter_map(sys::proc_cmdline)
        .collect();
    units
        .iter()
        .filter(|u| {
            let pattern = patterns.get(*u).cloned().unwrap_or_else(|| (*u).clone());
            !cmdlines.iter().any(|c| c.contains(&pattern))
        })
        .cloned()
        .collect()
}

pub async fn run(ctx: &Ctx, rec: &mut Recorder<'_>) {
    if !rec.wants_group("init") {
        return;
    }
    let manifest = &ctx.manifest.manifest;
    if !manifest.init.is_empty() {
        rec.push(
            Check::new(
                "init.system",
                super::verdict(manifest.init == ctx.init),
                match sys::platform_init() {
                    Some(pid1) => format!(
                        "the init is {} under {pid1}, the platform's PID 1 (the {} variant boots \
                         with {})",
                        ctx.init, manifest.variant, manifest.init
                    ),
                    None => format!(
                        "PID 1 is {} (the {} variant boots with {})",
                        ctx.init, manifest.variant, manifest.init
                    ),
                },
            )
            .fix("the image variant and its init system disagree; check the build"),
            &["manifest:init"],
        )
        .await;
    }
    let units = manifest.units.clone();
    if units.is_empty() {
        return;
    }
    let claims: &[&str] = &["manifest:units"];
    match ctx.init.as_str() {
        "supervisord" => {
            rec.run("init.units", claims, Duration::from_secs(20), async {
                let status =
                    sys::run_local("supervisorctl", &["status"], &[], Duration::from_secs(10))
                        .await;
                match status {
                    // supervisorctl exits 3 when a program is not RUNNING;
                    // parse either way.
                    Ok(out) if out.stdout.contains("RUNNING") || out.code == Some(3) => {
                        let states = parse_supervisorctl(&out.stdout);
                        let bad: Vec<String> = units
                            .iter()
                            .filter(|u| states.get(*u).map(String::as_str) != Some("RUNNING"))
                            .map(|u| {
                                format!(
                                    "{u}={}",
                                    states.get(u).cloned().unwrap_or_else(|| "absent".into())
                                )
                            })
                            .collect();
                        Check::new(
                            "init.units",
                            super::verdict(bad.is_empty()),
                            if bad.is_empty() {
                                format!("supervisord programs RUNNING: {}", units.join(", "))
                            } else {
                                format!("not RUNNING: {}", bad.join(", "))
                            },
                        )
                        .fact("method", "supervisorctl")
                    }
                    _ => {
                        // Unprivileged caller: the supervisor socket is root
                        // 0700. Fall back to the unit's process.
                        let missing = missing_by_process(&units, &manifest.unit_processes);
                        Check::new(
                            "init.units",
                            super::verdict(missing.is_empty()),
                            if missing.is_empty() {
                                format!("every program's process is running: {}", units.join(", "))
                            } else {
                                format!("no process for: {}", missing.join(", "))
                            },
                        )
                        .fact("method", "processes")
                    }
                }
            })
            .await;
            rec.run(
                "init.restart_policy",
                claims,
                Duration::from_secs(5),
                async {
                    let conf =
                        sys::read_capped(std::path::Path::new("/etc/supervisor/supervisord.conf"))
                            .unwrap_or_default();
                    let section = conf
                        .split("[program:cua-spacesd]")
                        .nth(1)
                        .map(|rest| rest.split("\n[").next().unwrap_or_default().to_owned())
                        .unwrap_or_default();
                    let ok = section
                        .lines()
                        .any(|l| l.trim().replace(' ', "") == "autorestart=true");
                    Check::new(
                        "init.restart_policy",
                        super::verdict(ok),
                        if ok {
                            "cua-spacesd autorestart=true"
                        } else {
                            "[program:cua-spacesd] lacks autorestart=true"
                        },
                    )
                },
            )
            .await;
        }
        "systemd" => {
            rec.run("init.units", claims, Duration::from_secs(30), async {
                let mut args = vec!["is-active"];
                args.extend(units.iter().map(String::as_str));
                let out =
                    match sys::run_local("systemctl", &args, &[], Duration::from_secs(20)).await {
                        Ok(out) => out,
                        Err(error) => {
                            return Check::new(
                                "init.units",
                                Status::Fail,
                                format!("systemctl: {error}"),
                            )
                        }
                    };
                let states: Vec<&str> = out.stdout.lines().collect();
                let bad: Vec<String> = units
                    .iter()
                    .zip(states.iter().chain(std::iter::repeat(&"unknown")))
                    .filter(|(_, s)| **s != "active")
                    .map(|(u, s)| format!("{u}={s}"))
                    .collect();
                Check::new(
                    "init.units",
                    super::verdict(bad.is_empty()),
                    if bad.is_empty() {
                        format!("active: {}", units.join(", "))
                    } else {
                        format!("not active: {}", bad.join(", "))
                    },
                )
                .fact("method", "systemctl")
            })
            .await;
            rec.run(
                "init.failed_units",
                claims,
                Duration::from_secs(20),
                async {
                    match sys::run_local(
                        "systemctl",
                        &["list-units", "--failed", "--plain", "--no-legend", "cua-*"],
                        &[],
                        Duration::from_secs(15),
                    )
                    .await
                    {
                        Ok(out) => {
                            let failed: Vec<&str> = out
                                .stdout
                                .lines()
                                .filter_map(|l| l.split_whitespace().next())
                                .collect();
                            Check::new(
                                "init.failed_units",
                                super::verdict(failed.is_empty()),
                                if failed.is_empty() {
                                    "no failed cua-* units".to_owned()
                                } else {
                                    format!("failed: {}", failed.join(", "))
                                },
                            )
                            .fix("journalctl -u <unit> in the guest")
                        }
                        Err(error) => Check::new(
                            "init.failed_units",
                            Status::Fail,
                            format!("systemctl: {error}"),
                        ),
                    }
                },
            )
            .await;
            rec.run(
                "init.restart_policy",
                claims,
                Duration::from_secs(20),
                async {
                    let enabled = sys::run_local(
                        "systemctl",
                        &["is-enabled", "cua-spacesd.service"],
                        &[],
                        Duration::from_secs(10),
                    )
                    .await;
                    let restart = sys::run_local(
                        "systemctl",
                        &["show", "-p", "Restart", "--value", "cua-spacesd.service"],
                        &[],
                        Duration::from_secs(10),
                    )
                    .await;
                    let enabled = enabled
                        .map(|o| o.stdout.trim().to_owned())
                        .unwrap_or_default();
                    let restart = restart
                        .map(|o| o.stdout.trim().to_owned())
                        .unwrap_or_default();
                    Check::new(
                        "init.restart_policy",
                        super::verdict(
                            enabled == "enabled"
                                && matches!(restart.as_str(), "always" | "on-failure"),
                        ),
                        format!("cua-spacesd.service is-enabled={enabled}, Restart={restart}"),
                    )
                },
            )
            .await;
        }
        "launchd" => {
            rec.run("init.units", claims, Duration::from_secs(20), async {
                let mut missing = Vec::new();
                for unit in &units {
                    match sys::run_local("launchctl", &["list", unit], &[], Duration::from_secs(5))
                        .await
                    {
                        Ok(out) if out.ok() => {}
                        _ => missing.push(unit.clone()),
                    }
                }
                Check::new(
                    "init.units",
                    super::verdict(missing.is_empty()),
                    if missing.is_empty() {
                        format!("loaded: {}", units.join(", "))
                    } else {
                        format!("not loaded: {}", missing.join(", "))
                    },
                )
            })
            .await;
        }
        "scm" => {
            rec.run("init.units", claims, Duration::from_secs(20), async {
                let mut bad = Vec::new();
                for unit in &units {
                    match sys::run_local("sc", &["query", unit], &[], Duration::from_secs(5)).await
                    {
                        Ok(out) if out.stdout.contains("RUNNING") => {}
                        _ => bad.push(unit.clone()),
                    }
                }
                Check::new(
                    "init.units",
                    super::verdict(bad.is_empty()),
                    if bad.is_empty() {
                        format!("running: {}", units.join(", "))
                    } else {
                        format!("not running: {}", bad.join(", "))
                    },
                )
            })
            .await;
        }
        other => {
            rec.push(
                Check::new(
                    "init.units",
                    Status::Fail,
                    format!("unrecognised init system {other:?} (PID 1)"),
                )
                .fix("the image must boot with supervisord (container) or systemd (VM)"),
                claims,
            )
            .await;
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_supervisorctl() {
        let states = super::parse_supervisorctl(
            "audio                            RUNNING   pid 51, uptime 0:01:02\n\
             cua-spacesd                       BACKOFF   Exited too quickly\n",
        );
        assert_eq!(states["audio"], "RUNNING");
        assert_eq!(states["cua-spacesd"], "BACKOFF");
    }
}
