// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `cua cloud`: Spaces in your own cloud account (AWS, Google Cloud,
//! Modal) as a location for sandboxes and Spaces. The providers are the
//! sandbox layer's (`cua-byoc`); this `cua` reaches them through the
//! `cloud_*` tools, in the daemon when one runs, else in process.

use std::io::Write;
use std::sync::Arc;

use clap::{Args, Subcommand};
use cua_sdk::{CloudTarget, Cua, CuaError};

use crate::util::line;

/// Where in a cloud account Spaces go.
#[derive(Args, Debug, Clone, Default)]
pub struct TargetArgs {
    /// `aws`, `gcp` or `modal`.
    pub provider: String,
    /// AWS profile (`~/.aws/config`) or Modal profile (`~/.modal.toml`).
    #[arg(long)]
    pub profile: Option<String>,
    /// AWS or Google Cloud region.
    #[arg(long)]
    pub region: Option<String>,
    /// Google Cloud zone (default: the region's `-a` zone).
    #[arg(long)]
    pub zone: Option<String>,
    /// Google Cloud project (default: `gcloud config get project`).
    #[arg(long)]
    pub project: Option<String>,
    /// Modal environment (default: the profile's).
    #[arg(long)]
    pub environment: Option<String>,
}

impl TargetArgs {
    fn target(&self) -> CloudTarget {
        CloudTarget {
            provider: self.provider.trim().to_ascii_lowercase(),
            profile: self.profile.clone(),
            region: self.region.clone(),
            zone: self.zone.clone(),
            project: self.project.clone(),
            environment: self.environment.clone(),
        }
    }
}

/// `cua cloud`.
#[derive(Subcommand, Debug)]
pub enum CloudCmd {
    /// Your clouds: found credentials, the connected account and region,
    /// what each runs at what hourly cost, and what Cua created there.
    #[command(
        visible_alias = "ls",
        after_help = "Examples:
  cua cloud status
  cua cloud status aws --json"
    )]
    Status {
        /// Only this cloud (`aws`, `gcp`, `modal`).
        provider: Option<String>,
    },
    /// Connect a cloud account Spaces can be created in (after the checks
    /// `cua cloud test` runs). Credentials stay in the cloud's own CLI; Cua
    /// stores only the profile, region, project or environment names.
    #[command(after_help = "Examples:
  cua cloud connect aws --region us-west-2 --default
  cua cloud connect aws --profile sandbox
  cua cloud connect gcp --project my-project --region us-central1
  cua cloud connect modal --environment spaces
  # Spaces there delete themselves after 4 hours (default 8; 0: never)
  cua cloud connect aws --ttl-hours 4")]
    Connect {
        #[command(flatten)]
        target: TargetArgs,
        /// Also make it the default location (`default.on`).
        #[arg(long = "default")]
        make_default: bool,
        /// Delete each Space there after this many hours (default 8; 0:
        /// never; Modal caps a sandbox at 24).
        #[arg(long)]
        ttl_hours: Option<u32>,
    },
    /// Check a cloud account without creating anything: credentials,
    /// permissions (by dry run), quota, the region and machine types.
    #[command(after_help = "Examples:
  cua cloud test aws
  cua cloud test gcp --project my-project")]
    Test {
        #[command(flatten)]
        target: TargetArgs,
    },
    /// Forget a connected cloud (nothing in it is deleted).
    #[command(after_help = "Examples:
  cua cloud disconnect aws")]
    Disconnect {
        /// `aws`, `gcp` or `modal`.
        provider: String,
    },
    /// Find what Cua left in your clouds: expired Spaces and resources
    /// whose Space is gone (with --all, every Cua resource). Lists them
    /// unless --delete. Only resources Cua tagged and recorded are ever
    /// touched.
    #[command(after_help = "Examples:
  cua cloud sweep
  cua cloud sweep aws --delete
  cua cloud sweep --all --delete")]
    Sweep {
        /// Only this cloud.
        provider: Option<String>,
        /// Delete what it finds (default: list only).
        #[arg(long)]
        delete: bool,
        /// Include Cua resources that have not expired (every Space there).
        #[arg(long)]
        all: bool,
    },
}

fn json_line(out: &mut dyn Write, v: serde_json::Value) {
    line(out, v.to_string());
}

