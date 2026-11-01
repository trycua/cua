//! A plan: the ordered, timed list of client operations and injected faults
//! of one chaos run, generated from a seed and a [`Profile`]. The plan is
//! plain JSON, so a failure replays from `(profile, seed)` or from the saved
//! plan file, and any Spaces client can execute it with its own driver.

use crate::rng::Rng;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// What a step is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StepKind {
    /// A client operation, executed by the driver.
    Op,
    /// A fault, executed by the injector.
    Fault,
}

/// One timed step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Step {
    /// Milliseconds after the run starts.
    pub at_ms: u64,
    /// The client (lane) an operation belongs to; faults use lane 0.
    pub actor: u32,
    pub kind: StepKind,
    pub name: String,
    #[serde(default)]
    pub args: BTreeMap<String, Value>,
}

/// A generated run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub seed: u64,
    pub profile: String,
    pub steps: Vec<Step>,
}

/// How an argument of an operation or fault is drawn.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ArgSpec {
    pub name: String,
    /// Pick one of these.
    #[serde(default)]
    pub choices: Vec<Value>,
    /// Or an integer in `lo..=hi`.
    #[serde(default)]
    pub lo: Option<i64>,
    #[serde(default)]
    pub hi: Option<i64>,
    /// Probability (per mille) that the argument is present; default always.
    #[serde(default)]
    pub permille: Option<u32>,
}

/// A weighted operation or fault of a profile.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Choice {
    pub name: String,
    #[serde(default = "one")]
    pub weight: u32,
    #[serde(default)]
    pub args: Vec<ArgSpec>,
    /// Faults only: at most this many per run (0: unlimited).
    #[serde(default)]
    pub max: u32,
    /// Faults only: an extra step `heal` follows after this many
    /// milliseconds (a partition and its repair, a gateway kill and its
    /// restart). Empty: none.
    #[serde(default)]
    pub heal: String,
    #[serde(default)]
    pub heal_after_ms: u64,
}

fn one() -> u32 {
    1
}

/// What a run draws from.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    /// Concurrent clients.
    pub actors: u32,
    /// Operations per client.
    pub ops_per_actor: u32,
    /// Gap between a client's operations, milliseconds.
    pub gap_ms: (u64, u64),
    /// Faults injected over the run.
    pub faults: u32,
    /// Faults land in `0..duration_ms`.
    pub duration_ms: u64,
    pub ops: Vec<Choice>,
    pub fault_menu: Vec<Choice>,
}

fn draw(rng: &mut Rng, specs: &[ArgSpec]) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    for s in specs {
        // Always draw, so presence never shifts later draws.
        let present = s.permille.is_none_or(|p| rng.chance(p));
        let v = if !s.choices.is_empty() {
            rng.pick(&s.choices).cloned().unwrap_or(Value::Null)
        } else {
            Value::from(rng.range(s.lo.unwrap_or(0), s.hi.unwrap_or(s.lo.unwrap_or(0))))
        };
        if present {
            out.insert(s.name.clone(), v);
        }
    }
    out
}

impl Profile {
    /// Generates the plan for `seed`. Same profile and seed: same plan.
    pub fn plan(&self, seed: u64) -> Plan {
        let root = Rng::new(seed);
        let mut steps = Vec::new();
        let weights: Vec<u32> = self.ops.iter().map(|o| o.weight).collect();
        for actor in 1..=self.actors {
            let mut r = root.fork(&format!("actor-{actor}"));
            let mut at = r.range(0, (self.gap_ms.1 as i64).max(1)) as u64;
            for _ in 0..self.ops_per_actor {
                let Some(i) = r.weighted(&weights) else { break };
                let op = &self.ops[i];
                steps.push(Step {
                    at_ms: at,
                    actor,
                    kind: StepKind::Op,
                    name: op.name.clone(),
                    args: draw(&mut r, &op.args),
                });
                at += r.range(self.gap_ms.0 as i64, self.gap_ms.1 as i64) as u64;
            }
        }
        let mut fr = root.fork("faults");
        let fweights: Vec<u32> = self.fault_menu.iter().map(|o| o.weight).collect();
        let mut used: BTreeMap<String, u32> = BTreeMap::new();
        for _ in 0..self.faults {
            let Some(i) = fr.weighted(&fweights) else {
                break;
            };
            let f = &self.fault_menu[i];
            let n = used.entry(f.name.clone()).or_default();
            // Draw the time first so a refused fault does not shift the rest.
            let at = fr.below(self.duration_ms.max(1));
            let args = draw(&mut fr, &f.args);
            if f.max > 0 && *n >= f.max {
                continue;
            }
            *n += 1;
            steps.push(Step {
                at_ms: at,
                actor: 0,
                kind: StepKind::Fault,
                name: f.name.clone(),
                args: args.clone(),
            });
            if !f.heal.is_empty() {
                steps.push(Step {
                    at_ms: at + f.heal_after_ms,
                    actor: 0,
                    kind: StepKind::Fault,
                    name: f.heal.clone(),
                    args,
                });
            }
        }
        steps.sort_by(|a, b| {
            (a.at_ms, a.actor, a.kind as u8, &a.name).cmp(&(
                b.at_ms,
                b.actor,
                b.kind as u8,
                &b.name,
            ))
        });
        Plan {
            seed,
            profile: self.name.clone(),
            steps,
        }
    }
}

