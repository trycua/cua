// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! VMs and containers a failed Space create left behind: finding them and
//! removing them.
//!
//! A local Space create that failed after its instance existed could leave
//! that instance (a running VM, a container) with no Space record: nothing
//! lists it, nothing stops it, and the memory and vCPUs it holds make every
//! later create on this machine fail for lack of room. Creates now clean up
//! after themselves; this sweep removes what earlier ones left. It runs a
//! little after the daemon starts, then every [`EVERY`].
//!
//! An instance is removed only when all of these hold:
//!
//! | rule | why |
//! |---|---|
//! | a local runtime lists it as Cua's own ([`crate::local::VmmLocal::cua_instances`]) | nothing another tool or the person made is listed |
//! | its name is in the form a Space create generates (`space-` and ten hex digits) | only a Space create names instances so |
//! | no Space record anywhere in this home names it ([`Known`]) | a Space (running, stopped, suspended, pooled, provided to another machine) or a create in flight is never touched |
//! | it has been all of the above for [`GRACE`] | a create that is just starting, or registering, is never hit |
//! | every runtime that is up answered, and every record could be read | a partial view never looks like "nothing there" |
//!
//! Anything listed whose ownership can't be proven (another name) is kept
//! and logged once. Each removal is logged with its name, runtime and how
//! long it was unclaimed; no paths. A removed instance's own sandbox record
//! goes with it, so nothing keeps counting its size as reserved.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

/// How long an instance must stay unclaimed before it is removed.
pub const GRACE: Duration = Duration::from_secs(10 * 60);
/// Spacing of passes.
pub const EVERY: Duration = Duration::from_secs(5 * 60);
/// The first pass after the daemon starts.
pub const FIRST: Duration = Duration::from_secs(30);
/// `CUA_SPACE_ORPHAN_SWEEP=0` turns the sweep off.
pub const ENV_SWEEP: &str = "CUA_SPACE_ORPHAN_SWEEP";

/// Record files of a home that may name a Space's instance. A name found
/// in any of them is a Space's (or a create's) and is kept.
const RECORD_FILES: &[&str] = &[
    cua_spaces::registry::SPACES_FILE,
    cua_spaces::registry::CREDENTIALS_FILE,
    cua_spaces::registry::META_FILE,
    cua_spaces::registry::DIRECT_HOSTS_FILE,
    cua_spaces::lifetimes::LIFETIMES_FILE,
    "host/provided-spaces.json",
];
/// Record directories, read whole: creates in flight (`creating`), child
/// Spaces (`spawn`), durable Spaces and the account's pool (`pool`),
/// prewarm and elastic settings (`elastic`), warm-pool members and
/// ephemeral leases (`sandboxes/.pool`, `sandboxes/.ephemeral`).
const RECORD_DIRS: &[&str] = &[
    "creating",
    "spawn",
    "pool",
    "elastic",
    "sandboxes/.pool",
    "sandboxes/.ephemeral",
];
/// A record file larger than this can't be read in full: nothing is
/// removed in that pass.
const MAX_RECORD_BYTES: u64 = 256 << 20;

/// One instance a local runtime lists as Cua's.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Found {
    /// Instance name.
    pub name: String,
    /// The runtime that lists it (`qemu`, `container`, `vz`, `lume`, ...).
    pub runtime: String,
}

/// Everything this home's Space records say, as raw bytes: a name that
/// appears anywhere in them is known. Matching text, not parsed fields,
/// keeps an instance whenever any record mentions it, including records of
/// shapes this sweep has never seen.
#[derive(Clone, Debug, Default)]
pub struct Known {
    corpus: Vec<Vec<u8>>,
}

impl Known {
    /// Reads `home`'s records. Missing ones are fine; one that exists but
    /// can't be read is an error (and nothing is removed).
    pub fn load(home: &Path) -> Result<Self, String> {
        let mut k = Self::default();
        for f in RECORD_FILES {
            k.read_file(&home.join(f))?;
        }
        for d in RECORD_DIRS {
            k.read_dir(&home.join(d), 4)?;
        }
        Ok(k)
    }

    /// Known from in-memory records (tests).
    pub fn from_texts<I: IntoIterator<Item = S>, S: Into<String>>(texts: I) -> Self {
        Self {
            corpus: texts.into_iter().map(|t| t.into().into_bytes()).collect(),
        }
    }

