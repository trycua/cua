// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `cua cloud add aws`: EC2 hosts in your connected AWS account (`cua cloud
//! connect aws`) that join your pool by themselves. A thin command over the
//! elastic pool's host lifecycle (`cua_byoc::hosts`), the same one every
//! cloud and the automatic scaler use: each host is tagged and recorded
//! before it exists, installs cua and runs `cua host setup --pair <code>
//! --no-desktop --provide-spaces --dedicated`, and powers off (EC2 then
//! terminates it) at its TTL. GPU types boot the AWS Deep Learning image.
//! Hosts made here are pinned: the scaler never removes them; `--down`
//! does. Hosts of this kind let each other in on every TCP and UDP port
//! (and nothing else in): frameworks such as PyTorch open rank ports of
//! their own choosing, which a run reaches directly inside the VPC.

use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

use cua_byoc::hosts::{HostOptions, Hosts};
use cua_byoc::state::Store;
use cua_sandbox_core::byoc::{AddHost, CloudHost};
use cua_sdk::CuaError;
use serde_json::json;

use crate::onboard::AwsArgs;
use crate::util::line;

/// The TCP ports open between pool hosts (and only between them): all of
/// them, since ranks of frameworks such as PyTorch (NCCL, Gloo) listen on
/// ports of their own choosing.
pub const GANG_PORTS: (u16, u16) = (0, 65535);
/// How long `cua cloud add aws` waits for its hosts to join.
const JOIN_WAIT: Duration = Duration::from_secs(20 * 60);

fn err(e: cua_sandbox_core::Error) -> CuaError {
    CuaError::from(e)
}

/// The lifecycle over this home's clouds.
fn lifecycle() -> Hosts {
    let home = crate::util::cua_home();
    Hosts::new(
        cua_byoc::builtin_apis(),
        Arc::new(Store::new(&home)),
        Some(cua_byoc::RelayAccess::from_env()),
    )
}

/// `cua cloud add aws`.
pub async fn run(a: AwsArgs, json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let hosts = lifecycle();
    if a.list {
        return list(&hosts, json, out).await;
    }
    if a.down {
        return down(&hosts, a.name.as_deref(), json, out).await;
    }
    add(a, &hosts, json, out).await
}

async fn add(a: AwsArgs, hosts: &Hosts, json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    if !(1..=20).contains(&a.count) {
        return Err(CuaError::InvalidArgument("--count is 1 to 20".into()));
    }
    if let Some(h) = a.ttl_hours
        && !(1..=168).contains(&h)
    {
        return Err(CuaError::InvalidArgument(
            "--ttl-hours is 1 to 168: pool hosts always end".into(),
        ));
    }
    if a.disk_gb.is_some_and(|d| d < 60) {
        return Err(CuaError::InvalidArgument(
            "--disk-gb is at least 60 (models and images)".into(),
        ));
    }
    let opts = HostOptions {
        disk_gb: a.disk_gb,
        peer_ports: Some(GANG_PORTS),
        extra_tags: cua_byoc::hosts::user_tags(&a.tags).map_err(err)?,
        env: crate::onboard::env_prefix()
            .split_whitespace()
            .map(str::to_string)
            .collect(),
        ..Default::default()
    };
    let made = hosts
        .add_many(
            &AddHost {
                provider: "aws".into(),
                type_name: a.instance_type.trim().to_ascii_lowercase(),
                region: a.region.clone().unwrap_or_default(),
                name: String::new(),
                pinned: true,
                ttl_secs: u64::from(a.ttl_hours.unwrap_or(8)) * 3600,
                prepull: vec![],
            },
            a.count,
            &opts,
        )
        .await
        .map_err(err)?;
    if !json {
        for h in &made {
            line(
                out,
                format!(
                    "{}: {} {} in {} (${:.4}/h, ends {})",
                    h.name,
                    h.id,
                    h.type_name,
                    h.region,
                    h.usd_per_hour,
                    cua_byoc::model::rfc3339(h.expires)
                ),
            );
        }
    }
    if a.no_wait {
        if json {
            line(out, json!({"hosts": rows(&made)}).to_string());
        } else {
            line(
                out,
                "They join your pool in a few minutes: cua cloud add aws --list, cua cloud hosts ls",
            );
        }
        return Ok(0);
    }
    if !json {
        line(
            out,
            "Waiting for them to boot, install cua and join (a few minutes; Ctrl-C is safe, they \
             join anyway)...",
        );
    }
    let names: Vec<String> = made.iter().map(|h| h.name.clone()).collect();
    let mut joined_lines = Vec::new();
    let all = hosts
        .wait_joined(&names, JOIN_WAIT, |h| {
            joined_lines.push(format!(
                "{}: joined as relay computer {}",
                h.name, h.machine
            ));
        })
        .await
        .map_err(err)?;
    let joined = all.iter().filter(|h| h.state == "joined").count();
    if json {
        line(out, json!({"hosts": rows(&all)}).to_string());
    } else {
        for l in joined_lines {
            line(out, l);
        }
        if joined == made.len() {
            line(
                out,
                format!(
                    "{joined} host{} in your pool: cua computers · cua sb run --on {} -- echo hi · \
                     cua cloud add aws --down when done",
                    if joined == 1 { " is" } else { "s are" },
                    made[0].name
                ),
            );
        } else {
            line(
                out,
                "Not all hosts joined within 20 minutes: see `cua cloud add aws --list`; its boot log \
                 says why: aws ec2 get-console-output --latest --instance-id <instance>",
            );
        }
    }
    Ok(if joined < made.len() { 1 } else { 0 })
}

