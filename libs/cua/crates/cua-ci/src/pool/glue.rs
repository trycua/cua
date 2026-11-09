//! The pool's side of the performance core ([`crate::perf`]): claiming a
//! job, planning the warm pool, delivering a job's caches and mirror, the
//! end of a job (the guest's marks, the cache write-back) and the
//! teardown guarantees.
//!
//! None of it runs unless the pool was given a
//! [`PerfRuntime`](crate::perf::runtime::PerfRuntime) with `enabled`.

use std::future::Future;
use std::pin::Pin;

use super::*;
use crate::perf::RegisterMode;
use crate::perf::boot;
use crate::perf::cachevol::{self, scope_key, toolchain_key};
use crate::perf::golden::{self, Cadence, Refresh, Verdict};
use crate::perf::mirror;
use crate::perf::registry;
use crate::perf::runtime::{JobCtx, Released, read_hashes};
use crate::perf::timers::{parse_guest_marks, parse_step_telemetry};
use crate::perf::warm::{self, Counts, Demand, WarmEntry};
use crate::spaces::HostMount;

/// A job this pool took.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Claimed {
    /// The slot running it.
    pub slot: String,
    /// Warm VM, fork or cold create.
    pub start_mode: StartMode,
    /// The runner's name at GitHub.
    pub runner_name: String,
}

/// Why a claim was not served here.
#[derive(Debug, thiserror::Error)]
pub enum ClaimError {
    /// The pool has no performance core in claim mode.
    #[error("this pool does not serve claimed jobs (perf.enabled and perf.register = \"claim\")")]
    NotEnabled,
    /// The pool's budget is used up.
    #[error("the pool's monthly budget is used up")]
    BudgetExhausted,
    /// No VM is free (the service leaves the job queued).
    #[error("no room for another job: {0}")]
    NoCapacity(String),
    /// The job needs something this computer cannot give (an enforced
    /// egress policy for a fork pull request).
    #[error("{0}")]
    PolicyRefused(String),
    /// The VM could not be made ready; the job is released.
    #[error("{0}")]
    Failed(String),
}

/// What the pool can take now (what a `claim` request reports).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Capacity {
    /// The most VMs at once.
    pub max: u32,
    /// VMs with a job.
    pub busy: u32,
    /// VMs being prepared as warm VMs.
    pub booting: u32,
    /// Ready warm VMs.
    pub warm: u32,
}

impl Capacity {
    /// Jobs the pool can start now: the warm VMs and the room for more.
    pub fn free_slots(&self) -> u32 {
        self.max.saturating_sub(self.busy)
    }
}

/// A fork of the golden checkpoint a new slot starts from.
pub(super) struct ForkChoice {
    pub image: String,
    pub key: String,
    pub variant: u32,
}

impl Pool {
    /// Gives the pool a performance core.
    pub fn with_perf(mut self, perf: Arc<PerfRuntime>) -> Self {
        self.perf = Some(perf);
        self
    }

    /// Jobs come from the hosted service: GitHub's runner list is never
    /// read (the service owns the registrations).
    pub fn hosted(mut self) -> Self {
        self.hosted = true;
        self
    }

    /// The performance core.
    pub fn perf(&self) -> Option<&Arc<PerfRuntime>> {
        self.perf.as_ref()
    }

    /// The phase timers (the bench's events).
    pub fn timers(&self) -> &Timers {
        &self.timers
    }

    /// Warm VMs wait unregistered and a claim registers them.
    pub(super) fn claim_mode(&self) -> bool {
        self.perf
            .as_ref()
            .is_some_and(|p| p.cfg.enabled && p.cfg.register == RegisterMode::Claim)
    }

    /// Jobs the pool took but could not run here (create failed, restart
    /// in the middle of a claim): the caller reports each `released` to
    /// the service and forgets it.
    pub fn drain_released(&self) -> Vec<Released> {
        self.perf
            .as_ref()
            .map(|p| std::mem::take(&mut *p.released.lock().expect("released")))
            .unwrap_or_default()
    }

    pub(super) fn job_unserved(&self, job: Option<&JobClaim>, why: &str) {
        let (Some(j), Some(p)) = (job, &self.perf) else {
            return;
        };
        p.released.lock().expect("released").push(Released {
            job_key: j.job_key.clone(),
            lease: j.lease.clone(),
            reason: why.to_string(),
            infra: true,
        });
    }

    /// Takes a refusal [`Pool::provision_with`] left for `job_key`.
    fn take_refusal(&self, job_key: &str, reason: &str) -> bool {
        let Some(p) = &self.perf else { return false };
        let mut r = p.released.lock().expect("released");
        let before = r.len();
        r.retain(|x| !(x.job_key == job_key && x.reason == reason));
        r.len() != before
    }

    /// Stops a claimed job's VM now (the service aborted it, or the
    /// caller gave up): the slot is torn down like any other.
    pub async fn abort_job(&self, slot: &str, reason: &str) {
        self.finish(slot, reason, None).await;
    }

    /// Forgets the claims given back for `job_key` (the caller already
    /// answers the service itself).
    pub fn forget_released(&self, job_key: &str) {
        if let Some(p) = &self.perf {
            p.released
                .lock()
                .expect("released")
                .retain(|r| r.job_key != job_key);
        }
    }

