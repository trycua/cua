// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The elastic pool on [`Spaces`]: plan where Spaces go and which cloud
//! machines get added (and apply it), list, add and remove the hosts it
//! runs in your clouds, set its budget, and run its scale-in pass. Every
//! call is one of the Spaces `cloud_*` tools, in process or in the daemon,
//! so the CLI, the SDKs, MCP and the apps reach the same core.

use serde::Deserialize;
use serde_json::{Value, json};

use super::run;
use super::spaces::{Spaces, parse};
use crate::Result;

/// What to plan for (`cloud_plan`). Pins are constraints of the same
/// solve: leave them unset to let the pool decide.
#[derive(Debug, Clone, PartialEq, Default, uniffi::Record)]
pub struct ElasticPlanRequest {
    /// How many Spaces (default 1).
    #[uniffi(default = None)]
    pub count: Option<u32>,
    /// Image (default linux).
    #[uniffi(default = None)]
    pub image: Option<String>,
    /// `container`, `vm` or `microvm` (default: from the image).
    #[uniffi(default = None)]
    pub kind: Option<String>,
    /// vCPUs each (default: learned from earlier Spaces of the image).
    #[uniffi(default = None)]
    pub cpus: Option<u32>,
    /// MiB each.
    #[uniffi(default = None)]
    pub memory_mb: Option<u64>,
    /// Keep them on one machine (forks of one checkpoint).
    #[uniffi(default = None)]
    pub together: Option<bool>,
    /// `interactive`, `batch`, `compat` or `persistent_agent`.
    #[uniffi(default = None)]
    pub caller: Option<String>,
    /// Expected lifetime, seconds.
    #[uniffi(default = None)]
    pub duration_secs: Option<u64>,
    /// Only this cloud.
    #[uniffi(default = None)]
    pub provider: Option<String>,
    /// Only this region.
    #[uniffi(default = None)]
    pub region: Option<String>,
    /// Only this machine type.
    #[uniffi(default = None)]
    pub instance_type: Option<String>,
    /// Exactly this many new machines.
    #[uniffi(default = None)]
    pub hosts: Option<u32>,
    /// Add no machines.
    #[uniffi(default = None)]
    pub no_scale: Option<bool>,
    /// Only your own devices.
    #[uniffi(default = None)]
    pub devices_only: Option<bool>,
    /// A $/hour cap for this plan.
    #[uniffi(default = None)]
    pub max_usd_per_hour: Option<f64>,
    /// Add the machines the plan needs.
    #[uniffi(default = false)]
    pub apply: bool,
}

impl ElasticPlanRequest {
    fn args(&self) -> Value {
        json!({
            "count": self.count,
            "image": self.image,
            "kind": self.kind,
            "cpus": self.cpus,
            "memory_mb": self.memory_mb,
            "together": self.together,
            "caller": self.caller,
            "duration_secs": self.duration_secs,
            "provider": self.provider,
            "region": self.region,
            "instance_type": self.instance_type,
            "hosts": self.hosts,
            "no_scale": self.no_scale,
            "devices_only": self.devices_only,
            "max_usd_per_hour": self.max_usd_per_hour,
            "apply": self.apply,
        })
    }
}

/// Some Spaces of one group on one machine.
#[derive(Debug, Clone, PartialEq, Default, uniffi::Record)]
pub struct ElasticAssignment {
    /// The group (`request-0`).
    pub group: String,
    /// A machine in the pool by name, or empty when it is a new one.
    pub host: String,
    /// The index into `new_hosts` (when `host` is empty).
    pub new_index: Option<u32>,
    /// How many Spaces.
    pub count: u32,
    /// The checkpoint is copied there first.
    pub sync: bool,
}

/// A machine the plan adds.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, uniffi::Record)]
pub struct ElasticNewHost {
    pub provider: String,
    pub type_name: String,
    pub region: String,
    pub usd_per_hour: f64,
    pub boot_secs: u32,
    /// Seconds billed for the work it gets.
    pub billed_secs: u64,
    /// A Spaces supplier's Space (Modal), not a host.
    #[serde(default)]
    pub one_space: bool,
    /// Estimated round trip from the requester, ms (0: unknown).
    #[serde(default)]
    pub rtt_ms: u32,
}

/// Spaces with no place, and why.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, uniffi::Record)]
pub struct ElasticUnplaced {
    pub group: String,
    pub count: u32,
    pub reason: String,
}

