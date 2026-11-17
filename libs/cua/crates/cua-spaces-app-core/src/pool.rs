// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Your computers as one pool: Home's total ("3 computers · 40 cores ·
//! 96 GB"), how much room each one has free now, and where a new job goes.
//!
//! Plain data in: one row per computer, from the account's computers
//! (`machines.list`) and the numbers each one measured
//! (`resources.get`'s `ComputerResources`). Plain data out: counts, bytes
//! and a reason code per computer. Every app (the web UI in the Mac and
//! Electron apps, later the phone and the CLI) asks this module, so the
//! same job lands on the same computer whichever app starts it.
//!
//! Placement is "most free room first", the spread rule schedulers use
//! (Kubernetes' LeastAllocated score, Nomad's spread): of the computers
//! that can take a job now, the one with the most free cores, then the
//! most free memory, then the fewest jobs. The computer you're working on
//! goes last, so agents don't slow you down; it is still used when it is
//! the only one. A computer that didn't send numbers is placed after the
//! ones that did. A computer you pick is used when it can take the job;
//! when it can't (offline, or its owner stopped sharing it) the job moves
//! to the best other one, and the result says it moved and from where.

use serde::{Deserialize, Serialize};

/// One computer, as the pool sees it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PoolComputer {
    /// The account's id for it (`machines.list`).
    pub id: String,
    /// The computer you're using now.
    pub current: bool,
    /// Connected now.
    pub online: bool,
    /// Online but its owner stopped sharing it: nothing here can reach it.
    pub not_sharing: bool,
    /// Its numbers, when it sent any.
    pub resources: Option<PoolResources>,
    /// Jobs running on it now.
    pub jobs: u32,
}

/// What a computer measured (the fields of `ComputerResources` the pool needs).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PoolResources {
    pub cpu_count: u32,
    /// Busy now, 0 to 100.
    pub cpu_percent: f64,
    pub memory_total: u64,
    /// In use, without reclaimable cache.
    pub memory_used: u64,
}

/// How much of a computer is free now.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Free {
    /// Whole cores not busy now.
    pub cores: u32,
    pub memory: u64,
    /// The bar: the smaller of the free share of cores and of memory, 0 to 1.
    pub fraction: f64,
}

/// What a computer has free now; None when it sent no numbers or is
/// offline (an offline computer has nothing free to give).
pub fn free(c: &PoolComputer) -> Option<Free> {
    if !c.online {
        return None;
    }
    let r = c.resources?;
    if r.cpu_count == 0 && r.memory_total == 0 {
        return None;
    }
    let busy = (r.cpu_percent.clamp(0.0, 100.0)) / 100.0;
    let cores_free = (f64::from(r.cpu_count) * (1.0 - busy)).floor().max(0.0);
    let memory = r.memory_total.saturating_sub(r.memory_used);
    let cpu_share = if r.cpu_count > 0 { 1.0 - busy } else { 1.0 };
    let mem_share = if r.memory_total > 0 {
        memory as f64 / r.memory_total as f64
    } else {
        1.0
    };
    Some(Free {
        cores: cores_free as u32,
        memory,
        fraction: cpu_share.min(mem_share).clamp(0.0, 1.0),
    })
}

/// The pool's total, for Home's heading.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PoolSummary {
    /// Every computer, online or not.
    pub computers: u32,
    pub online: u32,
    /// Cores and memory of every computer that sent numbers (offline ones
    /// included: they are still yours).
    pub cores: u32,
    pub memory: u64,
    /// Free now, on the computers that can take a job.
    pub free_cores: u32,
    pub free_memory: u64,
    /// Computers whose numbers are unknown (an older app on them).
    pub unknown: u32,
}

/// Adds the computers up.
pub fn summary(computers: &[PoolComputer]) -> PoolSummary {
    let mut s = PoolSummary {
        computers: computers.len() as u32,
        ..PoolSummary::default()
    };
    for c in computers {
        if c.online {
            s.online += 1;
        }
        match c.resources {
            Some(r) if r.cpu_count > 0 || r.memory_total > 0 => {
                s.cores += r.cpu_count;
                s.memory += r.memory_total;
            }
            _ => s.unknown += 1,
        }
        if can_take(c)
            && let Some(f) = free(c)
        {
            s.free_cores += f.cores;
            s.free_memory += f.memory;
        }
    }
    s
}

/// Online and shared: it can take a job now.
pub fn can_take(c: &PoolComputer) -> bool {
    c.online && !c.not_sharing
}

/// Why a computer sits where it does in the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlaceReason {
    /// The most room: picked for you.
    BestFit,
    /// Can take it, with less room than the best.
    Room,
    /// Can take it; its room isn't known.
    Unknown,
    /// The computer you're working on: used last.
    YouAreHere,
    /// Offline: can't take jobs now.
    Offline,
    /// Its owner stopped sharing it.
    NotSharing,
}

/// One row of "Where".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaceRow {
    pub id: String,
    pub eligible: bool,
    pub reason: PlaceReason,
    pub free: Option<Free>,
}

