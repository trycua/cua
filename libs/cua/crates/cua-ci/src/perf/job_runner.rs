//! The performance core as the hosted agent's [`JobRunner`].
//!
//! [`crate::job_runner::SpaceJobRunner`] is the plain cold path: one new
//! Space per job. [`PerfJobRunner`] runs the same jobs through a
//! [`Pool`] in claim mode instead: a warm VM, a fork of the golden image
//! or a cold create, the job's caches and repository mirror, the phase
//! events, teardown guarantees. It waits for the job to end (the pool's
//! own loop sees the runner exit) and turns what the pool recorded into
//! the [`JobOutcome`] and the usage the service meters.
//!
//! One pool serves one class (a macOS pool and a Linux pool for a computer
//! that serves both): [`hosted_pool_config`] and [`hosted_pool`] build
//! them. Their configuration is made here, not read from a file, because
//! a hosted pool has no GitHub scope or credential: the service registers
//! the runners.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::watch;

use super::runtime::{JitHandle, JitSource, JobClaim, JobEnd, PerfRuntime};
use super::timers::{normalize_result, parse_iso_ms};
use crate::cache::TarballSource;
use crate::config::{GuestOs, PoolConfig};
use crate::github::{DeleteOutcome, JitConfig, JitRequest as GhJitRequest, Runner, RunnerApi};
use crate::hosted::{JobHandle, JobOutcome, JobRunner, ServiceApi, ServiceClient, rfc3339};
use crate::paths::CiPaths;
use crate::perf::cachevol::CachePolicy;
use crate::pool::{ClaimError, Clock, Phase, Pool, PoolDeps};
use crate::service_api::{
    CacheAccess, CacheStats, ClaimedJob, Class, JitRequest, Phases as ApiPhases, ReportState, Usage,
};
use crate::spaces::Spaces;
use crate::{Error, Result};

/// What the service says about a repository (the claim does not carry it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoInfo {
    /// A private repository.
    pub private: bool,
    /// Its default branch.
    pub default_branch: String,
}

/// Where the repositories of the account are listed.
#[async_trait]
pub trait RepoDirectory: Send + Sync {
    /// Every repository the App sees, by `owner/name`; `None` when the
    /// service cannot say now.
    async fn repos(&self) -> Option<HashMap<String, RepoInfo>>;
}

#[async_trait]
impl RepoDirectory for ServiceClient {
    async fn repos(&self) -> Option<HashMap<String, RepoInfo>> {
        let (r, _) = ServiceClient::repos(self).await.ok()?;
        Some(
            r.repos
                .into_iter()
                .map(|x| {
                    (
                        x.full_name.to_ascii_lowercase(),
                        RepoInfo {
                            private: x.private,
                            default_branch: x.default_branch,
                        },
                    )
                })
                .collect(),
        )
    }
}

/// The service's `jit` call for a claimed job's lease.
struct ServiceJit {
    api: Arc<dyn ServiceApi>,
    machine_id: String,
}

#[async_trait]
impl JitSource for ServiceJit {
    async fn jit(&self, claim: &JobClaim, _slot_name: &str) -> Result<JitConfig> {
        let r = self
            .api
            .jit(
                &claim.job_id,
                &JitRequest {
                    lease_id: claim.lease.clone().unwrap_or_default(),
                    machine_id: self.machine_id.clone(),
                },
            )
            .await?;
        Ok(JitConfig {
            runner_id: r.runner_id,
            runner_name: r.runner_name,
            encoded: r.encoded_jit_config,
        })
    }
}

/// GitHub as a hosted pool sees it: not at all. The service holds the App
/// and registers the runners.
pub struct NoGitHub;

#[async_trait]
impl RunnerApi for NoGitHub {
    async fn runner_group_id(&self, _name: Option<&str>) -> Result<u64> {
        Ok(0)
    }
    async fn generate_jit(&self, _req: &GhJitRequest) -> Result<JitConfig> {
        Err(Error::GitHub(
            "a hosted pool gets its runner configuration from the Cua CI service".into(),
        ))
    }
    async fn list_runners(&self) -> Result<Vec<Runner>> {
        Ok(Vec::new())
    }
    async fn delete_runner(&self, _id: u64) -> Result<DeleteOutcome> {
        Ok(DeleteOutcome::NotFound)
    }
}

/// A pool name for a computer: `hosted-` and the first characters of its
/// id (lowercase letters, digits and `-`).
pub fn hosted_pool_name(machine_id: &str, class: Class) -> String {
    let id: String = machine_id
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(12)
        .collect();
    format!("hosted-{}-{id}", class.as_str())
}