/// A host the elastic pool added in one of your clouds.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, uniffi::Record)]
pub struct ElasticHost {
    /// Its pool name (`hetzner-1`).
    pub name: String,
    pub provider: String,
    pub type_name: String,
    pub region: String,
    /// The cloud's id.
    #[serde(default)]
    pub id: String,
    /// Its relay machine once joined.
    #[serde(default)]
    pub machine: String,
    /// `creating`, `booting`, `joined`, `cordoned`, `offline`, `gone`.
    pub state: String,
    /// Unix seconds.
    pub started: u64,
    /// Unix seconds it ends on its own.
    #[serde(default)]
    pub expires: u64,
    pub usd_per_hour: f64,
    pub billing_step_secs: u64,
    pub billing_min_secs: u64,
    /// Added by hand: never removed by the scaler.
    #[serde(default)]
    pub pinned: bool,
}

/// The elastic pool's decision (`cloud_plan`).
#[derive(Debug, Clone, PartialEq, Default, uniffi::Record)]
pub struct ElasticPlan {
    pub assignments: Vec<ElasticAssignment>,
    pub new_hosts: Vec<ElasticNewHost>,
    pub unplaced: Vec<ElasticUnplaced>,
    /// USD per hour the new machines add.
    pub added_usd_per_hour: f64,
    /// USD the new machines cost for the work, billed.
    pub projected_usd: f64,
    /// A lower bound for any plan of the same work.
    pub lower_bound_usd: f64,
    /// `exact`, `exact-incumbent` or `greedy`.
    pub solver: String,
    /// Seconds until the last Space can start.
    pub ready_in_secs: u32,
    /// One line per decision.
    pub explain: Vec<String>,
    /// The hosts added (with `apply`).
    pub added: Vec<ElasticHost>,
    /// The whole answer as JSON (the demand it detected, the pool it saw):
    /// what an agent inspects or edits.
    pub json: String,
}

/// The elastic pool's caps and switches (`cloud_budget`): set those given.
#[derive(Debug, Clone, PartialEq, Default, uniffi::Record)]
pub struct ElasticBudgetUpdate {
    #[uniffi(default = None)]
    pub max_usd_per_hour: Option<f64>,
    #[uniffi(default = None)]
    pub monthly_usd: Option<f64>,
    #[uniffi(default = None)]
    pub autoscale: Option<bool>,
    #[uniffi(default = None)]
    pub max_new_hosts: Option<u32>,
    #[uniffi(default = None)]
    pub warm: Option<bool>,
    #[uniffi(default = None)]
    pub allow_metal: Option<bool>,
}

/// The budget, and what the clouds cost now.
#[derive(Debug, Clone, PartialEq, Default, uniffi::Record)]
pub struct ElasticBudget {
    pub autoscale: bool,
    pub max_usd_per_hour: f64,
    pub monthly_usd: f64,
    pub max_new_hosts: u32,
    pub warm: bool,
    pub allow_metal: bool,
    /// USD spent on elastic hosts this month.
    pub month_usd: f64,
    /// USD per hour of the hosts running now.
    pub running_usd_per_hour: f64,
    /// Hosts running.
    pub hosts: u32,
}

/// One scale-in decision.
#[derive(Debug, Clone, PartialEq, Default, uniffi::Record)]
pub struct ElasticDecision {
    pub host: String,
    /// `keep`, `cordon` or `remove`.
    pub action: String,
    pub reason: String,
    /// `keep`: unix seconds it is looked at again.
    pub until: u64,
    /// `remove`: USD saved against keeping it a billing step longer.
    pub saves_usd: f64,
}

/// What `cloud_remove_host` did.
#[derive(Debug, Clone, PartialEq, Default, uniffi::Record)]
pub struct ElasticRemoved {
    pub removed: Vec<String>,
    /// `name: error` per host not removed.
    pub failed: Vec<String>,
}

fn plan_of(v: Value) -> Result<ElasticPlan> {
    let p = &v["plan"];
    let assignments = p["assignments"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|a| {
            let t = &a["target"];
            let new = t["on"] == "new";
            ElasticAssignment {
                group: a["group"].as_str().unwrap_or_default().into(),
                host: if new {
                    String::new()
                } else {
                    t["name"].as_str().unwrap_or_default().into()
                },
                new_index: new.then(|| t["index"].as_u64().unwrap_or(0) as u32),
                count: a["count"].as_u64().unwrap_or(0) as u32,
                sync: a["sync"].as_bool().unwrap_or(false),
            }
        })
        .collect();
    Ok(ElasticPlan {
        assignments,
        new_hosts: serde_json::from_value(p["new_hosts"].clone()).unwrap_or_default(),
        unplaced: serde_json::from_value(p["unplaced"].clone()).unwrap_or_default(),
        added_usd_per_hour: p["added_usd_per_hour"].as_f64().unwrap_or(0.0),
        projected_usd: p["projected_usd"].as_f64().unwrap_or(0.0),
        lower_bound_usd: p["lower_bound_usd"].as_f64().unwrap_or(0.0),
        solver: p["solver"].as_str().unwrap_or_default().into(),
        ready_in_secs: p["ready_in_secs"].as_u64().unwrap_or(0) as u32,
        explain: serde_json::from_value(p["explain"].clone()).unwrap_or_default(),
        added: serde_json::from_value(v["added"].clone()).unwrap_or_default(),
        json: v.to_string(),
    })
}

