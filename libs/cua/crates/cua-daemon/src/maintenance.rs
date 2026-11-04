//! Disk maintenance: reaping orphaned ephemeral sandboxes, the cache
//! garbage collection and log caps.
//!
//! An ephemeral sandbox is deleted with its handle. When its process dies
//! first (a crash, a kill, a closed notebook), the lease it took under
//! `sandboxes/.ephemeral/` names a process that no longer exists; after
//! `CUA_ORPHAN_REAP_MINUTES` (default 10) the daemon, `cua cache prune` or
//! the next `cua` run deletes the instance (container, VM disk, Lume clone)
//! and the lease. Unleased ephemeral and build instances require explicit
//! cleanup; their name, kind and age do not authorize deletion.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cua_disk::{CacheConfig, GcOptions, GcReport, Layout, Scanner};
use cua_sandbox_core::{LocalRuntime, RuntimeError, StateStore};
use serde::Serialize;

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

/// Compatibility entry point for lease reaping; inventory does not authorize deletion.
pub async fn reap_orphans(
    local: &dyn LocalRuntime,
    state: &StateStore,
    _items: &[cua_disk::Item],
    reap_after: Duration,
    now: SystemTime,
    dry_run: bool,
    alive: &(dyn Fn(u32) -> bool + Sync),
) -> Vec<Reaped> {
    reap_leases(local, state, reap_after, now, dry_run, alive).await
}

/// The collection the daemon's idle maintenance runs: budget eviction and
/// orphans with the automatic grace period ([`cua_disk::BASE_POLICY`] keeps
/// base images in use), `None` when `auto_gc` is off.
pub fn daemon_gc_options(config: &CacheConfig) -> Option<GcOptions> {
    config.auto_gc.then(|| GcOptions {
        grace: cua_disk::gc::AUTO_GRACE,
        // Orphans still go when the budget is off.
        budget: (config.budget == cua_disk::Budget::Off).then_some(cua_disk::Budget::Off),
        ..Default::default()
    })
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
    let scanner = match gc {
        Some(_) => Some(Scanner::system(layout.clone()).await),
        None => None,
    };
    run_with(local, state, &layout, scanner.as_ref(), gc, dry_run).await
}

