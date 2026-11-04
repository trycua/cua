// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Who keeps the volume: the machines list, their reliability, and the
//! roles that follow from them.
//!
//! The machines list is the account's (every machine signed in); this module
//! only reads it. Reliability is a number each machine computes about itself
//! (`cua_cluster::reliability::score`: a desktop beats a plugged-in laptop
//! beats a laptop on battery; the machine the user is sitting at loses ties)
//! and shares in its heartbeat. The *same* number places a durable drive, so
//! the drive and its volume land on the same machine.

use serde::{Deserialize, Serialize};

use super::MachineId;

/// What a machine says about itself (in `Hello` and every heartbeat).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Facts {
    /// Reliability score; higher keeps the volume. Categories are 1000
    /// apart, so a hysteresis margin of 1000 means "a whole step better".
    pub rank: u64,
    /// Free bytes where the volume's copy lives.
    pub free_disk: u64,
    /// Running on battery.
    pub on_battery: bool,
    /// Can sleep when its lid closes (a laptop).
    pub sleep_prone: bool,
    /// The user is working at this machine right now.
    pub user_here: bool,
    /// A durable run holds a sleep assertion here.
    pub sleep_prevented: bool,
    /// CPU architecture (`arm64`, `amd64`; empty: unknown). Replication is
    /// the same on every architecture; this only tells a takeover across
    /// architectures to say that built programs are not reused.
    #[serde(default)]
    pub arch: String,
}

/// `arm64` or `amd64` for the names the platforms use.
pub fn normalize_arch(a: &str) -> String {
    match a.trim().to_ascii_lowercase().as_str() {
        "aarch64" | "arm64" => "arm64".into(),
        "x86_64" | "amd64" | "x64" => "amd64".into(),
        other => other.to_string(),
    }
}

/// Paths in a workspace that hold programs built for one architecture
/// (build folders and native libraries), as short folder or file names; at
/// most `max`. The rest of a workspace (sources, notes, journals, data) is
/// the same on every architecture.
pub fn arch_specific(keys: impl IntoIterator<Item = String>, max: usize) -> Vec<String> {
    const DIRS: &[&str] = &[
        "target",
        "node_modules",
        ".venv",
        "venv",
        "build",
        "dist",
        "cmake-build-debug",
        ".gradle",
        "Pods",
    ];
    const EXTS: &[&str] = &["o", "so", "dylib", "a", "node", "rlib", "dll", "exe", "pyd"];
    let mut out: Vec<String> = vec![];
    for k in keys {
        let parts: Vec<&str> = k.split('/').collect();
        let hit = parts
            .iter()
            .enumerate()
            .find(|(i, p)| *i + 1 < parts.len() && DIRS.contains(p))
            .map(|(i, _)| format!("{}/", parts[..=i].join("/")))
            .or_else(|| {
                let ext = k.rsplit('.').next()?;
                (k.contains('.') && EXTS.contains(&ext)).then(|| k.clone())
            });
        if let Some(h) = hit {
            // Show the folder after the agent's home, not the whole key.
            let short = h.splitn(3, '/').nth(2).map(str::to_string).unwrap_or(h);
            if !out.contains(&short) {
                out.push(short);
                if out.len() >= max {
                    break;
                }
            }
        }
    }
    out
}

/// One machine of the group.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub id: MachineId,
    /// The name the machines list shows ("Mac Studio").
    pub name: String,
    pub facts: Facts,
    /// Wall time this machine was last heard from (0: never; display and
    /// the active-member window only).
    #[serde(default)]
    pub last_seen_ms: u64,
}

/// What a machine does for the volume.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Holds a full copy and acknowledges writes (the owner is one).
    Replica,
    /// Keeps a cache only and sends its writes to a replica.
    Client,
}

/// The preferred layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    /// The machine that should own the volume.
    pub owner: MachineId,
    /// Machines that hold a full copy, owner first.
    pub replicas: Vec<MachineId>,
}

impl Layout {
    pub fn role_of(&self, id: &str) -> Role {
        if self.replicas.iter().any(|r| r == id) {
            Role::Replica
        } else {
            Role::Client
        }
    }
}

/// Disk a machine needs free to hold a copy of `volume_bytes`: the volume
/// again plus a margin (10 percent, at least 1 GiB).
pub fn disk_needed(volume_bytes: u64) -> u64 {
    volume_bytes.saturating_add((volume_bytes / 10).max(1 << 30))
}