/// Where a job goes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Placement {
    /// The computer the job runs on; None when no computer can take it.
    pub chosen: Option<String>,
    /// The one with the most room (what "best fit" marks).
    pub best: Option<String>,
    /// The picked computer couldn't take it, so the job moved off it.
    pub moved_from: Option<String>,
    /// Every computer, best first, then the ones that can't take a job.
    pub rows: Vec<PlaceRow>,
}

/// What `place` is asked.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PlaceInput {
    pub computers: Vec<PoolComputer>,
    /// The computer the person picked, if any.
    pub picked: Option<String>,
    /// Computers that just failed to take this job (a start that couldn't
    /// reach them): treated as unreachable.
    pub exclude: Vec<String>,
}

fn rank(c: &PoolComputer, excluded: bool) -> (u8, PlaceReason) {
    if !c.online || excluded {
        return (5, PlaceReason::Offline);
    }
    if c.not_sharing {
        return (6, PlaceReason::NotSharing);
    }
    if c.current {
        return (3, PlaceReason::YouAreHere);
    }
    match free(c) {
        Some(_) => (1, PlaceReason::Room),
        None => (2, PlaceReason::Unknown),
    }
}

/// Orders the computers and picks where the job goes.
pub fn place(input: &PlaceInput) -> Placement {
    let excluded = |id: &str| input.exclude.iter().any(|x| x == id);
    let mut rows: Vec<(u8, &PoolComputer, PlaceRow)> = input
        .computers
        .iter()
        .map(|c| {
            let (tier, reason) = rank(c, excluded(&c.id));
            let row = PlaceRow {
                id: c.id.clone(),
                eligible: tier <= 3,
                reason,
                free: free(c),
            };
            (tier, c, row)
        })
        .collect();
    rows.sort_by(|(ta, a, ra), (tb, b, rb)| {
        let fa = ra.free.unwrap_or_default();
        let fb = rb.free.unwrap_or_default();
        ta.cmp(tb)
            .then(fb.cores.cmp(&fa.cores))
            .then(fb.memory.cmp(&fa.memory))
            .then(a.jobs.cmp(&b.jobs))
            .then(a.id.cmp(&b.id))
    });
    let best = rows
        .iter()
        .find(|(_, _, r)| r.eligible)
        .map(|(_, _, r)| r.id.clone());
    if let Some(id) = &best
        && let Some((_, _, r)) = rows.iter_mut().find(|(_, _, r)| &r.id == id)
        && (r.reason == PlaceReason::Room || r.reason == PlaceReason::Unknown)
    {
        r.reason = PlaceReason::BestFit;
    }
    let picked_ok = input
        .picked
        .as_ref()
        .filter(|p| rows.iter().any(|(_, _, r)| &r.id == *p && r.eligible));
    let (chosen, moved_from) = match (&input.picked, picked_ok) {
        (_, Some(p)) => (Some(p.clone()), None),
        (Some(p), None) if best.is_some() => (best.clone(), Some(p.clone())),
        (Some(_), None) => (None, None),
        (None, None) => (best.clone(), None),
    };
    Placement {
        chosen,
        best,
        moved_from,
        rows: rows.into_iter().map(|(_, _, r)| r).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1 << 30;

    fn pc(id: &str, cores: u32, busy: f64, mem_gb: u64, used_gb: u64) -> PoolComputer {
        PoolComputer {
            id: id.into(),
            online: true,
            resources: Some(PoolResources {
                cpu_count: cores,
                cpu_percent: busy,
                memory_total: mem_gb * GB,
                memory_used: used_gb * GB,
            }),
            ..PoolComputer::default()
        }
    }

    fn three() -> Vec<PoolComputer> {
        let studio = pc("studio", 20, 40.0, 64, 24);
        let mut mbp = pc("mbp", 12, 25.0, 24, 14);
        mbp.current = true;
        let mut linux = pc("linux", 8, 0.0, 8, 1);
        linux.online = false;
        vec![mbp, linux, studio]
    }

    #[test]
    fn free_is_cores_not_busy_and_memory_not_used() {
        let f = free(&pc("a", 20, 40.0, 64, 24)).unwrap();
        assert_eq!(f.cores, 12);
        assert_eq!(f.memory, 40 * GB);
        assert!((f.fraction - 0.6).abs() < 1e-9);
    }

    #[test]
    fn offline_or_unmeasured_has_nothing_free() {
        let mut off = pc("a", 8, 0.0, 8, 0);
        off.online = false;
        assert_eq!(free(&off), None);
        let blank = PoolComputer {
            id: "b".into(),
            online: true,
            ..PoolComputer::default()
        };
        assert_eq!(free(&blank), None);
    }

    #[test]
    fn summary_adds_every_computer_and_frees_only_reachable_ones() {
        let s = summary(&three());
        assert_eq!(s.computers, 3);
        assert_eq!(s.online, 2);
        assert_eq!(s.cores, 40);
        assert_eq!(s.memory, 96 * GB);
        assert_eq!(s.free_cores, 12 + 9);
        assert_eq!(s.free_memory, 50 * GB);
        assert_eq!(s.unknown, 0);
    }

    #[test]
    fn summary_counts_unknown_and_skips_not_sharing() {
        let mut ns = pc("ns", 4, 0.0, 4, 0);
        ns.not_sharing = true;
        let blank = PoolComputer {
            id: "old".into(),
            online: true,
            ..PoolComputer::default()
        };
        let s = summary(&[ns, blank]);
        assert_eq!(s.cores, 4);
        assert_eq!(s.free_cores, 0);
        assert_eq!(s.unknown, 1);
    }

    #[test]
    fn most_room_wins_and_this_computer_goes_last() {
        let p = place(&PlaceInput {
            computers: three(),
            ..PlaceInput::default()
        });
        assert_eq!(p.chosen.as_deref(), Some("studio"));
        assert_eq!(p.best.as_deref(), Some("studio"));
        assert_eq!(p.moved_from, None);
        let order: Vec<_> = p.rows.iter().map(|r| (r.id.as_str(), r.reason)).collect();
        assert_eq!(
            order,
            vec![
                ("studio", PlaceReason::BestFit),
                ("mbp", PlaceReason::YouAreHere),
                ("linux", PlaceReason::Offline)
            ]
        );
    }

    #[test]
    fn only_this_computer_is_still_used() {
        let mut me = pc("me", 8, 50.0, 16, 8);
        me.current = true;
        let p = place(&PlaceInput {
            computers: vec![me],
            ..PlaceInput::default()
        });
        assert_eq!(p.chosen.as_deref(), Some("me"));
        assert_eq!(p.rows[0].reason, PlaceReason::YouAreHere);
    }

    #[test]
    fn ties_go_to_more_memory_then_fewer_jobs() {
        let a = pc("a", 8, 0.0, 16, 8);
        let b = pc("b", 8, 0.0, 16, 4);
        assert_eq!(
            place(&PlaceInput {
                computers: vec![a.clone(), b],
                ..PlaceInput::default()
            })
            .chosen
            .as_deref(),
            Some("b")
        );
        let mut c = a.clone();
        c.id = "c".into();
        c.jobs = 0;
        let mut a2 = a;
        a2.jobs = 2;
        assert_eq!(
            place(&PlaceInput {
                computers: vec![a2, c],
                ..PlaceInput::default()
            })
            .chosen
            .as_deref(),
            Some("c")
        );
    }

    #[test]
    fn measured_computers_come_before_unmeasured_ones() {
        let blank = PoolComputer {
            id: "aaa".into(),
            online: true,
            ..PoolComputer::default()
        };
        let small = pc("zzz", 2, 90.0, 4, 3);
        let p = place(&PlaceInput {
            computers: vec![blank, small],
            ..PlaceInput::default()
        });
        assert_eq!(p.chosen.as_deref(), Some("zzz"));
        assert_eq!(p.rows[1].reason, PlaceReason::Unknown);
    }

    #[test]
    fn a_pick_that_can_take_it_is_kept() {
        let p = place(&PlaceInput {
            computers: three(),
            picked: Some("mbp".into()),
            ..PlaceInput::default()
        });
        assert_eq!(p.chosen.as_deref(), Some("mbp"));
        assert_eq!(p.moved_from, None);
    }

    #[test]
    fn an_offline_pick_moves_to_the_best_other_computer() {
        let p = place(&PlaceInput {
            computers: three(),
            picked: Some("linux".into()),
            ..PlaceInput::default()
        });
        assert_eq!(p.chosen.as_deref(), Some("studio"));
        assert_eq!(p.moved_from.as_deref(), Some("linux"));
    }

    #[test]
    fn a_computer_that_failed_to_take_the_job_is_skipped() {
        let p = place(&PlaceInput {
            computers: three(),
            picked: Some("studio".into()),
            exclude: vec!["studio".into()],
        });
        assert_eq!(p.chosen.as_deref(), Some("mbp"));
        assert_eq!(p.moved_from.as_deref(), Some("studio"));
        let studio = p.rows.iter().find(|r| r.id == "studio").unwrap();
        assert!(!studio.eligible);
    }

    #[test]
    fn nothing_can_take_it() {
        let mut off = pc("a", 8, 0.0, 8, 0);
        off.online = false;
        let p = place(&PlaceInput {
            computers: vec![off],
            picked: Some("a".into()),
            ..PlaceInput::default()
        });
        assert_eq!(p.chosen, None);
        assert_eq!(p.moved_from, None);
        assert_eq!(p.best, None);
    }

    #[test]
    fn not_sharing_is_never_picked() {
        let mut ns = pc("ns", 64, 0.0, 256, 0);
        ns.not_sharing = true;
        let p = place(&PlaceInput {
            computers: vec![ns, pc("ok", 2, 50.0, 4, 2)],
            ..PlaceInput::default()
        });
        assert_eq!(p.chosen.as_deref(), Some("ok"));
        assert_eq!(p.rows[1].reason, PlaceReason::NotSharing);
    }
}
