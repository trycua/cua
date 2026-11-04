//! Placing a batch of forks: whole groups on one host, worlds spread.
//!
//! A batch is made of groups. A group is the forks of one checkpoint (a
//! "world") that belong together, for example the 8 rollouts of one task.
//! Forks of one checkpoint on one host share its memory pages (copy on
//! write), so:
//!
//! - **A group goes to one host** whenever one has room for all of it, and
//!   is split over hosts only when none has.
//! - **Groups of the same world go to the same host** when it has room (the
//!   world affinity term): the checkpoint is copied and paged in there once.
//! - **Groups of different worlds spread** over the least loaded hosts
//!   ([`Policy::Spread`], the score of [`crate::placement`]), so a host
//!   failure takes out few worlds and every host's CPU is used.
//! - **A host that already holds the checkpoint** is preferred over one that
//!   would need a copy first.
//! - **A checkpoint restores only where its ABI is** (`qemu/hvf/aarch64` on
//!   a Mac, `qemu/kvm/aarch64` on an arm64 Linux host): a host that does
//!   not list it in [`pb::HostNodeStatus::snapshot_abis`] never gets it.
//!
//! Memory is counted the way copy-on-write forks use it: a world's shared
//! part ([`BatchGroup::shared_memory_mb`]) once per host it lands on, plus
//! each fork's own part ([`Need::memory_mb`]). What hosts report free (and
//! refuse, when admission says no) is theirs: this plan is a first choice;
//! a host that refuses a fork has it placed again by the caller.

use std::collections::{BTreeMap, BTreeSet};

use crate::placement::{Candidate, Need, Policy};
#[cfg(test)]
use cua_proto::env::v1 as pb;

/// The score bonus of a host that already holds the group's checkpoint (no
/// copy before the forks start): it wins while it is at most this share of
/// its capacity busier than the best other host, so worlds still spread
/// when one machine baked them all.
pub const HOLDS_SNAPSHOT_BONUS: f64 = 0.1;
/// The score bonus of a host that already got forks of the same world in
/// this batch (they share its pages).
pub const SAME_WORLD_BONUS: f64 = 0.5;

/// One group of a batch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BatchGroup {
    /// The checkpoint its forks start from.
    pub world: String,
    /// How many forks.
    pub count: u32,
    /// What one fork needs (its own CPU and memory, kind, OS, ...).
    pub member: Need,
    /// Memory (MiB) the forks of this world share on a host, counted once
    /// per host it lands on (0: none counted).
    pub shared_memory_mb: u64,
    /// The checkpoint's ABI (empty: any host).
    pub abi: String,
}

/// Part of a group placed on one host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupPart {
    /// The group (index into the batch's groups).
    pub group: usize,
    /// The host (index into the candidates).
    pub host: usize,
    /// How many of the group's forks go there.
    pub count: u32,
    /// The host does not hold the checkpoint yet.
    pub needs_sync: bool,
    /// Why there, for people.
    pub reason: String,
}

/// Forks that fit nowhere.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unplaced {
    /// The group.
    pub group: usize,
    /// How many of its forks.
    pub count: u32,
    /// Why, for people: each host's reason.
    pub reason: String,
}

/// Where a batch goes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BatchPlan {
    /// Parts, in the order they were placed (largest groups first).
    pub parts: Vec<GroupPart>,
    /// Forks that fit on no host.
    pub unplaced: Vec<Unplaced>,
}

impl BatchPlan {
    /// The forks of `group` placed on `host`.
    pub fn count_on(&self, group: usize, host: usize) -> u32 {
        self.parts
            .iter()
            .filter(|p| p.group == group && p.host == host)
            .map(|p| p.count)
            .sum()
    }

    /// The hosts `group` went to.
    pub fn hosts_of(&self, group: usize) -> BTreeSet<usize> {
        self.parts
            .iter()
            .filter(|p| p.group == group)
            .map(|p| p.host)
            .collect()
    }
}

/// Whether host `c` can restore a checkpoint of ABI `abi`.
pub fn restores(c: &Candidate, abi: &str) -> bool {
    abi.is_empty()
        || c.status
            .as_ref()
            .is_some_and(|s| s.snapshot_abis.iter().any(|a| a == abi))
}