/// [`run`] over an explicit scanner (the cache collection runs when both
/// `scanner` and `gc` are given).
pub async fn run_with(
    local: &dyn LocalRuntime,
    state: &StateStore,
    layout: &Layout,
    scanner: Option<&Scanner>,
    gc: Option<GcOptions>,
    dry_run: bool,
) -> MaintenanceReport {
    let config = CacheConfig::load();
    let now = SystemTime::now();
    let reaped = reap_leases(
        local,
        state,
        config.orphan_reap_after,
        now,
        dry_run,
        &cua_vmm::host::pid_alive,
    )
    .await;
    let gc = match (gc, scanner) {
        (Some(mut o), Some(scanner)) => {
            o.dry_run = dry_run;
            Some(cua_disk::collect(scanner, o).await)
        }
        _ => None,
    };
    let rotated_logs = if dry_run {
        vec![]
    } else {
        // Logs past their cap, plus old screenshots and temp leftovers.
        cua_disk::logs::rotate_all(layout)
            .into_iter()
            .chain(cua_disk::scratch::prune_all(layout))
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
    use cua_disk::Category;
    use cua_sandbox_core::{
        InstanceStatus, LocalEndpoints, LocalInstance, LocalStartSpec, LocalSummary, RuntimeResult,
    };
    use std::sync::{Arc, Mutex};

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
    async fn unleased_inventory_is_not_authority_to_reap() {
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
        let mut items = vec![
            item("cua-eph-ancient", "container-sandbox", true, 2 * 86_400),
            item("cua-eph-recent", "qemu", true, 3600),
            item("my-named-box", "qemu", false, 90 * 86_400),
            item("cua-build-ctr-9", "container-build", false, 7 * 3600),
            item("cua-build-1f", "build-vm", false, 7 * 3600),
        ];
        let mut stopped = item("cua-build-stopped", "build-vm", false, 7 * 3600);
        stopped.status = Some("stopped".into());
        items.push(stopped);
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
        assert!(r.is_empty(), "{r:?}");
        assert!(fake.deleted.lock().unwrap().is_empty());
        lease(&state, "cua-eph-ancient", 111, NOW - 3600);
        lease(&state, "my-named-box", 222, NOW - 3600);
        let r = reap_orphans(
            &fake,
            &state,
            &items,
            Duration::from_secs(600),
            now,
            true,
            &|p| p == 222,
        )
        .await;
        assert_eq!(r.len(), 1);
        assert!(fake.deleted.lock().unwrap().is_empty());
        assert_eq!(state.leases().len(), 2);
        let r = reap_orphans(
            &fake,
            &state,
            &items,
            Duration::from_secs(600),
            now,
            false,
            &|p| p == 222,
        )
        .await;
        assert_eq!(r.len(), 1);
        assert_eq!(fake.deleted.lock().unwrap().as_slice(), ["cua-eph-ancient"]);
        assert_eq!(state.leases().len(), 1);
    }

    struct FakeLume(Mutex<Vec<cua_disk::lume::LumeVm>>);

    #[async_trait]
    impl cua_disk::lume::LumeApi for FakeLume {
        async fn vms(&self) -> Result<Vec<cua_disk::lume::LumeVm>, String> {
            Ok(self.0.lock().unwrap().clone())
        }
        async fn delete(&self, name: &str) -> Result<(), String> {
            self.0.lock().unwrap().retain(|v| v.name != name);
            Ok(())
        }
    }

    #[tokio::test]
    async fn daemon_maintenance_keeps_bases_in_use_under_a_small_budget() {
        use cua_vmm::lume::{OwnedKind, OwnedVm, OwnedVms};
        let d = tempfile::tempdir().unwrap();
        let layout = Layout::new(d.path().join("home"));
        let state = StateStore::new(d.path().join("state"));
        let owned = OwnedVms::new(layout.lume_owned());
        let now = cua_vmm::host::now_secs();
        let lume = Arc::new(FakeLume(Mutex::new(vec![])));
        let record = |name: &str, kind: OwnedKind, source: &str, pulled_ago: u64| {
            std::fs::create_dir_all(owned.dir()).unwrap();
            let rec = OwnedVm {
                name: name.into(),
                kind,
                source: Some(source.into()),
                created_at: now - pulled_ago,
                last_used: None,
            };
            let f = owned.dir().join(format!("{name}.json"));
            std::fs::write(&f, serde_json::to_vec(&rec).unwrap()).unwrap();
            // Idle past the automatic grace period.
            cua_vmm::disk::mark_used_at(&f, SystemTime::now() - Duration::from_secs(3600));
            lume.0.lock().unwrap().push(cua_disk::lume::LumeVm {
                name: name.into(),
                status: "stopped".into(),
                allocated: 28 << 30,
            });
        };
        // An old base nothing uses, the base a Space was cloned from, and the
        // newest base, whose create failed.
        record(
            "cua-base-old",
            OwnedKind::Base,
            "ghcr.io/trycua/macos:15",
            90 * 86_400,
        );
        record(
            "cua-base-used",
            OwnedKind::Base,
            "ghcr.io/trycua/macos:26-slim",
            30 * 86_400,
        );
        record("my-mac", OwnedKind::Instance, "cua-base-used", 86_400);
        record(
            "cua-base-failed",
            OwnedKind::Base,
            "ghcr.io/trycua/macos:15@sha256:ff",
            3600,
        );
        // A budget smaller than one base (the automatic one is ~26 GiB on a
        // 236 GiB-free disk).
        let config = CacheConfig {
            budget: cua_disk::Budget::Bytes(26 << 30),
            ..CacheConfig::default()
        };
        let scanner = Scanner::new(
            layout.clone(),
            config,
            None,
            Some(lume.clone() as Arc<dyn cua_disk::lume::LumeApi>),
        );
        let opts = daemon_gc_options(&config).expect("auto_gc is on by default");
        let r = run_with(
            &Fake::default(),
            &state,
            &layout,
            Some(&scanner),
            Some(opts),
            false,
        )
        .await;
        let g = r.gc.expect("collected");
        let mut left: Vec<String> = lume
            .0
            .lock()
            .unwrap()
            .iter()
            .map(|v| v.name.clone())
            .collect();
        left.sort();
        assert_eq!(
            left,
            vec!["cua-base-failed", "cua-base-used", "my-mac"],
            "{g:?}"
        );
        assert_eq!(g.removed.len(), 1, "{g:?}");
        assert_eq!(g.removed[0].item.name, "cua-base-old");
        let why: Vec<(String, String)> = g
            .kept_bases
            .iter()
            .map(|k| (k.name.clone(), k.why.to_string()))
            .collect();
        assert_eq!(
            why,
            vec![
                (
                    "cua-base-failed".into(),
                    "kept for a retry: no newer base is in use".into()
                ),
                ("cua-base-used".into(), "in use by my-mac".into()),
            ]
        );
    }
}
