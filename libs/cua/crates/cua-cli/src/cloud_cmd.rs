// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `cua cloud`: Spaces in your own cloud account (AWS, Google Cloud,
//! Hetzner, Modal) as a location for sandboxes and Spaces. The providers are the
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
    /// `aws`, `gcp`, `hetzner` or `modal`.
    pub provider: String,
    /// AWS profile (`~/.aws/config`) or Modal profile (`~/.modal.toml`).
    #[arg(long)]
    pub profile: Option<String>,
    /// AWS or Google Cloud region, or Hetzner location (default `fsn1`).
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
    /// Hetzner: read the project's Read & Write API token from stdin
    /// (otherwise `HCLOUD_TOKEN`, the saved token or the hcloud CLI's
    /// context). It is saved in the Cua credential store once the checks
    /// pass.
    #[arg(long)]
    pub token_stdin: bool,
}

impl TargetArgs {
    fn target(&self) -> CloudTarget {
        let token = self
            .token_stdin
            .then(|| std::io::read_to_string(std::io::stdin()).unwrap_or_default())
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty());
        CloudTarget {
            token,
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
        /// Only this cloud (`aws`, `gcp`, `hetzner`, `modal`).
        provider: Option<String>,
    },
    /// Connect a cloud account Spaces can be created in (after the checks
    /// `cua cloud test` runs). AWS, Google Cloud and Modal credentials stay in
    /// the cloud's own CLI and Cua stores only the profile, region, project
    /// or environment names; Hetzner has no sign-in, so its project API
    /// token is saved in the Cua credential store.
    #[command(after_help = "Examples:
  cua cloud connect aws --region us-west-2 --default
  cua cloud connect aws --profile sandbox
  cua cloud connect gcp --project my-project --region us-central1
  cua cloud connect modal --environment spaces
  # Hetzner: the project's API token on standard input
  cua cloud connect hetzner --token-stdin --region fsn1
  # Spaces there delete themselves after 4 hours (default 8; 0: never)
  cua cloud connect aws --ttl-hours 4")]
    Connect {
        #[command(flatten)]
        target: TargetArgs,
        /// Also make it the default location (`default.on`).
        #[arg(long = "default")]
        make_default: bool,
        /// Delete each Space there after this many hours (default 8; 0:
        /// never; Modal caps a sandbox at 24; a Hetzner server shuts itself
        /// down then and `cua cloud sweep --delete` removes it).
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
    /// Forget a connected cloud. Refused while Spaces Cua created there
    /// still run (they would keep billing unseen); add --delete to delete
    /// everything Cua created there first, then disconnect.
    #[command(after_help = "Examples:
  cua cloud disconnect aws
  # delete everything Cua created in the account, then disconnect
  cua cloud disconnect aws --delete")]
    Disconnect {
        /// `aws`, `gcp`, `hetzner` or `modal`.
        provider: String,
        /// Delete everything Cua created there first; disconnect only
        /// once the cloud confirms nothing is left.
        #[arg(long)]
        delete: bool,
    },
    /// What Cua did in your clouds: connects, disconnects, and every
    /// resource recorded, created, confirmed and deleted (by you, a sweep,
    /// the automatic sweep or disconnect --delete). Newest last. Never
    /// shows a secret.
    #[command(after_help = "Examples:
  cua cloud audit
  cua cloud audit --limit 200
  cua cloud audit --json")]
    Audit {
        /// How many of the newest entries (default 50).
        #[arg(long, default_value_t = 50)]
        limit: u32,
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
    /// Add hosts in your cloud to your pool: computers made for Cua work
    /// that join by themselves (`cua cloud add aws`).
    #[command(after_help = "Examples:
  cua cloud add aws --count 2 --type g6.xlarge --ttl-hours 6
  cua cloud add aws --list
  cua cloud add aws --down")]
    Add {
        #[command(subcommand)]
        cloud: AddCloud,
    },
    /// What the elastic pool would do for some Spaces: where each goes
    /// (your devices first, then hosts with room, then new cloud machines
    /// at the least cost) and what it costs. Pins are constraints of the
    /// same solve. With --apply, adds the machines (then create the Spaces
    /// with --on auto).
    #[command(after_help = "Examples:
  cua cloud plan --count 256 --image linux:24.04-slim --caller batch --duration-secs 1800
  cua cloud plan --count 20 --provider aws --instance-type c7g.xlarge --json
  cua cloud plan --count 64 --no-scale
  cua cloud plan --count 32 --apply")]
    Plan(PlanArgs),
    /// The hosts the elastic pool runs in your clouds: add them by hand
    /// (pinned: the scaler never removes them), list or remove them.
    #[command(subcommand)]
    Hosts(HostsCmd),
    /// The elastic pool's budget: a $/hour cap, a monthly cap, the
    /// autoscale switch. Without flags, shows them and what the clouds cost
    /// now.
    #[command(after_help = "Examples:
  cua cloud budget
  cua cloud budget --max-per-hour 2 --monthly 40
  cua cloud budget --autoscale off")]
    Budget {
        /// Most USD per hour your clouds may cost at once.
        #[arg(long = "max-per-hour")]
        max_usd_per_hour: Option<f64>,
        /// Most USD per calendar month on elastic hosts.
        #[arg(long = "monthly")]
        monthly_usd: Option<f64>,
        /// `on` or `off`: add and remove cloud hosts by itself. Off, you
        /// manage machines: `--on auto` uses only what runs, and `--on
        /// <cloud>` gives each Space a cloud machine of its own.
        #[arg(long)]
        autoscale: Option<String>,
        /// Most hosts one placement adds.
        #[arg(long)]
        max_new_hosts: Option<u32>,
        /// `on` or `off`: keep an idle host warm when demand predicts a
        /// Space soon.
        #[arg(long)]
        warm: Option<String>,
        /// `on` or `off`: let one small VM Space buy a KVM metal host
        /// (dollars an hour). Off, such a request is refused and says so.
        #[arg(long)]
        allow_metal: Option<String>,
    },
    /// Run the elastic pool's scale-in pass now (the daemon runs one every
    /// 30 s): idle hosts go at the end of their paid time.
    #[command(after_help = "Examples:
  cua cloud reconcile
  cua cloud reconcile --watch")]
    Reconcile {
        /// Keep running a pass every 30 s.
        #[arg(long)]
        watch: bool,
    },
}

/// `cua cloud plan`.
#[derive(Args, Debug, Clone, Default)]
pub struct PlanArgs {
    /// How many Spaces.
    #[arg(long, default_value_t = 1)]
    pub count: u32,
    /// Image (default linux).
    #[arg(long)]
    pub image: Option<String>,
    /// `container`, `vm` or `microvm`.
    #[arg(long)]
    pub kind: Option<String>,
    /// vCPUs each.
    #[arg(long)]
    pub cpus: Option<u32>,
    /// Memory each, MiB.
    #[arg(long)]
    pub memory_mb: Option<u64>,
    /// Keep them on one machine (forks of one checkpoint).
    #[arg(long)]
    pub together: bool,
    /// `interactive`, `batch`, `compat` or `persistent_agent`.
    #[arg(long)]
    pub caller: Option<String>,
    /// Expected lifetime, seconds.
    #[arg(long)]
    pub duration_secs: Option<u64>,
    /// Only this cloud.
    #[arg(long)]
    pub provider: Option<String>,
    /// Only this region.
    #[arg(long)]
    pub region: Option<String>,
    /// Only this machine type.
    #[arg(long)]
    pub instance_type: Option<String>,
    /// Exactly this many new machines.
    #[arg(long)]
    pub hosts: Option<u32>,
    /// Add no machines.
    #[arg(long)]
    pub no_scale: bool,
    /// Only your own devices.
    #[arg(long)]
    pub devices_only: bool,
    /// A $/hour cap for this plan.
    #[arg(long = "max-per-hour")]
    pub max_usd_per_hour: Option<f64>,
    /// Add the machines the plan needs.
    #[arg(long)]
    pub apply: bool,
}

/// `cua cloud hosts`.
#[derive(Subcommand, Debug)]
pub enum HostsCmd {
    /// The hosts with their state and cost.
    #[command(
        visible_alias = "ls",
        after_help = "Examples:
  cua cloud hosts ls"
    )]
    List,
    /// Add hosts by hand (pinned: never removed by the scaler).
    #[command(after_help = "Examples:
  cua cloud hosts add --provider hetzner --type cax41 --region fsn1
  cua cloud hosts add --provider aws --type c7g.xlarge --count 2 --ttl-hours 4")]
    Add {
        /// `aws`, `gcp` or `hetzner`.
        #[arg(long)]
        provider: String,
        /// Machine type (`cua cloud plan --json` lists the offers).
        #[arg(long = "type")]
        instance_type: String,
        /// Region (default: the connection's).
        #[arg(long)]
        region: Option<String>,
        /// How many.
        #[arg(long, default_value_t = 1)]
        count: u32,
        /// Hours until each ends on its own (1 to 168, default 24).
        #[arg(long)]
        ttl_hours: Option<u32>,
    },
    /// Remove hosts this home added (their Spaces end).
    #[command(
        visible_alias = "rm",
        after_help = "Examples:
  cua cloud hosts rm hetzner-1
  cua cloud hosts rm aws-1 aws-2"
    )]
    Remove {
        /// Their names (`hetzner-1`).
        #[arg(required = true)]
        names: Vec<String>,
    },
}