    /// Where a slot is (`None`: it is gone).
    pub fn slot_phase(&self, slot: &str) -> Option<Phase> {
        self.slot(slot).map(|s| s.phase)
    }

    pub(super) fn job_released(&self, s: &Slot, why: &str) {
        let (Some(ctx), Some(p)) = (&s.perf.job, &self.perf) else {
            return;
        };
        p.released.lock().expect("released").push(Released {
            job_key: ctx.job_key.clone(),
            lease: ctx.lease.clone(),
            reason: why.to_string(),
            infra: true,
        });
        self.timers.infra_failure(
            self.now(),
            Some(&s.id),
            &format!("job {} released: {why}", ctx.job_key),
        );
    }

    /// Looks again soon while a job runs: the end of a job is noticed
    /// within a second instead of three.
    pub(super) fn tick_interval(&self) -> Duration {
        let busy =
            self.state.lock().expect("state").slots.iter().any(|s| {
                s.phase == Phase::Busy || (s.perf.job.is_some() && s.phase != Phase::Ending)
            });
        if busy && self.perf.as_ref().is_some_and(|p| p.cfg.enabled) {
            Duration::from_secs(1)
        } else {
            TICK
        }
    }

    // ---- counting and planning --------------------------------------------

    fn warm_counts(&self) -> Counts {
        let st = self.state.lock().expect("state");
        let mut c = Counts::default();
        for s in st.slots.iter() {
            match s.phase {
                Phase::Warm => c.warm.push(WarmEntry {
                    id: s.id.clone(),
                    ready_ms: s.perf.warm_ms.unwrap_or(s.created_ms),
                    image_key: s.perf.image_key.clone(),
                }),
                // A VM that is being torn down still holds its slot (and one
                // of Apple's two): a replacement waits for it to be gone.
                Phase::Ending if s.space_id.is_some() => c.busy += 1,
                Phase::Ending => {}
                Phase::Creating | Phase::Starting if s.perf.job.is_none() => c.booting += 1,
                _ => c.busy += 1,
            }
        }
        c
    }

    /// What the pool can take now.
    pub async fn capacity(&self) -> Capacity {
        let spaces = self.deps.spaces.list().await.unwrap_or_default();
        let lume = if self.cfg.guest_os() == GuestOs::Macos && self.cfg.on.trim() == "local" {
            self.deps.spaces.macos_vms_running().await
        } else {
            None
        };
        let max = self.cap_for(&spaces, lume);
        let c = self.warm_counts();
        Capacity {
            max,
            busy: c.busy,
            booting: c.booting,
            warm: c.warm.len() as u32,
        }
    }

    /// The claim-mode counterpart of `scale_up`: [`warm::plan`] decides,
    /// this does it (deletes first, creates in the next pass).
    pub(super) async fn scale_up_warm(
        &self,
        spaces: &[SpaceRow],
        lume: Option<u32>,
    ) -> Vec<String> {
        let Some(p) = self.perf.clone() else {
            return vec![];
        };
        let now = self.now();
        let cap = self.cap_for(spaces, lume);
        let counts = self.warm_counts();
        let spent = self.state.lock().expect("state").ledger.spent_in(now);
        let policy = p.warm_policy(self.cfg.effective_warm(), cap, spent);
        let demand = Demand {
            queued_jobs: p.queued_jobs(),
            last_demand_ms: p.last_demand_ms(),
        };
        let plan = warm::plan(&policy, now, &counts, &demand);
        for (id, why) in &plan.reap {
            // Only a VM that is still warm: a claim may have taken it since
            // the plan was made.
            if !self.reap_warm(id, why.as_str()) {
                continue;
            }
            self.record(
                Event::new(now, EventKind::WarmPool)
                    .slot(id)
                    .reason(why.as_str())
                    .detail(format!("warm VM {id} deleted")),
            );
            self.finish(id, why.as_str(), None).await;
        }
        if plan.create == 0 || now < self.state.lock().expect("state").next_create_ms {
            return vec![];
        }
        self.push_slots(plan.create, now, None)
    }

    /// Warm to ending in one step under the state lock, so a claim and a
    /// reap never both own a VM.
    fn reap_warm(&self, id: &str, reason: &str) -> bool {
        let took = {
            let mut st = self.state.lock().expect("state");
            match st
                .slots
                .iter_mut()
                .find(|s| s.id == id && s.phase == Phase::Warm)
            {
                Some(s) => {
                    s.phase = Phase::Ending;
                    s.ending = Some(reason.to_string());
                    true
                }
                None => false,
            }
        };
        if took {
            self.save_logged();
        }
        took
    }

    /// Warm to claimed in one step under the state lock.
    fn take_warm(&self, id: &str, now: u64, ctx: &JobCtx) -> Option<Slot> {
        let slot = {
            let mut st = self.state.lock().expect("state");
            let s = st
                .slots
                .iter_mut()
                .find(|s| s.id == id && s.phase == Phase::Warm)?;
            s.phase = Phase::Starting;
            s.perf.claimed_ms = Some(now);
            s.perf.job = Some(ctx.clone());
            // How the job's VM started: a warm VM, whatever it was made from.
            s.perf.start_mode = Some(StartMode::WarmPool);
            s.clone()
        };
        self.save_logged();
        Some(slot)
    }

