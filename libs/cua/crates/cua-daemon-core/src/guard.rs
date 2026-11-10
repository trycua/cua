//! VMs nobody uses: finding them and ending them.
//!
//! QEMU VMs outlive the process that started them (a daemon restart must
//! not kill live work), so a daemon that crashes, a home that is deleted or
//! a Space whose lifetime passed while no daemon ran leaves a VM holding
//! CPUs and memory that other work needs. [`sweep`] ends those, by rules
//! that never rest on a name or an age alone:
//!
//! | the VM | verdict |
//! |---|---|
//! | its Space's lifetime passed (the sandbox's record, or `space-lifetimes.json`) | ended (deleted, or turned off for `suspend`) |
//! | recorded as a Space (`sandboxes/<name>.json`) | kept |
//! | a warm-pool member the pool records, its daemon gone | removed once the daemon has been gone for [`Policy::abandoned_warm`] |
//! | recorded nowhere, its starter exited ([`cua_vmm::hostvms::Launcher`]) | removed after [`Policy::starter_gone`] |
//! | recorded nowhere, starter unknown (a VM from before records) | removed after [`Policy::unknown_starter`] |
//!
//! A VM is judged only against **its own home's** records, found from the
//! pid file QEMU runs with (`<home>/vmm/qemu/<name>/qemu.pid`, see
//! [`cua_vmm::hostvms`]). A daemon sweeps its own home always, and another
//! home only when that home has no daemon running (nobody else would ever
//! end its VMs); a home with a live daemon is never touched, and
//! processes that are not Cua VMs are never even listed.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use cua_sandbox_core::StateStore;
use cua_vmm::hostvms::{self, HostVm, Launcher};
use serde::Serialize;

/// The file of Space lifetimes in a home (`cua-spaces`).
pub const LIFETIMES_FILE: &str = "space-lifetimes.json";

/// How long a VM nobody records, or whose daemon is gone, may run.
#[derive(Clone, Copy, Debug)]
pub struct Policy {
    /// Recorded nowhere and its starter exited: it can no longer be mid
    /// create, so this only covers clock and file-system races.
    pub starter_gone: Duration,
    /// Recorded nowhere and no record of who started it.
    pub unknown_starter: Duration,
    /// A warm-pool member whose daemon is not running.
    pub abandoned_warm: Duration,
    /// Recorded nowhere, its starter still running: the longest a create
    /// can take (image pull and boot budget) before the VM is judged lost.
    pub create_max: Duration,
    /// The same for a golden-snapshot recorder.
    pub recorder_max: Duration,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            starter_gone: Duration::from_secs(15),
            unknown_starter: Duration::from_secs(3600),
            abandoned_warm: Duration::from_secs(45),
            create_max: Duration::from_secs(15 * 60),
            recorder_max: Duration::from_secs(3600),
        }
    }
}

/// Why a VM is ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    /// The Space's lifetime passed and it deletes at the end.
    Expired,
    /// The Space's lifetime passed and it turns off at the end.
    ExpiredSuspend,
    /// A warm-pool member of a daemon that is gone.
    AbandonedWarm,
    /// Nothing records it and whatever started it is gone.
    Unrecorded,
    /// Its whole home was deleted: nothing can ever record or stop it.
    HomeGone,
}

/// A VM that was ended (or, in a dry run, would be).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Ended {
    /// Instance name.
    pub name: String,
    /// QEMU's pid.
    pub pid: u32,
    /// The home it belongs to.
    pub home: PathBuf,
    /// Why.
    pub reason: Reason,
    /// Ending it failed (it is tried again next time).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What a home records about its VMs.
#[derive(Clone, Debug, Default)]
pub struct Records {
    /// `sandboxes/<name>.json`: Spaces and sandboxes.
    pub sandboxes: HashSet<String>,
    /// `sandboxes/.pool/<name>.json`: warm-pool members.
    pub pool: HashSet<String>,
    /// Space lifetimes: instance name to (unix seconds, `delete|suspend`).
    pub lifetimes: HashMap<String, (u64, String)>,
}

