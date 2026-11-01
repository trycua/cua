// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Who runs a durable agent, and who takes over when that machine goes.
//!
//! A durable agent's *driver* (the loop that starts the agent, watches it
//! and re-drives it) must not live on the machine the user will close. It
//! runs on the most reliable machine that holds the agent's files, and moves
//! when that machine goes away. This module is that rule, built on the
//! Volume's ordered writes (they are atomic across machines when the
//! replicated store is on):
//!
//! ```text
//! .cua-runs/<agent>.json          the run: its task, and whether it is done
//! agents/<agent>/.cua-supervisor  the lease: holder, epoch, beat, progress
//! ```
//!
//! Every machine runs the same small loop. The *preferred* machine is the
//! most reliable one that is up and holds a copy (the same score placement
//! uses; ties by id). It takes a free lease, or a lease whose holder is
//! down, by compare-and-swap with `epoch + 1`; the old holder, if it wakes,
//! finds the epoch moved on and stops (it is fenced), and anything it wrote
//! in between merges by the order of the lease, not by luck. A laptop that
//! holds a run yields it to a clearly more reliable machine that has been
//! up and caught up for a while. Nothing here needs a clock: a holder is
//! alive while its beat keeps changing as the observer sees it.
//!
//! What a takeover says is told from facts: the new holder compares the
//! journal steps it holds with the progress the old holder last reported,
//! and says "resumed" or "restarted from step N, steps M to K are redone".

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::node::{MachineView, ReplicaNode};
use super::words::{Summary, closing_summary};
use crate::backend::{Backend, Condition};
use crate::{Error, Result};

/// The lease object's name inside an agent's home.
pub const LEASE: &str = ".cua-supervisor";
/// The folder of run records.
pub const RUNS_DIR: &str = ".cua-runs/";

/// A run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunRecord {
    pub agent: String,
    /// What to run (the driver reads it).
    pub spec: serde_json::Value,
    /// `running` or `done`.
    pub state: String,
    pub created_by: String,
    pub created_ms: u64,
    /// A line for people when it is done.
    #[serde(default)]
    pub outcome: String,
}

/// The lease.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseRec {
    pub holder: String,
    pub name: String,
    pub epoch: u64,
    /// Changes at every renewal: a holder is alive while this keeps moving.
    pub beat: u64,
    /// Steps of the run the holder says are done.
    pub progress: u64,
    /// The holder before this one (for the takeover sentence).
    #[serde(default)]
    pub previous: String,
    /// The holder's CPU architecture.
    #[serde(default)]
    pub arch: String,
    /// The previous holder's.
    #[serde(default)]
    pub previous_arch: String,
}

fn lease_key(agent: &str) -> Result<String> {
    Ok(format!("{}{LEASE}", crate::path::agent_home(agent)?))
}

fn run_key(agent: &str) -> String {
    format!("{RUNS_DIR}{agent}.json")
}

/// What the new holder knows about where the run stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resume {
    /// The first holder (nothing to resume).
    pub fresh: bool,
    /// The step the previous holder last reported.
    pub reported: u64,
    /// The step this machine's copy of the journal reaches (the driver
    /// fills it in; the supervisor leaves it at the reported step until then).
    pub held: u64,
}

impl Resume {
    /// The sentence for a takeover.
    pub fn words(&self, to: &str, from: &str) -> String {
        if self.fresh {
            return format!("Started on {to}.");
        }
        if self.held >= self.reported {
            format!(
                "Resumed on {to} where {from} left off (step {}).",
                self.held
            )
        } else {
            format!(
                "Resumed on {to} from step {}; steps {} to {} had not reached it and are done again.",
                self.held,
                self.held + 1,
                self.reported
            )
        }
    }
}

/// Handed to the driver while it holds a run.
pub struct RunCtx {
    pub agent: String,
    pub epoch: u64,
    pub spec: serde_json::Value,
    pub machine: String,
    pub backend: Arc<dyn Backend>,
    pub resume: Resume,
    fenced: Arc<AtomicBool>,
    progress: Arc<AtomicU64>,
    held: Arc<AtomicU64>,
}