    fn push_slots(&self, n: u32, now: u64, job: Option<&JobCtx>) -> Vec<String> {
        let mut ids = Vec::new();
        {
            let mut st = self.state.lock().expect("state");
            for _ in 0..n {
                let id = self.new_slot_id();
                st.slots.push(Slot {
                    name: format!("{}-{id}", self.cfg.prefix()),
                    id: id.clone(),
                    space_id: None,
                    runner_id: None,
                    phase: Phase::Creating,
                    created_ms: now,
                    ready_ms: None,
                    registered_ms: None,
                    online_ms: None,
                    busy_ms: None,
                    home: None,
                    egress: None,
                    ending: None,
                    perf: SlotPerf {
                        job: job.cloned(),
                        claimed_ms: job.map(|_| now),
                        ..Default::default()
                    },
                });
                ids.push(id);
            }
        }
        if !ids.is_empty() {
            self.save_logged();
            for id in &ids {
                let name = self.slot(id).map(|s| s.name).unwrap_or_default();
                self.record(
                    Event::new(now, EventKind::SlotCreated)
                        .slot(id)
                        .runner(Some(&name), None),
                );
            }
        }
        ids
    }

    /// A VM that became ready and prepared without a job.
    pub(super) fn warm_ready(&self, id: &str) {
        let now = self.now();
        let Some(s) = self.update(id, |s| {
            s.phase = Phase::Warm;
            s.perf.warm_ms = Some(now);
        }) else {
            return;
        };
        let mut e = Event::new(now, EventKind::WarmPool)
            .slot(id)
            .space(s.space_id.as_deref())
            .reason("ready")
            .ms(now.saturating_sub(s.created_ms));
        if let Some(m) = s.perf.start_mode {
            e = e.with("start_mode", m.as_str());
        }
        self.record(e);
    }

    /// Warm VMs are still there and still answer. A Space that vanished is
    /// handled with the others; a guest that stops answering is replaced
    /// after the usual number of misses.
    pub(super) async fn check_warm(&self) {
        let warm: Vec<Slot> = self
            .slots()
            .into_iter()
            .filter(|s| s.phase == Phase::Warm)
            .collect();
        for s in warm {
            let (Some(space), Some(home)) = (s.space_id.clone(), s.home.clone()) else {
                continue;
            };
            let out = self
                .deps
                .spaces
                .sh(
                    &space,
                    &guest::status_script(&home),
                    Duration::from_secs(15),
                )
                .await;
            match out {
                Ok(o) if o.ok() => {
                    self.rt.lock().expect("rt").guest_errors.remove(&s.id);
                }
                _ => self.guest_error(&s).await,
            }
        }
    }

    // ---- the folders a VM sees --------------------------------------------

    /// The read-only share of a new slot's VM, when the Spaces backend can
    /// mount host folders.
    pub(super) fn slot_mounts(&self, id: &str) -> Vec<HostMount> {
        let Some(p) = &self.perf else { return vec![] };
        if !self.claim_mode()
            || !self.deps.spaces.supports_mounts()
            || !(p.cfg.cache.enabled || p.cfg.mirror.enabled)
        {
            return vec![];
        }
        let Ok(dir) = p.create_slot_dir(id) else {
            return vec![];
        };
        self.update(id, |s| s.perf.mounted = true);
        vec![HostMount {
            host: dir,
            guest: cachevol::guest_mount_path(self.cfg.guest_os(), id),
            read_only: true,
        }]
    }

    /// Puts a job's caches and mirror where its VM reads them (a link in
    /// the slot's share, or an upload where the backend cannot mount) and
    /// writes `job.env`. A failure costs the job its caches, never the job.
    pub(super) async fn deliver_job_files(
        &self,
        id: &str,
        space: &str,
        home: &str,
        job: &JobClaim,
    ) -> std::result::Result<(), String> {
        let Some(p) = self.perf.clone() else {
            return Ok(());
        };
        let meta = job.meta();
        let plan = cachevol::plan_for(&meta, &p.cfg.cache);
        let want = if p.cfg.cache.enabled {
            p.cfg.cache.stores.clone()
        } else {
            vec![]
        };
        // The mirror never makes the job wait: a missing one starts its
        // fill in the background and the job clones the normal way.
        let mirror_ref = match (
            &p.mirrors,
            mirror::request_for(&meta, &p.cfg.mirror, job.credential_scope.as_deref()),
        ) {
            (Some(m), Some(req)) => m.ensure(&req, job.mirror_token.as_ref()).await,
            _ => None,
        };
        let populated = {
            let (p2, id2, plan2, want2, m2) = (
                p.clone(),
                id.to_string(),
                plan.clone(),
                want.clone(),
                mirror_ref.clone(),
            );
            tokio::task::spawn_blocking(move || {
                p2.populate_slot_dir(&id2, &plan2, &want2, m2.as_ref())
            })
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?
        };
        if let Some(key) = &plan.read {
            for store in &want {
                let hit = populated.stores.iter().any(|(s, _)| s == store);
                let (k, w, r) = read_hashes(key.as_str(), &job.repo);
                let mut e = Event::new(self.now(), EventKind::CacheRead)
                    .slot(id)
                    .with("store", store.as_str())
                    .with("key_hash", k)
                    .with("hit", hit)
                    .with("source_repo_hash", w)
                    .with("reader_repo_hash", r);
                if let Some(c) = job.class {
                    e = e.with("class", c.as_str());
                }
                self.record(e.with("job_key", job.job_key.as_str()));
            }
        }
        self.update(id, |s| {
            if let Some(j) = &mut s.perf.job {
                j.cache_scope = plan.scope.to_string();
                j.cache_write = plan.write;
                j.cache_key = plan.read.as_ref().map(|k| k.as_str().to_string());
                j.stores = populated.stores.clone();
                j.report_denied = plan.report_denied_writes;
                j.mirror = populated.mirror;
            }
        });
        let mounted = self.slot(id).is_some_and(|s| s.perf.mounted);
        // Docker registry mirrors (Linux guests): a line of `job.env`, the
        // job-start hook applies it. Needs no mount.
        let registry = registry::env_line(p.cfg.registry.for_guest(self.cfg.guest_os()));
        if populated.stores.is_empty() && !populated.mirror {
            // No caches or mirror to hand over. A shared folder stays (the VM
            // has it mounted; it goes with the VM), an upload's folder is not
            // needed.
            if !mounted {
                p.remove_slot_dir(id);
            }
            if registry.is_empty() {
                return Ok(());
            }
            let env = format!("# written by cua ci before the job\n{registry}");
            return self
                .deps
                .spaces
                .write_file(space, &boot::job_env_path(home), env.as_bytes(), 0o600)
                .await
                .map_err(|e| e.to_string());
        }
        let root = if mounted {
            cachevol::guest_mount_path(self.cfg.guest_os(), id)
        } else {
            self.upload_slot_dir(&p, id, space, home).await?
        };
        let mut env = cachevol::job_env(
            &plan,
            &format!("{root}/cache"),
            &populated.stores,
            populated
                .mirror
                .then(|| format!("{root}/mirror"))
                .as_deref(),
        );
        env.push_str(&registry);
        self.deps
            .spaces
            .write_file(space, &boot::job_env_path(home), env.as_bytes(), 0o600)
            .await
            .map_err(|e| e.to_string())
    }