fn stems(dir: &Path) -> HashSet<String> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| {
            e.path()
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
        })
        .collect()
}

impl Records {
    /// Reads `home`'s records.
    pub fn load(home: &Path) -> Self {
        let state = home.join("sandboxes");
        let mut lifetimes = HashMap::new();
        if let Some(Ok(map)) = std::fs::read(home.join(LIFETIMES_FILE))
            .ok()
            .map(|b| serde_json::from_slice::<HashMap<String, serde_json::Value>>(&b))
        {
            for (id, v) in map {
                let at = v.get("expires_at_unix").and_then(|x| x.as_u64());
                let how = v
                    .get("on_expire")
                    .and_then(|x| x.as_str())
                    .unwrap_or("delete");
                if let Some(at) = at {
                    // `local:<name>` is the Space of instance `<name>`.
                    let name = id.strip_prefix("local:").unwrap_or(&id);
                    lifetimes.insert(name.to_string(), (at, how.to_string()));
                }
            }
        }
        // A local Space's lifetime is its sandbox's own record
        // (`sandboxes/<name>.json`), which wins over the file's.
        for name in stems(&state) {
            let Some(v) = std::fs::read(state.join(format!("{name}.json")))
                .ok()
                .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            else {
                continue;
            };
            if let Some(at) = v.get("expires_at_unix").and_then(|x| x.as_u64()) {
                let how = v
                    .get("on_expire")
                    .and_then(|x| x.as_str())
                    .unwrap_or("delete");
                lifetimes.insert(name, (at, how.to_string()));
            }
        }
        Self {
            sandboxes: stems(&state),
            pool: stems(&state.join(".pool")),
            lifetimes,
        }
    }
}

/// Whether a daemon is serving `home` (or starting there). Reads only.
pub fn daemon_alive(home: &Path) -> bool {
    let up = crate::Discovery::read(&home.join("daemon.json")).is_some_and(|d| d.listening());
    let starting = std::fs::read_to_string(home.join("daemon.starting"))
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .is_some_and(crate::pid_alive);
    up || starting
}

/// The verdict on one VM, from its home's records. Pure.
pub fn judge(
    vm: &HostVm,
    rec: &Records,
    daemon_alive: bool,
    starter: Option<&Launcher>,
    starter_alive: bool,
    now: u64,
    p: &Policy,
) -> Option<Reason> {
    if let Some((at, how)) = rec.lifetimes.get(&vm.name)
        && *at <= now
        && (rec.sandboxes.contains(&vm.name) || rec.pool.contains(&vm.name))
    {
        return Some(if how == "suspend" {
            Reason::ExpiredSuspend
        } else {
            Reason::Expired
        });
    }
    if cua_vmm::is_pool_member(&vm.name) {
        if rec.pool.contains(&vm.name) {
            // The pool of a running daemon decides about its own members.
            return (!daemon_alive && vm.age >= p.abandoned_warm).then_some(Reason::AbandonedWarm);
        }
    } else if rec.sandboxes.contains(&vm.name) {
        return None;
    }
    // Recorded nowhere (or a pool member the pool has no record of).
    let unused = match starter {
        // Its starter is running: mid create, unless the create outlived
        // anything it can take (a cancelled create that lost its record
        // while QEMU was starting leaves a VM only this daemon could end).
        Some(_) if starter_alive => {
            let max = if vm.recorder_dir.is_some() {
                p.recorder_max
            } else {
                p.create_max
            };
            vm.age >= max
        }
        Some(_) => vm.age >= p.starter_gone,
        None => vm.age >= p.unknown_starter,
    };
    unused.then_some(Reason::Unrecorded)
}

fn now_secs() -> u64 {
    cua_vmm::host::now_secs()
}