/// Whether host `c` holds checkpoint `world`.
pub fn holds(c: &Candidate, world: &str) -> bool {
    c.status
        .as_ref()
        .is_some_and(|s| s.snapshots.contains_key(world))
}

/// Why `g`'s forks cannot go to `c` at all, if they cannot.
fn misfit(c: &Candidate, g: &BatchGroup) -> Option<String> {
    if !restores(c, &g.abi) {
        return Some(format!(
            "{} cannot restore {} checkpoints",
            c.name,
            if g.abi.is_empty() { "these" } else { &g.abi }
        ));
    }
    // CPU is not a limit here: forks of a world are mostly idle, and what a
    // host will admit is its own call (it refuses a fork it cannot take and
    // the caller places that fork again). CPU still steers the score
    // ([`free_after`]), so busy hosts are the last choice.
    // (Forks already planned here count against `free_after`, not against
    // this check, so a long batch does not run the host "out of CPU".)
    let mut idle = c.clone();
    idle.pending_millicpus = 0;
    idle.misfit(&Need {
        millicpus: 0,
        ..g.member.clone()
    })
}

/// How many of `g`'s forks fit on `c` now (`shared` is whether the world's
/// shared memory is still to be counted there), at most `want`.
fn room(c: &Candidate, g: &BatchGroup, shared: bool, want: u32) -> u32 {
    if misfit(c, g).is_some() {
        return 0;
    }
    let Some(s) = &c.status else {
        // A host that does not report capacity: as much as asked.
        return want;
    };
    // Memory the way placement counts it (measured VM memory on a host
    // that measures its VMs, else unreserved memory).
    let mut mem = c.free_memory_for(&g.member);
    if shared {
        mem -= g.shared_memory_mb as i64;
    }
    let by_mem = (mem.max(0) as u64)
        .checked_div(c.memory_need(&g.member))
        .unwrap_or(u64::MAX);
    let by_jobs = if s.jobs_max == 0 {
        u64::MAX
    } else {
        u64::from(s.jobs_max.saturating_sub(s.jobs_running + c.pending_jobs))
    };
    by_mem.min(by_jobs).min(u64::from(want)) as u32
}

/// The share of `c` left free after `n` of `g`'s forks (0..1).
fn free_after(c: &Candidate, g: &BatchGroup, n: u32, shared: bool) -> f64 {
    let Some(s) = &c.status else {
        return 0.0;
    };
    let cpu = if s.millicpus_total == 0 {
        0.0
    } else {
        (c.free_millicpus() - i64::from(c.cpu_need(&g.member)) * i64::from(n)) as f64
            / f64::from(s.millicpus_total)
    };
    let shared_mb = if shared { g.shared_memory_mb } else { 0 };
    let mem = if s.memory_total_mb == 0 {
        0.0
    } else {
        (c.free_memory_for(&g.member)
            - (c.memory_need(&g.member) * u64::from(n) + shared_mb) as i64) as f64
            / s.memory_total_mb as f64
    };
    cpu.min(mem).clamp(0.0, 1.0)
}