/// The configuration of a hosted pool: one class on `on`, up to
/// `max_runners` VMs, `warm` kept ready. It carries a placeholder GitHub
/// scope (never contacted).
pub fn hosted_pool_config(
    name: &str,
    class: Class,
    image: &str,
    on: &str,
    max_runners: u32,
    warm: u32,
) -> Result<PoolConfig> {
    let os = match class {
        Class::Macos => "macos",
        Class::Linux => "linux",
    };
    PoolConfig::parse(&format!(
        r#"
name = "{name}"
scope = "org:cua-hosted"
image = "{image}"
os = "{os}"
on = "{on}"
max_runners = {max_runners}
warm = {warm}

[github]
app_id = 1
secret_service = "cua-ci-hosted"
secret_account = "{name}"
"#
    ))
}

/// Opens a hosted pool in claim mode.
pub fn hosted_pool(
    cfg: PoolConfig,
    paths: CiPaths,
    spaces: Arc<dyn Spaces>,
    tarballs: Arc<dyn TarballSource>,
    clock: Arc<dyn Clock>,
    host: Option<crate::config::HostResources>,
    perf: Arc<PerfRuntime>,
) -> Result<Arc<Pool>> {
    let pool = Pool::open(
        cfg,
        paths,
        PoolDeps {
            spaces,
            github: Arc::new(NoGitHub),
            tarballs,
            clock,
            host,
        },
    )?
    .with_perf(perf)
    .hosted();
    Ok(Arc::new(pool))
}

/// One class a hosted computer serves through a performance pool.
#[derive(Clone, Debug)]
pub struct PerfClass {
    /// Image, size and class (the same record the cold runner takes).
    pub image: crate::job_runner::ClassImage,
    /// `arm64` or `x64`.
    pub arch: String,
    /// Jobs at once.
    pub slots: u32,
    /// Warm VMs kept ready (the pool's planner caps it: Apple allows two
    /// VMs at once, budgets and the idle TTL still apply).
    pub warm: u32,
}

/// The `actions/runner` platform of a class.
fn runner_platform(class: Class, arch: &str) -> &'static str {
    match (class, arch) {
        (Class::Macos, "x64") => "osx-x64",
        (Class::Macos, _) => "osx-arm64",
        (Class::Linux, "arm64") => "linux-arm64",
        (Class::Linux, _) => "linux-x64",
    }
}

/// The settings every hosted pool starts with when it has no `perf.toml`:
/// the performance core on, everything else at its default. Written once,
/// so the file is there to edit (and for `cua ci bench apply` to patch).
pub const DEFAULT_PERF_TOML: &str =
    "# Cua CI performance settings of this pool; see the Cua CI docs.\nenabled = true\n";

/// Writes [`DEFAULT_PERF_TOML`] for `pool` unless a file exists (or
/// `CUA_CI_PERF=off`, which leaves the plain cold path).
pub fn ensure_default_perf_file(paths: &CiPaths, pool: &str) -> Result<()> {
    if std::env::var("CUA_CI_PERF").is_ok_and(|v| v.eq_ignore_ascii_case("off")) {
        return Ok(());
    }
    let p = crate::perf::PerfConfig::path(paths, pool);
    if p.exists() {
        return Ok(());
    }
    cua_home::guard_write(&p)?;
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(&p, DEFAULT_PERF_TOML)?;
    Ok(())
}

/// The performance runtime of a hosted pool: its `perf.toml`, a repository
/// mirror store, and the golden image spec (the runner version comes from
/// the tarball cache). `None` when the pool has no enabled settings.
pub async fn hosted_runtime(
    paths: &CiPaths,
    pool: &str,
    class: &PerfClass,
    tarballs: &dyn TarballSource,
) -> Result<Option<Arc<PerfRuntime>>> {
    pool_runtime(paths, pool, class, tarballs, false).await
}

