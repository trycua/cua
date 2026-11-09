//! What a pool holds to run the performance core: the settings, the golden
//! image's state, the cache and mirror stores, the per-slot folders a VM
//! sees, and what a claimed job brings.
//!
//! The pool's side of it (claiming, planning the warm pool, delivering a
//! job's files, the end of a job) is `pool/glue.rs`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::cachevol::{CachePlan, CachePolicy, CacheRoot, JobMeta, hash_for_event};
use super::golden::{GoldenSpec, GoldenState};
use super::mirror::{MirrorRef, RepoMirrors};
use super::timers::JobTimeline;
use super::warm::{Budget, WarmPolicy};
use super::{PerfConfig, StartMode};
use crate::config::GuestOs;
use crate::github::JitConfig;
use crate::{Result, Secret};

/// A job the service gave this computer (the `claim` response of
/// `ci-contract/CONTRACT.md`, as the pool needs it).
#[derive(Clone, Debug, Default)]
pub struct JobClaim {
    /// The service's id (`job_...`).
    pub job_id: String,
    /// `<run id>/<job id>`.
    pub job_key: String,
    /// The lease the JIT call and the reports use (opaque to the pool).
    pub lease: Option<String>,
    /// `owner/name`.
    pub repo: String,
    /// The CI account (`acct_...`).
    pub account: String,
    /// `GITHUB_EVENT_NAME`.
    pub event: Option<String>,
    /// `GITHUB_REF`.
    pub git_ref: Option<String>,
    /// The repository the head commit is in (a fork).
    pub head_repo: Option<String>,
    /// The repository's default branch.
    pub default_branch: Option<String>,
    /// The repository is private.
    pub private: Option<bool>,
    /// The service says this is a fork pull request.
    pub fork: Option<bool>,
    /// The service's cache policy (`policy.cache`).
    pub cache_policy: Option<CachePolicy>,
    /// `macos` or `linux`.
    pub class: Option<GuestOs>,
    /// `arm64` or `x64`.
    pub arch: String,
    /// The labels the runner registers with.
    pub labels: Vec<String>,
    /// The runner's name (the service chose it).
    pub runner_name: Option<String>,
    /// GitHub's `created_at` of the job (Unix ms).
    pub queued_at_ms: Option<u64>,
    /// Names the credential that may read a private repository (for the
    /// mirror); `None`: private repositories are not mirrored.
    pub credential_scope: Option<String>,
    /// A read credential for that scope that stays on the host; the hosted
    /// service never hands one out, so this is for `--own-app` pools.
    pub mirror_token: Option<Secret>,
    /// Where this job's JIT configuration comes from (the hosted
    /// service's `jit` call for its lease); `None`: the pool's own source.
    pub jit: Option<JitHandle>,
    /// The job must run with an enforced egress policy (a fork pull
    /// request): a VM that cannot enforce one is refused, not used bare.
    pub require_egress: bool,
    /// The account turned the egress policy off for this job.
    pub egress_off: bool,
}

/// A [`JitSource`] a claim carries.
#[derive(Clone)]
pub struct JitHandle(pub Arc<dyn JitSource>);

impl std::fmt::Debug for JitHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("JitHandle")
    }
}

impl JobClaim {
    /// What the cache and mirror rules read.
    pub fn meta(&self) -> JobMeta {
        JobMeta {
            account: self.account.clone(),
            repo: self.repo.clone(),
            private: self.private,
            event: self.event.clone(),
            git_ref: self.git_ref.clone(),
            head_repo: self.head_repo.clone(),
            default_branch: self.default_branch.clone(),
            fork: self.fork,
            policy_cache: self.cache_policy,
        }
    }
}

/// Where a job's JIT runner configuration comes from. The default is
/// GitHub with the pool's credentials; the hosted service replaces it with
/// its `jit` endpoint.
#[async_trait]
pub trait JitSource: Send + Sync {
    /// A one-job configuration for `claim`, registering a runner called
    /// `slot_name` unless the claim names its own.
    async fn jit(&self, claim: &JobClaim, slot_name: &str) -> Result<JitConfig>;
}