fn decisions_of(v: Value) -> Vec<ElasticDecision> {
    v.as_array()
        .into_iter()
        .flatten()
        .map(|d| {
            let a = &d["action"];
            ElasticDecision {
                host: d["host"].as_str().unwrap_or_default().into(),
                action: a["action"].as_str().unwrap_or_default().into(),
                reason: a["reason"].as_str().unwrap_or_default().into(),
                until: a["until"].as_u64().unwrap_or(0),
                saves_usd: a["saves_usd"].as_f64().unwrap_or(0.0),
            }
        })
        .collect()
}

fn budget_of(v: Value) -> ElasticBudget {
    let p = &v["policy"];
    ElasticBudget {
        autoscale: p["autoscale"].as_bool().unwrap_or(true),
        max_usd_per_hour: p["max_usd_per_hour"].as_f64().unwrap_or(0.0),
        monthly_usd: p["monthly_usd"].as_f64().unwrap_or(0.0),
        max_new_hosts: p["max_new_hosts"].as_u64().unwrap_or(0) as u32,
        warm: p["warm"].as_bool().unwrap_or(true),
        allow_metal: p["allow_metal"].as_bool().unwrap_or(false),
        month_usd: v["month_usd"].as_f64().unwrap_or(0.0),
        running_usd_per_hour: v["running_usd_per_hour"].as_f64().unwrap_or(0.0),
        hosts: v["hosts"].as_u64().unwrap_or(0) as u32,
    }
}

/// Where a prewarm keeps Spaces ready.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, uniffi::Record)]
pub struct PrewarmPlacement {
    /// The machine's name.
    pub host: String,
    /// `local` or its relay machine id.
    pub machine: String,
    pub count: u32,
    pub ready: u32,
}

/// Spaces of one shape the elastic pool keeps ready: the next creates of
/// that shape claim them in a fraction of a second.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, uniffi::Record)]
pub struct Prewarm {
    /// `pw-xxxxxx`.
    pub id: String,
    pub image: String,
    pub kind: String,
    pub cpus: u32,
    pub memory_mb: u64,
    /// Asked.
    pub count: u32,
    /// Ready now.
    pub ready: u32,
    pub placements: Vec<PrewarmPlacement>,
    /// Unix seconds.
    pub expires_at: u64,
    /// Hosts added for it.
    #[serde(default)]
    pub new_hosts: Vec<String>,
}

/// What to keep ready (`Spaces::prewarm`).
#[derive(Debug, Clone, PartialEq, Default, uniffi::Record)]
pub struct PrewarmOptions {
    /// How many, 1 to 256 (default 1).
    #[uniffi(default = None)]
    pub count: Option<u32>,
    /// `auto` (default), `local`, a linked cloud or `host:<machine>`.
    #[uniffi(default = None)]
    pub on: Option<String>,
    /// `container`, `vm` or `microvm` (default: the image's).
    #[uniffi(default = None)]
    pub kind: Option<String>,
    #[uniffi(default = None)]
    pub cpus: Option<u32>,
    #[uniffi(default = None)]
    pub memory_mb: Option<u64>,
    /// Seconds to keep them (default 3600).
    #[uniffi(default = None)]
    pub ttl_secs: Option<u64>,
    /// Only this cloud machine type.
    #[uniffi(default = None)]
    pub instance_type: Option<String>,
    /// Only this cloud region.
    #[uniffi(default = None)]
    pub region: Option<String>,
}

impl PrewarmOptions {
    fn args(&self, image: &str) -> serde_json::Value {
        let mut a = json!({ "image": image, "count": self.count.unwrap_or(1) });
        let o = a.as_object_mut().expect("object");
        if let Some(v) = &self.on {
            o.insert("on".into(), json!(v));
        }
        if let Some(v) = &self.kind {
            o.insert("kind".into(), json!(v));
        }
        if let Some(v) = self.cpus {
            o.insert("cpus".into(), json!(v));
        }
        if let Some(v) = self.memory_mb {
            o.insert("memory_mb".into(), json!(v));
        }
        if let Some(v) = self.ttl_secs {
            o.insert("ttl_secs".into(), json!(v));
        }
        if let Some(v) = &self.instance_type {
            o.insert("instance_type".into(), json!(v));
        }
        if let Some(v) = &self.region {
            o.insert("region".into(), json!(v));
        }
        a
    }
}