/// [`hosted_runtime`] for any pool. `eager`: warm VMs register as runners
/// ahead of time (a pool that owns its GitHub App has no service to mint a
/// claim's configuration), whatever the file says.
pub async fn pool_runtime(
    paths: &CiPaths,
    pool: &str,
    class: &PerfClass,
    tarballs: &dyn TarballSource,
    eager: bool,
) -> Result<Option<Arc<PerfRuntime>>> {
    let Some(mut rt) = PerfRuntime::for_pool(paths, pool, crate::unix_ms())? else {
        return Ok(None);
    };
    if !rt.cfg.enabled {
        return Ok(None);
    }
    if eager {
        rt.cfg.register = super::RegisterMode::Eager;
    }
    if rt.cfg.mirror.enabled {
        rt = rt.with_mirrors(Arc::new(super::mirror::GitMirrors::new(
            paths.root().join("perf").join("mirrors"),
            "github.com",
        )));
    }
    if rt.cfg.fork {
        let os = match class.image.class {
            Class::Macos => GuestOs::Macos,
            Class::Linux => GuestOs::Linux,
        };
        match tarballs
            .tarball(runner_platform(class.image.class, &class.arch))
            .await
        {
            Ok(t) => rt.set_golden_spec(super::golden::GoldenSpec::basic(
                os,
                &class.arch,
                &class.image.image,
                &t.version,
                rt.cfg.trim_guest,
            )),
            // No golden image without a runner version: jobs start cold (or
            // from a golden image an earlier run built).
            Err(e) => tracing::warn!(pool, "no golden image spec: {e}"),
        }
    }
    Ok(Some(Arc::new(rt)))
}

/// A [`PerfJobRunner`] for `classes` on `on`, started (each pool's loop
/// runs). `None` when any class has no enabled `perf.toml`: the caller
/// then uses the plain cold runner for every class, so one computer never
/// serves half its classes through a pool and half without.
#[allow(clippy::too_many_arguments)]
pub async fn perf_runner(
    paths: &CiPaths,
    api: Arc<dyn ServiceApi>,
    repos: Option<Arc<dyn RepoDirectory>>,
    machine_id: &str,
    account: &str,
    spaces: Arc<dyn Spaces>,
    tarballs: Arc<dyn TarballSource>,
    on: &str,
    classes: &[PerfClass],
) -> Result<Option<PerfJobRunner>> {
    let mut built = Vec::new();
    for c in classes {
        let name = hosted_pool_name(machine_id, c.image.class);
        let Some(rt) = hosted_runtime(paths, &name, c, tarballs.as_ref()).await? else {
            return Ok(None);
        };
        let mut cfg =
            hosted_pool_config(&name, c.image.class, &c.image.image, on, c.slots, c.warm)?;
        cfg.cpus = c.image.cpus;
        cfg.memory_mb = c.image.memory_mb;
        cfg.disk_gb = c.image.disk_gb;
        built.push((c.image.class, cfg, rt));
    }
    let mut runner = PerfJobRunner::new(api, machine_id, account);
    if let Some(r) = repos {
        runner = runner.with_repos(r);
    }
    for (class, cfg, rt) in built {
        let pool = hosted_pool(
            cfg,
            paths.clone(),
            spaces.clone(),
            tarballs.clone(),
            Arc::new(crate::pool::SystemClock),
            crate::config::HostResources::detect(),
            rt,
        )?;
        runner = runner.with_pool(class, pool);
    }
    runner.start();
    Ok(Some(runner))
}

/// Repository facts, refreshed now and then.
struct RepoCache {
    at: Option<Instant>,
    map: HashMap<String, RepoInfo>,
}

const REPO_TTL: Duration = Duration::from_secs(600);