fn on_off(v: Option<String>, flag: &str) -> Result<Option<bool>, CuaError> {
    match v.as_deref().map(str::trim) {
        None => Ok(None),
        Some("on" | "true" | "yes") => Ok(Some(true)),
        Some("off" | "false" | "no") => Ok(Some(false)),
        Some(o) => Err(CuaError::InvalidArgument(format!(
            "--{flag} {o}: on or off"
        ))),
    }
}

fn host_line(h: &cua_sdk::ElasticHost) -> String {
    format!(
        "{:<12} {:<8} {:<12} {:<10} {:<9} ${:.4}/h{}",
        h.name,
        h.provider,
        h.type_name,
        h.region,
        h.state,
        h.usd_per_hour,
        if h.pinned { "  pinned" } else { "" }
    )
}

/// `cua cloud add <cloud>`.
#[derive(Subcommand, Debug)]
pub enum AddCloud {
    /// EC2 hosts in your connected AWS account (`cua cloud connect aws`)
    /// that join your pool by themselves: Ubuntu 24.04 for CPU types, the
    /// AWS Deep Learning Base GPU image for g*/p* types. Each one is
    /// dedicated (made only for this work: jobs run in its own Space, with
    /// its GPU) and powers off (and is terminated) at its TTL.
    #[command(after_help = "Examples:
  cua cloud add aws --count 2 --type c7i.large
  cua cloud add aws --count 2 --type g6.xlarge --ttl-hours 6 --tag team=ml
  cua cloud add aws --list
  cua cloud add aws --down")]
    Aws(crate::onboard::AwsArgs),
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
        CloudCmd::Disconnect { provider, delete } => {
            let d = spaces
                .cloud_disconnect(provider.to_ascii_lowercase(), delete)
                .await?;
            if json {
                json_line(
                    out,
                    serde_json::json!({
                        "provider": d.provider,
                        "disconnected": d.disconnected,
                        "deleted": d.deleted.iter().map(resource_json).collect::<Vec<_>>(),
                        "left": d.left.iter().map(resource_json).collect::<Vec<_>>(),
                    }),
                );
                return Ok(0);
            }
            if !d.deleted.is_empty() {
                line(out, "Deleted from that account:");
                for r in &d.deleted {
                    line(out, resource_line(r));
                }
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
                    "Still recorded for that account (`cua cloud sweep --all --delete` after reconnecting removes them):",
                );
                for r in &d.left {
                    line(out, resource_line(r));
                }
            }
        }
        CloudCmd::Audit { limit } => {
            let entries = spaces.cloud_audit(Some(limit)).await?;
            if json {
                json_line(
                    out,
                    serde_json::Value::Array(entries.iter().map(audit_json).collect()),
                );
                return Ok(0);
            }
            if entries.is_empty() {
                line(out, "Nothing recorded yet.");
            }
            for e in &entries {
                line(out, audit_line(e));
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
        CloudCmd::Plan(a) => {
            let p = spaces
                .cloud_plan(cua_sdk::ElasticPlanRequest {
                    count: Some(a.count),
                    image: a.image,
                    kind: a.kind,
                    cpus: a.cpus,
                    memory_mb: a.memory_mb,
                    together: Some(a.together),
                    caller: a.caller,
                    duration_secs: a.duration_secs,
                    provider: a.provider,
                    region: a.region,
                    instance_type: a.instance_type,
                    hosts: a.hosts,
                    no_scale: Some(a.no_scale),
                    devices_only: Some(a.devices_only),
                    max_usd_per_hour: a.max_usd_per_hour,
                    apply: a.apply,
                })
                .await?;
            if json {
                line(out, p.json);
                return Ok(0);
            }
            for e in &p.explain {
                line(out, e);
            }
            line(
                out,
                format!(
                    "{} new machine{} (+${:.4}/h), ${:.4} for the work (lower bound ${:.4}); ready in ~{} s; {} solver",
                    p.new_hosts.len(),
                    if p.new_hosts.len() == 1 { "" } else { "s" },
                    p.added_usd_per_hour,
                    p.projected_usd,
                    p.lower_bound_usd,
                    p.ready_in_secs,
                    p.solver
                ),
            );
            for h in &p.added {
                line(out, format!("added {}", host_line(h)));
            }
            if !p.unplaced.is_empty() {
                return Ok(1);
            }
        }
        CloudCmd::Hosts(HostsCmd::List) => {
            let hosts = spaces.cloud_hosts().await?;
            if json {
                json_line(
                    out,
                    hosts
                        .iter()
                        .map(|h| {
                            serde_json::json!({"name": h.name, "provider": h.provider, "type": h.type_name,
                                "region": h.region, "id": h.id, "machine": h.machine, "state": h.state,
                                "started": h.started, "expires": h.expires, "usd_per_hour": h.usd_per_hour,
                                "pinned": h.pinned})
                        })
                        .collect(),
                );
                return Ok(0);
            }
            if hosts.is_empty() {
                line(
                    out,
                    "No elastic hosts (they are added when your machines are full, or with cua cloud hosts add).",
                );
            }
            let total: f64 = hosts
                .iter()
                .filter(|h| h.state != "gone")
                .map(|h| h.usd_per_hour)
                .sum();
            for h in &hosts {
                line(out, host_line(h));
            }
            if !hosts.is_empty() {
                line(out, format!("{} hosts, ${total:.4}/h", hosts.len()));
            }
        }
        CloudCmd::Hosts(HostsCmd::Add {
            provider,
            instance_type,
            region,
            count,
            ttl_hours,
        }) => {
            let hosts = spaces
                .cloud_add_host(provider, instance_type, region, Some(count), ttl_hours)
                .await?;
            for h in &hosts {
                if json {
                    json_line(
                        out,
                        serde_json::json!({"name": h.name, "provider": h.provider, "type": h.type_name, "region": h.region, "state": h.state}),
                    );
                } else {
                    line(out, format!("added {}", host_line(h)));
                }
            }
        }
        CloudCmd::Hosts(HostsCmd::Remove { names }) => {
            let r = spaces.cloud_remove_host(names).await?;
            if json {
                json_line(
                    out,
                    serde_json::json!({"removed": r.removed, "failed": r.failed}),
                );
            } else {
                for n in &r.removed {
                    line(out, format!("removed {n}"));
                }
                for f in &r.failed {
                    line(out, format!("NOT removed {f}"));
                }
            }
            if !r.failed.is_empty() {
                return Ok(1);
            }
        }
        CloudCmd::Budget {
            max_usd_per_hour,
            monthly_usd,
            autoscale,
            max_new_hosts,
            warm,
            allow_metal,
        } => {
            let b = spaces
                .cloud_budget(cua_sdk::ElasticBudgetUpdate {
                    max_usd_per_hour,
                    monthly_usd,
                    autoscale: on_off(autoscale, "autoscale")?,
                    max_new_hosts,
                    warm: on_off(warm, "warm")?,
                    allow_metal: on_off(allow_metal, "allow-metal")?,
                })
                .await?;
            if json {
                json_line(
                    out,
                    serde_json::json!({"autoscale": b.autoscale, "max_usd_per_hour": b.max_usd_per_hour,
                        "monthly_usd": b.monthly_usd, "max_new_hosts": b.max_new_hosts, "warm": b.warm,
                        "allow_metal": b.allow_metal, "month_usd": b.month_usd, "running_usd_per_hour": b.running_usd_per_hour, "hosts": b.hosts}),
                );
            } else {
                line(
                    out,
                    format!(
                        "autoscale {} · cap ${:.2}/h · ${:.2}/month (spent ${:.2}) · {} hosts at ${:.4}/h now",
                        if b.autoscale { "on" } else { "off" },
                        b.max_usd_per_hour,
                        b.monthly_usd,
                        b.month_usd,
                        b.hosts,
                        b.running_usd_per_hour
                    ),
                );
            }
        }
        CloudCmd::Reconcile { watch } => loop {
            let d = spaces.cloud_reconcile().await?;
            for x in &d {
                if json {
                    json_line(
                        out,
                        serde_json::json!({"host": x.host, "action": x.action, "reason": x.reason, "until": x.until, "saves_usd": x.saves_usd}),
                    );
                } else {
                    line(out, format!("{:<12} {:<7} {}", x.host, x.action, x.reason));
                }
            }
            if !watch {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        },
        // Runs without a Space client (`cua cloud add aws` in the CLI).
        CloudCmd::Add { .. } => {
            return Err(CuaError::Internal(
                "cua cloud add runs in the CLI, not through Spaces".into(),
            ));
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

fn audit_json(e: &cua_sdk::CloudAuditEntry) -> serde_json::Value {
    serde_json::json!({
        "ts": e.ts, "action": e.action, "provider": e.provider, "kind": e.kind,
        "id": e.id, "space": e.space, "actor": e.actor, "outcome": e.outcome,
        "detail": e.detail,
    })
}

fn audit_line(e: &cua_sdk::CloudAuditEntry) -> String {
    let mut s = format!("{} {:<6} {:<18}", e.ts, e.provider, e.action);
    for part in [&e.kind, &e.id] {
        if !part.is_empty() {
            s.push_str(&format!(" {part}"));
        }
    }
    if !e.space.is_empty() {
        s.push_str(&format!(" · {}", e.space));
    }
    s.push_str(&format!(" · {} · {}", e.actor, e.outcome));
    if !e.detail.is_empty() {
        s.push_str(&format!(" · {}", e.detail));
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

    fn parse_err(a: &[&str]) -> bool {
        T::try_parse_from(std::iter::once("cloud").chain(a.iter().copied())).is_err()
    }

    #[test]
    fn an_audit_line_names_who_did_what() {
        let e = cua_sdk::CloudAuditEntry {
            ts: "2026-10-21T09:00:00Z".into(),
            action: "resource_deleted".into(),
            provider: "aws".into(),
            kind: "instance".into(),
            id: "i-1".into(),
            space: "research".into(),
            actor: "auto-sweep".into(),
            outcome: "ok".into(),
            detail: "expired".into(),
        };
        let l = audit_line(&e);
        assert!(
            l.ends_with("instance i-1 · research · auto-sweep · ok · expired"),
            "{l}"
        );
        assert_eq!(audit_json(&e)["actor"], "auto-sweep");
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
        match parse(&[
            "add",
            "aws",
            "--count",
            "2",
            "--type",
            "g6.xlarge",
            "--ttl-hours",
            "6",
            "--tag",
            "team=ml",
        ]) {
            CloudCmd::Add {
                cloud: AddCloud::Aws(a),
            } => {
                assert_eq!((a.count, a.instance_type.as_str()), (2, "g6.xlarge"));
                assert_eq!(
                    (a.ttl_hours, a.tags.as_slice()),
                    (Some(6), &["team=ml".to_string()][..])
                );
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            parse(&["add", "aws", "--down", "--name", "aws-3"]),
            CloudCmd::Add { cloud: AddCloud::Aws(a) } if a.down && a.name.as_deref() == Some("aws-3")
        ));
        assert!(parse_err(&["add", "aws", "--list", "--down"]));
        assert!(matches!(
            parse(&["disconnect", "AWS"]),
            CloudCmd::Disconnect { provider, delete: false } if provider == "AWS"
        ));
        assert!(matches!(
            parse(&["disconnect", "aws", "--delete"]),
            CloudCmd::Disconnect { delete: true, .. }
        ));
        assert!(matches!(parse(&["audit"]), CloudCmd::Audit { limit: 50 }));
        assert!(matches!(
            parse(&["audit", "--limit", "7"]),
            CloudCmd::Audit { limit: 7 }
        ));
        assert!(parse_err(&["audit", "--limit", "many"]));
        assert!(matches!(
            parse(&["ls"]),
            CloudCmd::Status { provider: None }
        ));
        match parse(&["connect", "hetzner", "--token-stdin", "--region", "nbg1"]) {
            CloudCmd::Connect { target, .. } => {
                assert_eq!(target.provider, "hetzner");
                assert!(target.token_stdin);
                assert_eq!(target.region.as_deref(), Some("nbg1"));
            }
            other => panic!("{other:?}"),
        }
        // Without the flag the token is never read from stdin.
        match parse(&["connect", "hetzner"]) {
            CloudCmd::Connect { target, .. } => {
                assert!(!target.token_stdin);
                assert_eq!(target.target().token, None);
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            parse(&["test", "MODAL", "--environment", "e"]),
            CloudCmd::Test { target } if target.target().provider == "modal"
        ));
    }
}