#[uniffi::export]
impl Spaces {
    /// Keeps Spaces of `image` ready where the elastic pool puts them (the
    /// same solve as `on` `auto`, under the budget and pins), so the next
    /// creates of that shape start in a fraction of a second. Refused, and
    /// nothing bought, when the budget or pins cannot fit it.
    pub async fn prewarm(&self, image: String, options: PrewarmOptions) -> Result<Prewarm> {
        let host = self.tool_caller();
        run(async move { parse(host.tool_value("prewarm", options.args(&image)).await?) }).await
    }

    /// The prewarms, with how many each keeps ready now.
    pub async fn prewarms(&self) -> Result<Vec<Prewarm>> {
        let host = self.tool_caller();
        run(async move { parse(host.tool_value("prewarm_list", json!({})).await?) }).await
    }

    /// Ends prewarm `id` (its ready Spaces go). `false`: no such prewarm.
    pub async fn remove_prewarm(&self, id: String) -> Result<bool> {
        let host = self.tool_caller();
        run(async move {
            let v = host
                .tool_value("prewarm_remove", json!({ "id": id }))
                .await?;
            Ok(v["removed"].as_bool().unwrap_or(false))
        })
        .await
    }

    /// What the elastic pool would do for some Spaces: where each goes
    /// (your devices first, then hosts with room, then new cloud machines
    /// at the least cost) and what that costs. With `apply`, it adds the
    /// machines (create the Spaces with `on` `auto`).
    pub async fn cloud_plan(&self, request: ElasticPlanRequest) -> Result<ElasticPlan> {
        let host = self.tool_caller();
        run(async move { plan_of(host.tool_value("cloud_plan", request.args()).await?) }).await
    }

    /// The hosts the elastic pool added in your clouds, with their cost.
    pub async fn cloud_hosts(&self) -> Result<Vec<ElasticHost>> {
        let host = self.tool_caller();
        run(async move { parse(host.tool_value("cloud_hosts", json!({})).await?) }).await
    }

    /// Adds `count` hosts of `instance_type` in a linked cloud by hand
    /// (the scaler never removes them); each ends at its TTL.
    pub async fn cloud_add_host(
        &self,
        provider: String,
        instance_type: String,
        region: Option<String>,
        count: Option<u32>,
        ttl_hours: Option<u32>,
    ) -> Result<Vec<ElasticHost>> {
        let host = self.tool_caller();
        run(async move {
            parse(
                host.tool_value(
                    "cloud_add_host",
                    json!({"provider": provider, "instance_type": instance_type, "region": region, "count": count, "ttl_hours": ttl_hours}),
                )
                .await?,
            )
        })
        .await
    }

    /// Removes hosts this home added to the pool.
    pub async fn cloud_remove_host(&self, names: Vec<String>) -> Result<ElasticRemoved> {
        let host = self.tool_caller();
        run(async move {
            let v = host
                .tool_value("cloud_remove_host", json!({"names": names}))
                .await?;
            Ok(ElasticRemoved {
                removed: serde_json::from_value(v["removed"].clone()).unwrap_or_default(),
                failed: v["failed"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|f| {
                        format!(
                            "{}: {}",
                            f["name"].as_str().unwrap_or_default(),
                            f["error"].as_str().unwrap_or_default()
                        )
                    })
                    .collect(),
            })
        })
        .await
    }

    /// The elastic pool's budget and autoscale switch, after setting the
    /// fields `update` gives.
    pub async fn cloud_budget(&self, update: ElasticBudgetUpdate) -> Result<ElasticBudget> {
        let host = self.tool_caller();
        run(async move {
            let v = host
                .tool_value(
                    "cloud_budget",
                    json!({
                        "max_usd_per_hour": update.max_usd_per_hour,
                        "monthly_usd": update.monthly_usd,
                        "autoscale": update.autoscale,
                        "max_new_hosts": update.max_new_hosts,
                        "warm": update.warm,
                        "allow_metal": update.allow_metal,
                    }),
                )
                .await?;
            Ok(budget_of(v))
        })
        .await
    }