impl RunCtx {
    /// The run was taken from this machine (a later epoch holds it, or it
    /// was handed to a better machine): stop.
    pub fn fenced(&self) -> bool {
        self.fenced.load(Ordering::SeqCst)
    }

    /// Reports how far the run is (in steps); renewals carry it.
    pub fn report(&self, step: u64) {
        self.progress.store(step, Ordering::SeqCst);
    }

    /// Sets what this machine's journal copy reaches (call once at start).
    pub fn held_reaches(&mut self, step: u64) {
        self.resume.held = step;
        self.held.store(step, Ordering::SeqCst);
    }
}

/// How a drive ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DriveEnd {
    /// The run finished; the line says how.
    Done(String),
    /// The driver stopped because it was fenced.
    Fenced,
    /// It failed; the supervisor leaves the lease to be adopted.
    Failed(String),
}

/// What actually runs the agent.
#[async_trait::async_trait]
pub trait Driver: Send + Sync + 'static {
    async fn drive(&self, ctx: RunCtx) -> DriveEnd;
}

/// Held while a run is active on this machine (a sleep assertion): dropping
/// it releases whatever it holds.
pub type AwakeToken = Box<dyn std::any::Any + Send>;

/// Keeps this machine awake for a reason people can read.
pub type Awake = Arc<dyn Fn(&str) -> AwakeToken + Send + Sync>;

/// Timing.
#[derive(Clone, Debug)]
pub struct SupervisionConfig {
    /// A lease whose beat has not changed this long is dead.
    pub ttl: Duration,
    pub tick: Duration,
    /// A machine must have been up this long (and the run's files with it)
    /// before a laptop yields the run to it.
    pub yield_after: Duration,
    /// A machine does not adopt a run of a holder it still sees up until the
    /// beat stalls; a holder it cannot see (down) is adopted after this.
    pub down_grace: Duration,
}

impl SupervisionConfig {
    pub fn standard() -> Self {
        SupervisionConfig {
            ttl: Duration::from_secs(20),
            tick: Duration::from_millis(500),
            yield_after: Duration::from_secs(120),
            down_grace: Duration::from_secs(1),
        }
    }

    pub fn fast() -> Self {
        SupervisionConfig {
            ttl: Duration::from_millis(1800),
            tick: Duration::from_millis(60),
            yield_after: Duration::from_millis(1500),
            down_grace: Duration::from_millis(100),
        }
    }
}

/// What a run looks like from here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RunView {
    pub agent: String,
    pub state: String,
    pub holder: String,
    pub holder_name: String,
    pub epoch: u64,
    pub progress: u64,
    pub running_here: bool,
    pub outcome: String,
    /// What happened at the last takeover, in plain words.
    pub note: String,
}

/// What `start_run` says.
#[derive(Clone, Debug, PartialEq)]
pub struct StartInfo {
    /// Where the run will go (a name from the machines list).
    pub run_on: String,
    /// The one plain paragraph about closing this machine.
    pub words: String,
}

/// What a takeover said, kept until the run ends (the words are made when
/// asked, so they use what the driver found in its copy of the journal).
struct Note {
    fresh: bool,
    reported: u64,
    held: Arc<AtomicU64>,
    to: String,
    from: String,
    /// Set when the run moved between architectures: what in the workspace
    /// was built for the old one.
    arch_change: Option<(String, Vec<String>)>,
}

impl Note {
    fn words(&self) -> String {
        Resume {
            fresh: self.fresh,
            reported: self.reported,
            held: self.held.load(Ordering::SeqCst),
        }
        .words(&self.to, &self.from)
            + &self.arch_sentence()
    }

    fn arch_sentence(&self) -> String {
        let Some((from_arch, built)) = &self.arch_change else {
            return String::new();
        };
        let what = if built.is_empty() {
            String::new()
        } else {
            format!(" ({})", built.join(", "))
        };
        format!(
            " {} is {from_arch}, so programs built there{what} are not reused here and should be built again; files, notes and the run's journal carry over as they are.",
            self.from
        )
    }
}

struct Live {
    fenced: Arc<AtomicBool>,
    task: tokio::task::JoinHandle<()>,
    _awake: Option<AwakeToken>,
}

#[derive(Default)]
struct Seen {
    etag: String,
    since: Option<Instant>,
}