/// Runs claimed jobs through hosted pools.
pub struct PerfJobRunner {
    pools: Vec<(Class, Arc<Pool>)>,
    api: Arc<dyn ServiceApi>,
    machine_id: String,
    account: String,
    repos: Option<Arc<dyn RepoDirectory>>,
    repo_cache: tokio::sync::Mutex<RepoCache>,
    poll: Duration,
    stop: watch::Sender<bool>,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl PerfJobRunner {
    /// A runner for `account` (`acct_...`, the key of its caches) on this
    /// computer.
    pub fn new(api: Arc<dyn ServiceApi>, machine_id: &str, account: &str) -> Self {
        Self {
            pools: Vec::new(),
            api,
            machine_id: machine_id.to_string(),
            account: account.to_string(),
            repos: None,
            repo_cache: tokio::sync::Mutex::new(RepoCache {
                at: None,
                map: HashMap::new(),
            }),
            poll: Duration::from_millis(500),
            stop: watch::channel(false).0,
            tasks: Mutex::new(Vec::new()),
        }
    }

    /// Serves `class` jobs with `pool`.
    pub fn with_pool(mut self, class: Class, pool: Arc<Pool>) -> Self {
        self.pools.push((class, pool));
        self
    }

    /// Where default branches and privacy come from.
    pub fn with_repos(mut self, d: Arc<dyn RepoDirectory>) -> Self {
        self.repos = Some(d);
        self
    }

    /// How often a waiting job looks at its slot.
    pub fn with_poll(mut self, p: Duration) -> Self {
        self.poll = p;
        self
    }

    /// Starts each pool's own loop (warm VM upkeep, end-of-job detection,
    /// the golden image). Stopped when the runner is dropped.
    pub fn start(&self) {
        let mut tasks = self.tasks.lock().expect("tasks");
        for (_, pool) in &self.pools {
            let mut rx = self.stop.subscribe();
            let pool = pool.clone();
            tasks.push(tokio::spawn(async move {
                let shutdown = async move {
                    while !*rx.borrow() {
                        if rx.changed().await.is_err() {
                            break;
                        }
                    }
                };
                let _ = pool.serve(shutdown).await;
            }));
        }
    }

    fn pool_for(&self, class: Class) -> Option<&Arc<Pool>> {
        self.pools.iter().find(|(c, _)| *c == class).map(|(_, p)| p)
    }

    async fn repo_info(&self, repo: &str) -> Option<RepoInfo> {
        let dir = self.repos.clone()?;
        let key = repo.to_ascii_lowercase();
        let mut c = self.repo_cache.lock().await;
        let fresh = c.at.is_some_and(|t| t.elapsed() <= REPO_TTL);
        let known = c.map.contains_key(&key);
        // A repository that is not listed yet is looked for again, but at
        // most once a minute.
        let can_ask = c.at.is_none_or(|t| t.elapsed() > Duration::from_secs(60));
        if (!fresh || !known)
            && can_ask
            && let Some(m) = dir.repos().await
        {
            c.map = m;
            c.at = Some(Instant::now());
        }
        c.map.get(&key).cloned()
    }

    /// What the pool needs to know about a claimed job.
    async fn claim_of(&self, job: &ClaimedJob) -> JobClaim {
        let info = self.repo_info(&job.repo).await;
        let nonempty = |s: &str| (!s.is_empty()).then(|| s.to_string());
        JobClaim {
            job_id: job.job_id.clone(),
            job_key: job.job_key.clone(),
            lease: Some(job.lease_id.clone()),
            repo: job.repo.clone(),
            account: self.account.clone(),
            event: nonempty(&job.event),
            git_ref: nonempty(&job.git_ref),
            head_repo: nonempty(&job.head_repo),
            // What the claim says wins; the repository list fills the gaps.
            default_branch: nonempty(&job.default_branch)
                .or_else(|| info.as_ref().and_then(|i| nonempty(&i.default_branch))),
            private: job.private.or_else(|| info.map(|i| i.private)),
            fork: Some(job.fork || job.restricted),
            cache_policy: Some(match job.policy.cache {
                CacheAccess::Rw => CachePolicy::Rw,
                CacheAccess::Ro => CachePolicy::Ro,
                CacheAccess::Off => CachePolicy::Off,
            }),
            class: Some(match job.class {
                Class::Macos => GuestOs::Macos,
                Class::Linux => GuestOs::Linux,
            }),
            arch: job.arch.clone(),
            labels: job.labels.clone(),
            runner_name: nonempty(&job.runner_name),
            queued_at_ms: parse_iso_ms(&job.queued_at),
            credential_scope: None,
            mirror_token: None,
            jit: Some(JitHandle(Arc::new(ServiceJit {
                api: self.api.clone(),
                machine_id: self.machine_id.clone(),
            }))),
            require_egress: (job.fork || job.restricted) && job.policy.egress != "off",
            egress_off: job.policy.egress == "off",
        }
    }

    fn usage(job: &ClaimedJob, end: &JobEnd, now: u64) -> Usage {
        let tl = &end.timeline;
        let p = tl.phases();
        Usage {
            vm_started_at: rfc3339(end.claimed_ms.unwrap_or(end.created_ms)),
            job_started_at: tl.started_ms.map(rfc3339),
            job_ended_at: tl.finished_ms.map(rfc3339),
            vm_ended_at: rfc3339(tl.teardown_ms.unwrap_or(now)),
            class: job.class,
            arch: job.arch.clone(),
            cpus: None,
            memory_mb: None,
            start_mode: end
                .start_mode
                .map(|m| m.as_str())
                .unwrap_or("cold_create")
                .to_string(),
            cost_micros: None,
            phases: ApiPhases {
                vm_claim_ms: p.vm_claim,
                vm_boot_ms: p.vm_boot,
                runner_register_ms: p.runner_register,
                assign_ms: p.assign,
                job_start_ms: p.job_start,
                checkout_ms: p.checkout,
                teardown_ms: p.teardown,
            },
            cache: CacheStats {
                hits: tl.cache_hits,
                misses: tl.cache_misses,
                bytes_restored: tl.cache_bytes,
            },
        }
    }

    /// What the pool recorded of a slot, as the agent's outcome.
    fn outcome(job: &ClaimedJob, end: Option<JobEnd>, aborted: bool, now: u64) -> JobOutcome {
        let Some(end) = end else {
            return JobOutcome::Failed {
                infra: true,
                detail: "the VM ended without a record".into(),
                usage: Self::bare_usage(job, now),
            };
        };
        let usage = Self::usage(job, &end, now);
        if aborted {
            return JobOutcome::Failed {
                infra: false,
                detail: "stopped on the service's request".into(),
                usage,
            };
        }
        let why = end.reason.as_str();
        if !end.ran {
            return JobOutcome::Released {
                detail: format!("the VM could not take the job ({})", why.replace('_', " ")),
            };
        }
        match why {
            "job_done" | "runner_exited" => JobOutcome::Finished {
                result: match (&end.runner_result, why) {
                    (Some(r), _) => normalize_result(r).to_string(),
                    (None, "job_done") => "success".to_string(),
                    _ => "failure".to_string(),
                },
                usage,
            },
            "job_timeout" => JobOutcome::Failed {
                infra: false,
                detail: "the job ran past its time limit".into(),
                usage,
            },
            other => JobOutcome::Failed {
                infra: true,
                detail: format!("the VM was lost ({})", other.replace('_', " ")),
                usage,
            },
        }
    }

    fn bare_usage(job: &ClaimedJob, now: u64) -> Usage {
        Usage {
            vm_started_at: rfc3339(now),
            job_started_at: None,
            job_ended_at: None,
            vm_ended_at: rfc3339(now),
            class: job.class,
            arch: job.arch.clone(),
            cpus: None,
            memory_mb: None,
            start_mode: "cold_create".into(),
            cost_micros: None,
            phases: ApiPhases::default(),
            cache: CacheStats::default(),
        }
    }
}

impl Drop for PerfJobRunner {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
        for t in self
            .tasks
            .lock()
            .map(|mut t| std::mem::take(&mut *t))
            .unwrap_or_default()
        {
            t.abort();
        }
    }
}