    /// One scale-in pass now: idle hosts go at the end of their paid time,
    /// the least-used one is cordoned when the others have room.
    pub async fn cloud_reconcile(&self) -> Result<Vec<ElasticDecision>> {
        let host = self.tool_caller();
        run(async move {
            Ok(decisions_of(
                host.tool_value("cloud_reconcile", json!({})).await?,
            ))
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A core prewarm decodes into the binding record (drift guard).
    #[test]
    fn the_core_prewarm_decodes_into_the_binding_record() {
        let core = cua_spaces::prewarm::Prewarm {
            id: "pw-abc123".into(),
            image: "ghcr.io/trycua/linux:24.04-slim".into(),
            kind: "container".into(),
            cpus: 1,
            memory_mb: 512,
            count: 8,
            ready: 6,
            placements: vec![cua_spaces::prewarm::PrewarmPlacement {
                host: "aws-1".into(),
                machine: "m-1".into(),
                count: 8,
                ready: 6,
            }],
            expires_at: 1_800_000_000,
            new_hosts: vec!["aws-1".into()],
        };
        let p: Prewarm = parse(serde_json::to_value(&core).unwrap()).unwrap();
        assert_eq!((p.count, p.ready, p.memory_mb), (8, 6, 512));
        assert_eq!(p.placements[0].host, "aws-1");
        assert_eq!(p.new_hosts, vec!["aws-1".to_string()]);
        let a = PrewarmOptions {
            count: Some(8),
            on: Some("aws".into()),
            ttl_secs: Some(600),
            ..Default::default()
        }
        .args("linux");
        assert_eq!(a["count"], 8);
        assert_eq!(a["on"], "aws");
        assert_eq!(a["ttl_secs"], 600);
        assert!(a.get("kind").is_none());
    }

    /// The core's wire rows decode into the binding records (drift guard
    /// between cua-spaces / cua-cluster and every SDK language).
    #[test]
    fn the_core_plan_decodes_into_the_binding_records() {
        use cua_cluster::elastic as el;
        let plan = el::Plan {
            assignments: vec![
                el::Assignment {
                    group: "request-0".into(),
                    target: el::Target::Host {
                        name: "studio".into(),
                    },
                    count: 8,
                    sync: false,
                },
                el::Assignment {
                    group: "request-0".into(),
                    target: el::Target::New { index: 0 },
                    count: 24,
                    sync: true,
                },
            ],
            new_hosts: vec![el::NewHost {
                provider: "hetzner".into(),
                type_name: "cax41".into(),
                region: "fsn1".into(),
                usd_per_hour: 0.0684,
                boot_secs: 75,
                billed_secs: 3600,
                one_space: false,
                rtt_ms: 12,
            }],
            unplaced: vec![el::Unplaced {
                group: "request-0".into(),
                count: 2,
                reason: "cap".into(),
            }],
            added_usd_per_hour: 0.0684,
            projected_usd: 0.0684,
            lower_bound_usd: 0.05,
            solver: "exact".into(),
            ready_in_secs: 75,
            explain: vec!["8 x request-0 -> studio".into()],
            ..Default::default()
        };
        let host = cua_sandbox_core::byoc::CloudHost {
            name: "hetzner-1".into(),
            provider: "hetzner".into(),
            type_name: "cax41".into(),
            region: "fsn1".into(),
            state: "booting".into(),
            started: 10,
            usd_per_hour: 0.0684,
            billing_step_secs: 3600,
            billing_min_secs: 3600,
            ..Default::default()
        };
        let wire = json!({"plan": plan, "groups": [], "pool": {}, "added": [host]});
        let p = plan_of(wire).unwrap();
        assert_eq!(p.assignments[0].host, "studio");
        assert_eq!(p.assignments[1].new_index, Some(0));
        assert!(p.assignments[1].sync);
        assert_eq!(p.new_hosts[0].type_name, "cax41");
        assert_eq!(p.unplaced[0].reason, "cap");
        assert_eq!(p.solver, "exact");
        assert_eq!(p.added[0].name, "hetzner-1");
        assert!(p.json.contains("\"cax41\""));

        let d = vec![el::scale::Decision {
            host: "hetzner-1".into(),
            action: el::scale::Action::Remove {
                reason: "idle".into(),
                saves_usd: 0.07,
            },
        }];
        let d = decisions_of(serde_json::to_value(d).unwrap());
        assert_eq!(d[0].action, "remove");
        assert!((d[0].saves_usd - 0.07).abs() < 1e-9);

        let b = budget_of(
            json!({"policy": cua_spaces::elastic::ElasticPolicy::default(), "month_usd": 1.5, "running_usd_per_hour": 0.2, "hosts": 2}),
        );
        assert!(b.autoscale);
        assert_eq!(b.hosts, 2);
    }
}