struct Inner {
    node: ReplicaNode,
    backend: Arc<dyn Backend>,
    cfg: SupervisionConfig,
    driver: Arc<dyn Driver>,
    awake: Option<Awake>,
    live: tokio::sync::Mutex<HashMap<String, Live>>,
    seen: tokio::sync::Mutex<HashMap<String, Seen>>,
    notes: std::sync::Mutex<HashMap<String, Note>>,
    progress: std::sync::Mutex<HashMap<String, Arc<AtomicU64>>>,
    stop: AtomicBool,
    stable_since: tokio::sync::Mutex<HashMap<String, Instant>>,
}

/// The supervisor of one machine.
#[derive(Clone)]
pub struct Supervision {
    inner: Arc<Inner>,
}

/// The machine that should run durable work now: the most reliable machine
/// that is up and holds a copy (any up machine when none does), ties by id.
pub fn preferred(machines: &[MachineView]) -> Option<&MachineView> {
    let pick = |only_replicas: bool| {
        machines
            .iter()
            .filter(|m| m.alive && (!only_replicas || m.replica))
            .max_by(|a, b| {
                a.facts
                    .rank
                    .cmp(&b.facts.rank)
                    .then_with(|| b.id.cmp(&a.id))
            })
    };
    pick(true).or_else(|| pick(false))
}

impl Supervision {
    pub fn new(
        node: ReplicaNode,
        backend: Arc<dyn Backend>,
        cfg: SupervisionConfig,
        driver: Arc<dyn Driver>,
        awake: Option<Awake>,
    ) -> Supervision {
        Supervision {
            inner: Arc::new(Inner {
                node,
                backend,
                cfg,
                driver,
                awake,
                live: Default::default(),
                seen: Default::default(),
                notes: Default::default(),
                progress: Default::default(),
                stop: AtomicBool::new(false),
                stable_since: Default::default(),
            }),
        }
    }

    /// Starts the loop.
    pub fn spawn(&self) {
        let me = self.inner.clone();
        tokio::spawn(async move {
            while !me.stop.load(Ordering::SeqCst) {
                tokio::time::sleep(me.cfg.tick).await;
                if let Err(e) = me.tick().await {
                    tracing::debug!(error = %e, "supervision tick");
                }
            }
        });
    }

    /// Stops the loop and the runs here (they are not fenced: another
    /// machine takes them when the lease goes stale).
    pub async fn stop(&self) {
        self.inner.stop.store(true, Ordering::SeqCst);
        let all: Vec<Live> = self
            .inner
            .live
            .lock()
            .await
            .drain()
            .map(|(_, v)| v)
            .collect();
        for l in all {
            l.task.abort();
        }
    }

    /// Records a run. The most reliable machine takes it on its next look;
    /// this machine does if it is the only one.
    pub async fn start_run(&self, agent: &str, spec: serde_json::Value) -> Result<StartInfo> {
        // A name the Volume would refuse fails here, not silently later.
        crate::path::agent_home(agent)?;
        let rec = RunRecord {
            agent: agent.to_string(),
            spec,
            state: "running".into(),
            created_by: self.inner.node.machine().to_string(),
            created_ms: crate::now_ms(),
            outcome: String::new(),
        };
        self.inner
            .backend
            .put(&run_key(agent), serde_json::to_vec(&rec)?, Condition::None)
            .await?;
        let machines = self.inner.node.machines().await;
        let run_on = preferred(&machines).map(|m| m.name.clone());
        let words = self.words(run_on.as_deref()).await;
        Ok(StartInfo {
            run_on: run_on.unwrap_or_default(),
            words,
        })
    }

    /// The paragraph about closing this machine for a run on `run_on`.
    pub async fn words(&self, run_on: Option<&str>) -> String {
        let s: Summary = self.inner.node.summary(run_on).await;
        closing_summary(&s)
    }