/// Stops the process of a VM (TERM, then KILL), after checking that the pid
/// is still that VM.
async fn end_process(vm: &HostVm) -> Result<(), String> {
    if !hostvms::still_running(vm) {
        return Ok(());
    }
    cua_vmm::host::signal(vm.pid, "TERM");
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        if !hostvms::still_running(vm) {
            return Ok(());
        }
    }
    cua_vmm::host::signal(vm.pid, "KILL");
    tokio::time::sleep(Duration::from_millis(500)).await;
    if hostvms::still_running(vm) {
        return Err(format!("pid {} did not exit", vm.pid));
    }
    Ok(())
}

/// Ends one VM: the process, its disk and its records. Verifies first that
/// the pid still is that VM (a reused pid is never signalled).
async fn end(vm: &HostVm, reason: &Reason) -> Result<(), String> {
    if !hostvms::still_running(vm) {
        return Ok(());
    }
    let home = vm.home();
    let rt = cua_vmm::qemu::QemuRuntime::new(cua_vmm::qemu::QemuConfig {
        root: vm.root.clone(),
        ..Default::default()
    });
    use cua_vmm::Runtime as _;
    match reason {
        Reason::ExpiredSuspend => {
            // Turned off, kept: `start` brings it back.
            rt.stop(&vm.name).await.map_err(|e| e.to_string())?;
        }
        // A golden recorder is no Space the runtime knows: end its process
        // and drop its recording directory.
        _ if vm.recorder_dir.is_some() => {
            end_process(vm).await.map_err(|k| k.to_string())?;
            let _ = std::fs::remove_dir_all(vm.dir());
        }
        _ => match rt.delete(&vm.name).await {
            Ok(()) | Err(cua_vmm::VmmError::NotFound(_)) => {}
            Err(e) => {
                // No usable state file: stop the process by its pid, then
                // drop the directory (only this VM's own).
                tracing::warn!(vm = %vm.name, error = %e, "ending a VM by its process");
                end_process(vm).await.map_err(|k| format!("{e}; {k}"))?;
                let _ = std::fs::remove_dir_all(vm.dir());
            }
        },
    }
    if !matches!(reason, Reason::ExpiredSuspend) {
        let state = StateStore::new(home.join("sandboxes"));
        let _ = state.delete(&vm.name);
        let _ = state.remove_lease(&vm.name);
        let _ = std::fs::remove_file(
            home.join("sandboxes/.pool")
                .join(format!("{}.json", vm.name)),
        );
        // And the Space's row in the registry, so it is not listed as a
        // Space with nothing behind it.
        #[cfg(feature = "spaces")]
        let _ = cua_spaces::registry::Registry::new(&home).remove(&format!("local:{}", vm.name));
    }
    Ok(())
}

/// One pass over `home`'s VMs. `dry_run` reports without ending anything.
pub async fn sweep_home(
    home: &Path,
    vms: &[HostVm],
    daemon_alive: bool,
    policy: &Policy,
    dry_run: bool,
) -> Vec<Ended> {
    let rec = Records::load(home);
    let now = now_secs();
    let mut out = Vec::new();
    for vm in vms {
        let starter = Launcher::read(&vm.dir());
        let starter_alive = starter.as_ref().is_some_and(Launcher::alive);
        let Some(reason) = judge(
            vm,
            &rec,
            daemon_alive,
            starter.as_ref(),
            starter_alive,
            now,
            policy,
        ) else {
            continue;
        };
        let error = if dry_run {
            None
        } else {
            let r = end(vm, &reason).await.err();
            if r.is_none() {
                tracing::info!(vm = %vm.name, pid = vm.pid, home = %home.display(), reason = ?reason, "ended a VM nobody uses");
            }
            r
        };
        out.push(Ended {
            name: vm.name.clone(),
            pid: vm.pid,
            home: home.to_path_buf(),
            reason,
            error,
        });
    }
    out
}