#[async_trait]
impl JobRunner for PerfJobRunner {
    async fn run(&self, job: &ClaimedJob, handle: &JobHandle) -> JobOutcome {
        let Some(pool) = self.pool_for(job.class) else {
            return JobOutcome::Released {
                detail: format!("this computer does not serve {} jobs", job.class.as_str()),
            };
        };
        if handle.aborted() {
            return JobOutcome::Released {
                detail: "stopped before it started".into(),
            };
        }
        handle.state(ReportState::Booting).await;
        let claim = self.claim_of(job).await;
        let now = crate::unix_ms();
        let claimed = match pool.claim(claim).await {
            Ok(c) => c,
            Err(ClaimError::NoCapacity(m)) | Err(ClaimError::PolicyRefused(m)) => {
                pool.forget_released(&job.job_key);
                return JobOutcome::Released { detail: m };
            }
            Err(ClaimError::BudgetExhausted) => {
                return JobOutcome::Released {
                    detail: "this computer's monthly budget is used up".into(),
                };
            }
            Err(ClaimError::NotEnabled) => {
                return JobOutcome::Released {
                    detail: "this computer's pool does not serve claimed jobs".into(),
                };
            }
            Err(ClaimError::Failed(m)) => {
                pool.forget_released(&job.job_key);
                return JobOutcome::Failed {
                    infra: true,
                    detail: m,
                    usage: Self::bare_usage(job, now),
                };
            }
        };
        handle.state(ReportState::RunnerOnline).await;

        // The pool's own loop sees the runner exit and tears the VM down;
        // this only waits for the slot to be gone (and for an abort).
        let mut running_reported = false;
        let aborted = loop {
            tokio::select! {
                _ = handle.wait_aborted() => {
                    pool.abort_job(&claimed.slot, "aborted").await;
                    break true;
                }
                _ = tokio::time::sleep(self.poll) => {}
            }
            match pool.slot_phase(&claimed.slot) {
                None => break false,
                Some(Phase::Busy) if !running_reported => {
                    handle.state(ReportState::Running).await;
                    running_reported = true;
                }
                _ => {}
            }
        };
        if aborted {
            // Torn down as asked; the service is told through the outcome.
            pool.forget_released(&job.job_key);
        }
        let end = pool.perf().and_then(|p| p.take_end(&claimed.slot));
        Self::outcome(job, end, aborted, crate::unix_ms())
    }
}

#[cfg(test)]
mod tests;