    fn read_file(&mut self, path: &Path) -> Result<(), String> {
        let meta = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(format!("a Space record could not be read ({})", e.kind())),
        };
        if !meta.is_file() {
            return Ok(());
        }
        if meta.len() > MAX_RECORD_BYTES {
            return Err("a Space record is too large to read".into());
        }
        match std::fs::read(path) {
            Ok(b) => {
                self.corpus.push(b);
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("a Space record could not be read ({})", e.kind())),
        }
    }

    fn read_dir(&mut self, dir: &Path, depth: u32) -> Result<(), String> {
        let rd = match std::fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => {
                return Err(format!(
                    "a Space record folder could not be read ({})",
                    e.kind()
                ));
            }
        };
        let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                if depth == 0 {
                    return Err("Space records are nested too deep to read".into());
                }
                self.read_dir(&p, depth - 1)?;
            } else {
                self.read_file(&p)?;
            }
        }
        Ok(())
    }

    /// Whether any record names `name`.
    pub fn mentions(&self, name: &str) -> bool {
        let n = name.as_bytes();
        !n.is_empty()
            && self
                .corpus
                .iter()
                .any(|b| b.len() >= n.len() && b.windows(n.len()).any(|w| w == n))
    }
}

/// The verdict on one listed instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// A Space record names it: kept.
    Known,
    /// Not in the form a Space create names instances: Cua can't prove a
    /// failed create left it, so it is kept.
    Unproven,
    /// Unclaimed, but not yet for [`GRACE`]: kept for now.
    Young {
        /// Seconds it has been unclaimed.
        age: u64,
    },
    /// Unclaimed for at least [`GRACE`]: removed.
    Orphan {
        /// Seconds it has been unclaimed.
        age: u64,
    },
}

/// The verdict on one instance. Pure.
pub fn judge(found: &Found, known: &Known, unclaimed_since: Option<u64>, now: u64) -> Verdict {
    if known.mentions(&found.name) {
        return Verdict::Known;
    }
    if !cua_spaces::is_generated_name(&found.name) {
        return Verdict::Unproven;
    }
    let age = unclaimed_since.map_or(0, |t| now.saturating_sub(t));
    if age >= GRACE.as_secs() {
        Verdict::Orphan { age }
    } else {
        Verdict::Young { age }
    }
}

/// What one pass decided.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// To remove: the instance and how long it was unclaimed (seconds).
    pub remove: Vec<(Found, u64)>,
    /// Kept because ownership can't be proven (first seen this pass).
    pub unproven: Vec<Found>,
    /// Why nothing at all is removed this pass (a runtime or a record
    /// could not be read).
    pub halted: Option<String>,
}

/// The sweep's memory between passes: since when each candidate has been
/// unclaimed. Kept in memory only, so a daemon restart starts the clock
/// again (an instance is never removed sooner than [`GRACE`] after this
/// daemon first saw it unclaimed).
#[derive(Debug, Default)]
pub struct Sweeper {
    unclaimed: HashMap<Found, u64>,
    reported: HashSet<Found>,
}

impl Sweeper {
    /// Decides one pass from what the runtimes listed and what the records
    /// say. Pure but for the sweeper's memory.
    pub fn plan(
        &mut self,
        listed: Result<Vec<Found>, String>,
        known: Result<Known, String>,
        now: u64,
    ) -> Plan {
        let (listed, known) = match (listed, known) {
            (Ok(l), Ok(k)) => (l, k),
            (Err(e), _) | (_, Err(e)) => {
                // An unanswered listing proves nothing: the clock starts
                // again once it answers.
                self.unclaimed.clear();
                return Plan {
                    halted: Some(e),
                    ..Default::default()
                };
            }
        };
        let mut plan = Plan::default();
        let mut still = HashMap::new();
        for f in listed {
            match judge(&f, &known, self.unclaimed.get(&f).copied(), now) {
                Verdict::Known => {}
                Verdict::Unproven => {
                    if self.reported.insert(f.clone()) {
                        plan.unproven.push(f);
                    }
                }
                Verdict::Young { .. } => {
                    let since = self.unclaimed.get(&f).copied().unwrap_or(now);
                    still.insert(f, since);
                }
                Verdict::Orphan { age } => {
                    let since = self.unclaimed[&f];
                    still.insert(f.clone(), since);
                    plan.remove.push((f, age));
                }
            }
        }
        self.unclaimed = still;
        plan
    }

