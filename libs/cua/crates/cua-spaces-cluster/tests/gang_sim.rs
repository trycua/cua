// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Seeded fault simulation of gangs through the pool: real
//! [`Coordinator`]s recording into the in-memory pool log with the relay's
//! rules ([`MemPool`], sealed as on the relay), real host job runners, and
//! a seeded schedule of faults: machines that go off mid-run (what ran
//! there stops) and come back, coordinators that crash or pause past their
//! lease while another takes over from the log, lost appends and lost
//! answers, and members that fail on their own.
//!
//! After each run the history is checked against the gang invariants:
//!
//! 1. **Never two live attempts of one member**: no rank of a gang ever
//!    runs twice at once (an attempt is placed only once every member of
//!    the one before ended).
//! 2. **No partial gang beyond the cancel window**: once a member of an
//!    attempt ended badly, every other member of that attempt stopped
//!    within the window (a lost host is known lost after
//!    `node_down_after`).
//! 3. **One terminal state per run**: every gang ends, its end is in the
//!    log once, and a gang that succeeded returns the result rank 0 of its
//!    last attempt produced.
//!
//! Machines are lost by going off (their processes stop with them) and by
//! being cut off (`cut`: every call to them fails, but what runs there
//! keeps running): a cut-off host stops its own gang members once it has
//! not heard from a coordinator for its self-fence time, and the
//! coordinator places their gang again only after that. Every member waits
//! at a start barrier the coordinator releases once its whole attempt is
//! ready, and reports progress while it works. Invariant 1 then also
//! covers the stale workers of cut-off hosts; the run counts them.
//! `CUA_GANG_SIM_SEEDS=a..b` runs more seeds.

mod common;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use cua_cluster::JobSpec;
use cua_cluster::link::{LinkError, LocalLink, NodeLink};
use cua_cluster::node::{ExecEnd, ExecOutcome, JobExecutor, NodeConfig, NodeJobs, Offers};
use cua_proto::env::v1 as pb;
use cua_spaces_cluster::coordinator::journal::{self, LogReader as _};
use cua_spaces_cluster::coordinator::requests::{self, Caller};
use cua_spaces_cluster::coordinator::store::{self, NodeRow};
use cua_spaces_cluster::coordinator::{Config, Coordinator, LinkFactory};
use cua_spaces_cluster::pool::keys::{PoolKeys, SealedLog};
use cua_spaces_cluster::pool::memlog::MemPool;
use cua_spaces_cluster::pool::seal::PoolSecret;
use rand::{Rng, SeedableRng, rngs::StdRng};
use serde_json::json;
use tokio_util::sync::CancellationToken;

/// The cancel window: a lost host is found lost after `node_down_after`
/// (1.2 s here) plus a feed call; a cancel reaches a live host at once.
const WINDOW: Duration = Duration::from_secs(5);

#[derive(Clone, Debug)]
struct Exe {
    run: String,
    attempt: u32,
    rank: u32,
    host: String,
    job: String,
    start: Instant,
    end: Option<Instant>,
    /// Ended badly (failed or its machine went off).
    bad: bool,
    output: String,
    /// It ran on while its host was cut off (a stale worker), and ended.
    stale: bool,
}

#[derive(Clone, Default)]
struct History {
    exes: Vec<Exe>,
    /// Members that started while another attempt of the same rank ran.
    doubles: Vec<String>,
}

type Hist = Arc<Mutex<History>>;

/// A running member's start barrier and progress, as the cua jobs runtime
/// would report them.
#[derive(Clone, Default)]
struct Ctl {
    attempt: u32,
    ready: bool,
    released: bool,
    last_progress_ms: i64,
    step: i64,
}

#[derive(Clone)]
struct Exec {
    host: String,
    hist: Hist,
    kill: Arc<Mutex<CancellationToken>>,
    seed: u64,
    /// Calls to this host fail (it is off or cut off).
    off: Arc<AtomicBool>,
    ctl: Arc<Mutex<HashMap<String, Ctl>>>,
}

fn fails(seed: u64, run: &str, attempt: u32, rank: u32) -> bool {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (seed, run, attempt, rank).hash(&mut h);
    h.finish() % 100 < 12
}

