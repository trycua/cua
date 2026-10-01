//! Disk maintenance: reaping orphaned ephemeral sandboxes, the cache
//! garbage collection and log caps.
//!
//! An ephemeral sandbox is deleted with its handle. When its process dies
//! first (a crash, a kill, a closed notebook), the lease it took under
//! `sandboxes/.ephemeral/` names a process that no longer exists; after
//! `CUA_ORPHAN_REAP_MINUTES` (default 10) the daemon, `cua cache prune` or
//! the next `cua` run deletes the instance (container, VM disk, Lume clone)
//! and the lease. Ephemeral instances with no lease at all (an SDK older
//! than leases) are reaped after 24 hours, abandoned build VMs and build
//! containers after 6 hours. Named sandboxes are never reaped.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cua_disk::{CacheConfig, Category, GcOptions, GcReport, Layout, Scanner};
use cua_sandbox_core::{LocalRuntime, RuntimeError, StateStore};
use serde::Serialize;

/// Ephemeral instances without a lease are reaped after this long.
pub const UNLEASED_REAP_AFTER: Duration = Duration::from_secs(24 * 3600);
/// Build VMs and build containers left by a crashed build are removed after
/// this long.
pub const BUILD_LEFTOVER_AFTER: Duration = Duration::from_secs(6 * 3600);
/// `CUA_DAEMON_MAINTENANCE=0` turns the daemon's periodic maintenance off.
pub const ENV_DAEMON_MAINTENANCE: &str = "CUA_DAEMON_MAINTENANCE";

/// One reaped (or, for a dry run, reapable) instance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Reaped {
    /// Instance name.
    pub name: String,
    /// Why.
    pub reason: String,
    /// The delete failed (the lease stays and it is retried next time).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What one maintenance pass did.
#[derive(Clone, Debug, Serialize)]
pub struct MaintenanceReport {
    /// Orphaned sandboxes removed.
    pub reaped: Vec<Reaped>,
    /// The cache collection.
    pub gc: Option<GcReport>,
    /// Logs rotated or trimmed.
    pub rotated_logs: Vec<String>,
}