    /// Forgets `f` (it was removed).
    fn removed(&mut self, f: &Found) {
        self.unclaimed.remove(f);
    }
}

/// What the sweep removes instances through.
#[async_trait::async_trait]
pub trait Host: Send + Sync {
    /// Every instance Cua made, in every local runtime that is up.
    async fn list(&self) -> Result<Vec<Found>, String>;
    /// Stops and deletes `name` on `runtime` only (gone already is fine).
    async fn delete(&self, runtime: &str, name: &str) -> Result<(), String>;
}

#[async_trait::async_trait]
impl Host for crate::local::VmmLocal {
    async fn list(&self) -> Result<Vec<Found>, String> {
        self.cua_instances().await
    }
    async fn delete(&self, runtime: &str, name: &str) -> Result<(), String> {
        self.delete_on(runtime, name).await
    }
}

/// One removal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Removed {
    /// The instance.
    pub found: Found,
    /// How long it was unclaimed (seconds).
    pub age: u64,
    /// The delete failed (it is tried again next pass).
    pub error: Option<String>,
}

/// One pass over `home`: lists, decides, and removes what [`Sweeper::plan`]
/// marks, reading the records again right before (a create that registered
/// meanwhile keeps its instance). A removed instance's sandbox record and
/// lease go too: they would keep its size counted as reserved.
pub async fn pass(home: &Path, host: &dyn Host, sweeper: &mut Sweeper, now: u64) -> Vec<Removed> {
    let plan = sweeper.plan(host.list().await, Known::load(home), now);
    if let Some(why) = &plan.halted {
        tracing::info!(reason = %why, "skipped the sweep for VMs left by failed Space creates");
        return vec![];
    }
    for f in &plan.unproven {
        tracing::info!(
            id = %f.name,
            runtime = %f.runtime,
            "kept a Cua VM or container no Space names: it is not provably left by a failed create"
        );
    }
    if plan.remove.is_empty() {
        return vec![];
    }
    let Ok(again) = Known::load(home) else {
        return vec![];
    };
    let state = cua_sandbox_core::StateStore::new(home.join("sandboxes"));
    let mut out = Vec::new();
    for (f, age) in plan.remove {
        if again.mentions(&f.name) {
            sweeper.removed(&f);
            continue;
        }
        let error = host.delete(&f.runtime, &f.name).await.err();
        match &error {
            None => {
                let _ = state.delete(&f.name);
                let _ = state.remove_lease(&f.name);
                sweeper.removed(&f);
                tracing::info!(
                    id = %f.name,
                    runtime = %f.runtime,
                    age_secs = age,
                    "removed a VM or container a failed Space create left"
                );
            }
            Some(e) => tracing::warn!(
                id = %f.name,
                runtime = %f.runtime,
                age_secs = age,
                error = %e,
                "could not remove a VM or container a failed Space create left; trying again later"
            ),
        }
        out.push(Removed {
            found: f,
            age,
            error,
        });
    }
    out
}

fn unix_now() -> u64 {
    cua_vmm::host::now_secs()
}