/// Runs `cmd` through `cua`'s Spaces (the daemon's when one runs).
pub async fn run(
    cua: &Arc<Cua>,
    cmd: CloudCmd,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let spaces = cua.spaces();
    match cmd {
        CloudCmd::Status { provider } => {
            let s = spaces
                .cloud_status(provider.map(|p| p.to_ascii_lowercase()))
                .await?;
            if json {
                json_line(out, status_json(&s));
                return Ok(0);
            }
            for p in &s.providers {
                line(out, provider_line(p));
            }
            for p in s.providers.iter().filter(|p| p.connected) {
                let kinds: Vec<String> = p
                    .kinds
                    .iter()
                    .filter(|k| k.supported)
                    .map(|k| format!("{} {} ~${:.2}/h", k.image, k.machine_type, k.usd_per_hour))
                    .collect();
                if !kinds.is_empty() {
                    line(out, format!("  {}: {}", p.title, kinds.join(", ")));
                }
            }
            if !s.resources.is_empty() {
                line(out, "");
                line(out, "Created by Cua:");
                for r in &s.resources {
                    line(out, resource_line(r));
                }
            }
        }
        CloudCmd::Connect {
            target,
            make_default,
            ttl_hours,
        } => {
            let c = spaces
                .cloud_connect(target.target(), make_default, ttl_hours)
                .await?;
            if json {
                let mut v = provider_json(&c.provider);
                v["checks"] = checks_json(&c.checks);
                json_line(out, v);
                return Ok(0);
            }
            for c in &c.checks {
                line(out, check_line(c));
            }
            line(
                out,
                format!(
                    "Connected {}{}. Create a sandbox there: cua sb create linux --on {} (a Space: cua spaces create --on {})",
                    c.provider.label,
                    if c.provider.default { " (default)" } else { "" },
                    c.provider.name,
                    c.provider.name
                ),
            );
        }
        CloudCmd::Test { target } => {
            let r = spaces.cloud_test(target.target()).await?;
            if json {
                json_line(
                    out,
                    serde_json::json!({
                        "provider": r.provider,
                        "ok": r.ok,
                        "account": r.account,
                        "checks": checks_json(&r.checks),
                    }),
                );
            } else {
                for c in &r.checks {
                    line(out, check_line(c));
                }
                line(
                    out,
                    if r.ok {
                        "Ready. Nothing was created.".to_string()
                    } else {
                        "Not ready: fix what failed above. Nothing was created.".to_string()
                    },
                );
            }
            return Ok(if r.ok { 0 } else { 1 });
        }
        CloudCmd::Disconnect { provider } => {
            let d = spaces
                .cloud_disconnect(provider.to_ascii_lowercase())
                .await?;
            if json {
                json_line(
                    out,
                    serde_json::json!({
                        "provider": d.provider,
                        "disconnected": d.disconnected,
                        "left": d.left.iter().map(resource_json).collect::<Vec<_>>(),
                    }),
                );
                return Ok(0);
            }
            line(
                out,
                if d.disconnected {
                    format!("Disconnected {}.", d.provider)
                } else {
                    format!("{} was not connected.", d.provider)
                },
            );
            if !d.left.is_empty() {
                line(
                    out,
                    "Still in that account (delete their Spaces, or `cua cloud sweep --all --delete`):",
                );
                for r in &d.left {
                    line(out, resource_line(r));
                }
            }
        }
        CloudCmd::Sweep {
            provider,
            delete,
            all,
        } => {
            let r = spaces
                .cloud_sweep(provider.map(|p| p.to_ascii_lowercase()), !delete, all)
                .await?;
            if json {
                json_line(
                    out,
                    serde_json::json!({
                        "dry_run": r.dry_run,
                        "resources": r.resources.iter().map(|i| {
                            let mut v = resource_json(&i.resource);
                            v["action"] = i.action.clone().into();
                            v["reason"] = i.reason.clone().into();
                            v
                        }).collect::<Vec<_>>(),
                    }),
                );
                return Ok(0);
            }
            if r.resources.is_empty() {
                line(out, "Nothing to clean up.");
            }
            for i in &r.resources {
                line(
                    out,
                    format!(
                        "{:<8} {}  ({})",
                        i.action,
                        resource_line(&i.resource).trim_start(),
                        i.reason
                    ),
                );
            }
            if r.dry_run && r.resources.iter().any(|i| i.action == "delete") {
                line(out, "Dry run: add --delete to delete these.");
            }
            if r.resources.iter().any(|i| i.action == "failed") {
                return Ok(1);
            }
        }
    }
    Ok(0)
}

fn provider_line(p: &cua_sdk::CloudProvider) -> String {
    let state = if p.connected {
        // The label leads with the title ("AWS · us-west-2"): print the place.
        let place = p
            .label
            .strip_prefix(&p.title)
            .map(|r| r.trim_start_matches([' ', '\u{b7}']))
            .filter(|r| !r.is_empty())
            .unwrap_or(&p.label);
        let mut s = format!("connected · {place}");
        if !p.account.is_empty() {
            s.push_str(&format!(" · {}", p.account));
        }
        if p.default {
            s.push_str(" · default");
        }
        s
    } else if p.credentials.found {
        format!(
            "not connected · credentials found ({})",
            p.credentials.source
        )
    } else {
        "not connected · no credentials found".to_string()
    };
    format!("{:<13} {state}", p.title)
}