/// Places a batch on `cands` (which are not changed). Groups are placed
/// largest first; each goes whole to the best host with room for it (see
/// the module docs for the score), else is split over the hosts with the
/// most room. Forks that fit nowhere are [`BatchPlan::unplaced`].
pub fn plan_batch(cands: &[Candidate], groups: &[BatchGroup], policy: Policy) -> BatchPlan {
    let mut hosts: Vec<Candidate> = cands.to_vec();
    // Worlds each host has forks of in this plan (their shared memory is
    // counted there already).
    let mut placed_worlds: Vec<BTreeSet<String>> = vec![BTreeSet::new(); hosts.len()];
    let mut order: Vec<usize> = (0..groups.len()).collect();
    order.sort_by(|&a, &b| {
        let size = |g: &BatchGroup| u64::from(g.count) * g.member.memory_mb.max(1);
        size(&groups[b])
            .cmp(&size(&groups[a]))
            .then_with(|| a.cmp(&b))
    });
    let mut plan = BatchPlan::default();
    for gi in order {
        let g = &groups[gi];
        let mut left = g.count;
        while left > 0 {
            let score = |i: usize, n: u32| -> Option<f64> {
                let c = &hosts[i];
                let shared = !placed_worlds[i].contains(&g.world);
                if room(c, g, shared, n) < n {
                    return None;
                }
                if c.status.is_none() {
                    return Some(-1.0);
                }
                // Placement's own score (policy, memory pressure, warm,
                // elastic), plus the batch's two affinity terms.
                let mut s = c.policy_score(free_after(c, g, n, shared), &g.member, policy);
                if holds(c, &g.world) {
                    s += HOLDS_SNAPSHOT_BONUS;
                }
                if !shared {
                    s += SAME_WORLD_BONUS;
                }
                Some(s)
            };
            let best = |n: u32| -> Option<usize> {
                let mut best: Option<(usize, f64)> = None;
                for i in 0..hosts.len() {
                    let Some(s) = score(i, n) else { continue };
                    let better = match best {
                        None => true,
                        Some((b, bs)) => s > bs || (s == bs && hosts[i].name < hosts[b].name),
                    };
                    if better {
                        best = Some((i, s));
                    }
                }
                best.map(|(i, _)| i)
            };
            // The whole rest of the group on one host, else the host with
            // the most room takes what fits.
            let (host, n, whole) = match best(left) {
                Some(i) => (i, left, true),
                None => {
                    let most = (0..hosts.len())
                        .map(|i| {
                            let shared = !placed_worlds[i].contains(&g.world);
                            (i, room(&hosts[i], g, shared, left))
                        })
                        .filter(|(_, r)| *r > 0)
                        .max_by(|a, b| {
                            a.1.cmp(&b.1)
                                .then_with(|| hosts[b.0].name.cmp(&hosts[a.0].name))
                        });
                    match most {
                        Some((i, r)) => (i, r, false),
                        None => {
                            let why = if hosts.is_empty() {
                                "there are no hosts".to_string()
                            } else {
                                hosts
                                    .iter()
                                    .map(|c| {
                                        misfit(c, g).unwrap_or_else(|| {
                                            format!("{} has no room left", c.name)
                                        })
                                    })
                                    .collect::<Vec<_>>()
                                    .join("; ")
                            };
                            plan.unplaced.push(Unplaced {
                                group: gi,
                                count: left,
                                reason: why,
                            });
                            break;
                        }
                    }
                }
            };
            let shared = !placed_worlds[host].contains(&g.world);
            let held = holds(&hosts[host], &g.world);
            let reason = match (whole, held, shared) {
                (false, _, _) => format!("split: {} had room for {n}", hosts[host].name),
                (true, true, _) => format!("{} holds {}", hosts[host].name, g.world),
                (true, false, false) => {
                    format!("{} runs other forks of {}", hosts[host].name, g.world)
                }
                (true, false, true) => format!("{} is the least loaded", hosts[host].name),
            };
            let c = &mut hosts[host];
            // Pending in the units placement compares against (a VM's
            // vCPUs by the host's overcommit, its measured memory).
            c.pending_millicpus += u64::from(c.cpu_need(&g.member)) * u64::from(n);
            c.pending_memory_mb += c.memory_need(&g.member) * u64::from(n)
                + if shared { g.shared_memory_mb } else { 0 };
            c.pending_jobs += n;
            placed_worlds[host].insert(g.world.clone());
            plan.parts.push(GroupPart {
                group: gi,
                host,
                count: n,
                needs_sync: !held,
                reason,
            });
            left -= n;
        }
    }
    plan
}

/// Groups of a batch from one checkpoint: `count` forks in groups of
/// `group_size` (0: one group).
pub fn split_into_groups(count: u32, group_size: u32) -> Vec<u32> {
    if count == 0 {
        return vec![];
    }
    let size = if group_size == 0 { count } else { group_size };
    let mut out = vec![size; (count / size) as usize];
    if !count.is_multiple_of(size) {
        out.push(count % size);
    }
    out
}