/// Runs [`pass`] [`FIRST`] after start, then every [`EVERY`], until
/// shutdown.
pub async fn sweep_loop(home: PathBuf, host: Arc<dyn Host>, mut shutdown: watch::Receiver<bool>) {
    let mut sweeper = Sweeper::default();
    let mut wait = FIRST;
    loop {
        tokio::select! {
            _ = shutdown.changed() => return,
            _ = tokio::time::sleep(wait) => {}
        }
        pass(&home, host.as_ref(), &mut sweeper, unix_now()).await;
        wait = EVERY;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    const ORPHAN: &str = "space-00000000aa";
    const SPACE: &str = "space-00000000bb";
    const NOW: u64 = 1_800_000_000;

    fn found(name: &str, runtime: &str) -> Found {
        Found {
            name: name.into(),
            runtime: runtime.into(),
        }
    }

    #[derive(Default)]
    struct FakeHost {
        listed: Mutex<Option<Result<Vec<Found>, String>>>,
        deleted: Mutex<Vec<(String, String)>>,
    }

    impl FakeHost {
        fn with(rows: Result<Vec<Found>, String>) -> Self {
            Self {
                listed: Mutex::new(Some(rows)),
                ..Default::default()
            }
        }
    }

    #[async_trait::async_trait]
    impl Host for FakeHost {
        async fn list(&self) -> Result<Vec<Found>, String> {
            self.listed.lock().unwrap().clone().unwrap_or(Ok(vec![]))
        }
        async fn delete(&self, runtime: &str, name: &str) -> Result<(), String> {
            self.deleted
                .lock()
                .unwrap()
                .push((runtime.into(), name.into()));
            Ok(())
        }
    }

    fn home_with_space(name: &str) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join(cua_spaces::registry::SPACES_FILE),
            format!(r#"[{{"id":"local:{name}","name":"{name}","status":"stopped"}}]"#),
        )
        .unwrap();
        d
    }

    #[test]
    fn a_known_space_is_kept_whatever_its_state_or_age() {
        let k = Known::from_texts([format!(r#"[{{"id":"local:{SPACE}"}}]"#)]);
        assert_eq!(
            judge(&found(SPACE, "qemu"), &k, Some(0), NOW),
            Verdict::Known
        );
        // A record of any shape that names it keeps it: a warm-pool member,
        // a Space provided to another machine, a create in flight.
        for rec in [
            format!(r#"{{"name":"{ORPHAN}","pid":1,"made":[]}}"#),
            format!(r#"{{"spaces":[{{"instance":"{ORPHAN}"}}]}}"#),
        ] {
            let k = Known::from_texts([rec]);
            assert_eq!(
                judge(&found(ORPHAN, "container"), &k, Some(0), NOW),
                Verdict::Known
            );
        }
    }

    #[test]
    fn an_unmarked_instance_is_never_removed() {
        let k = Known::default();
        for name in [
            "my-vm",
            "space-1",
            "space-00000000AA",
            "cua-base-abc",
            "build",
        ] {
            assert_eq!(
                judge(&found(name, "qemu"), &k, Some(0), NOW),
                Verdict::Unproven,
                "{name}"
            );
        }
        let mut s = Sweeper::default();
        let listed = || Ok(vec![found("my-vm", "container")]);
        let p = s.plan(listed(), Ok(Known::default()), NOW);
        assert!(p.remove.is_empty());
        assert_eq!(p.unproven, vec![found("my-vm", "container")]);
        // Reported once, never removed.
        let p = s.plan(listed(), Ok(Known::default()), NOW + 86_400);
        assert!(p.remove.is_empty() && p.unproven.is_empty());
    }

    #[test]
    fn an_orphan_is_removed_only_after_the_grace_period() {
        let mut s = Sweeper::default();
        let listed = || Ok(vec![found(ORPHAN, "qemu")]);
        let p = s.plan(listed(), Ok(Known::default()), NOW);
        assert!(p.remove.is_empty(), "first seen: {p:?}");
        let p = s.plan(listed(), Ok(Known::default()), NOW + GRACE.as_secs() - 1);
        assert!(p.remove.is_empty(), "younger than the grace period: {p:?}");
        let p = s.plan(listed(), Ok(Known::default()), NOW + GRACE.as_secs());
        assert_eq!(p.remove, vec![(found(ORPHAN, "qemu"), GRACE.as_secs())]);
    }

    #[test]
    fn a_space_registered_meanwhile_restarts_the_clock() {
        let mut s = Sweeper::default();
        let listed = || Ok(vec![found(ORPHAN, "qemu")]);
        s.plan(listed(), Ok(Known::default()), NOW);
        let k = Known::from_texts([format!(r#"[{{"id":"local:{ORPHAN}"}}]"#)]);
        assert!(s.plan(listed(), Ok(k), NOW + 60).remove.is_empty());
        // Its record went (it was deleted, then the name came back unclaimed):
        // a fresh grace period.
        let p = s.plan(listed(), Ok(Known::default()), NOW + GRACE.as_secs() + 60);
        assert!(p.remove.is_empty(), "{p:?}");
    }

    #[test]
    fn a_failed_listing_removes_nothing_and_restarts_the_clock() {
        let mut s = Sweeper::default();
        let listed = || Ok(vec![found(ORPHAN, "qemu")]);
        s.plan(listed(), Ok(Known::default()), NOW);
        let p = s.plan(
            Err("container: engine did not answer".into()),
            Ok(Known::default()),
            NOW + GRACE.as_secs(),
        );
        assert!(p.remove.is_empty());
        assert!(p.halted.is_some());
        // Unreadable records halt it too.
        let p = s.plan(listed(), Err("unreadable".into()), NOW + GRACE.as_secs());
        assert!(p.remove.is_empty() && p.halted.is_some());
        // Once both answer again, the grace period starts over.
        let p = s.plan(listed(), Ok(Known::default()), NOW + 2 * GRACE.as_secs());
        assert!(p.remove.is_empty(), "{p:?}");
    }

    #[test]
    fn records_are_read_from_the_home() {
        let home = home_with_space(SPACE);
        let journal = home.path().join("creating");
        std::fs::create_dir_all(&journal).unwrap();
        std::fs::write(
            journal.join("c1.json"),
            r#"{"name":"space-00000000cc","pid":1}"#,
        )
        .unwrap();
        let k = Known::load(home.path()).unwrap();
        assert!(k.mentions(SPACE));
        assert!(k.mentions("space-00000000cc"));
        assert!(!k.mentions(ORPHAN));
        // An empty home has no records, which is fine.
        let empty = tempfile::tempdir().unwrap();
        assert!(!Known::load(empty.path()).unwrap().mentions(SPACE));
    }

    #[tokio::test]
    async fn a_pass_removes_the_orphan_frees_its_reservation_and_keeps_the_rest() {
        let home = home_with_space(SPACE);
        // The orphan's own sandbox record (a create that started it, then
        // failed to connect) counts its size as reserved until it goes.
        let state = cua_sandbox_core::StateStore::new(home.path().join("sandboxes"));
        state
            .save(&cua_sandbox_core::state::SandboxState::Local(
                cua_sandbox_core::state::LocalState {
                    name: ORPHAN.into(),
                    runtime_type: "qemu".into(),
                    status: "running".into(),
                    memory_mb: Some(4096),
                    cpu_count: Some(2),
                    ..Default::default()
                },
            ))
            .unwrap();
        let host = FakeHost::with(Ok(vec![
            found(ORPHAN, "qemu"),
            found(SPACE, "qemu"),
            found("theirs", "container"),
        ]));
        let mut s = Sweeper::default();
        assert!(pass(home.path(), &host, &mut s, NOW).await.is_empty());
        assert!(state.load(ORPHAN).is_some());
        let r = pass(home.path(), &host, &mut s, NOW + GRACE.as_secs()).await;
        assert_eq!(r.len(), 1, "{r:?}");
        assert_eq!(r[0].found, found(ORPHAN, "qemu"));
        assert!(r[0].error.is_none());
        assert_eq!(
            *host.deleted.lock().unwrap(),
            vec![("qemu".to_string(), ORPHAN.to_string())]
        );
        // Its reservation is gone with it.
        assert!(state.load(ORPHAN).is_none());
        assert!(state.list_all().iter().all(|s| s.name() != ORPHAN));
    }

    #[tokio::test]
    async fn a_pass_with_a_failed_listing_deletes_nothing() {
        let home = tempfile::tempdir().unwrap();
        let host = FakeHost::with(Err("qemu: unreadable".into()));
        let mut s = Sweeper::default();
        for t in [NOW, NOW + GRACE.as_secs(), NOW + 2 * GRACE.as_secs()] {
            assert!(pass(home.path(), &host, &mut s, t).await.is_empty());
        }
        assert!(host.deleted.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_space_registered_before_the_grace_period_ends_keeps_its_instance() {
        let home = tempfile::tempdir().unwrap();
        let host = FakeHost::with(Ok(vec![found(ORPHAN, "vz")]));
        let mut s = Sweeper::default();
        pass(home.path(), &host, &mut s, NOW).await;
        // The create finished registering its Space.
        std::fs::write(
            home.path().join(cua_spaces::registry::SPACES_FILE),
            format!(r#"[{{"id":"local:{ORPHAN}"}}]"#),
        )
        .unwrap();
        assert!(
            pass(home.path(), &host, &mut s, NOW + GRACE.as_secs())
                .await
                .is_empty()
        );
        assert!(host.deleted.lock().unwrap().is_empty());
    }
}