/// How a claimed job's slot ended, for the runner that is waiting on it.
#[derive(Clone, Debug)]
pub struct JobEnd {
    /// Why the slot ended (`job_done`, `runner_crashed`, `start_failed`,
    /// `aborted`, ...).
    pub reason: String,
    /// The job ran (a runner took it).
    pub ran: bool,
    /// The runner's word on the result (`Succeeded`, `Failed`).
    pub runner_result: Option<String>,
    /// The runner's exit code.
    pub exit_code: Option<i32>,
    /// The job's moments.
    pub timeline: JobTimeline,
    /// When the VM was claimed for the job.
    pub claimed_ms: Option<u64>,
    /// When the slot was created.
    pub created_ms: u64,
    /// How the VM started.
    pub start_mode: Option<StartMode>,
}

/// A job that was claimed but will not run here: the service puts it back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Released {
    /// The job.
    pub job_key: String,
    /// The lease.
    pub lease: Option<String>,
    /// Why (`capacity`, `create_failed`, `start_failed`, `interrupted`, ...).
    pub reason: String,
    /// The failure was the computer's, not the job's.
    pub infra: bool,
}

/// What a slot remembers of the job it was claimed for (kept in the state
/// file: a restart still knows what to release and what may be written
/// back).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobCtx {
    /// `<run id>/<job id>`.
    pub job_key: String,
    /// The lease.
    #[serde(default)]
    pub lease: Option<String>,
    /// `owner/name`.
    pub repo: String,
    /// The account.
    pub account: String,
    /// What the controller was told the event and ref were; the guest's
    /// own report must agree before a cache write is taken.
    #[serde(default)]
    pub event: Option<String>,
    /// The ref.
    #[serde(default)]
    pub git_ref: Option<String>,
    /// A fork's pull request.
    #[serde(default)]
    pub fork: bool,
    /// `default_branch`, `repo`, `fork_readonly` or empty.
    #[serde(default)]
    pub cache_scope: String,
    /// The job may hand back changed cache files when it succeeds.
    #[serde(default)]
    pub cache_write: bool,
    /// The scope key's folder name.
    #[serde(default)]
    pub cache_key: Option<String>,
    /// Stores delivered, with their toolchain keys.
    #[serde(default)]
    pub stores: Vec<(String, String)>,
    /// Report a refused write (fork pull request).
    #[serde(default)]
    pub report_denied: bool,
    /// The mirror was delivered.
    #[serde(default)]
    pub mirror: bool,
}

/// The performance core's notes on one slot.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotPerf {
    /// The golden image digest it was forked from.
    #[serde(default)]
    pub image_key: Option<String>,
    /// Which checkpoint variant.
    #[serde(default)]
    pub variant: Option<u32>,
    /// How it started.
    #[serde(default)]
    pub start_mode: Option<StartMode>,
    /// When it became a ready warm VM (Unix ms).
    #[serde(default)]
    pub warm_ms: Option<u64>,
    /// When a job claimed it.
    #[serde(default)]
    pub claimed_ms: Option<u64>,
    /// The job.
    #[serde(default)]
    pub job: Option<JobCtx>,
    /// The guest's clock minus the host's, ms, from the probe.
    #[serde(default)]
    pub guest_offset_ms: Option<i64>,
    /// The VM shares its per-slot folder (the Spaces backend mounts it).
    #[serde(default)]
    pub mounted: bool,
    /// Deletes of its Space that failed.
    #[serde(default)]
    pub delete_failed: u32,
    /// The runner's word on its job (`Succeeded`), kept for whoever waits.
    #[serde(default)]
    pub runner_result: Option<String>,
    /// The runner's exit code.
    #[serde(default)]
    pub exit_code: Option<i32>,
    /// A runner took a job in this slot.
    #[serde(default)]
    pub ran: bool,
}

/// Slots, caches and mirrors of one pool, plus the golden image.
pub struct PerfRuntime {
    /// The settings.
    pub cfg: PerfConfig,
    /// `<ci home>/perf`.
    pub root: PathBuf,
    /// The read-only caches (`<root>/cache`).
    pub caches: CacheRoot,
    /// Repository mirrors, when used.
    pub mirrors: Option<Arc<dyn RepoMirrors>>,
    /// JIT configurations, when not GitHub directly.
    pub jit: Option<Arc<dyn JitSource>>,
    pub(crate) golden: Mutex<GoldenState>,
    pub(crate) golden_spec: Mutex<Option<GoldenSpec>>,
    pub(crate) refreshing: AtomicBool,
    pub(crate) last_golden_check_ms: AtomicU64,
    pub(crate) released: Mutex<Vec<Released>>,
    pub(crate) ends: Mutex<std::collections::HashMap<String, JobEnd>>,
    /// Held while a cold claim checks for room and takes its slot, so two
    /// claims never count the same free VM.
    pub(crate) claim_gate: tokio::sync::Mutex<()>,
    queued_jobs: AtomicU32,
    last_demand_ms: AtomicU64,
}