/// Per host, how many forks a plan puts there.
pub fn per_host(plan: &BatchPlan, cands: &[Candidate]) -> BTreeMap<String, u32> {
    let mut out = BTreeMap::new();
    for p in &plan.parts {
        *out.entry(cands[p.host].name.clone()).or_insert(0) += p.count;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const HVF: &str = "qemu/hvf/aarch64";
    const KVM: &str = "qemu/kvm/aarch64";

    fn host(name: &str, cpus: u32, mem_gb: u64, abi: &str) -> Candidate {
        Candidate {
            name: name.into(),
            ready: true,
            os: "macos".into(),
            status: Some(pb::HostNodeStatus {
                millicpus_total: cpus * 1000,
                millicpus_free: cpus * 1000,
                memory_total_mb: mem_gb * 1024,
                memory_free_mb: mem_gb * 1024,
                kinds: vec!["vm".into()],
                runtimes: vec!["qemu".into()],
                snapshot_abis: vec![abi.into()],
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn holding(mut c: Candidate, worlds: &[&str]) -> Candidate {
        for w in worlds {
            c.status
                .as_mut()
                .unwrap()
                .snapshots
                .insert((*w).into(), format!("digest-{w}"));
        }
        c
    }

    fn group(world: &str, count: u32) -> BatchGroup {
        BatchGroup {
            world: world.into(),
            count,
            member: Need {
                kind: "vm".into(),
                millicpus: 500,
                memory_mb: 1024,
                ..Default::default()
            },
            shared_memory_mb: 2048,
            abi: HVF.into(),
        }
    }

    fn measured(
        mut c: Candidate,
        vm_free_gb: u64,
        typical_mb: u64,
        overcommit: f32,
        pressure: u32,
    ) -> Candidate {
        c.status.as_mut().unwrap().measured = Some(pb::HostMeasured {
            memory_available_mb: vm_free_gb * 1024,
            memory_pressure: pressure,
            vcpu_overcommit: overcommit,
            vm_memory_known: true,
            vm_memory_free_mb: vm_free_gb * 1024,
            vm_typical_mb: typical_mb,
        });
        c
    }

    /// Batch placement scores the way single placement does (one model): a
    /// host under memory pressure goes after the others, and a host that
    /// measures its VMs is filled by what a VM there uses, not by guest RAM.
    #[test]
    fn batch_uses_placement_measured_capacity_and_pressure() {
        // Same size; mac-a is under memory pressure.
        let hosts = vec![
            measured(host("mac-a", 24, 128, HVF), 100, 300, 8.0, 1),
            measured(host("mac-b", 24, 128, HVF), 100, 300, 8.0, 0),
        ];
        let plan = plan_batch(&hosts, &[group("w0", 8)], Policy::Spread);
        assert_eq!(per_host(&plan, &hosts)["mac-b"], 8, "{plan:?}");

        // 8 GiB of measured VM memory with forks using ~300 MiB each: far
        // more than 8 GiB / 1 GiB of guest RAM fit.
        let small = vec![measured(host("mac-c", 24, 128, HVF), 8, 300, 8.0, 0)];
        let plan = plan_batch(&small, &[group("w1", 16)], Policy::Spread);
        assert!(plan.unplaced.is_empty(), "{plan:?}");
        assert_eq!(per_host(&plan, &small)["mac-c"], 16);
        // And the same host without measurements holds only what guest RAM
        // allows (8 GiB - 2 GiB shared) / 1 GiB = 6.
        let mut raw = host("mac-d", 24, 8, HVF);
        raw.status.as_mut().unwrap().memory_free_mb = 8 * 1024;
        let plan = plan_batch(&[raw.clone()], &[group("w2", 16)], Policy::Spread);
        assert_eq!(per_host(&plan, &[raw])["mac-d"], 6, "{plan:?}");
    }

    /// 8 worlds of 8 forks on two equal Macs: every world whole on one
    /// Mac, and the worlds split 4 and 4.
    #[test]
    fn groups_stay_whole_and_worlds_spread() {
        let hosts = vec![host("mac-a", 24, 128, HVF), host("mac-b", 24, 128, HVF)];
        let groups: Vec<BatchGroup> = (0..8).map(|w| group(&format!("w{w}"), 8)).collect();
        let plan = plan_batch(&hosts, &groups, Policy::Spread);
        assert!(plan.unplaced.is_empty(), "{plan:?}");
        for g in 0..8 {
            assert_eq!(plan.hosts_of(g).len(), 1, "group {g} split: {plan:?}");
        }
        let per = per_host(&plan, &hosts);
        assert_eq!(per["mac-a"], 32);
        assert_eq!(per["mac-b"], 32);
    }

    /// Two groups of the same world share a host (its pages, one copy of
    /// the checkpoint) while it has room; a different world goes to the
    /// other, idle host.
    #[test]
    fn groups_of_one_world_stay_together() {
        let hosts = vec![host("mac-a", 24, 128, HVF), host("mac-b", 24, 128, HVF)];
        let groups = vec![group("w0", 8), group("w0", 8), group("w1", 8)];
        let plan = plan_batch(&hosts, &groups, Policy::Spread);
        assert_eq!(plan.hosts_of(0), plan.hosts_of(1));
        assert_ne!(plan.hosts_of(0), plan.hosts_of(2));
        // The second group of w0 is there because of its sibling.
        let second = plan.parts.iter().find(|p| p.group == 1).unwrap();
        assert!(
            second.reason.contains("runs other forks of w0"),
            "{second:?}"
        );
    }

    /// The world's shared memory is counted once per host: 8 forks of 1 GiB
    /// own memory plus a 2 GiB world fit in 10 GiB, not in 9.
    #[test]
    fn shared_memory_is_counted_once_per_host() {
        let ten = host("ten", 64, 10, HVF);
        let plan = plan_batch(&[ten], &[group("w0", 8)], Policy::Spread);
        assert!(plan.unplaced.is_empty(), "{plan:?}");
        let nine = host("nine", 64, 9, HVF);
        let plan = plan_batch(&[nine], &[group("w0", 8)], Policy::Spread);
        // 7 fit (7 + 2 = 9 GiB); one does not.
        assert_eq!(plan.count_on(0, 0), 7);
        assert_eq!(plan.unplaced.len(), 1);
        assert_eq!(plan.unplaced[0].count, 1);
        assert!(
            plan.unplaced[0]
                .reason
                .contains("nine has not enough memory free"),
            "{plan:?}"
        );
    }

    /// A host that holds the checkpoint wins over a slightly freer one that
    /// would need a copy, but not over a much freer one: worlds still
    /// spread when one machine baked them all.
    #[test]
    fn a_host_holding_the_checkpoint_wins_when_loads_are_close() {
        let mut holder = holding(host("holder", 24, 128, HVF), &["w0"]);
        holder.status.as_mut().unwrap().memory_free_mb = 120 * 1024;
        let idle = host("idle", 24, 128, HVF);
        let plan = plan_batch(
            &[idle.clone(), holder.clone()],
            &[group("w0", 8)],
            Policy::Spread,
        );
        assert_eq!(plan.parts.len(), 1);
        assert_eq!(plan.parts[0].host, 1);
        assert!(!plan.parts[0].needs_sync);
        assert!(plan.parts[0].reason.contains("holds w0"));
        // Much busier: the idle host, with a copy first.
        holder.status.as_mut().unwrap().memory_free_mb = 64 * 1024;
        let plan = plan_batch(&[idle, holder], &[group("w0", 8)], Policy::Spread);
        assert_eq!(plan.parts[0].host, 0);
        assert!(plan.parts[0].needs_sync);
        // One machine baked every world: the worlds still spread.
        let worlds: Vec<String> = (0..8).map(|i| format!("w{i}")).collect();
        let wr: Vec<&str> = worlds.iter().map(String::as_str).collect();
        let baker = holding(host("baker", 24, 128, HVF), &wr);
        let other = host("other", 24, 128, HVF);
        let groups: Vec<BatchGroup> = worlds.iter().map(|w| group(w, 8)).collect();
        let plan = plan_batch(&[baker, other], &groups, Policy::Spread);
        let per = per_host(&plan, &[host("baker", 1, 1, HVF), host("other", 1, 1, HVF)]);
        assert_eq!((per["baker"], per["other"]), (32, 32), "{plan:?}");
    }

    /// A checkpoint goes only where its ABI restores: a Mac's checkpoint
    /// never to a Linux KVM host, whatever its room.
    #[test]
    fn checkpoints_go_only_where_their_abi_restores() {
        let linux = host("linux", 64, 256, KVM);
        let mac = host("mac", 8, 32, HVF);
        let plan = plan_batch(&[linux.clone(), mac], &[group("w0", 8)], Policy::Spread);
        assert_eq!(plan.hosts_of(0), BTreeSet::from([1]));
        let plan = plan_batch(&[linux], &[group("w0", 8)], Policy::Spread);
        assert_eq!(plan.unplaced.len(), 1);
        assert!(
            plan.unplaced[0]
                .reason
                .contains("linux cannot restore qemu/hvf/aarch64 checkpoints"),
            "{plan:?}"
        );
        // A host that does not say what it restores gets none.
        let mut old = host("old", 64, 256, HVF);
        old.status.as_mut().unwrap().snapshot_abis.clear();
        assert!(!restores(&old, HVF));
        assert!(restores(&old, ""));
    }

    /// A group no host has room for is split, the host with the most room
    /// first, and what fits nowhere is reported.
    #[test]
    fn a_group_too_big_for_one_host_is_split() {
        let a = host("a", 64, 8, HVF); // 6 forks (2 GiB shared)
        let b = host("b", 64, 6, HVF); // 4 forks
        let plan = plan_batch(&[a, b], &[group("w0", 12)], Policy::Spread);
        assert_eq!(plan.count_on(0, 0), 6);
        assert_eq!(plan.count_on(0, 1), 4);
        assert!(plan.parts.iter().all(|p| p.reason.starts_with("split")));
        assert_eq!(plan.unplaced.iter().map(|u| u.count).sum::<u32>(), 2);
    }

    /// CPU does not limit a batch (forks of a world are mostly idle): a
    /// host with 4 CPUs takes 9 half-CPU forks, but a busy one is the last
    /// choice, and memory still bounds the room.
    #[test]
    fn cpu_steers_but_does_not_bound_the_room() {
        let small = host("small", 4, 128, HVF);
        let plan = plan_batch(
            std::slice::from_ref(&small),
            &[group("w0", 9)],
            Policy::Spread,
        );
        assert_eq!(plan.count_on(0, 0), 9);
        let mut busy = host("busy", 4, 128, HVF);
        busy.pending_millicpus = 2000;
        let idle = host("idle", 4, 128, HVF);
        let plan = plan_batch(&[busy, idle], &[group("w0", 4)], Policy::Spread);
        assert_eq!(plan.hosts_of(0), BTreeSet::from([1]));
    }

    /// A long batch does not run a host "out of CPU": the forks planned
    /// there first do not make the later groups unplaceable.
    #[test]
    fn many_groups_on_one_small_host_all_place() {
        let small = host("small", 4, 512, HVF);
        let groups: Vec<_> = (0..4).map(|i| group(&format!("w{i}"), 16)).collect();
        let plan = plan_batch(&[small], &groups, Policy::Spread);
        assert!(plan.unplaced.is_empty(), "{:?}", plan.unplaced);
        assert_eq!(plan.parts.iter().map(|p| p.count).sum::<u32>(), 64);
    }

    /// Hosts that are not ready, draining or full are skipped with their
    /// reason; candidates themselves are not changed.
    #[test]
    fn unready_hosts_are_skipped_and_candidates_untouched() {
        let mut down = host("down", 24, 128, HVF);
        down.ready = false;
        let mut drain = host("drain", 24, 128, HVF);
        drain.status.as_mut().unwrap().draining = true;
        let ok = host("ok", 24, 128, HVF);
        let cands = vec![down, drain, ok];
        let before = format!("{cands:?}");
        let plan = plan_batch(&cands, &[group("w0", 8)], Policy::Spread);
        assert_eq!(plan.hosts_of(0), BTreeSet::from([2]));
        assert_eq!(before, format!("{cands:?}"));
        let plan = plan_batch(&cands[..2], &[group("w0", 8)], Policy::Spread);
        let why = &plan.unplaced[0].reason;
        assert!(
            why.contains("down is not ready") && why.contains("drain is draining"),
            "{why}"
        );
    }

    #[test]
    fn groups_from_a_count() {
        assert_eq!(split_into_groups(64, 8), vec![8; 8]);
        assert_eq!(split_into_groups(10, 4), vec![4, 4, 2]);
        assert_eq!(split_into_groups(5, 0), vec![5]);
        assert!(split_into_groups(0, 8).is_empty());
    }

    /// The plan is the same every time for the same input (ties by name).
    #[test]
    fn plans_are_deterministic() {
        let hosts = vec![host("b", 24, 128, HVF), host("a", 24, 128, HVF)];
        let groups: Vec<BatchGroup> = (0..5).map(|w| group(&format!("w{w}"), 8)).collect();
        let one = plan_batch(&hosts, &groups, Policy::Spread);
        for _ in 0..10 {
            assert_eq!(plan_batch(&hosts, &groups, Policy::Spread), one);
        }
        // The first (largest-first, then index) group goes to "a" on a tie.
        assert_eq!(one.parts[0].host, 1);
    }
}