#[async_trait]
impl JobExecutor for Exec {
    async fn run(&self, job: &JobSpec, _space: &str, cancel: CancellationToken) -> ExecOutcome {
        let env = |k: &str| job.env.get(k).cloned().unwrap_or_default();
        let run = env("CUA_RUN_ID");
        let rank: u32 = env("CUA_RANK").parse().unwrap_or(0);
        let attempt: u32 = env("CUA_ATTEMPT").parse().unwrap_or(0);
        let kill = self.kill.lock().unwrap().clone();
        let idx = {
            let mut h = self.hist.lock().unwrap();
            if let Some(o) = h
                .exes
                .iter()
                .find(|e| e.run == run && e.rank == rank && e.end.is_none())
            {
                let msg = format!(
                    "{run} rank {rank}: attempt {attempt} on {} started while attempt {} on {} ran",
                    self.host, o.attempt, o.host
                );
                h.doubles.push(msg);
            }
            h.exes.push(Exe {
                run: run.clone(),
                attempt,
                rank,
                host: self.host.clone(),
                job: job.job_id.clone(),
                start: Instant::now(),
                end: None,
                bad: false,
                output: String::new(),
                stale: false,
            });
            h.exes.len() - 1
        };
        let ms: u64 = job.argv.get(1).and_then(|a| a.parse().ok()).unwrap_or(300);
        let fail = fails(self.seed, &run, attempt, rank);
        let sleep = if fail { ms / 3 } else { ms };
        let barrier = job.env.contains_key("CUA_BARRIER_TIMEOUT");
        let id = job.job_id.clone();
        self.ctl.lock().unwrap().insert(
            id.clone(),
            Ctl {
                attempt: job.attempt,
                ready: true,
                released: !barrier,
                last_progress_ms: 0,
                step: -1,
            },
        );
        let ctl = self.ctl.clone();
        let work = async move {
            // The start barrier, then the work, with a heartbeat per step.
            while !ctl.lock().unwrap().get(&id).is_some_and(|c| c.released) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            let t0 = Instant::now();
            let mut step = 0;
            while t0.elapsed() < Duration::from_millis(sleep) {
                step += 1;
                if let Some(c) = ctl.lock().unwrap().get_mut(&id) {
                    c.step = step;
                    c.last_progress_ms = cua_cluster::model::now_ms();
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        };
        let end = tokio::select! {
            _ = cancel.cancelled() => ExecEnd::Canceled,
            _ = kill.cancelled() => ExecEnd::Error("the machine went off".into()),
            _ = work => ExecEnd::Exited(if fail { 1 } else { 0 }),
        };
        // Stopped while its host was unreachable and still powered: its
        // host fenced it (a stale worker that did not outlive its gang).
        let stale = matches!(end, ExecEnd::Canceled) && self.off.load(Ordering::SeqCst);
        self.ctl.lock().unwrap().remove(&job.job_id);
        let output = format!("{}:{}:{run}:{attempt}:{rank}", self.host, job.job_id);
        {
            let mut h = self.hist.lock().unwrap();
            let e = &mut h.exes[idx];
            e.end = Some(Instant::now());
            e.bad = matches!(end, ExecEnd::Exited(1) | ExecEnd::Error(_));
            e.output = output.clone();
            e.stale = stale;
        }
        let mut o = ExecOutcome::ended(end);
        o.result = output.into_bytes();
        o
    }

    async fn remove_space(&self, _space: &str) {}

    fn offers(&self) -> Offers {
        Offers {
            kinds: vec!["container".into(), "machine".into()],
            runtimes: vec![],
        }
    }

    fn progress(&self, job_id: &str, attempt: u32) -> Option<pb::HostJobProgress> {
        let g = self.ctl.lock().unwrap();
        let c = g.get(job_id).filter(|c| c.attempt == attempt)?;
        Some(pb::HostJobProgress {
            job_id: job_id.into(),
            attempt,
            ready: c.ready,
            released: c.released,
            last_progress_ms: c.last_progress_ms,
            step: c.step,
        })
    }

    fn release(&self, job_id: &str, attempt: u32) {
        if let Some(c) = self.ctl.lock().unwrap().get_mut(job_id)
            && c.attempt == attempt
        {
            c.released = true;
        }
    }
}

/// A host's self-fence time in the sim (the coordinator's `node_down_after`
/// is 1.2 s).
const SELF_FENCE: Duration = Duration::from_millis(1500);

/// A host link every coordinator loses while its machine is off.
struct HostLink {
    inner: Arc<dyn NodeLink>,
    off: Arc<AtomicBool>,
}

impl HostLink {
    fn check(&self) -> Result<(), LinkError> {
        if self.off.load(Ordering::SeqCst) {
            Err(LinkError::Unreachable("off".into()))
        } else {
            Ok(())
        }
    }
}

#[async_trait]
impl NodeLink for HostLink {
    async fn run(&self, req: pb::RunHostJobsRequest) -> Result<pb::RunHostJobsResponse, LinkError> {
        self.check()?;
        let r = self.inner.run(req).await;
        self.check()?;
        r
    }
    async fn get(&self, job_id: &str) -> Result<pb::GetHostJobResponse, LinkError> {
        self.check()?;
        self.inner.get(job_id).await
    }
    async fn list(
        &self,
        req: pb::ListHostJobsRequest,
    ) -> Result<pb::ListHostJobsResponse, LinkError> {
        self.check()?;
        let r = self.inner.list(req).await;
        self.check()?;
        r
    }
    async fn cancel(
        &self,
        req: pb::CancelHostJobRequest,
    ) -> Result<pb::CancelHostJobResponse, LinkError> {
        self.check()?;
        self.inner.cancel(req).await
    }
    async fn host(&self) -> Result<pb::GetHostSpacesResponse, LinkError> {
        self.check()?;
        self.inner.host().await
    }
    async fn create_space(
        &self,
        req: pb::CreateHostSpaceRequest,
    ) -> Result<pb::HostSpace, LinkError> {
        self.inner.create_space(req).await
    }
    async fn delete_space(&self, space: &str) -> Result<String, LinkError> {
        self.inner.delete_space(space).await
    }
    fn describe(&self) -> String {
        self.inner.describe()
    }
}

struct Sim {
    root: tempfile::TempDir,
    pool: MemPool,
    keys: HashMap<String, Arc<PoolKeys>>,
    links: HashMap<String, Arc<dyn NodeLink>>,
    off: HashMap<String, Arc<AtomicBool>>,
    kills: HashMap<String, Arc<Mutex<CancellationToken>>>,
    hist: Hist,
}

fn config(dir: &std::path::Path) -> Config {
    let mut c = Config::new(dir);
    c.poll_wait = Duration::from_millis(200);
    c.node_down_after = Duration::from_millis(1200);
    c.ack_grace = Duration::from_millis(1500);
    c.run_grace = Duration::from_secs(5);
    c
}

impl Sim {
    fn new(n_hosts: usize, seed: u64) -> Self {
        let root = tempfile::tempdir().unwrap();
        let hist: Hist = Arc::default();
        let mut links = HashMap::new();
        let mut off = HashMap::new();
        let mut kills = HashMap::new();
        for i in 0..n_hosts {
            let name = format!("h{i}");
            let kill = Arc::new(Mutex::new(CancellationToken::new()));
            let o = Arc::new(AtomicBool::new(false));
            let jobs = NodeJobs::open(
                NodeConfig {
                    dir: root.path().join(&name),
                    millicpus_total: 4000,
                    memory_total_mb: 1 << 20,
                    jobs_max: 0,
                    min_available_mb: 0,
                    retention: Duration::from_secs(3600),
                    output_retention: Duration::from_secs(3600),
                    boot_id: name.clone(),
                },
                Arc::new(Exec {
                    host: name.clone(),
                    hist: hist.clone(),
                    kill: kill.clone(),
                    seed,
                    off: o.clone(),
                    ctl: Arc::default(),
                }),
            )
            .unwrap();
            jobs.set_self_fence_after(SELF_FENCE);
            links.insert(
                name.clone(),
                Arc::new(HostLink {
                    inner: Arc::new(LocalLink::new(jobs, name.clone(), "linux")),
                    off: o.clone(),
                }) as Arc<dyn NodeLink>,
            );
            off.insert(name.clone(), o);
            kills.insert(name, kill);
        }
        let pool = MemPool::new();
        let dir = Arc::new(pool.keys("sim"));
        let keys = ["a", "b", "c"]
            .iter()
            .map(|n| {
                (
                    n.to_string(),
                    PoolKeys::new(
                        "sim",
                        common::TestDevice::new(),
                        PoolSecret::generate(),
                        dir.clone(),
                    ),
                )
            })
            .collect();
        Self {
            root,
            pool,
            keys,
            links,
            off,
            kills,
            hist,
        }
    }

    async fn keys_up(&self) {
        for _ in 0..2 {
            for k in self.keys.values() {
                let _ = k.sync().await;
            }
        }
        for k in self.keys.values() {
            k.sync().await.expect("keys");
        }
    }

    /// `who` takes the lease and goes on from the log.
    async fn take_over(&self, who: &str) -> Arc<Coordinator> {
        self.pool.expire();
        let epoch = self.pool.acquire(who).expect("vacant");
        let dir = self.root.path().join(who);
        let keys = self.keys[who].clone();
        keys.sync().await.expect("keys");
        let log = SealedLog::new(self.pool.log(epoch), keys);
        journal::rebuild(&dir.join("cluster.db"), "sim", &log)
            .await
            .expect("rebuild");
        let links = self.links.clone();
        let factory: LinkFactory = Arc::new(move |row: &NodeRow| links.get(&row.name).cloned());
        let c = Coordinator::start_with(config(&dir), Some(factory), Arc::new(log))
            .await
            .expect("start");
        let mut names: Vec<&String> = self.links.keys().collect();
        names.sort();
        for name in names {
            let _ = c
                .add_node(NodeRow {
                    name: name.clone(),
                    via: "local".into(),
                    state: "ready".into(),
                    os: "linux".into(),
                    ..Default::default()
                })
                .await;
        }
        c
    }

    /// The host is cut off from every coordinator; what runs there keeps
    /// running (until the host fences it).
    fn cut(&self, host: &str, cut: bool) {
        self.off[host].store(cut, Ordering::SeqCst);
    }

    fn power(&self, host: &str, on: bool) {
        if !on {
            self.off[host].store(true, Ordering::SeqCst);
            let mut k = self.kills[host].lock().unwrap();
            k.cancel();
            *k = CancellationToken::new();
        } else {
            self.off[host].store(false, Ordering::SeqCst);
        }
    }
}

async fn run_seed(seed: u64) {
    let mut rng = StdRng::seed_from_u64(seed);
    let hosts = 4;
    let sim = Sim::new(hosts, seed);
    sim.keys_up().await;
    let names = ["a", "b", "c"];
    let mut who = 0usize;
    let mut coord = sim.take_over(names[who]).await;
    let runs: Vec<String> = (0..5).map(|i| format!("g{i}")).collect();
    for run in &runs {
        let min = rng.random_range(1..3u32);
        let max = rng.random_range(min..4u32);
        let ms = rng.random_range(300..1500u64);
        requests::handle(
            &coord,
            "submit",
            json!({
                "run": run, "gang": true, "nodes": {"min": min, "max": max},
                "kind": "machine", "command": ["work", ms.to_string()],
                "cpus": 1, "memory_mb": 64, "timeout_secs": 60, "retries": 3,
                "result_path": "out", "barrier_timeout_secs": 20,
            }),
            &Caller::local(),
        )
        .await
        .unwrap();
    }
    let mut old: Vec<Arc<Coordinator>> = Vec::new();
    let steps = rng.random_range(5..10);
    for _ in 0..steps {
        tokio::time::sleep(Duration::from_millis(rng.random_range(200..900))).await;
        match rng.random_range(0..7) {
            // A machine goes off for a while (what ran there stops).
            0 | 1 => {
                let h = format!("h{}", rng.random_range(0..hosts));
                sim.power(&h, false);
                let ms = rng.random_range(500..3000);
                let (off, _) = (sim.off[&h].clone(), ());
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(ms)).await;
                    off.store(false, Ordering::SeqCst);
                });
            }
            // The holder pauses past its lease; another takes over.
            2 => {
                old.push(coord);
                who = (who + 1) % names.len();
                coord = sim.take_over(names[who]).await;
            }
            // The holder crashes; another takes over.
            3 => {
                coord.stop();
                who = (who + 1) % names.len();
                coord = sim.take_over(names[who]).await;
            }
            4 => sim.pool.drop_requests(rng.random_range(1..3)),
            // A host is cut off for a while: what runs there keeps running
            // until the host fences it.
            5 => {
                let h = format!("h{}", rng.random_range(0..hosts));
                sim.cut(&h, true);
                let ms = rng.random_range(500..4000);
                let off = sim.off[&h].clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(ms)).await;
                    off.store(false, Ordering::SeqCst);
                });
            }
            _ => sim.pool.drop_answers(rng.random_range(1..3)),
        }
    }
    for h in sim.off.keys() {
        sim.power(h, true);
    }
    // Every gang ends.
    let deadline = Instant::now() + Duration::from_secs(150);
    loop {
        let mut pending = Vec::new();
        for run in &runs {
            let v = requests::handle(&coord, "run", json!({"run": run}), &Caller::local())
                .await
                .unwrap();
            let st = v["state"].as_str().unwrap_or("").to_string();
            if !matches!(st.as_str(), "succeeded" | "failed" | "canceled") {
                pending.push(format!("{run}: {st} {}", v["gang"]));
            }
        }
        if pending.is_empty() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "seed {seed}: gangs did not end: {pending:?} (holder fenced: {})",
            coord.is_fenced()
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    for o in &old {
        o.stop();
    }
    // Every execution has ended by now (nothing runs past its gang).
    tokio::time::sleep(Duration::from_millis(500)).await;
    let hist = sim.hist.lock().unwrap().clone();
    // 1. Never two live attempts of one member.
    assert!(hist.doubles.is_empty(), "seed {seed}: {:?}", hist.doubles);
    assert!(
        hist.exes.iter().all(|e| e.end.is_some()),
        "seed {seed}: a member still runs after its gang ended"
    );
    // ... and an attempt starts only after every member of the one before
    // it ended.
    for run in &runs {
        let mut by_attempt: std::collections::BTreeMap<u32, (Instant, Instant)> =
            std::collections::BTreeMap::new();
        for e in hist.exes.iter().filter(|e| &e.run == run) {
            let end = e.end.expect("ended");
            let s = by_attempt.entry(e.attempt).or_insert((e.start, end));
            s.0 = s.0.min(e.start);
            s.1 = s.1.max(end);
        }
        let spans: Vec<(u32, (Instant, Instant))> = by_attempt.into_iter().collect();
        for w in spans.windows(2) {
            assert!(
                w[1].1.0 >= w[0].1.1,
                "seed {seed}: {run} attempt {} started before attempt {} ended",
                w[1].0,
                w[0].0
            );
        }
    }
    // 2. No partial gang beyond the cancel window.
    let mut attempts: HashMap<(String, u32), Vec<&Exe>> = HashMap::new();
    for e in &hist.exes {
        attempts
            .entry((e.run.clone(), e.attempt))
            .or_default()
            .push(e);
    }
    for ((run, attempt), members) in &attempts {
        let Some(first_bad) = members.iter().filter(|e| e.bad).filter_map(|e| e.end).min() else {
            continue;
        };
        for e in members {
            let end = e.end.expect("ended");
            assert!(
                end <= first_bad + WINDOW,
                "seed {seed}: {run} attempt {attempt} rank {} ran {:?} past a failed member",
                e.rank,
                end - first_bad
            );
        }
    }
    // 3. One terminal state per run, recorded once, and a success returns
    //    what rank 0 of its last attempt produced.
    let log = SealedLog::new(sim.pool.log(0), sim.keys[names[who]].clone());
    let mut entries = Vec::new();
    let mut after = 0;
    while let Some((page, head)) = log.read(after).await.unwrap() {
        if page.is_empty() {
            break;
        }
        after = page.last().unwrap().seq;
        entries.extend(page);
        if after >= head {
            break;
        }
    }
    let mut ends: HashMap<String, u32> = HashMap::new();
    for e in &entries {
        let op: store::Op = serde_json::from_value(e.data.clone()).unwrap();
        if let store::Op::GangEnd { run, .. } = op {
            *ends.entry(run).or_default() += 1;
        }
    }
    let mut states = HashSet::new();
    for run in &runs {
        assert_eq!(ends.get(run), Some(&1), "seed {seed}: {run} ended {ends:?}");
        let v = requests::handle(&coord, "run", json!({"run": run}), &Caller::local())
            .await
            .unwrap();
        let st = v["state"].as_str().unwrap().to_string();
        states.insert(st.clone());
        if st == "succeeded" {
            let res = requests::handle(&coord, "run_result", json!({"run": run}), &Caller::local())
                .await
                .unwrap();
            use base64::Engine as _;
            let out = String::from_utf8(
                base64::engine::general_purpose::STANDARD
                    .decode(res["result_b64"].as_str().unwrap())
                    .unwrap(),
            )
            .unwrap();
            let last = v["gang"]["attempt"].as_u64().unwrap() as u32;
            let produced = hist
                .exes
                .iter()
                .find(|e| &e.run == run && e.attempt == last && e.rank == 0 && !e.bad)
                .unwrap_or_else(|| panic!("seed {seed}: {run} rank 0 never ran in attempt {last}"));
            assert_eq!(out, produced.output, "seed {seed}: {run}");
            assert!(out.contains(&produced.job));
        }
    }
    let bad = hist.exes.iter().filter(|e| e.bad).count();
    let stale = hist.exes.iter().filter(|e| e.stale).count();
    eprintln!(
        "seed {seed}: {} executions ({bad} bad, {stale} stale workers fenced by their cut-off host), {} log entries, gang states {states:?}",
        hist.exes.len(),
        entries.len()
    );
    coord.stop();
}

fn seeds() -> std::ops::Range<u64> {
    std::env::var("CUA_GANG_SIM_SEEDS")
        .ok()
        .and_then(|v| {
            let (a, b) = v.split_once("..")?;
            Some(a.parse().ok()?..b.parse().ok()?)
        })
        .unwrap_or(1..4)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gangs_survive_lost_machines_takeovers_and_failures() {
    for seed in seeds() {
        run_seed(seed).await;
        eprintln!("seed {seed}: ok");
    }
}