    /// Every run, as this machine sees it.
    pub async fn runs(&self) -> Result<Vec<RunView>> {
        let mut out = vec![];
        for m in self.inner.backend.list(RUNS_DIR).await? {
            let Ok((b, _)) = self.inner.backend.get(&m.key, None).await else {
                continue;
            };
            let Ok(rec) = serde_json::from_slice::<RunRecord>(&b) else {
                continue;
            };
            let lease = self.inner.read_lease(&rec.agent).await.ok().flatten();
            let running_here = self.inner.live.lock().await.contains_key(&rec.agent);
            out.push(RunView {
                agent: rec.agent.clone(),
                state: rec.state,
                holder: lease
                    .as_ref()
                    .map(|l| l.0.holder.clone())
                    .unwrap_or_default(),
                holder_name: lease.as_ref().map(|l| l.0.name.clone()).unwrap_or_default(),
                epoch: lease.as_ref().map_or(0, |l| l.0.epoch),
                progress: lease.as_ref().map_or(0, |l| l.0.progress),
                running_here,
                outcome: rec.outcome,
                note: self
                    .inner
                    .notes
                    .lock()
                    .expect("notes")
                    .get(&rec.agent)
                    .map(Note::words)
                    .unwrap_or_default(),
            });
        }
        Ok(out)
    }
}

/// The name of machine `id` in the machines list.
async fn machines_name(node: &ReplicaNode, id: &str) -> String {
    node.machines()
        .await
        .into_iter()
        .find(|m| m.id == id)
        .map(|m| m.name)
        .unwrap_or_else(|| id.to_string())
}