impl PerfRuntime {
    /// Under `root`; `now_ms` starts the scale-to-zero clock.
    pub fn new(cfg: PerfConfig, root: impl Into<PathBuf>, now_ms: u64) -> Self {
        let root = root.into();
        let golden = GoldenState::load(&root.join("golden.json"));
        Self {
            caches: CacheRoot::new(root.join("cache")),
            cfg,
            root,
            mirrors: None,
            jit: None,
            golden: Mutex::new(golden),
            golden_spec: Mutex::new(None),
            refreshing: AtomicBool::new(false),
            last_golden_check_ms: AtomicU64::new(0),
            released: Mutex::new(Vec::new()),
            ends: Mutex::new(Default::default()),
            claim_gate: tokio::sync::Mutex::new(()),
            queued_jobs: AtomicU32::new(0),
            last_demand_ms: AtomicU64::new(now_ms),
        }
    }

    /// The runtime of a pool from its `perf.toml` (under the pool's own
    /// folder: `<pool dir>/perf/` holds the slots, caches and golden
    /// state); `None` when the pool has no such file.
    pub fn for_pool(
        paths: &crate::paths::CiPaths,
        pool: &str,
        now_ms: u64,
    ) -> Result<Option<Self>> {
        let Some(cfg) = PerfConfig::load(&PerfConfig::path(paths, pool))? else {
            return Ok(None);
        };
        Ok(Some(Self::new(
            cfg,
            paths.pool_dir(pool).join("perf"),
            now_ms,
        )))
    }

    /// With repository mirrors.
    pub fn with_mirrors(mut self, m: Arc<dyn RepoMirrors>) -> Self {
        self.mirrors = Some(m);
        self
    }

    /// With a JIT source.
    pub fn with_jit(mut self, j: Arc<dyn JitSource>) -> Self {
        self.jit = Some(j);
        self
    }

    /// What the golden image should contain; with none set the pool forks
    /// only from a golden image already built.
    pub fn set_golden_spec(&self, spec: GoldenSpec) {
        *self.golden_spec.lock().expect("golden spec") = Some(spec);
    }

    /// The golden image's state file.
    pub fn golden_path(&self) -> PathBuf {
        self.root.join("golden.json")
    }

    /// A copy of the golden state.
    pub fn golden_state(&self) -> GoldenState {
        self.golden.lock().expect("golden").clone()
    }

    /// Jobs the service says are queued for this computer (a hint that
    /// raises the warm target; `claim` loops update it).
    pub fn set_queued_jobs(&self, n: u32) {
        self.queued_jobs.store(n, Ordering::Relaxed);
    }

    pub(crate) fn queued_jobs(&self) -> u32 {
        self.queued_jobs.load(Ordering::Relaxed)
    }

    /// How a claimed job's slot ended; taken once.
    pub fn take_end(&self, slot: &str) -> Option<JobEnd> {
        self.ends.lock().expect("ends").remove(slot)
    }

    /// A job arrived or is queued: the scale-to-zero clock restarts.
    pub fn mark_demand(&self, now_ms: u64) {
        self.last_demand_ms.fetch_max(now_ms, Ordering::Relaxed);
    }

    pub(crate) fn last_demand_ms(&self) -> u64 {
        self.last_demand_ms.load(Ordering::Relaxed)
    }

    /// The warm pool's policy at `cap`, for `warm` VMs, with the month's
    /// spend.
    pub fn warm_policy(&self, warm: u32, cap: u32, spent_usd: f64) -> WarmPolicy {
        WarmPolicy {
            warm,
            cap,
            ttl_ms: self.cfg.warm_ttl().as_millis() as u64,
            scale_to_zero_after_ms: self.cfg.scale_to_zero_after().as_millis() as u64,
            golden_key: if self.cfg.fork {
                self.golden.lock().expect("golden").key()
            } else {
                None
            },
            budget: self.cfg.budget.as_ref().map(|b| Budget {
                limit_usd: b.monthly_usd,
                spent_usd,
                usd_per_hour: b.usd_per_hour,
            }),
        }
    }

    // ---- the per-slot folder a VM sees ------------------------------------