impl Plan {
    /// A stable fingerprint, to print next to the seed in reports.
    pub fn digest(&self) -> String {
        let mut h: u64 = 0xCBF2_9CE4_8422_2325;
        for b in serde_json::to_vec(self).unwrap_or_default() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01B3);
        }
        format!("{h:016x}")
    }

    pub fn from_json(s: &str) -> Result<Plan, serde_json::Error> {
        serde_json::from_str(s)
    }
}

fn op(name: &str, weight: u32, args: Vec<ArgSpec>) -> Choice {
    Choice {
        name: name.into(),
        weight,
        args,
        ..Default::default()
    }
}

fn slot(n: i64) -> ArgSpec {
    ArgSpec {
        name: "slot".into(),
        lo: Some(0),
        hi: Some(n - 1),
        ..Default::default()
    }
}

fn ints(name: &str, lo: i64, hi: i64, permille: Option<u32>) -> ArgSpec {
    ArgSpec {
        name: name.into(),
        lo: Some(lo),
        hi: Some(hi),
        permille,
        ..Default::default()
    }
}

fn fault(name: &str, weight: u32, max: u32, heal: &str, heal_after_ms: u64) -> Choice {
    Choice {
        name: name.into(),
        weight,
        max,
        heal: heal.into(),
        heal_after_ms,
        ..Default::default()
    }
}

