// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! What the Spaces on this machine reserve of it, for `on="auto"`'s view of
//! this machine ([`cua_cluster::sysinfo::apply_reservations`] does the math).
//!
//! Live ones come from the local sandboxes ([`Spaces::local_reservations`]);
//! a create that has not produced its sandbox yet is held here from the
//! moment it is sized until it ends, so a burst of creates sees itself.

use cua_cluster::sysinfo::LocalReservation;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// The size a Space gets when a create leaves it out (what `provision_local`
/// uses).
pub(crate) const DEFAULT_CPUS: u32 = 2;
/// The memory (MiB) a Space gets when a create leaves it out.
pub(crate) const DEFAULT_MEMORY_MB: u64 = 4096;

type Table = Mutex<HashMap<(String, String), LocalReservation>>;

fn table() -> &'static Table {
    static T: OnceLock<Table> = OnceLock::new();
    T.get_or_init(Default::default)
}

/// A create in flight on this machine; it stops reserving when dropped.
pub(crate) struct Pending {
    key: (String, String),
}

impl Pending {
    /// Holds `cpus` vCPUs and `memory_mb` for the Space `name` of the Cua
    /// home `home` until the guard drops.
    pub(crate) fn hold(
        home: &std::path::Path,
        name: &str,
        container: bool,
        cpus: u32,
        memory_mb: u64,
    ) -> Self {
        let key = (home.display().to_string(), name.to_string());
        table().lock().unwrap_or_else(|e| e.into_inner()).insert(
            key.clone(),
            LocalReservation {
                container,
                millicpus: cpus.saturating_mul(1000),
                memory_mb,
                starting: true,
            },
        );
        Self { key }
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        table()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.key);
    }
}

/// The creates in flight under `home`, by Space name.
pub(crate) fn pending(home: &std::path::Path) -> Vec<(String, LocalReservation)> {
    let home = home.display().to_string();
    table()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|((h, _), _)| *h == home)
        .map(|((_, n), r)| (n.clone(), *r))
        .collect()
}

/// Whether a local runtime word is a container's (its vCPUs share CPUs),
/// not a VM's.
pub(crate) fn is_container_runtime(runtime: &str) -> bool {
    matches!(
        runtime,
        "container" | "docker" | "podman" | "gvisor" | "runc" | "containerization"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_create_in_flight_reserves_until_it_ends() {
        let home = std::path::Path::new("/nonexistent/home-a");
        let g = Pending::hold(home, "one", true, 2, 4096);
        let p = pending(home);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].0, "one");
        assert!(p[0].1.starting && p[0].1.container);
        // Another home does not see it.
        assert!(pending(std::path::Path::new("/nonexistent/home-b")).is_empty());
        drop(g);
        assert!(pending(home).is_empty());
    }

    #[test]
    fn container_runtimes_are_told_from_vm_runtimes() {
        for c in ["docker", "gvisor", "runc", "container"] {
            assert!(is_container_runtime(c), "{c}");
        }
        for v in ["qemu", "lume", "firecracker", "kubevirt", ""] {
            assert!(!is_container_runtime(v), "{v}");
        }
    }
}