/// Sweeps `own` (`own_serving`: a daemon serves it) and every other home
/// with VMs on this host that has no daemon. Never touches another home
/// that has a live daemon.
pub async fn sweep(own: &Path, own_serving: bool, policy: &Policy, dry_run: bool) -> Vec<Ended> {
    sweep_vms(
        own,
        own_serving,
        hostvms::host_vms(),
        hostvms::homeless_vms(),
        policy,
        dry_run,
    )
    .await
}

/// [`sweep`] for `home` alone: another home's VMs and VMs whose home was
/// deleted are not looked at.
pub async fn sweep_own(home: &Path, serving: bool, policy: &Policy, dry_run: bool) -> Vec<Ended> {
    let vms = hostvms::vms_of(&home.join("vmm").join("qemu"));
    sweep_vms(home, serving, vms, Vec::new(), policy, dry_run).await
}

/// [`sweep`] over the VMs it is given (what `host_vms` and `homeless_vms`
/// found), so a test judges only VMs it started.
pub async fn sweep_vms(
    own: &Path,
    own_serving: bool,
    all: Vec<HostVm>,
    homeless: Vec<HostVm>,
    policy: &Policy,
    dry_run: bool,
) -> Vec<Ended> {
    let mut homes: Vec<PathBuf> = Vec::new();
    for vm in &all {
        let h = vm.home();
        if !homes.contains(&h) {
            homes.push(h);
        }
    }
    let mut out = Vec::new();
    // VMs whose home was deleted: only the process is left.
    for vm in homeless {
        let error = if dry_run {
            None
        } else {
            let r = end_process(&vm).await.err();
            if r.is_none() {
                tracing::info!(vm = %vm.name, pid = vm.pid, home = %vm.home().display(), "ended a VM whose home was deleted");
            }
            r
        };
        out.push(Ended {
            name: vm.name.clone(),
            pid: vm.pid,
            home: vm.home(),
            reason: Reason::HomeGone,
            error,
        });
    }
    for home in homes {
        let is_own = home == own;
        if !is_own && daemon_alive(&home) {
            continue;
        }
        let vms: Vec<HostVm> = all.iter().filter(|v| v.home() == home).cloned().collect();
        out.extend(sweep_home(&home, &vms, is_own && own_serving, policy, dry_run).await);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vm(name: &str, age: u64) -> HostVm {
        HostVm {
            pid: 1,
            name: name.into(),
            root: PathBuf::from("/h/vmm/qemu"),
            age: Duration::from_secs(age),
            recorder_dir: None,
        }
    }

    fn rec(sandboxes: &[&str], pool: &[&str], lifetimes: &[(&str, u64, &str)]) -> Records {
        Records {
            sandboxes: sandboxes.iter().map(|s| s.to_string()).collect(),
            pool: pool.iter().map(|s| s.to_string()).collect(),
            lifetimes: lifetimes
                .iter()
                .map(|(n, at, how)| (n.to_string(), (*at, how.to_string())))
                .collect(),
        }
    }

    #[test]
    fn a_sandboxes_own_lifetime_is_read_and_wins() {
        let home = tempfile::tempdir().unwrap();
        let state = home.path().join("sandboxes");
        std::fs::create_dir_all(&state).unwrap();
        std::fs::write(
            state.join("a.json"),
            r#"{"name":"a","expires_at_unix":500,"on_expire":"suspend"}"#,
        )
        .unwrap();
        std::fs::write(state.join("b.json"), r#"{"name":"b"}"#).unwrap();
        std::fs::write(
            home.path().join(LIFETIMES_FILE),
            r#"{"local:a":{"expires_at_unix":900,"on_expire":"delete"},"local:b":{"expires_at_unix":700,"on_expire":"delete"}}"#,
        )
        .unwrap();
        let r = Records::load(home.path());
        assert_eq!(r.lifetimes["a"], (500, "suspend".to_string()));
        assert_eq!(r.lifetimes["b"], (700, "delete".to_string()));
    }

    const NOW: u64 = 1_000_000;
    fn j(v: &HostVm, r: &Records, daemon: bool, starter: Option<bool>) -> Option<Reason> {
        let l = starter.map(|_| Launcher { pid: 1, at: 0 });
        judge(
            v,
            r,
            daemon,
            l.as_ref(),
            starter == Some(true),
            NOW,
            &Policy::default(),
        )
    }

    #[test]
    fn a_recorded_space_is_kept_without_a_daemon() {
        let r = rec(&["web"], &[], &[]);
        assert_eq!(j(&vm("web", 99_999), &r, false, Some(false)), None);
    }

    #[test]
    fn an_expired_lifetime_ends_the_space_with_or_without_a_daemon() {
        let r = rec(
            &["web", "db"],
            &[],
            &[("web", NOW - 1, "delete"), ("db", NOW - 5, "suspend")],
        );
        assert_eq!(
            j(&vm("web", 50), &r, false, Some(false)),
            Some(Reason::Expired)
        );
        assert_eq!(
            j(&vm("web", 50), &r, true, Some(true)),
            Some(Reason::Expired)
        );
        assert_eq!(
            j(&vm("db", 50), &r, false, None),
            Some(Reason::ExpiredSuspend)
        );
    }

    #[test]
    fn a_lifetime_still_running_keeps_the_space() {
        let r = rec(&["web"], &[], &[("web", NOW + 60, "delete")]);
        assert_eq!(j(&vm("web", 9999), &r, false, None), None);
    }

    #[test]
    fn a_lifetime_for_a_vm_nothing_else_records_does_not_make_it_ours_to_judge_as_a_space() {
        // Unrecorded VMs follow the unrecorded rules, not the lifetime's.
        let r = rec(&[], &[], &[("x", NOW - 1, "delete")]);
        assert_eq!(j(&vm("x", 10), &r, false, Some(true)), None);
    }

    #[test]
    fn warm_members_stay_with_a_running_daemon_and_go_without_one() {
        let r = rec(&[], &["cua-pool-1"], &[]);
        assert_eq!(j(&vm("cua-pool-1", 99_999), &r, true, Some(false)), None);
        assert_eq!(
            j(&vm("cua-pool-1", 99_999), &r, false, Some(false)),
            Some(Reason::AbandonedWarm)
        );
        // Not before the daemon has been gone a while.
        assert_eq!(j(&vm("cua-pool-1", 10), &r, false, Some(false)), None);
    }

    #[test]
    fn an_unrecorded_vm_goes_once_its_starter_is_gone() {
        let r = rec(&[], &[], &[]);
        assert_eq!(
            j(&vm("x", 600), &r, true, Some(false)),
            Some(Reason::Unrecorded)
        );
        // Mid create: the starter is running.
        assert_eq!(j(&vm("x", 600), &r, true, Some(true)), None);
        // A create that outlived any create (its record was lost while QEMU
        // started): ended even though its starter still runs.
        assert_eq!(
            j(&vm("x", 99_999), &r, true, Some(true)),
            Some(Reason::Unrecorded)
        );
        // Just started and gone: not yet (`starter_gone` is 15 s).
        assert_eq!(j(&vm("x", 10), &r, true, Some(false)), None);
    }

    #[test]
    fn an_unrecorded_vm_of_unknown_origin_waits_an_hour() {
        let r = rec(&[], &[], &[]);
        assert_eq!(j(&vm("x", 3000), &r, true, None), None);
        assert_eq!(j(&vm("x", 3601), &r, true, None), Some(Reason::Unrecorded));
    }

    #[test]
    fn an_unknown_pool_member_is_unrecorded() {
        let r = rec(&[], &[], &[]);
        assert_eq!(
            j(&vm("cua-pool-9", 600), &r, true, Some(false)),
            Some(Reason::Unrecorded)
        );
    }
}