fn aws_hosts(all: Vec<CloudHost>) -> Vec<CloudHost> {
    all.into_iter().filter(|h| h.provider == "aws").collect()
}

fn rows(hosts: &[CloudHost]) -> Vec<serde_json::Value> {
    hosts
        .iter()
        .map(|h| {
            json!({
                "name": h.name,
                "instance": h.id,
                "type": h.type_name,
                "region": h.region,
                "state": h.state,
                "machine": h.machine,
                "usd_per_hour": h.usd_per_hour,
                "expires": cua_byoc::model::rfc3339(h.expires),
            })
        })
        .collect()
}

/// `cua cloud add aws --list`: the hosts, their state and whether they joined.
async fn list(hosts: &Hosts, json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let hs = aws_hosts(hosts.list().await.map_err(err)?);
    if json {
        line(out, json!({"hosts": rows(&hs)}).to_string());
        return Ok(0);
    }
    if hs.is_empty() {
        line(out, "No AWS hosts here.");
        return Ok(0);
    }
    line(
        out,
        format!(
            "{:<8}  {:<20}  {:<12}  {:<10}  {:>8}  {:<20}  MACHINE",
            "NAME", "INSTANCE", "TYPE", "STATE", "$/H", "ENDS"
        ),
    );
    for h in &hs {
        line(
            out,
            format!(
                "{:<8}  {:<20}  {:<12}  {:<10}  {:>8}  {:<20}  {}",
                h.name,
                h.id,
                h.type_name,
                h.state,
                format!("${:.4}", h.usd_per_hour),
                cua_byoc::model::rfc3339(h.expires),
                if h.machine.is_empty() {
                    "(not joined yet)"
                } else {
                    &h.machine
                }
            ),
        );
    }
    Ok(0)
}

/// `cua cloud add aws --down`: removes the hosts (only ones whose tags say
/// this home made them) and their relay machines.
async fn down(
    hosts: &Hosts,
    only: Option<&str>,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let hs: Vec<CloudHost> = aws_hosts(hosts.list().await.map_err(err)?)
        .into_iter()
        .filter(|h| only.is_none_or(|n| h.name == n))
        .collect();
    if hs.is_empty() {
        line(
            out,
            match only {
                Some(n) => format!("No AWS host {n} here."),
                None => "No AWS hosts here.".into(),
            },
        );
        return Ok(if only.is_some() { 1 } else { 0 });
    }
    let mut done = Vec::new();
    let mut failed = 0;
    for h in hs {
        match hosts.remove(&h.name).await {
            Ok(()) => {
                if !json {
                    line(out, format!("{}: {} terminated", h.name, h.id));
                }
                done.push(json!({"name": h.name, "instance": h.id, "machine": h.machine}));
            }
            Err(e) => {
                failed += 1;
                line(out, format!("{}: {} not terminated: {e}", h.name, h.id));
            }
        }
    }
    if json {
        line(out, json!({"terminated": done}).to_string());
    }
    Ok(if failed > 0 { 1 } else { 0 })
}