fn secs(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

async fn delete(local: &dyn LocalRuntime, name: &str) -> Option<String> {
    match local.delete(name).await {
        Ok(()) | Err(RuntimeError::NotFound(_)) => None,
        Err(e) => Some(e.to_string()),
    }
}

/// Leases whose process is gone and that are older than the reap delay:
/// deletes the instance and the lease. Cheap (reads the lease directory
/// only); run on every CLI start.
pub async fn reap_leases(
    local: &dyn LocalRuntime,
    state: &StateStore,
    reap_after: Duration,
    now: SystemTime,
    dry_run: bool,
    alive: &(dyn Fn(u32) -> bool + Sync),
) -> Vec<Reaped> {
    let mut out = Vec::new();
    for l in state.leases() {
        if l.pid == std::process::id() || alive(l.pid) {
            continue;
        }
        if secs(now).saturating_sub(l.created_at) < reap_after.as_secs() {
            continue;
        }
        let error = if dry_run {
            None
        } else {
            let e = delete(local, &l.name).await;
            if e.is_none() {
                let _ = state.remove_lease(&l.name);
                let _ = state.delete(&l.name);
            }
            e
        };
        out.push(Reaped {
            name: l.name,
            reason: format!("ephemeral sandbox whose process ({}) exited", l.pid),
            error,
        });
    }
    out
}

/// [`reap_leases`] plus instances the scan found: ephemeral ones without a
/// lease (after 24 h) and abandoned build VMs and containers (after 6 h).
pub async fn reap_orphans(
    local: &dyn LocalRuntime,
    state: &StateStore,
    items: &[cua_disk::Item],
    reap_after: Duration,
    now: SystemTime,
    dry_run: bool,
    alive: &(dyn Fn(u32) -> bool + Sync),
) -> Vec<Reaped> {
    let leases: Vec<String> = state.leases().into_iter().map(|l| l.name).collect();
    let mut out = reap_leases(local, state, reap_after, now, dry_run, alive).await;
    let age = |i: &cua_disk::Item| i.created.map(|c| secs(now).saturating_sub(c));
    for i in items {
        let running = i.status.as_deref() == Some("running");
        let reason = if i.category == Category::Sandboxes
            && i.ephemeral
            && !leases.contains(&i.name)
            && age(i).is_some_and(|a| a >= UNLEASED_REAP_AFTER.as_secs())
        {
            "ephemeral sandbox without an owner for over 24 hours"
        } else if matches!(i.kind.as_str(), "build-vm" | "container-build")
            && age(i).is_some_and(|a| a >= BUILD_LEFTOVER_AFTER.as_secs())
            && !(running && i.kind == "build-vm")
        {
            "build instance left by an interrupted build"
        } else {
            continue;
        };
        if out.iter().any(|r| r.name == i.name) {
            continue;
        }
        let error = if dry_run {
            None
        } else {
            delete(local, &i.name).await
        };
        out.push(Reaped {
            name: i.name.clone(),
            reason: reason.into(),
            error,
        });
    }
    out
}

/// One full pass: reap orphans, collect the cache (budget and orphans; the
/// automatic grace period), cap logs. `dry_run` reports only.
pub async fn run(
    local: &dyn LocalRuntime,
    state: &StateStore,
    layout: Layout,
    gc: Option<GcOptions>,
    dry_run: bool,
) -> MaintenanceReport {
    let config = CacheConfig::load();
    let scanner = Scanner::system(layout.clone()).await;
    let now = SystemTime::now();
    let report = scanner.scan_at(now).await;
    let reaped = reap_orphans(
        local,
        state,
        &report.items,
        config.orphan_reap_after,
        now,
        dry_run,
        &cua_vmm::host::pid_alive,
    )
    .await;
    let gc = match gc {
        Some(mut o) => {
            o.dry_run = dry_run;
            Some(cua_disk::collect(&scanner, o).await)
        }
        None => None,
    };
    let rotated_logs = if dry_run {
        vec![]
    } else {
        // Logs past their cap, plus old screenshots and temp leftovers.
        cua_disk::logs::rotate_all(&layout)
            .into_iter()
            .chain(cua_disk::scratch::prune_all(&layout))
            .map(|p| p.display().to_string())
            .collect()
    };
    MaintenanceReport {
        reaped,
        gc,
        rotated_logs,
    }
}

/// Runs the automatic collection in the background (after a pull or a
/// build). Never blocks or fails the caller.
pub fn spawn_auto_gc(reason: &'static str) {
    if let Ok(h) = tokio::runtime::Handle::try_current() {
        h.spawn(async move {
            cua_disk::auto_gc(reason).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use cua_sandbox_core::{
        InstanceStatus, LocalEndpoints, LocalInstance, LocalStartSpec, LocalSummary, RuntimeResult,
    };
    use std::sync::Mutex;

    #[derive(Default)]
    struct Fake {
        deleted: Mutex<Vec<String>>,
        fail: Vec<String>,
    }

    #[async_trait]
    impl LocalRuntime for Fake {
        fn backend(&self) -> String {
            "fake".into()
        }
        async fn start(&self, _: &LocalStartSpec) -> RuntimeResult<LocalInstance> {
            unreachable!()
        }
        async fn stop(&self, _: &str) -> RuntimeResult<()> {
            Ok(())
        }
        async fn resume(&self, _: &str) -> RuntimeResult<LocalInstance> {
            unreachable!()
        }
        async fn list(&self) -> RuntimeResult<Vec<LocalSummary>> {
            Ok(vec![])
        }
        async fn status(&self, _: &str) -> RuntimeResult<InstanceStatus> {
            Ok(InstanceStatus::Running)
        }
        async fn delete(&self, name: &str) -> RuntimeResult<()> {
            if self.fail.iter().any(|f| f == name) {
                return Err(RuntimeError::Other("engine busy".into()));
            }
            self.deleted.lock().unwrap().push(name.into());
            Ok(())
        }
        async fn endpoints(&self, _: &str) -> RuntimeResult<LocalEndpoints> {
            Ok(LocalEndpoints::default())
        }
    }

    fn lease(state: &StateStore, name: &str, pid: u32, created_at: u64) {
        let dir = state.dir().join(cua_sandbox_core::LEASE_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        let l = cua_sandbox_core::EphemeralLease {
            name: name.into(),
            pid,
            created_at,
        };
        std::fs::write(
            dir.join(format!("{name}.json")),
            serde_json::to_vec(&l).unwrap(),
        )
        .unwrap();
    }

    const NOW: u64 = 1_800_000_000;

    #[tokio::test]
    async fn dead_owners_are_reaped_after_the_delay_and_live_ones_never() {
        let d = tempfile::tempdir().unwrap();
        let state = StateStore::new(d.path());
        let now = UNIX_EPOCH + Duration::from_secs(NOW);
        lease(&state, "cua-eph-dead-old", 111, NOW - 3600);
        lease(&state, "cua-eph-dead-new", 112, NOW - 60);
        lease(&state, "cua-eph-alive", 222, NOW - 86_400);
        lease(&state, "cua-eph-stuck", 113, NOW - 3600);
        let fake = Fake {
            fail: vec!["cua-eph-stuck".into()],
            ..Default::default()
        };
        let alive = |pid: u32| pid == 222;
        let r = reap_leases(&fake, &state, Duration::from_secs(600), now, false, &alive).await;
        let names: Vec<&str> = r.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, vec!["cua-eph-dead-old", "cua-eph-stuck"]);
        assert!(r[1].error.is_some());
        assert_eq!(*fake.deleted.lock().unwrap(), vec!["cua-eph-dead-old"]);
        let left: Vec<String> = state.leases().into_iter().map(|l| l.name).collect();
        // The reaped lease is gone; a failed delete keeps its lease (retry).
        assert_eq!(
            left,
            vec!["cua-eph-alive", "cua-eph-dead-new", "cua-eph-stuck"]
        );

        // Dry run: reported, nothing deleted.
        let fake = Fake::default();
        let later = now + Duration::from_secs(3600);
        let r = reap_leases(&fake, &state, Duration::from_secs(600), later, true, &alive).await;
        assert_eq!(r.len(), 2);
        assert!(fake.deleted.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn unleased_ephemerals_and_build_leftovers_age_out_named_never() {
        let d = tempfile::tempdir().unwrap();
        let state = StateStore::new(d.path());
        let now = UNIX_EPOCH + Duration::from_secs(NOW);
        let item = |name: &str, kind: &str, eph: bool, age: u64| cua_disk::Item {
            category: if kind == "build-vm" {
                Category::Builds
            } else {
                Category::Sandboxes
            },
            kind: kind.into(),
            name: name.into(),
            ephemeral: eph,
            created: Some(NOW - age),
            status: Some("running".into()),
            ..Default::default()
        };
        let items = vec![
            item("cua-eph-ancient", "container-sandbox", true, 2 * 86_400),
            item("cua-eph-recent", "qemu", true, 3600),
            item("my-named-box", "qemu", false, 90 * 86_400),
            item("cua-build-ctr-9", "container-build", false, 7 * 3600),
            item("cua-build-1f", "build-vm", false, 7 * 3600),
        ];
        let fake = Fake::default();
        let r = reap_orphans(
            &fake,
            &state,
            &items,
            Duration::from_secs(600),
            now,
            false,
            &|_| false,
        )
        .await;
        let names: Vec<&str> = r.iter().map(|x| x.name.as_str()).collect();
        // A running build VM is still building; the container is reaped.
        assert_eq!(names, vec!["cua-eph-ancient", "cua-build-ctr-9"]);
        assert!(
            !fake
                .deleted
                .lock()
                .unwrap()
                .contains(&"my-named-box".to_string())
        );
    }
}
