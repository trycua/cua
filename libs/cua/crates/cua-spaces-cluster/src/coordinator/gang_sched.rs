// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The scheduler's half of gangs ([`super::gang`]): placing a gang on
//! distinct hosts, all or nothing; looking at a gang again whenever one of
//! its members ends (stop the others, queue it again, or end it); and the
//! lookups the pool requests need (a member's live output).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cua_cluster::link::NodeLink;
use cua_cluster::model::{JobSpec, now_ms};
use cua_cluster::placement::{self, Candidate};
use cua_proto::env::v1 as pb;

use super::gang::{self, GangSpec, Verdict, gstate};
use super::store::{self, Change, Op, OpOut, state};
use super::{
    CoordError, Coordinator, Flight, GangPlan, GangRt, Health, Metrics, Phase, Sched, host_job_id,
    need_of,
};

/// A live gang member: job, rank, host, job attempt, gang attempt.
type LiveMember = (i64, u32, String, u32, u32);

/// Margin over a lost host's own self-fence time before its gang is placed
/// again.
const FENCE_MARGIN: Duration = Duration::from_secs(1);

impl Coordinator {
    /// A flight that ends without its host's record of it (it never
    /// arrived, its host refused or was lost or removed): a gang member
    /// ends now (`started`: it may have run, which counts as a failure of
    /// its gang) and its gang decides what is next; any other job goes back
    /// to the queue (`retry`: counted as a lost attempt).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn give_up(
        &self,
        g: &mut Sched,
        id: i64,
        f: Flight,
        error: String,
        retry: bool,
        started: bool,
        changes: &mut Vec<Change>,
    ) {
        if g.is_member(id) {
            g.land(&f);
            let error = if started {
                error
            } else {
                format!("{}{error}", gang::NOT_STARTED)
            };
            changes.push(super::member_end(
                id,
                f.attempt,
                if f.user_cancel {
                    state::CANCELED
                } else {
                    state::ERROR
                },
                error,
            ));
        } else {
            changes.push(Change::Requeue {
                id,
                attempt: f.attempt,
                error,
                retry,
            });
            g.requeue_front(id, &f, retry);
        }
    }

    /// Jobs `ended` (recorded): their gangs are looked at again.
    pub(super) fn gangs_changed(&self, ended: &[i64]) {
        if ended.is_empty() {
            return;
        }
        let mut g = self.sched.lock().expect("sched");
        for id in ended {
            if let Some(run) = g.gang_jobs.get(id).cloned() {
                g.gang_dirty.insert(run);
            }
        }
    }

    /// Submits gang run `run`. A run submitted before with the same spec
    /// returns `false` (and nothing changes): submits are idempotent per
    /// run. One submitted before with another spec is a
    /// [`CoordError::Conflict`] naming the fields that differ.
    pub async fn submit_gang(&self, run: &str, spec: GangSpec) -> Result<bool, CoordError> {
        if run.trim().is_empty() {
            return Err(CoordError::Invalid("a run needs an id".into()));
        }
        if spec.min == 0 || spec.max < spec.min || spec.max > gang::MAX_NODES {
            return Err(CoordError::Invalid(format!(
                "nodes must be 1 <= min <= max <= {} (got {}:{})",
                gang::MAX_NODES,
                spec.min,
                spec.max
            )));
        }
        if spec.spec.argv.is_empty() || spec.spec.argv[0].trim().is_empty() {
            return Err(CoordError::Invalid("a run needs a command".into()));
        }
        let outs = self
            .persist_ops(vec![Op::GangSubmit {
                run: run.to_string(),
                now_ms: now_ms(),
                gang: spec.clone(),
            }])
            .await
            .map_err(CoordError::Store)?;
        let fresh = outs.first().is_some_and(OpOut::applied);
        if !fresh {
            // The first submit's spec is the run's: the same one attaches,
            // another one is refused (it would silently not run).
            let r = run.to_string();
            let existing = self
                .store
                .call(move |c| gang::gang(c, &r))
                .await
                .map_err(CoordError::Store)?;
            if let Some(row) = existing {
                let stored = gang::GangSpec {
                    min: row.min,
                    max: row.max,
                    retries: row.retries,
                    spec: row.spec,
                    policy: row.policy,
                };
                let diff = gang::spec_diff(&stored, &spec);
                if !diff.is_empty() {
                    return Err(CoordError::Conflict(format!(
                        "run {run} exists with a different spec ({}); submit the same spec to attach to it, or use a new run id",
                        diff.join(", ")
                    )));
                }
            }
        }
        if fresh {
            let row = gang::GangRow {
                run: run.to_string(),
                min: spec.min,
                max: spec.max,
                retries: spec.retries,
                spec: spec.spec,
                policy: spec.policy,
                state: gstate::QUEUED.into(),
                ..Default::default()
            };
            self.sched
                .lock()
                .expect("sched")
                .gangs
                .insert(run.to_string(), GangRt::from_row(&row));
            self.metrics
                .submitted
                .fetch_add(u64::from(spec.max), std::sync::atomic::Ordering::Relaxed);
            self.wake.notify_one();
        }
        Ok(fresh)
    }

    /// Places the queued gangs that may run now, in memory: each on as
    /// many distinct hosts as can take a member now (at most `max`), the
    /// hosts that ran it before first, or not at all below `min`.
    pub(super) fn plan_gangs(&self, g: &mut Sched, cands: &mut [Candidate]) -> Vec<GangPlan> {
        let now = Instant::now();
        let runs: Vec<String> = g
            .gangs
            .iter()
            .filter(|(_, r)| r.state == gstate::QUEUED && !r.placing && r.ready_at <= now)
            .map(|(k, _)| k.clone())
            .collect();
        let mut plans = Vec::new();
        for run in runs {
            let Some(r) = g.gangs.get_mut(&run) else {
                continue;
            };
            let need = need_of(&r.spec);
            // Each member needs a host of its own; elastic capacity (one
            // provider) is no gang member.
            let mut order: Vec<usize> = placement::rank(cands, &need, self.cfg.policy)
                .into_iter()
                .filter(|&i| !cands[i].elastic)
                .collect();
            // The hosts that ran it hold its checkpoints: first (a stable
            // sort keeps the ranking within each group).
            order.sort_by_key(|&i| !r.hosts.contains(&cands[i].name));
            let min = r.min.max(1) as usize;
            let n = order.len().min(r.max.max(1) as usize);
            if n < min {
                r.short = (min - n) as u32;
                r.waiting = if order.is_empty() {
                    format!(
                        "no computer can take a member now: {}",
                        placement::why_nothing_fits(cands, &need)
                    )
                } else {
                    format!(
                        "needs {min} computers that can each take a member now; {} can",
                        order.len()
                    )
                };
                continue;
            }
            r.short = 0;
            let chosen = &order[..n];
            let addresses: Vec<Vec<String>> = chosen
                .iter()
                .map(|&i| {
                    cands[i]
                        .status
                        .as_ref()
                        .map(|s| s.addresses.clone())
                        .unwrap_or_default()
                })
                .collect();
            let attempt = r.attempt + 1;
            // Each member's machine: its relay id, else its name here.
            let machines: Vec<String> = chosen
                .iter()
                .map(|&i| {
                    g.nodes
                        .get(&cands[i].name)
                        .map(|n| n.row.relay_machine.clone())
                        .filter(|m| !m.is_empty())
                        .unwrap_or_else(|| cands[i].name.clone())
                })
                .collect();
            let Some(r) = g.gangs.get_mut(&run) else {
                continue;
            };
            let names: Vec<String> = chosen.iter().map(|&i| cands[i].name.clone()).collect();
            let specs = gang::member_specs(
                &run,
                attempt,
                &r.spec,
                &addresses,
                &machines,
                &names,
                r.policy.barrier_timeout_secs,
            );
            let members = chosen
                .iter()
                .zip(specs)
                .enumerate()
                .map(|(rank, (&i, spec))| gang::GangMember {
                    rank: rank as u32,
                    node: cands[i].name.clone(),
                    usd_per_hour: cands[i].status.as_ref().map_or(0.0, |s| s.usd_per_hour),
                    spec,
                })
                .collect();
            for &i in chosen {
                cands[i].pending_millicpus += u64::from(need.millicpus);
                cands[i].pending_memory_mb += need.memory_mb;
                cands[i].pending_jobs += 1;
            }
            r.placing = true;
            r.waiting.clear();
            plans.push(GangPlan {
                run,
                attempt,
                members,
                need,
            });
        }
        plans
    }

    /// Records a gang placement (its member jobs), then sends each member
    /// to its host.
    pub(super) async fn launch_gang(self: &Arc<Self>, gp: GangPlan) {
        let op = Op::GangPlace {
            run: gp.run.clone(),
            attempt: gp.attempt,
            now_ms: now_ms(),
            members: gp.members.clone(),
        };
        let ids: Vec<i64> = match self.persist_ops(vec![op]).await {
            Ok(outs) => match outs.into_iter().next() {
                Some(OpOut::Submitted(v)) => v.into_iter().map(|(id, _)| id).collect(),
                _ => vec![],
            },
            Err(_) => vec![],
        };
        let mut sends: BTreeMap<String, Vec<(i64, JobSpec)>> = BTreeMap::new();
        let mut links: HashMap<String, Arc<dyn NodeLink>> = HashMap::new();
        let mut lost = Vec::new();
        {
            let mut g = self.sched.lock().expect("sched");
            if ids.len() != gp.members.len() {
                // Not recorded (canceled meanwhile, or this coordinator
                // stopped): release what the plan reserved.
                for m in &gp.members {
                    if let Some(n) = g.nodes.get_mut(&m.node) {
                        n.pending_cpu = n.pending_cpu.saturating_sub(u64::from(gp.need.millicpus));
                        n.pending_mem = n.pending_mem.saturating_sub(gp.need.memory_mb);
                        n.pending_jobs = n.pending_jobs.saturating_sub(1);
                    }
                }
                if let Some(r) = g.gangs.get_mut(&gp.run) {
                    r.placing = false;
                }
                return;
            }
            if let Some(top) = ids.iter().max() {
                self.max_job
                    .fetch_max(*top, std::sync::atomic::Ordering::SeqCst);
            }
            if let Some(r) = g.gangs.get_mut(&gp.run) {
                r.placing = false;
                r.state = gstate::RUNNING.into();
                r.attempt = gp.attempt;
                r.size = gp.members.len() as u32;
                r.placed_at = Instant::now();
                r.released = false;
                for m in &gp.members {
                    if !r.hosts.contains(&m.node) {
                        r.hosts.push(m.node.clone());
                    }
                }
            }
            let now = Instant::now();
            for (m, id) in gp.members.iter().zip(&ids) {
                let mut spec = m.spec.clone();
                spec.job_id = host_job_id(&self.cluster_id, *id);
                spec.attempt = 1;
                g.gang_jobs.insert(*id, gp.run.clone());
                *g.run_inflight.entry(gp.run.clone()).or_default() += 1;
                g.inflight.insert(
                    *id,
                    Flight {
                        node: m.node.clone(),
                        attempt: 1,
                        run: gp.run.clone(),
                        spec: Arc::new(m.spec.clone()),
                        phase: Phase::Dispatched,
                        // The lease starts once it is sent (`send`).
                        deadline: now + Duration::from_secs(3600),
                        started: None,
                        ready_at: now,
                        submitted_ms: now_ms(),
                        retries: 0,
                        max_attempts: 1,
                        user_cancel: false,
                        reconciling: false,
                    },
                );
                match g.nodes.get(&m.node) {
                    Some(n) => {
                        links.insert(m.node.clone(), n.link.clone());
                        sends.entry(m.node.clone()).or_default().push((*id, spec));
                    }
                    None => lost.push(*id),
                }
            }
            let mut changes = Vec::new();
            for id in &lost {
                if let Some(f) = g.inflight.remove(id) {
                    let node = f.node.clone();
                    self.give_up(
                        &mut g,
                        *id,
                        f,
                        format!("{node} left the pool"),
                        false,
                        false,
                        &mut changes,
                    );
                }
            }
            drop(g);
            if !changes.is_empty() {
                let me = self.clone();
                tokio::spawn(async move { me.finish(changes, vec![]).await });
            }
        }
        self.metrics
            .dispatched
            .fetch_add(ids.len() as u64, std::sync::atomic::Ordering::Relaxed);
        tracing::info!(run = %gp.run, attempt = gp.attempt, size = ids.len(), "gang placed");
        for (node, jobs) in sends {
            if let Some(link) = links.get(&node).cloned() {
                let me = self.clone();
                tokio::spawn(async move { me.send(node, link, jobs).await });
            }
        }
    }

    /// Looks again at every gang whose members changed.
    pub(super) async fn gang_tick(self: &Arc<Self>) {
        let dirty: Vec<String> = {
            let mut g = self.sched.lock().expect("sched");
            g.gang_dirty.drain().collect()
        };
        for run in dirty {
            self.gang_step(&run).await;
        }
    }

    /// One gang's next step from its current attempt's members (as
    /// recorded): nothing, stop the members still running, queue it again,
    /// or end it.
    async fn gang_step(self: &Arc<Self>, run: &str) {
        let Some((gst, attempt, failures, retries, policy, infra_used)) =
            self.sched.lock().expect("sched").gangs.get(run).map(|r| {
                (
                    r.state.clone(),
                    r.attempt,
                    r.failures,
                    r.retries,
                    r.policy.clone(),
                    r.infra_used,
                )
            })
        else {
            return;
        };
        if gst != gstate::RUNNING && gst != gstate::CANCELING {
            return;
        }
        let r = run.to_string();
        let members = match self
            .store
            .call(move |c| gang::members(c, &r, Some(attempt), now_ms()))
            .await
        {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(%run, "reading a gang's members: {e}");
                self.sched
                    .lock()
                    .expect("sched")
                    .gang_dirty
                    .insert(run.to_string());
                return;
            }
        };
        let view: Vec<gang::Ended> = members.iter().map(gang::Ended::from).collect();
        let v = gang::verdict(&view);
        let canceling = gst == gstate::CANCELING;
        let at = now_ms();
        let end = |to: &str, error: String, result_job: i64| Op::GangEnd {
            run: run.to_string(),
            attempt,
            state: to.to_string(),
            error,
            result_job,
            at_ms: at,
        };
        let mut delay = Duration::ZERO;
        let mut waiting = String::new();
        let op = match (&v, canceling) {
            (Verdict::Succeeded, _) => {
                let rank0 = members.iter().find(|m| m.rank == 0).map_or(0, |m| m.job);
                end(gstate::SUCCEEDED, String::new(), rank0)
            }
            (Verdict::Running, false) => return,
            (Verdict::Running | Verdict::Stopping, true) => {
                self.stop_members(&members, "canceled by its owner").await;
                return;
            }
            (Verdict::Stopping, false) => {
                // The member that failed first, not the lowest rank.
                let why = gang::first_failure(
                    view.iter()
                        .filter(|m| state::is_terminal(&m.state) && m.state != state::SUCCEEDED),
                )
                .map(|m| format!("rank {} on {} {}", m.rank, m.host, m.state))
                .unwrap_or_default();
                self.stop_members(&members, &format!("{}{why}", gang::STOPPED))
                    .await;
                return;
            }
            (Verdict::Failed { .. }, true) => {
                end(gstate::CANCELED, "canceled by its owner".into(), 0)
            }
            (Verdict::Failed { code, infra, why }, false) => {
                // A member that ran long enough refills the budget for lost
                // machines: a run of days may lose many over its life.
                let refill = members
                    .iter()
                    .any(|m| m.seconds >= gang::INFRA_REFILL_AFTER.as_secs_f64());
                let total = failures + u32::from(*code);
                let used = if refill { 0 } else { infra_used } + u32::from(*infra);
                if *code && total > retries {
                    end(
                        gstate::FAILED,
                        format!(
                            "{why} (attempt {attempt}; {total} code failures, {retries} retries allowed)"
                        ),
                        0,
                    )
                } else if *infra && used > policy.infra_retries {
                    end(
                        gstate::FAILED,
                        format!(
                            "{why} (attempt {attempt}; {used} lost computers or starts since a member last ran 10 minutes, {} allowed)",
                            policy.infra_retries
                        ),
                        0,
                    )
                } else {
                    delay = if *code {
                        policy.backoff.delay(total)
                    } else {
                        Duration::from_secs(1)
                    };
                    // A lost host stops its members itself once it stops
                    // hearing from us: place the next attempt only after it
                    // must have, so no rank ever runs twice at once.
                    let (wait, fencing) = self.fence_wait(&members);
                    delay = delay.max(wait);
                    waiting = fencing;
                    Metrics::inc(&self.metrics.retries);
                    Op::GangRequeue {
                        run: run.to_string(),
                        attempt,
                        failed: *code,
                        infra: *infra,
                        refill,
                        error: why.clone(),
                        ready_ms: at + delay.as_millis() as i64,
                    }
                }
            }
        };
        let applied = self
            .persist_ops(vec![op.clone()])
            .await
            .ok()
            .and_then(|o| o.first().map(OpOut::applied))
            .unwrap_or(false);
        if !applied {
            return;
        }
        let mut g = self.sched.lock().expect("sched");
        match op {
            Op::GangEnd { state: to, .. } => {
                tracing::info!(%run, attempt, state = %to, "gang ended");
                g.gangs.remove(run);
                g.gang_jobs.retain(|_, r| r != run);
            }
            Op::GangRequeue {
                failed,
                infra,
                refill,
                error,
                ..
            } => {
                tracing::info!(%run, attempt, failed, infra, ?delay, "gang queued again: {error}");
                if let Some(r) = g.gangs.get_mut(run) {
                    r.state = gstate::QUEUED.into();
                    r.failures += u32::from(failed);
                    r.infra_used = if refill { 0 } else { r.infra_used } + u32::from(infra);
                    r.ready_at = Instant::now() + delay;
                    r.waiting = waiting;
                }
            }
            _ => {}
        }
        drop(g);
        self.wake.notify_one();
    }

    /// Stops the members still running (fenced by their attempts).
    async fn stop_members(&self, members: &[gang::MemberRow], reason: &str) {
        self.stop_jobs(
            members
                .iter()
                .filter(|m| !state::is_terminal(&m.state))
                .map(|m| (m.job, reason.to_string()))
                .collect(),
        )
        .await;
    }

    /// Stops running jobs, each for its own reason (its host records it as
    /// the job's error).
    async fn stop_jobs(&self, jobs: Vec<(i64, String)>) {
        let mut changes = Vec::new();
        let mut sends = Vec::new();
        {
            let mut g = self.sched.lock().expect("sched");
            for (id, reason) in jobs {
                let Some(f) = g.inflight.get_mut(&id) else {
                    continue;
                };
                if f.user_cancel {
                    continue;
                }
                f.user_cancel = true;
                let (node, attempt) = (f.node.clone(), f.attempt);
                changes.push(Change::Canceling { id });
                sends.push((
                    g.nodes.get(&node).map(|n| n.link.clone()),
                    id,
                    attempt,
                    reason,
                ));
            }
        }
        // Only what this coordinator recorded is sent: a holder that lost
        // its lease stops nobody's members.
        let applied = self.persist(changes).await;
        let sends: Vec<_> = sends
            .into_iter()
            .zip(applied)
            .filter_map(|((link, id, attempt, reason), ok)| {
                ok.then_some((link?, id, attempt, reason))
            })
            .collect();
        for (link, id, attempt, reason) in sends {
            let job_id = host_job_id(&self.cluster_id, id);
            Metrics::inc(&self.metrics.fences);
            tokio::spawn(async move {
                let _ = link
                    .cancel(pb::CancelHostJobRequest {
                        job_id,
                        attempt,
                        reason,
                    })
                    .await;
            });
        }
    }

    /// How long until every lost host among `members`' hosts must have
    /// stopped its members itself (it reports how long that takes after it
    /// last heard from us; zero for hosts that do not), and what to say
    /// meanwhile.
    fn fence_wait(&self, members: &[gang::MemberRow]) -> (Duration, String) {
        let g = self.sched.lock().expect("sched");
        let now = Instant::now();
        let waits: Vec<(Duration, String, u32)> = members
            .iter()
            .filter_map(|m| {
                let n = g.nodes.get(&m.host)?;
                if n.health != Health::Down {
                    return None;
                }
                let fence = n.status.as_ref().map_or(0, |s| s.self_fence_after_ms);
                if fence == 0 {
                    return None;
                }
                let last = n.last_ok?;
                Some((
                    (last + Duration::from_millis(u64::from(fence)) + FENCE_MARGIN)
                        .saturating_duration_since(now),
                    m.host.clone(),
                    fence,
                ))
            })
            .collect();
        let Some(max) = waits.iter().map(|w| w.0).max() else {
            return (Duration::ZERO, String::new());
        };
        let hosts: Vec<String> = waits
            .iter()
            .map(|(_, h, f)| format!("{h} (self-fence {} s)", f.div_ceil(1000)))
            .collect();
        (
            max,
            format!(
                "re-placing once {} must have stopped its member: a host that cannot reach the pool stops its gang members itself",
                hosts.join(", ")
            ),
        )
    }

    /// Start barriers and the progress watchdog of the running gangs:
    /// releases a barrier once every member of its attempt is ready, ends
    /// an attempt whose barrier took longer than its timeout (an
    /// infrastructure failure), and ends one with a member that made no
    /// progress for its stall timeout (a code failure).
    pub(super) async fn gang_watch(self: &Arc<Self>) {
        let mut stops: Vec<(i64, String)> = Vec::new();
        {
            let mut guard = self.sched.lock().expect("sched");
            let g = &mut *guard;
            let live: HashSet<i64> = g.inflight.keys().copied().collect();
            g.progress.retain(|id, _| live.contains(id));
            // The live members of each gang: run -> (job, rank, host, job
            // attempt, gang attempt).
            let mut by_run: HashMap<String, Vec<LiveMember>> = HashMap::new();
            for (id, run) in &g.gang_jobs {
                if let Some(f) = g.inflight.get(id) {
                    let env = |k: &str| f.spec.env.get(k).and_then(|v| v.parse::<u32>().ok());
                    by_run.entry(run.clone()).or_default().push((
                        *id,
                        env("CUA_RANK").unwrap_or(0),
                        f.node.clone(),
                        f.attempt,
                        env("CUA_ATTEMPT").unwrap_or(0),
                    ));
                }
            }
            for (run, mut members) in by_run {
                let Some(r) = g.gangs.get_mut(&run) else {
                    continue;
                };
                if r.state != gstate::RUNNING || r.watched_out == r.attempt {
                    continue;
                }
                let attempt = r.attempt;
                members.retain(|m| m.4 == attempt);
                members.sort_by_key(|m| m.1);
                if members.is_empty() {
                    continue;
                }
                let prog = |id: i64| g.progress.get(&id);
                // The start barrier.
                if r.policy.barrier_timeout_secs > 0 {
                    let all_here = members.len() as u32 == r.size;
                    let not_ready: Vec<String> = members
                        .iter()
                        .filter(|m| !prog(m.0).is_some_and(|p| p.ready && p.attempt == m.3))
                        .map(|m| format!("rank {} on {}", m.1, m.2))
                        .collect();
                    if !r.released && all_here && not_ready.is_empty() {
                        r.released = true;
                        tracing::info!(%run, attempt, size = r.size, waited = ?r.placed_at.elapsed(), "gang ready: start barrier released");
                    }
                    if !r.released {
                        let limit = Duration::from_secs(u64::from(r.policy.barrier_timeout_secs));
                        if r.placed_at.elapsed() > limit {
                            let why = format!(
                                "{}after {} s, {} not ready",
                                gang::BARRIER_TIMEOUT,
                                limit.as_secs(),
                                if not_ready.is_empty() {
                                    "a member".to_string()
                                } else {
                                    not_ready.join(", ")
                                }
                            );
                            tracing::warn!(%run, attempt, "{why}");
                            r.watched_out = attempt;
                            stops.extend(members.iter().map(|m| (m.0, why.clone())));
                        }
                        continue;
                    }
                    // Released: tell each host until it says it let its
                    // member go.
                    for m in &members {
                        if prog(m.0).is_some_and(|p| p.ready && !p.released) {
                            g.releases.entry(m.2.clone()).or_default().insert(m.0, m.3);
                        }
                    }
                }
                // The progress watchdog.
                if r.policy.stall_timeout_secs > 0 {
                    let limit = Duration::from_secs(u64::from(r.policy.stall_timeout_secs));
                    let stalled = members.iter().find_map(|m| {
                        let p = prog(m.0)?;
                        let quiet = p.quiet_for()?;
                        (quiet > limit).then(|| (m.0, m.1, m.2.clone(), quiet, p.step))
                    });
                    if let Some((id, rank, host, quiet, step)) = stalled {
                        let why = format!(
                            "{}rank {rank} on {host} made no progress for {} s (last step {step})",
                            gang::STALLED,
                            quiet.as_secs()
                        );
                        tracing::warn!(%run, attempt, "{why}");
                        r.watched_out = attempt;
                        stops.push((id, why.clone()));
                        stops.extend(
                            members
                                .iter()
                                .filter(|m| m.0 != id)
                                .map(|m| (m.0, format!("{}{why}", gang::STOPPED))),
                        );
                    }
                }
            }
        }
        if !stops.is_empty() {
            self.stop_jobs(stops).await;
        }
    }

    /// Its owner cancels gang `run` (when it is one that has not ended).
    /// Whether it was a gang.
    pub(super) async fn cancel_gang(&self, run: &str) -> bool {
        let live = self
            .sched
            .lock()
            .expect("sched")
            .gangs
            .get(run)
            .map(|r| r.state.clone());
        let Some(was) = live else {
            return false;
        };
        let applied = self
            .persist_ops(vec![Op::GangCancel {
                run: run.to_string(),
                at_ms: now_ms(),
            }])
            .await
            .ok()
            .and_then(|o| o.first().map(OpOut::applied))
            .unwrap_or(false);
        if applied {
            let mut g = self.sched.lock().expect("sched");
            if was == gstate::QUEUED {
                g.gangs.remove(run);
                g.gang_jobs.retain(|_, r| r != run);
            } else if let Some(r) = g.gangs.get_mut(run) {
                r.state = gstate::CANCELING.into();
                g.gang_dirty.insert(run.to_string());
            }
        }
        self.wake.notify_one();
        true
    }

    /// Why a queued gang is not placed (empty: it is, or it waits for its
    /// backoff).
    pub fn gang_waiting(&self, run: &str) -> String {
        self.sched
            .lock()
            .expect("sched")
            .gangs
            .get(run)
            .map(|r| r.waiting.clone())
            .unwrap_or_default()
    }

    /// What an hour of host `name` costs, as it last said.
    pub fn node_usd_per_hour(&self, name: &str) -> f64 {
        self.sched
            .lock()
            .expect("sched")
            .nodes
            .get(name)
            .and_then(|n| n.status.as_ref())
            .map_or(0.0, |s| s.usd_per_hour)
    }

    /// Job `id`'s output on its host from byte `from` (`GetHostJob` with
    /// `output_from`): its host's name and answer.
    pub async fn job_output(
        &self,
        id: i64,
        from: u64,
    ) -> Result<(String, pb::GetHostJobResponse), CoordError> {
        let node = self
            .sched
            .lock()
            .expect("sched")
            .inflight
            .get(&id)
            .map(|f| f.node.clone());
        let node = match node {
            Some(n) => n,
            None => {
                self.store
                    .call(move |c| store::job(c, id))
                    .await
                    .map_err(CoordError::Store)?
                    .ok_or_else(|| CoordError::NotFound(format!("no job {id}")))?
                    .node
            }
        };
        if node.is_empty() {
            return Err(CoordError::Unavailable(format!(
                "job {id} has not run on a computer yet"
            )));
        }
        let link = self
            .sched
            .lock()
            .expect("sched")
            .nodes
            .get(&node)
            .map(|n| n.link.clone())
            .ok_or_else(|| {
                CoordError::NotFound(format!("job {id}'s computer {node} left the pool"))
            })?;
        let r = link
            .get_output(&host_job_id(&self.cluster_id, id), from)
            .await
            .map_err(|e| CoordError::Unavailable(format!("{node}: {e}")))?;
        Ok((node, r))
    }
}