fn resource_line(r: &cua_sdk::CloudResource) -> String {
    let mut s = format!("  {:<6} {:<15} {}", r.provider, r.resource_type, r.id);
    if !r.state.is_empty() {
        s.push_str(&format!(" {}", r.state));
    }
    if !r.sandbox.is_empty() {
        s.push_str(&format!(" · {}", r.sandbox));
    }
    if r.expired {
        s.push_str(" · expired");
    } else if !r.expires.is_empty() {
        s.push_str(&format!(" · expires {}", r.expires));
    }
    s
}

fn check_line(c: &cua_sdk::CloudCheck) -> String {
    format!(
        "{} {:<12} {}",
        if c.ok { "ok  " } else { "FAIL" },
        c.name,
        c.detail
    )
}

fn checks_json(checks: &[cua_sdk::CloudCheck]) -> serde_json::Value {
    checks
        .iter()
        .map(|c| serde_json::json!({"name": c.name, "ok": c.ok, "detail": c.detail}))
        .collect()
}

fn provider_json(p: &cua_sdk::CloudProvider) -> serde_json::Value {
    serde_json::json!({
        "name": p.name,
        "title": p.title,
        "tier": p.tier,
        "connected": p.connected,
        "default": p.default,
        "credentials": {"found": p.credentials.found, "source": p.credentials.source},
        "account": p.account,
        "profile": p.profile,
        "region": p.region,
        "zone": p.zone,
        "project": p.project,
        "environment": p.environment,
        "label": p.label,
        "ttl_hours": p.ttl_hours,
        "kinds": p.kinds.iter().map(|k| serde_json::json!({
            "image": k.image, "kind": k.kind, "supported": k.supported, "reason": k.reason,
            "machine_type": k.machine_type, "usd_per_hour": k.usd_per_hour,
        })).collect::<Vec<_>>(),
    })
}

fn resource_json(r: &cua_sdk::CloudResource) -> serde_json::Value {
    serde_json::json!({
        "provider": r.provider,
        "id": r.id,
        "type": r.resource_type,
        "sandbox": r.sandbox,
        "machine": r.machine,
        "region": r.region,
        "state": r.state,
        "created": r.created,
        "expires": r.expires,
        "expired": r.expired,
    })
}

fn status_json(s: &cua_sdk::CloudStatus) -> serde_json::Value {
    serde_json::json!({
        "default_on": s.default_on,
        "providers": s.providers.iter().map(provider_json).collect::<Vec<_>>(),
        "resources": s.resources.iter().map(resource_json).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct T {
        #[command(subcommand)]
        cmd: CloudCmd,
    }

    fn parse(a: &[&str]) -> CloudCmd {
        T::try_parse_from(std::iter::once("cloud").chain(a.iter().copied()))
            .unwrap()
            .cmd
    }

    #[test]
    fn a_connected_cloud_prints_its_place_once() {
        let p = cua_sdk::CloudProvider {
            name: "gcp".into(),
            title: "Google Cloud".into(),
            connected: true,
            label: "Google Cloud \u{b7} us-central1".into(),
            account: "cua-byoc-test".into(),
            ..Default::default()
        };
        let l = provider_line(&p);
        assert_eq!(l.matches("Google Cloud").count(), 1, "{l}");
        assert!(l.contains("connected · us-central1 · cua-byoc-test"), "{l}");
    }

    #[test]
    fn the_commands_parse() {
        match parse(&["connect", "aws", "--region", "us-west-2", "--default"]) {
            CloudCmd::Connect {
                target,
                make_default,
                ttl_hours,
            } => {
                assert_eq!(target.target().provider, "aws");
                assert_eq!(target.region.as_deref(), Some("us-west-2"));
                assert!(make_default);
                assert_eq!(ttl_hours, None);
            }
            other => panic!("{other:?}"),
        }
        match parse(&["sweep", "gcp"]) {
            CloudCmd::Sweep {
                provider,
                delete,
                all,
            } => assert_eq!(
                (provider.as_deref(), delete, all),
                (Some("gcp"), false, false)
            ),
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            parse(&["ls"]),
            CloudCmd::Status { provider: None }
        ));
        assert!(matches!(
            parse(&["test", "MODAL", "--environment", "e"]),
            CloudCmd::Test { target } if target.target().provider == "modal"
        ));
    }
}