/// The preferred layout for `members`: the `replicas` most reliable
/// machines that have room for a copy, the best of them owning. Ties break
/// by id so every machine computes the same answer.
pub fn layout(members: &[Member], replicas: usize, volume_bytes: u64) -> Layout {
    let need = disk_needed(volume_bytes);
    let mut able: Vec<&Member> = members
        .iter()
        .filter(|m| m.facts.free_disk >= need)
        .collect();
    if able.is_empty() {
        // Nobody reports room: the most reliable machines hold it anyway
        // (a store that is full fails loudly; a refusal helps nobody).
        able = members.iter().collect();
    }
    able.sort_by(|a, b| {
        b.facts
            .rank
            .cmp(&a.facts.rank)
            .then_with(|| a.id.cmp(&b.id))
    });
    let replicas: Vec<MachineId> = able
        .iter()
        .take(replicas.max(1))
        .map(|m| m.id.clone())
        .collect();
    Layout {
        owner: replicas.first().cloned().unwrap_or_default(),
        replicas,
    }
}

/// One class step of reliability (see `cua_cluster::reliability::STEP`).
pub const STEP_RANK: u64 = 1000;

/// Copies to keep: 2 when two or more machines exist, else 1.
pub fn replica_count(machines: usize) -> usize {
    if machines >= 2 { 2 } else { 1 }
}

/// Whether `candidate` is better than the current owner by a whole step.
pub fn clearly_better(candidate: &Facts, owner: &Facts) -> bool {
    candidate.rank >= owner.rank.saturating_add(STEP_RANK)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(id: &str, rank: u64, disk_gib: u64) -> Member {
        Member {
            id: id.into(),
            name: id.into(),
            facts: Facts {
                rank,
                free_disk: disk_gib << 30,
                ..Default::default()
            },
            last_seen_ms: 0,
        }
    }

    #[test]
    fn the_most_reliable_machine_owns_and_the_next_holds_a_copy() {
        let ms = [
            m("laptop", 2000, 200),
            m("studio", 9000, 800),
            m("mini", 7000, 500),
        ];
        let l = layout(&ms, 2, 10 << 30);
        assert_eq!(l.owner, "studio");
        assert_eq!(l.replicas, ["studio", "mini"]);
        assert_eq!(l.role_of("laptop"), Role::Client);
        assert_eq!(l.role_of("mini"), Role::Replica);
    }

    #[test]
    fn architecture_names_normalize_and_built_programs_are_found() {
        assert_eq!(normalize_arch("aarch64"), "arm64");
        assert_eq!(normalize_arch("x86_64"), "amd64");
        let keys = [
            "agents/ada/src/main.rs",
            "agents/ada/target/debug/ada",
            "agents/ada/target/release/libx.rlib",
            "agents/ada/notes.md",
            "agents/ada/vendor/libfoo.so",
            "agents/ada/node_modules/x/y.js",
        ]
        .map(String::from);
        let found = arch_specific(keys, 5);
        assert_eq!(found, ["target/", "vendor/libfoo.so", "node_modules/"]);
    }

    #[test]
    fn a_machine_without_room_becomes_a_client() {
        let ms = [m("studio", 9000, 2), m("laptop", 2000, 400)];
        // A 100 GiB volume does not fit the studio's 2 GiB.
        let l = layout(&ms, 2, 100 << 30);
        assert_eq!(l.owner, "laptop");
        assert_eq!(l.replicas, ["laptop"]);
        assert_eq!(l.role_of("studio"), Role::Client);
        // Everyone is full: the most reliable still hold it.
        let l = layout(&[m("a", 1, 0), m("b", 2, 0)], 2, 100 << 30);
        assert_eq!(l.replicas, ["b", "a"]);
    }

    #[test]
    fn equal_ranks_break_by_id_and_counts_follow_the_machines() {
        let ms = [m("b", 5000, 100), m("a", 5000, 100)];
        assert_eq!(layout(&ms, 2, 0).owner, "a");
        assert_eq!(replica_count(1), 1);
        assert_eq!(replica_count(2), 2);
        assert_eq!(replica_count(9), 2);
        assert!(clearly_better(
            &Facts {
                rank: 9000,
                ..Default::default()
            },
            &Facts {
                rank: 7999,
                ..Default::default()
            }
        ));
        assert!(!clearly_better(
            &Facts {
                rank: 8500,
                ..Default::default()
            },
            &Facts {
                rank: 8000,
                ..Default::default()
            }
        ));
    }
}