impl Inner {
    async fn read_lease(&self, agent: &str) -> Result<Option<(LeaseRec, String)>> {
        let k = lease_key(agent)?;
        match self.backend.get(&k, None).await {
            Ok((b, meta)) => {
                let l: LeaseRec = serde_json::from_slice(&b)?;
                Ok(Some((l, meta.etag)))
            }
            Err(Error::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn tick(self: &Arc<Self>) -> Result<()> {
        let machines = self.node.machines().await;
        let me = self.node.machine().to_string();
        let pref = preferred(&machines).map(|m| m.id.clone());
        for m in self.backend.list(RUNS_DIR).await? {
            let Ok((b, _)) = self.backend.get(&m.key, None).await else {
                continue;
            };
            let Ok(rec) = serde_json::from_slice::<RunRecord>(&b) else {
                continue;
            };
            if rec.state == "done" {
                self.drop_live(&rec.agent, true).await;
                continue;
            }
            self.look(&rec, &machines, &me, pref.as_deref()).await;
        }
        Ok(())
    }

    async fn drop_live(&self, agent: &str, fence: bool) {
        if let Some(l) = self.live.lock().await.remove(agent) {
            if fence {
                l.fenced.store(true, Ordering::SeqCst);
            }
            // Give the driver a moment to stop at a step boundary.
            let t = l.task;
            let _ = tokio::time::timeout(self.cfg.ttl, t).await;
        }
    }

    async fn look(
        self: &Arc<Self>,
        rec: &RunRecord,
        machines: &[MachineView],
        me: &str,
        pref: Option<&str>,
    ) {
        let agent = &rec.agent;
        let lease = match self.read_lease(agent).await {
            Ok(l) => l,
            Err(_) => return,
        };
        let running_here = self.live.lock().await.contains_key(agent);
        match lease {
            None => {
                if pref == Some(me) {
                    let l = LeaseRec {
                        holder: me.to_string(),
                        name: self.node.name().to_string(),
                        epoch: 1,
                        beat: 0,
                        progress: 0,
                        previous: String::new(),
                        arch: String::new(),
                        previous_arch: String::new(),
                    };
                    self.claim(rec, l, Condition::IfNoneMatch, true).await;
                }
            }
            Some((l, etag)) => {
                // Observe the beat on this machine's own clock.
                let (dead, quiet_for) = {
                    let mut seen = self.seen.lock().await;
                    let s = seen.entry(agent.clone()).or_default();
                    if s.etag != etag {
                        s.etag = etag.clone();
                        s.since = Some(Instant::now());
                    }
                    let q = s.since.map_or(Duration::ZERO, |t| t.elapsed());
                    (q >= self.cfg.ttl, q)
                };
                if l.holder == me {
                    if !running_here {
                        // A restart of this machine's process while it held
                        // the run: take it again at the next epoch.
                        let next = LeaseRec {
                            epoch: l.epoch + 1,
                            beat: 0,
                            previous: me.to_string(),
                            previous_arch: l.arch.clone(),
                            ..l.clone()
                        };
                        self.claim(rec, next, Condition::IfMatch(etag), false).await;
                        return;
                    }
                    self.renew(rec, &l, &etag, machines, me, pref).await;
                    return;
                }
                // Someone else holds it. If this machine runs it, it is
                // fenced.
                if running_here {
                    tracing::info!(
                        agent,
                        epoch = l.epoch,
                        "supervision: this machine was fenced"
                    );
                    self.fence_here(agent).await;
                    return;
                }
                if pref != Some(me) {
                    return;
                }
                // Confirmed down, not merely slow: a holder that is only
                // late keeps its run until its beat stalls for the whole ttl.
                let holder_down = machines
                    .iter()
                    .find(|m| m.id == l.holder)
                    .is_none_or(|m| m.down);
                // The preferred machine takes a run whose holder is down
                // (after a short grace) or whose beat stopped.
                let take = dead || (holder_down && quiet_for >= self.cfg.down_grace);
                // Or the holder handed it to this machine.
                let handed = l.holder == me;
                if take || handed {
                    let next = LeaseRec {
                        holder: me.to_string(),
                        name: self.node.name().to_string(),
                        epoch: l.epoch + 1,
                        beat: 0,
                        progress: l.progress,
                        previous: l.holder.clone(),
                        arch: String::new(),
                        previous_arch: l.arch.clone(),
                    };
                    self.claim(rec, next, Condition::IfMatch(etag), false).await;
                }
            }
        }
    }

    /// Takes the lease with `cond` and starts the driver.
    async fn claim(
        self: &Arc<Self>,
        rec: &RunRecord,
        mut l: LeaseRec,
        cond: Condition,
        fresh: bool,
    ) {
        // The holder's own architecture goes into the lease.
        l.arch = self
            .node
            .machines()
            .await
            .into_iter()
            .find(|m| m.is_me)
            .map(|m| super::roles::normalize_arch(&m.facts.arch))
            .unwrap_or_default();
        let Ok(k) = lease_key(&rec.agent) else { return };
        let Ok(bytes) = serde_json::to_vec(&l) else {
            return;
        };
        match self.backend.put(&k, bytes, cond).await {
            Ok(_) => {}
            Err(_) => return,
        }
        let note = Resume {
            fresh,
            reported: l.progress,
            held: l.progress,
        };
        tracing::info!(agent = %rec.agent, epoch = l.epoch, "supervision: this machine runs the agent now");
        self.start_driver(rec, l, note).await;
    }

    async fn start_driver(self: &Arc<Self>, rec: &RunRecord, l: LeaseRec, resume: Resume) {
        let fenced = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(AtomicU64::new(l.progress));
        self.progress
            .lock()
            .expect("progress")
            .insert(rec.agent.clone(), progress.clone());
        let awake = self
            .awake
            .as_ref()
            .map(|f| f(&format!("Cua: the durable agent {} is running", rec.agent)));
        let held = Arc::new(AtomicU64::new(resume.held));
        let ctx = RunCtx {
            agent: rec.agent.clone(),
            epoch: l.epoch,
            spec: rec.spec.clone(),
            machine: self.node.machine().to_string(),
            backend: self.backend.clone(),
            resume: resume.clone(),
            fenced: fenced.clone(),
            progress,
            held: held.clone(),
        };
        let driver = self.driver.clone();
        let me = self.clone();
        let agent = rec.agent.clone();
        let from = l.previous.clone();
        let to = self.node.name().to_string();
        let from_name = machines_name(&self.node, &from).await;
        let arch_change =
            if !l.previous_arch.is_empty() && !l.arch.is_empty() && l.previous_arch != l.arch {
                let keys = self
                    .backend
                    .list(&crate::path::agent_home(&rec.agent).unwrap_or_default())
                    .await
                    .map(|v| v.into_iter().map(|m| m.key).collect::<Vec<_>>())
                    .unwrap_or_default();
                Some((
                    l.previous_arch.clone(),
                    super::roles::arch_specific(keys, 4),
                ))
            } else {
                None
            };
        self.notes.lock().expect("notes").insert(
            agent.clone(),
            Note {
                fresh: resume.fresh,
                reported: resume.reported,
                held,
                to,
                from: from_name,
                arch_change,
            },
        );
        let rec2 = rec.clone();
        let task = tokio::spawn(async move {
            let end = driver.drive(ctx).await;
            match end {
                DriveEnd::Done(outcome) => {
                    let mut done = rec2.clone();
                    done.state = "done".into();
                    done.outcome = outcome;
                    if let Ok(b) = serde_json::to_vec(&done) {
                        let _ = me.backend.put(&run_key(&agent), b, Condition::None).await;
                    }
                }
                DriveEnd::Fenced => {}
                DriveEnd::Failed(why) => {
                    tracing::warn!(agent = %agent, %why, "supervision: the driver failed");
                    // The lease goes stale and the preferred machine re-drives.
                    me.live.lock().await.remove(&agent);
                }
            }
        });
        self.live.lock().await.insert(
            rec.agent.clone(),
            Live {
                fenced,
                task,
                _awake: awake,
            },
        );
    }

    async fn fence_here(&self, agent: &str) {
        if let Some(l) = self.live.lock().await.remove(agent) {
            l.fenced.store(true, Ordering::SeqCst);
            let t = l.task;
            let _ = tokio::time::timeout(self.cfg.ttl, t).await;
        }
    }

    /// The holder's tick: renew by compare-and-swap, and hand the run to a
    /// clearly better machine when this one is a laptop.
    async fn renew(
        self: &Arc<Self>,
        rec: &RunRecord,
        l: &LeaseRec,
        etag: &str,
        machines: &[MachineView],
        me: &str,
        pref: Option<&str>,
    ) {
        // A driver that ended (failed) is not running here any more: handled
        // by the caller through `running_here`.
        let progress = self
            .progress
            .lock()
            .expect("progress")
            .get(&rec.agent)
            .map_or(l.progress, |p| p.load(Ordering::SeqCst));
        let mut next = LeaseRec {
            beat: l.beat + 1,
            progress,
            ..l.clone()
        };
        // Yield to a clearly better machine that has been up for a while.
        if let (Some(p), Some(mine)) = (pref, machines.iter().find(|m| m.id == me))
            && p != me
            && mine.facts.rank + super::roles::STEP_RANK
                <= machines
                    .iter()
                    .find(|m| m.id == p)
                    .map_or(0, |m| m.facts.rank)
            && machines.iter().any(|m| m.id == p && m.stable)
        {
            let since = {
                let mut s = self.stable_since.lock().await;
                *s.entry(rec.agent.clone()).or_insert_with(Instant::now)
            };
            if since.elapsed() >= self.cfg.yield_after {
                let target = machines.iter().find(|m| m.id == p).expect("pref");
                next = LeaseRec {
                    holder: target.id.clone(),
                    name: target.name.clone(),
                    epoch: l.epoch + 1,
                    beat: 0,
                    progress,
                    previous: me.to_string(),
                    arch: target.facts.arch.clone(),
                    previous_arch: l.arch.clone(),
                };
                if let Ok(k) = lease_key(&rec.agent)
                    && let Ok(b) = serde_json::to_vec(&next)
                    && self
                        .backend
                        .put(&k, b, Condition::IfMatch(etag.to_string()))
                        .await
                        .is_ok()
                {
                    tracing::info!(agent = %rec.agent, to = %target.name, "supervision: handed the run to a more reliable machine");
                    self.stable_since.lock().await.remove(&rec.agent);
                    self.fence_here(&rec.agent).await;
                }
                return;
            }
        } else {
            self.stable_since.lock().await.remove(&rec.agent);
        }
        let Ok(k) = lease_key(&rec.agent) else { return };
        let Ok(b) = serde_json::to_vec(&next) else {
            return;
        };
        match self
            .backend
            .put(&k, b, Condition::IfMatch(etag.to_string()))
            .await
        {
            Ok(_) => {}
            Err(Error::Precondition(_)) => {
                // Someone changed the lease: if it is not ours any more, we
                // are fenced; the next look decides.
            }
            Err(_) => {
                // The owner is out of reach: the run goes on (its files are
                // saved here and sync later); the beat just does not move.
            }
        }
    }
}