    /// Where the backend cannot share a folder: one tar of the slot folder,
    /// unpacked in the guest. Returns the guest folder.
    async fn upload_slot_dir(
        &self,
        p: &Arc<PerfRuntime>,
        id: &str,
        space: &str,
        home: &str,
    ) -> std::result::Result<String, String> {
        let dir = p.slot_dir(id);
        let tar = p.root.join("slots").join(format!("{id}.tar"));
        let (d2, t2) = (dir.clone(), tar.clone());
        let made = tokio::task::spawn_blocking(move || {
            std::process::Command::new("tar")
                .arg("-cf")
                .arg(&t2)
                .arg("-C")
                .arg(&d2)
                .arg(".")
                .output()
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
        if !made.status.success() {
            return Err(format!(
                "tar: {}",
                String::from_utf8_lossy(&made.stderr).trim()
            ));
        }
        let h = home.trim_end_matches('/');
        let remote = format!("{h}/{}/in.tar", boot::DIR);
        let r = async {
            self.deps
                .spaces
                .upload_file(space, &tar, &remote)
                .await
                .map_err(|e| e.to_string())?;
            let into = format!("{h}/{}/in", boot::DIR);
            let o = self
                .deps
                .spaces
                .sh(
                    space,
                    &format!(
                        "set -e\nmkdir -p {i}\ntar -xf {t} -C {i}\nrm -f {t}\n",
                        i = guest::sh_quote(&into),
                        t = guest::sh_quote(&remote)
                    ),
                    Duration::from_secs(300),
                )
                .await
                .map_err(|e| e.to_string())?;
            if o.ok() {
                Ok(into)
            } else {
                Err(format!("unpacking the caches failed: {}", o.stderr.trim()))
            }
        }
        .await;
        let _ = std::fs::remove_file(&tar);
        r
    }

    // ---- claiming ----------------------------------------------------------

    /// Takes a job: a warm VM when one is ready (registered for the job in
    /// about a second), else a fork of the golden checkpoint or a cold
    /// create, registered as soon as it is up. The caller reports the
    /// outcome to the service ([`Pool::drain_released`] lists the jobs it
    /// must give back).
    pub async fn claim(&self, job: JobClaim) -> std::result::Result<Claimed, ClaimError> {
        let Some(p) = self.perf.clone() else {
            return Err(ClaimError::NotEnabled);
        };
        if !self.claim_mode() {
            return Err(ClaimError::NotEnabled);
        }
        let now = self.now();
        let class = job.class.unwrap_or(self.cfg.guest_os());
        if class != self.cfg.guest_os() {
            return Err(ClaimError::NoCapacity(format!(
                "this pool runs {} jobs",
                self.cfg.guest_os().as_str()
            )));
        }
        if let Some(b) = &p.cfg.budget {
            let spent = self.state.lock().expect("state").ledger.spent_in(now);
            let budget = warm::Budget {
                limit_usd: b.monthly_usd,
                spent_usd: spent,
                usd_per_hour: b.usd_per_hour,
            };
            if !budget.admits_job() {
                return Err(ClaimError::BudgetExhausted);
            }
        }
        p.mark_demand(now);
        self.timers
            .job_queued(now, &job.job_key, &job.repo, job.queued_at_ms);
        let ctx = ctx_of(&job);

        // A warm VM (one whose egress policy is enforced, when the job
        // needs that).
        let mut entries = self.warm_counts().warm;
        if job.require_egress {
            entries.retain(|w| {
                self.slot(&w.id)
                    .and_then(|s| s.egress)
                    .is_some_and(|e| e == "host")
            });
        }
        let golden = p.golden_state().key().filter(|_| p.cfg.fork);
        while let Some(w) = warm::pick(&entries, golden.as_deref()) {
            let id = w.id.clone();
            entries.retain(|e| e.id != id);
            match self.claim_warm(&id, &job, &ctx, now).await {
                Ok(c) => return Ok(c),
                // Taken by another claim, or dead: try the next one.
                Err(ClaimError::NoCapacity(_)) => continue,
                Err(e) => return Err(e),
            }
        }

        // No warm VM: room for another? The check and the slot are one step.
        let id = {
            let _gate = p.claim_gate.lock().await;
            let spaces = self
                .deps
                .spaces
                .list()
                .await
                .map_err(|e| ClaimError::Failed(format!("listing Spaces: {e}")))?;
            let lume = if self.cfg.guest_os() == GuestOs::Macos && self.cfg.on.trim() == "local" {
                self.deps.spaces.macos_vms_running().await
            } else {
                None
            };
            let cap = self.cap_for(&spaces, lume);
            let c = self.warm_counts();
            if c.busy + c.booting + c.warm.len() as u32 >= cap {
                return Err(ClaimError::NoCapacity(format!(
                    "{cap} VM(s) are all in use"
                )));
            }
            self.push_slots(1, now, Some(&ctx))
                .into_iter()
                .next()
                .ok_or_else(|| ClaimError::Failed("no slot".into()))?
        };
        let mode = if self.fork_choice(&id).is_some() || self.cfg.is_checkpoint() {
            StartMode::Fork
        } else {
            StartMode::ColdCreate
        };
        self.timers.vm_claimed(
            now,
            &id,
            Some(&job.job_key),
            class,
            &job.arch,
            mode,
            Some(now),
            job.queued_at_ms,
        );
        if self.provision_with(&id, Some(&job)).await {
            let name = self.slot(&id).map(|s| s.name).unwrap_or_default();
            Ok(Claimed {
                slot: id,
                start_mode: mode,
                runner_name: job.runner_name.clone().unwrap_or(name),
            })
        } else if self.take_refusal(&job.job_key, "capacity") {
            Err(ClaimError::NoCapacity(
                "the computer refused another VM (Apple allows two macOS VMs)".into(),
            ))
        } else if self.take_refusal(&job.job_key, "egress_unenforceable") {
            Err(ClaimError::PolicyRefused(
                "this computer cannot enforce an egress policy, which this job needs".into(),
            ))
        } else {
            Err(ClaimError::Failed("the VM could not be started".into()))
        }
    }

    async fn claim_warm(
        &self,
        id: &str,
        job: &JobClaim,
        ctx: &JobCtx,
        now: u64,
    ) -> std::result::Result<Claimed, ClaimError> {
        let Some(slot) = self.take_warm(id, now, ctx) else {
            return Err(ClaimError::NoCapacity("the warm VM is gone".into()));
        };
        let (Some(space), Some(home)) = (slot.space_id.clone(), slot.home.clone()) else {
            return Err(ClaimError::NoCapacity("the warm VM has no Space".into()));
        };
        // One cheap call proves the VM still answers before the job is
        // committed to it.
        let alive = matches!(
            self.deps
                .spaces
                .sh(&space, &guest::status_script(&home), Duration::from_secs(10))
                .await,
            Ok(o) if o.ok() && guest::parse_status(&o.stdout) == GuestState::Starting
        );
        if !alive {
            // Not the job's fault and not released: the claim goes on with
            // another VM.
            self.update(id, |s| s.perf.job = None);
            self.finish(id, "guest_unreachable", None).await;
            return Err(ClaimError::NoCapacity("the warm VM did not answer".into()));
        }
        // The VM answers: it is the job's.
        self.timers.vm_claimed(
            now,
            id,
            Some(&job.job_key),
            job.class.unwrap_or(self.cfg.guest_os()),
            &job.arch,
            StartMode::WarmPool,
            Some(now),
            job.queued_at_ms,
        );
        self.record(
            Event::new(now, EventKind::WarmPool)
                .slot(id)
                .reason("claimed"),
        );
        match self
            .register_runner(id, &space, &home, &job.arch, Some(job))
            .await
        {
            Ok(()) => {
                self.state.lock().expect("state").failures = 0;
                self.save_logged();
                let name = slot.name.clone();
                Ok(Claimed {
                    slot: id.to_string(),
                    start_mode: StartMode::WarmPool,
                    runner_name: job.runner_name.clone().unwrap_or(name),
                })
            }
            Err(e) => {
                self.timers.infra_failure(
                    self.now(),
                    Some(id),
                    &format!("registering the runner in {}: {e}", slot.name),
                );
                // finish() gives the job back (it never ran).
                self.finish(id, "start_failed", None).await;
                Err(ClaimError::Failed(format!("registering the runner: {e}")))
            }
        }
    }

    // ---- the golden image ---------------------------------------------------

    /// The golden checkpoint a new slot forks from, when there is a
    /// verified one and forks are on.
    pub(super) fn fork_choice(&self, id: &str) -> Option<ForkChoice> {
        let p = self.perf.as_ref()?;
        if !p.cfg.enabled || !p.cfg.fork || self.cfg.is_checkpoint() {
            return None;
        }
        let g = p.golden_state();
        let rec = g.current.as_ref().filter(|r| r.verified_ms.is_some())?;
        let used: Vec<u32> = self
            .slots()
            .iter()
            .filter(|s| s.id != id && s.phase != Phase::Ending)
            .filter_map(|s| s.perf.variant)
            .collect();
        let variant = golden::pick_variant(&used, rec.spec.variants());
        let image = rec
            .checkpoints
            .get(variant as usize)
            .map(|n| format!("checkpoint:{n}"))?;
        Some(ForkChoice {
            image,
            key: rec.digest.clone(),
            variant,
        })
    }

    pub(super) fn golden_fork_failed(&self, id: &str, f: &ForkChoice, why: &str) {
        let Some(p) = &self.perf else { return };
        {
            let mut g = p.golden.lock().expect("golden");
            g.fork_failures += 1;
            let _ = g.save(&p.golden_path());
        }
        self.record(
            Event::new(self.now(), EventKind::Golden)
                .slot(id)
                .reason("fork_failed")
                .detail(format!("fork of {} failed, starting cold: {why}", f.image)),
        );
    }

    /// A fork started: failures in a row start over.
    pub(super) fn golden_fork_ok(&self) {
        let Some(p) = &self.perf else { return };
        let mut g = p.golden.lock().expect("golden");
        if g.fork_failures != 0 {
            g.fork_failures = 0;
            let _ = g.save(&p.golden_path());
        }
    }

    /// When the golden image is due (or an old one can be retired) a
    /// future that does it; `None` when there is nothing to do. At most one
    /// runs at a time and the check itself happens once a minute.
    pub(super) fn golden_task(self: Arc<Self>) -> Option<Pin<Box<dyn Future<Output = ()> + Send>>> {
        use std::sync::atomic::Ordering;
        let p = self.perf.clone()?;
        if !p.cfg.enabled || !p.cfg.fork || self.cfg.is_checkpoint() {
            return None;
        }
        let now = self.now();
        if now.saturating_sub(p.last_golden_check_ms.load(Ordering::Relaxed)) < 60_000 {
            return None;
        }
        p.last_golden_check_ms.store(now, Ordering::Relaxed);
        if p.refreshing.swap(true, Ordering::SeqCst) {
            return None;
        }
        Some(Box::pin(async move {
            self.golden_cycle(&p).await;
            p.refreshing.store(false, Ordering::SeqCst);
        }))
    }

    async fn golden_cycle(&self, p: &Arc<PerfRuntime>) {
        let spaces = self.deps.spaces.clone();
        // Retire the replaced image once no VM runs from it.
        let prev = p.golden_state().previous;
        if let Some(prev) = prev {
            let in_use = self
                .slots()
                .iter()
                .any(|s| s.perf.image_key.as_deref() == Some(prev.digest.as_str()));
            if !in_use {
                for n in &prev.checkpoints {
                    let _ = spaces.delete_checkpoint(n).await;
                }
                let mut g = p.golden.lock().expect("golden");
                g.previous = None;
                let _ = g.save(&p.golden_path());
            }
        }
        let Some(mut spec) = p.golden_spec.lock().expect("golden spec").clone() else {
            return;
        };
        // A golden VM has the size of the job VMs it forks into, so a new
        // size is a new golden image.
        let size = self.cfg.space_size(self.deps.host);
        if let Some(c) = size.0 {
            spec.tools.insert("vm_cpus".into(), c.to_string());
        }
        if let Some(m) = size.1 {
            spec.tools.insert("vm_memory_mb".into(), m.to_string());
        }
        let st = p.golden_state();
        let existing = match spaces.checkpoints().await {
            Ok(v) => v,
            // Cannot tell: do not rebuild on a listing error.
            Err(_) => st
                .current
                .as_ref()
                .map(|r| r.checkpoints.clone())
                .unwrap_or_default(),
        };
        let cadence = Cadence {
            max_age: p.cfg.golden_max_age(),
            ..Default::default()
        };
        let now = self.now();
        let Verdict::Rebuild(why) = golden::assess(&spec, &st, now, cadence, &existing) else {
            return;
        };
        self.record(
            Event::new(now, EventKind::Golden)
                .reason("building")
                .detail(why.describe()),
        );
        {
            let mut g = p.golden.lock().expect("golden");
            g.last_attempt_ms = now;
            let _ = g.save(&p.golden_path());
        }
        let r = Refresh {
            spaces: spaces.clone(),
            tarballs: self.deps.tarballs.clone(),
            clock: self.deps.clock.clone(),
            on: self.cfg.on.clone(),
            size,
        };
        let me = self.live_image_keys();
        let cur = st.current.clone();
        let out = golden::refresh(&r, &spec, cur.as_ref(), &move |n: &str| {
            me.iter().any(|k| n.contains(k.as_str()))
        })
        .await;
        match out {
            Ok(done) => {
                let ms = done.record.build_ms;
                let fork_ms = done.record.fork_ready_ms;
                {
                    let mut g = p.golden.lock().expect("golden");
                    g.previous = g.current.take();
                    g.current = Some(done.record);
                    g.fork_failures = 0;
                    let _ = g.save(&p.golden_path());
                }
                self.record(
                    Event::new(self.now(), EventKind::Golden)
                        .reason("built")
                        .ms(ms)
                        .with("fork_ready_ms", fork_ms.unwrap_or(0))
                        .detail(format!("golden image {} ready", spec.short())),
                );
            }
            Err(e) => self.record(
                Event::new(self.now(), EventKind::Golden)
                    .reason("failed")
                    .detail(format!("building the golden image failed: {e}")),
            ),
        }
    }

    /// Digests of the golden images live slots run from (a checkpoint name
    /// contains its digest's first 12 characters).
    fn live_image_keys(&self) -> Vec<String> {
        self.slots()
            .iter()
            .filter_map(|s| {
                s.perf
                    .image_key
                    .as_ref()
                    .map(|k| k[..12.min(k.len())].to_string())
            })
            .collect()
    }

    // ---- the end of a job ---------------------------------------------------

    /// While the VM is still there: the guest's marks (first step, end of
    /// job, cache restores), the checkout step's duration, and the cache
    /// write-back of a trusted job that succeeded.
    pub(super) async fn job_end(
        &self,
        s: &Slot,
        space: &str,
        home: &str,
        reason: &str,
        summary: &guest::JobSummary,
    ) {
        let Some(p) = self.perf.clone() else { return };
        if s.busy_ms.is_none() && s.perf.job.is_none() {
            return;
        }
        let out = self
            .deps
            .spaces
            .sh(space, &boot::collect_script(home), Duration::from_secs(30))
            .await;
        let Ok(o) = out else { return };
        let (marks_text, worker) = boot::split_collect(&o.stdout);
        let marks = parse_guest_marks(marks_text, s.perf.guest_offset_ms.unwrap_or(0));
        self.timers.guest_marks(&s.id, &marks);
        // After the job, the VM must hold nothing beyond the one-job
        // configuration (names only reach the event).
        if let Ok(o) = self
            .deps
            .spaces
            .sh(space, &boot::secret_scan_script(), Duration::from_secs(30))
            .await
            && let Some((found, detail)) = boot::parse_secret_scan(&o.stdout)
        {
            self.timers.secret_scan(self.now(), &s.id, found, &detail);
        }
        let mirror_used =
            marks.mirror_ready == Some(true) || s.perf.job.as_ref().is_some_and(|j| j.mirror);
        if let Some(step) = parse_step_telemetry(worker)
            .iter()
            .find(|x| x.is_checkout())
        {
            self.timers
                .checkout_done(self.now(), &s.id, step.ms(), mirror_used);
        }
        let Some(ctx) = &s.perf.job else { return };
        let ok = reason == "job_done" && summary.result.as_deref() == Some("Succeeded");
        let same = |a: &Option<String>, b: &Option<String>| match (a, b) {
            (Some(x), Some(y)) => x.eq_ignore_ascii_case(y),
            _ => false,
        };
        // The guest's own report of its event, repository and ref must
        // match what the service said: a mismatch writes nothing.
        let agrees = same(&marks.event, &ctx.event)
            && same(&marks.repo, &Some(ctx.repo.clone()))
            && same(&marks.git_ref, &ctx.git_ref);
        if ctx.cache_write && ok && agrees {
            self.write_back(&p, s, ctx, space, home).await;
        } else if ctx.report_denied || (ctx.cache_write && ok && !agrees) {
            // A refused write is reported, a mismatch is an error to look at.
            let o = self
                .deps
                .spaces
                .sh(
                    space,
                    &boot::delta_script(home, false),
                    Duration::from_secs(120),
                )
                .await;
            let dirty = o.map(|o| boot::parse_delta(&o.stdout)).unwrap_or_default();
            for (store, _) in dirty
                .iter()
                .fold(Vec::<(String, ())>::new(), |mut a, (s, _)| {
                    if !a.iter().any(|(x, _)| x == s) {
                        a.push((s.clone(), ()));
                    }
                    a
                })
            {
                let key = ctx
                    .cache_key
                    .clone()
                    .unwrap_or_else(|| scope_key(&ctx.account, &ctx.repo).as_str().to_string());
                self.record(
                    Event::new(self.now(), EventKind::CacheWrite)
                        .slot(&s.id)
                        .with("store", store.as_str())
                        .with("key_hash", cachevol::hash_for_event(&key))
                        .with("scope", "fork_readonly_denied")
                        .with("bytes", 0u64)
                        .with("job_key", ctx.job_key.as_str()),
                );
            }
            if ctx.cache_write && !agrees {
                self.record(Event::new(self.now(), EventKind::Error).slot(&s.id).detail(
                    "the guest's event, repository or ref differ from the claim: no cache write",
                ));
            }
        }
    }

    async fn write_back(
        &self,
        p: &Arc<PerfRuntime>,
        s: &Slot,
        ctx: &JobCtx,
        space: &str,
        home: &str,
    ) {
        let o = self
            .deps
            .spaces
            .sh(
                space,
                &boot::delta_script(home, true),
                Duration::from_secs(300),
            )
            .await;
        let dirty = match o {
            Ok(o) if o.ok() => boot::parse_delta(&o.stdout),
            _ => return,
        };
        if dirty.is_empty() {
            return;
        }
        let tmp = p.root.join("incoming").join(&s.id);
        if cua_home::guard_write(&tmp).is_err() || std::fs::create_dir_all(&tmp).is_err() {
            return;
        }
        let dl = tokio::time::timeout(
            Duration::from_secs(600),
            self.deps
                .spaces
                .download(space, &boot::delta_dir(home), &tmp),
        )
        .await;
        if !matches!(dl, Ok(Ok(()))) {
            self.record(
                Event::new(self.now(), EventKind::Error)
                    .slot(&s.id)
                    .detail("copying the cache delta out of the VM failed"),
            );
            let _ = std::fs::remove_dir_all(&tmp);
            return;
        }
        let key = scope_key(&ctx.account, &ctx.repo);
        let versions = p
            .golden_state()
            .current
            .as_ref()
            .map(|r| r.versions.clone())
            .unwrap_or_default();
        let mut stores: Vec<String> = dirty.iter().map(|(s, _)| s.clone()).collect();
        stores.dedup();
        for store in stores {
            let Some(st) = cachevol::store(&store) else {
                continue;
            };
            let tc = toolchain_key(st, &versions);
            let subs: Vec<(String, std::path::PathBuf)> = dirty
                .iter()
                .filter(|(s, _)| *s == store)
                .map(|(s, sub)| {
                    (
                        sub.clone(),
                        tmp.join("delta").join(format!("{s}--{sub}.tar")),
                    )
                })
                .filter(|(_, f)| f.is_file())
                .collect();
            if subs.is_empty() {
                continue;
            }
            let (root, key2, store2, tc2) =
                (p.caches.clone(), key.clone(), store.clone(), tc.clone());
            let res = tokio::task::spawn_blocking(move || {
                let refs: Vec<(&str, &std::path::Path)> = subs
                    .iter()
                    .map(|(a, b)| (a.as_str(), b.as_path()))
                    .collect();
                root.promote(
                    &key2,
                    &store2,
                    &tc2,
                    &refs,
                    cachevol::PromoteLimits::default(),
                )
            })
            .await;
            match res {
                Ok(Ok(done)) => self.record(
                    Event::new(self.now(), EventKind::CacheWrite)
                        .slot(&s.id)
                        .with("store", store.as_str())
                        .with("key_hash", cachevol::hash_for_event(key.as_str()))
                        .with("scope", "default_branch")
                        .with("bytes", done.bytes)
                        .with("job_key", ctx.job_key.as_str()),
                ),
                Ok(Err(e)) => self.record(
                    Event::new(self.now(), EventKind::Error)
                        .slot(&s.id)
                        .detail(format!("cache write-back of {store} refused: {e}")),
                ),
                Err(e) => self.record(
                    Event::new(self.now(), EventKind::Error)
                        .slot(&s.id)
                        .detail(format!("cache write-back of {store}: {e}")),
                ),
            }
        }
        let _ = std::fs::remove_dir_all(&tmp);
        let cap = u64::from(p.cfg.cache.size_cap_gb) << 30;
        let root = p.caches.clone();
        let _ =
            tokio::task::spawn_blocking(move || root.evict(cap, Duration::from_secs(600))).await;
    }

    /// After the Space is deleted and the slot forgotten: the spend, the
    /// slot's folder, `teardown_done`, and a sweep for leaked VMs.
    pub(super) async fn after_teardown(&self, s: &Slot, reason: &str, ran_job: bool) {
        let now = self.now();
        if let Some(p) = &self.perf {
            if let Some(b) = &p.cfg.budget {
                {
                    let mut st = self.state.lock().expect("state");
                    st.ledger
                        .charge(now, now.saturating_sub(s.created_ms), b.usd_per_hour);
                }
                self.save_logged();
            }
            p.remove_slot_dir(&s.id);
        }
        if ran_job || s.perf.job.is_some() {
            let timeline = self.timers.teardown_done(now, &s.id, reason);
            if let (Some(p), Some(_)) = (&self.perf, &s.perf.job) {
                p.ends.lock().expect("ends").insert(
                    s.id.clone(),
                    crate::perf::runtime::JobEnd {
                        reason: reason.to_string(),
                        ran: ran_job,
                        runner_result: s.perf.runner_result.clone(),
                        exit_code: s.perf.exit_code,
                        timeline,
                        claimed_ms: s.perf.claimed_ms,
                        created_ms: s.created_ms,
                        start_mode: s.perf.start_mode,
                    },
                );
            }
            self.leak_check(now).await;
        } else {
            self.timers.forget(&s.id);
        }
    }

    /// `resource_sweep`: Spaces of this pool that no slot owns after a job's
    /// teardown (the next sweep deletes them; the bench counts them).
    async fn leak_check(&self, now: u64) {
        let Ok(list) = self.deps.spaces.list().await else {
            return;
        };
        let slots = self.slots();
        let leaked: Vec<String> = list
            .iter()
            .filter(|r| self.ours(r))
            .filter(|r| {
                !r.label(SPACE_LABEL_SLOT)
                    .is_some_and(|l| slots.iter().any(|s| s.id == l))
            })
            .map(|r| r.id.clone())
            .collect();
        if !leaked.is_empty() {
            // Look at it soon.
            self.rt.lock().expect("rt").last_sweep_ms = None;
        }
        self.timers
            .resource_sweep(now, leaked.len() as u32, 0, &leaked.join(","));
    }
}

fn ctx_of(job: &JobClaim) -> JobCtx {
    JobCtx {
        job_key: job.job_key.clone(),
        lease: job.lease.clone(),
        repo: job.repo.clone(),
        account: job.account.clone(),
        event: job.event.clone(),
        git_ref: job.git_ref.clone(),
        fork: job.fork.unwrap_or(false),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests;