    /// `<root>/slots/<slot>`: what the VM of this slot sees read-only
    /// (`cache/` and `mirror/`).
    pub fn slot_dir(&self, slot: &str) -> PathBuf {
        self.root.join("slots").join(slot)
    }

    /// Creates an empty slot folder.
    pub fn create_slot_dir(&self, slot: &str) -> std::io::Result<PathBuf> {
        let d = self.slot_dir(slot);
        std::fs::create_dir_all(d.join("cache"))?;
        std::fs::create_dir_all(d.join("mirror"))?;
        Ok(d)
    }

    /// Removes a slot folder (and with it the links into the caches).
    pub fn remove_slot_dir(&self, slot: &str) {
        let _ = std::fs::remove_dir_all(self.slot_dir(slot));
    }

    /// Fills a slot folder for a job: the stores of the plan's scope that
    /// have a current version (linked, not copied, so it is instant and
    /// the host's version stays immutable) and the mirror.
    pub fn populate_slot_dir(
        &self,
        slot: &str,
        plan: &CachePlan,
        want: &[String],
        mirror: Option<&MirrorRef>,
    ) -> std::io::Result<Populated> {
        let dir = self.create_slot_dir(slot)?;
        for sub in ["cache", "mirror"] {
            // A previous job's links never survive into this one.
            let _ = std::fs::remove_dir_all(dir.join(sub));
            std::fs::create_dir_all(dir.join(sub))?;
        }
        let versions = self
            .golden
            .lock()
            .expect("golden")
            .current
            .as_ref()
            .map(|r| r.versions.clone())
            .unwrap_or_default();
        let mut out = Populated::default();
        if let Some(key) = &plan.read {
            for (store, tc) in self.caches.available(key, want, &versions) {
                if let Some(src) = self.caches.current(key, &store, &tc) {
                    link_tree(
                        &src,
                        &dir.join("cache").join(&store).join(&tc).join("current"),
                    )?;
                    out.stores.push((store, tc));
                }
            }
            self.caches.touch(key);
        }
        if let Some(m) = mirror {
            let src = m.folder.join(super::mirror::REPO_DIR);
            if src.is_dir() {
                link_tree(&src, &dir.join("mirror").join(super::mirror::REPO_DIR))?;
                out.mirror = true;
            }
        }
        Ok(out)
    }
}

/// What [`PerfRuntime::populate_slot_dir`] put in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Populated {
    /// Stores delivered, with toolchain keys.
    pub stores: Vec<(String, String)>,
    /// The mirror was delivered.
    pub mirror: bool,
}

/// The event hashes of a cache read: the scope key's folder, the writer's
/// and the reader's repository.
pub fn read_hashes(scope_key: &str, repo: &str) -> (String, String, String) {
    (
        hash_for_event(scope_key),
        hash_for_event(repo),
        hash_for_event(repo),
    )
}

/// Links `src` into `dst` as a tree of its own: a clone where the
/// filesystem has one (APFS `cp -c`), hard links where it can, copies
/// otherwise. The result is an independent directory; a file is never
/// modified in place anywhere in a cache, so a shared inode is safe.
pub fn link_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    if let Some(p) = dst.parent() {
        std::fs::create_dir_all(p)?;
    }
    let _ = std::fs::remove_dir_all(dst);
    #[cfg(target_os = "macos")]
    let fast = std::process::Command::new("cp")
        .arg("-cR")
        .arg(src)
        .arg(dst)
        .output();
    #[cfg(all(unix, not(target_os = "macos")))]
    let fast = std::process::Command::new("cp")
        .arg("-al")
        .arg(src)
        .arg(dst)
        .output();
    #[cfg(unix)]
    if fast.is_ok_and(|o| o.status.success()) {
        return Ok(());
    }
    let _ = std::fs::remove_dir_all(dst);
    walk_link(src, dst)
}

fn walk_link(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for e in std::fs::read_dir(src)? {
        let e = e?;
        let t = e.file_type()?;
        let to = dst.join(e.file_name());
        if t.is_dir() {
            walk_link(&e.path(), &to)?;
        } else if t.is_symlink() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(std::fs::read_link(e.path())?, &to)?;
        } else if t.is_file() && std::fs::hard_link(e.path(), &to).is_err() {
            std::fs::copy(e.path(), &to)?;
        }
    }
    Ok(())
}

/// A map of `store -> toolchain key` as the job's stores.
pub fn stores_map(v: &[(String, String)]) -> BTreeMap<String, String> {
    v.iter().cloned().collect()
}

#[cfg(test)]
mod tests;