impl Profile {
    /// The built-in profiles. `sandbox` is the lifecycle a provider SDK
    /// client drives (create, exec, upload, stream, delete, races);
    /// `burst` is 64+ parallel creates with random cancels; `faults` adds
    /// every injected failure to the sandbox lifecycle.
    pub fn preset(name: &str) -> Option<Profile> {
        let ops = |create_w: u32| {
            vec![
                op(
                    "create",
                    create_w,
                    vec![
                        slot(8),
                        ints("timeout_s", 5, 40, None),
                        ints("kill_after_ms", 50, 4000, Some(120)),
                        ints("abandon_after_ms", 50, 3000, Some(120)),
                    ],
                ),
                op(
                    "exec",
                    4,
                    vec![
                        slot(8),
                        ints("seconds", 0, 6, None),
                        ints("kill_after_ms", 50, 3000, Some(150)),
                    ],
                ),
                op(
                    "upload",
                    2,
                    vec![
                        slot(8),
                        ints("kib", 1, 4096, None),
                        ints("kill_after_ms", 20, 2000, Some(150)),
                    ],
                ),
                op(
                    "stream",
                    2,
                    vec![
                        slot(8),
                        ints("seconds", 1, 8, None),
                        ints("kill_after_ms", 50, 3000, Some(200)),
                    ],
                ),
                op("delete", 3, vec![slot(8)]),
                op("double_delete", 1, vec![slot(8)]),
                op("delete_during_create", 1, vec![slot(8)]),
                op(
                    "ttl_exec",
                    1,
                    vec![ints("timeout_s", 2, 6, None), ints("seconds", 4, 12, None)],
                ),
            ]
        };
        let faults = vec![
            fault("gateway_kill", 3, 0, "gateway_start", 1500),
            fault("daemon_kill", 3, 0, "daemon_start", 1500),
            fault("spacesd_kill", 2, 0, "", 0),
            fault("partition", 2, 0, "partition_heal", 3000),
            fault("disk_full", 1, 0, "disk_free", 3000),
            fault("oom", 1, 0, "", 0),
            fault("host_loss", 1, 1, "", 0),
        ];
        match name {
            "sandbox" => Some(Profile {
                name: name.into(),
                actors: 4,
                ops_per_actor: 12,
                gap_ms: (50, 1500),
                faults: 0,
                duration_ms: 20_000,
                ops: ops(4),
                fault_menu: faults,
            }),
            // Faults a host can inject without reaching into a Space.
            "host" => Some(Profile {
                name: name.into(),
                actors: 4,
                ops_per_actor: 10,
                gap_ms: (50, 1500),
                faults: 4,
                duration_ms: 20_000,
                ops: ops(4),
                fault_menu: faults
                    .into_iter()
                    .filter(|f| {
                        ["gateway_kill", "daemon_kill", "partition"].contains(&f.name.as_str())
                    })
                    .collect(),
            }),
            // The Cua daemon dying and coming back, over and over: Spaces
            // (VMs, containers) must be adopted or reaped, never orphaned.
            "daemon" => Some(Profile {
                name: name.into(),
                actors: 3,
                ops_per_actor: 8,
                gap_ms: (50, 1500),
                faults: 4,
                duration_ms: 20_000,
                ops: ops(4),
                fault_menu: faults
                    .into_iter()
                    .filter(|f| f.name == "daemon_kill")
                    .collect(),
            }),
            // The machine that provides the Spaces disappears mid-batch.
            "hostloss" => Some(Profile {
                name: name.into(),
                actors: 4,
                ops_per_actor: 8,
                gap_ms: (200, 2500),
                faults: 1,
                duration_ms: 12_000,
                ops: ops(4),
                fault_menu: faults
                    .into_iter()
                    .filter(|f| f.name == "host_loss")
                    .map(|mut f| {
                        f.weight = 1;
                        f
                    })
                    .collect(),
            }),
            // Modal: an ephemeral App's client dies mid-run (its heartbeats
            // stop) and the App's Sandboxes must end.
            "modal" => Some(Profile {
                name: name.into(),
                actors: 4,
                ops_per_actor: 10,
                gap_ms: (50, 1500),
                faults: 3,
                duration_ms: 20_000,
                ops: {
                    let mut o = ops(4);
                    o.push(op(
                        "create_ephemeral",
                        3,
                        vec![
                            ints("seconds", 5, 20, None),
                            ints("kill_after_ms", 2000, 9000, Some(700)),
                        ],
                    ));
                    o
                },
                fault_menu: faults
                    .into_iter()
                    .filter(|f| {
                        ["gateway_kill", "daemon_kill", "partition"].contains(&f.name.as_str())
                    })
                    .collect(),
            }),
            "faults" => Some(Profile {
                name: name.into(),
                actors: 4,
                ops_per_actor: 14,
                gap_ms: (50, 1500),
                faults: 5,
                duration_ms: 25_000,
                ops: ops(4),
                fault_menu: faults,
            }),
            "burst" => Some(Profile {
                name: name.into(),
                actors: 64,
                ops_per_actor: 2,
                gap_ms: (0, 400),
                faults: 1,
                duration_ms: 10_000,
                ops: vec![
                    op(
                        "create",
                        6,
                        vec![
                            slot(1),
                            ints("timeout_s", 20, 60, None),
                            ints("kill_after_ms", 50, 6000, Some(250)),
                            ints("abandon_after_ms", 50, 6000, Some(250)),
                        ],
                    ),
                    op("delete", 3, vec![slot(1)]),
                    op("delete_during_create", 1, vec![slot(1)]),
                ],
                fault_menu: faults,
            }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_seed_is_the_plan() {
        let p = Profile::preset("faults").unwrap();
        let a = p.plan(1234);
        let b = p.plan(1234);
        assert_eq!(a, b);
        assert_eq!(a.digest(), b.digest());
        assert_ne!(a.digest(), p.plan(1235).digest());
        let json = serde_json::to_string(&a).unwrap();
        assert_eq!(Plan::from_json(&json).unwrap(), a);
    }

    #[test]
    fn steps_are_ordered_and_faults_heal() {
        let p = Profile::preset("faults").unwrap();
        for seed in 0..50 {
            let plan = p.plan(seed);
            assert!(plan.steps.windows(2).all(|w| w[0].at_ms <= w[1].at_ms));
            let faults: Vec<_> = plan
                .steps
                .iter()
                .filter(|s| s.kind == StepKind::Fault)
                .collect();
            for f in &faults {
                if f.name == "gateway_kill" {
                    assert!(
                        faults
                            .iter()
                            .any(|h| h.name == "gateway_start" && h.at_ms == f.at_ms + 1500)
                    );
                }
            }
            assert!(faults.iter().filter(|f| f.name == "host_loss").count() <= 1);
        }
    }

    #[test]
    fn burst_has_64_clients() {
        let plan = Profile::preset("burst").unwrap().plan(5);
        let actors: std::collections::BTreeSet<_> = plan
            .steps
            .iter()
            .filter(|s| s.kind == StepKind::Op)
            .map(|s| s.actor)
            .collect();
        assert_eq!(actors.len(), 64);
        assert!(plan.steps.iter().any(
            |s| s.args.contains_key("kill_after_ms") || s.args.contains_key("abandon_after_ms")
        ));
    }

    #[test]
    fn every_preset_exists() {
        for n in [
            "sandbox", "faults", "host", "daemon", "hostloss", "modal", "burst",
        ] {
            assert!(Profile::preset(n).is_some());
        }
        assert!(Profile::preset("nope").is_none());
    }
}
