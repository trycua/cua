// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Seeded fault simulation of the real coordinator code behind the pool:
//! real [`Coordinator`]s recording into an in-memory pool log with the
//! relay's rules ([`MemPool`]), real host job runners ([`NodeJobs`]) behind
//! links that can be cut per coordinator, and a seeded schedule of faults:
//! takeovers while the old holder still runs (a paused process waking up),
//! coordinator crashes, partitions between a coordinator and a host, and
//! lost appends and lost append answers. The log is sealed end to end as
//! on the relay ([`SealedLog`] over the account's keyring), and devices
//! come and go during the run: a device that holds the keys is revoked
//! (the old keys stop at once; the holder makes new ones without it) and a
//! new one joins (it is given the keys). Each seed's fault schedule is
//! reproducible (thread interleavings are not: the code runs on tokio's
//! multi-threaded runtime and SQLite threads); `CUA_POOL_SIM_SEEDS=a..b`
//! runs more seeds.
//!
//! After every run the history is checked against the pool's invariants:
//! one writer per epoch, at most one result per job, every result produced
//! by a real execution of the attempt it names, and every submitted job
//! ended (succeeded) with nothing lost; and against the keys': every entry
//! is sealed, none appended after the revocation opens with the keys the
//! revoked device had, and the device that joined reads the whole log.

mod common;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use cua_cluster::JobSpec;
use cua_cluster::link::{LinkError, LocalLink, NodeLink};
use cua_cluster::node::{ExecEnd, ExecOutcome, JobExecutor, NodeConfig, NodeJobs, Offers};
use cua_proto::env::v1 as pb;
use cua_spaces_cluster::coordinator::journal;
use cua_spaces_cluster::coordinator::store::{self, NewJob, NodeRow};
use cua_spaces_cluster::coordinator::{Config, Coordinator, LinkFactory};
use cua_spaces_cluster::pool::keys::{PoolKeys, SealedLog};
use cua_spaces_cluster::pool::memlog::{Event, MemPool};
use cua_spaces_cluster::pool::seal::{Bundle, Envelope, PoolSecret};
use rand::{Rng, SeedableRng, rngs::StdRng};
use tokio_util::sync::CancellationToken;

/// Every execution any host finished: (job id, attempt) -> its output.
type Produced = Arc<Mutex<HashMap<(String, u32), String>>>;

#[derive(Clone)]
struct Exec {
    host: String,
    produced: Produced,
    ms: (u64, u64),
}

#[async_trait]
impl JobExecutor for Exec {
    async fn run(&self, job: &JobSpec, _space: &str, cancel: CancellationToken) -> ExecOutcome {
        let ms = self.ms.0 + (rand::random::<u64>() % (self.ms.1 - self.ms.0 + 1));
        tokio::select! {
            _ = cancel.cancelled() => return ExecOutcome::ended(ExecEnd::Canceled),
            _ = tokio::time::sleep(Duration::from_millis(ms)) => {}
        }
        let out = format!("{}:{}:{}", self.host, job.job_id, job.attempt);
        self.produced
            .lock()
            .unwrap()
            .insert((job.job_id.clone(), job.attempt), out.clone());
        let mut o = ExecOutcome::ended(ExecEnd::Exited(0));
        o.stdout = out.into_bytes();
        o.run_ms = ms as u32;
        o
    }

    async fn remove_space(&self, _space: &str) {}

    fn offers(&self) -> Offers {
        Offers {
            kinds: vec!["container".into()],
            runtimes: vec!["runc".into()],
        }
    }
}

/// A link that a coordinator loses (a partition) while `cut` is set.
struct CutLink {
    inner: Arc<dyn NodeLink>,
    cut: Arc<AtomicBool>,
}

impl CutLink {
    fn check(&self) -> Result<(), LinkError> {
        if self.cut.load(Ordering::SeqCst) {
            Err(LinkError::Unreachable("partitioned".into()))
        } else {
            Ok(())
        }
    }
}

#[async_trait]
impl NodeLink for CutLink {
    async fn run(&self, req: pb::RunHostJobsRequest) -> Result<pb::RunHostJobsResponse, LinkError> {
        self.check()?;
        let r = self.inner.run(req).await;
        // A partition that starts while the call is in flight loses the
        // answer, not the request.
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
        let mut req = req;
        req.wait_ms = req.wait_ms.min(200);
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
        self.check()?;
        self.inner.create_space(req).await
    }
    async fn delete_space(&self, space: &str) -> Result<String, LinkError> {
        self.check()?;
        self.inner.delete_space(space).await
    }
    fn describe(&self) -> String {
        self.inner.describe()
    }
}

type Cuts = Arc<Mutex<HashMap<(String, String), Arc<AtomicBool>>>>;

struct Sim {
    root: tempfile::TempDir,
    pool: MemPool,
    /// Each device's keys, by name (the machines a, b, c; the laptop x
    /// that is revoked; the machine y that joins).
    keys: HashMap<String, Arc<PoolKeys>>,
    hosts: Vec<(String, Arc<NodeJobs>)>,
    /// Partition switches: (coordinator, host) -> cut.
    cuts: Cuts,
    produced: Produced,
}

fn config(dir: &std::path::Path) -> Config {
    let mut c = Config::new(dir);
    c.poll_wait = Duration::from_millis(200);
    c.node_down_after = Duration::from_millis(1200);
    c.ack_grace = Duration::from_millis(1500);
    c.run_grace = Duration::from_secs(5);
    c.default_max_attempts = 50;
    c
}

impl Sim {
    fn new(n_hosts: usize) -> Self {
        let root = tempfile::tempdir().unwrap();
        let produced: Produced = Arc::default();
        let hosts = (0..n_hosts)
            .map(|i| {
                let name = format!("h{i}");
                let cfg = NodeConfig {
                    dir: root.path().join(&name),
                    millicpus_total: 8000,
                    memory_total_mb: 1 << 20,
                    jobs_max: 0,
                    min_available_mb: 0,
                    retention: Duration::from_secs(3600),
                    output_retention: Duration::from_secs(3600),
                    boot_id: name.clone(),
                };
                let exec = Exec {
                    host: name.clone(),
                    produced: produced.clone(),
                    ms: (20, 150),
                };
                (name.clone(), NodeJobs::open(cfg, Arc::new(exec)).unwrap())
            })
            .collect();
        let pool = MemPool::new();
        let dir = Arc::new(pool.keys("sim"));
        let keys = ["a", "b", "c", "x", "y"]
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
            hosts,
            cuts: Arc::default(),
            produced,
        }
    }

    fn cut(&self, coord: &str, host: &str) -> Arc<AtomicBool> {
        self.cuts
            .lock()
            .unwrap()
            .entry((coord.to_string(), host.to_string()))
            .or_default()
            .clone()
    }

    fn links(&self, coord: &str) -> LinkFactory {
        let links: HashMap<String, Arc<dyn NodeLink>> = self
            .hosts
            .iter()
            .map(|(name, jobs)| {
                (
                    name.clone(),
                    Arc::new(CutLink {
                        inner: Arc::new(LocalLink::new(jobs.clone(), name.clone(), "linux")),
                        cut: self.cut(coord, name),
                    }) as Arc<dyn NodeLink>,
                )
            })
            .collect();
        Arc::new(move |row: &NodeRow| links.get(&row.name).cloned())
    }

    /// The machines and the laptop register their pool keys; a makes the
    /// first ones and shares them with the others (y joins later).
    async fn keys_up(&self) {
        for n in ["a", "b", "c", "x"] {
            let _ = self.keys[n].sync().await;
        }
        for n in ["a", "b", "c", "x"] {
            self.keys[n].sync().await.expect("keys");
        }
    }

    /// `who` takes the lease and the coordinator over from the log, as the
    /// pool agent does.
    async fn take_over(&self, who: &str) -> Arc<Coordinator> {
        self.pool.expire();
        let epoch = self.pool.acquire(who).expect("vacant");
        let dir = self.root.path().join(who);
        let keys = self.keys[who].clone();
        keys.sync().await.expect("the new holder's keys");
        let log = SealedLog::new(self.pool.log(epoch), keys);
        journal::rebuild(&dir.join("cluster.db"), "sim", &log)
            .await
            .expect("rebuild");
        let c = Coordinator::start_with(config(&dir), Some(self.links(who)), Arc::new(log))
            .await
            .expect("start");
        for (name, _) in &self.hosts {
            let row = NodeRow {
                name: name.clone(),
                via: "local".into(),
                state: "ready".into(),
                os: "linux".into(),
                ..Default::default()
            };
            // Known from the log or new: (re)added with this link.
            let _ = c.add_node(row).await;
        }
        c
    }
}

fn jobs(run: &str, n: usize) -> Vec<NewJob> {
    (0..n)
        .map(|i| NewJob {
            key: format!("{run}/{i}"),
            spec: JobSpec {
                argv: vec!["work".into()],
                millicpus: 500,
                memory_mb: 64,
                timeout_secs: 60,
                max_output_bytes: 1024,
                ..Default::default()
            },
            max_attempts: 50,
        })
        .collect()
}

/// The invariants over the whole history of a run. `revoked`: the keys
/// the revoked device had when it left, if it did.
async fn check(
    sim: &Sim,
    n: usize,
    final_counts: &std::collections::BTreeMap<String, u64>,
    seed: u64,
    revoked: Option<Arc<Bundle>>,
) {
    let sealed = sim.pool.entries();
    let epochs = sim.pool.entry_epochs();
    // Single writer per epoch: the epochs of appended entries never go
    // back, and no entry of an epoch was appended after a later grant.
    let history = sim.pool.history();
    let mut granted = 0u64;
    let mut after_revoke: Vec<(u64, u64)> = Vec::new();
    let mut gone = false;
    for ev in &history {
        match ev {
            Event::Granted { epoch, .. } => granted = *epoch,
            Event::Appended { epoch, from, head } => {
                assert_eq!(
                    *epoch, granted,
                    "seed {seed}: epoch {epoch} appended after epoch {granted} was granted"
                );
                if gone {
                    after_revoke.push((*from, *head));
                }
            }
            Event::Revoked { .. } => gone = true,
            Event::Fenced { .. } | Event::Keyring { .. } | Event::Rekey { .. } => {}
        }
    }
    // Everything on the relay is sealed: nothing of what the jobs are or
    // produce is readable there.
    for e in &sealed {
        let env = Envelope::parse(&e.data)
            .unwrap_or_else(|| panic!("seed {seed}: entry {} is not sealed: {}", e.seq, e.data));
        // Exactly an envelope (no clear field beside it), and the job's
        // command is not in its bytes.
        let ct = {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD
                .decode(&env.c)
                .unwrap()
        };
        assert!(
            !ct.windows(4).any(|w| w == b"work"),
            "seed {seed}: entry {} shows its job",
            e.seq
        );
        // Nothing appended after the revocation opens with the keys the
        // revoked device had.
        if let Some(old) = &revoked
            && after_revoke
                .iter()
                .any(|(from, head)| e.seq > *from && e.seq <= *head)
        {
            assert!(
                env.g > old.generation(),
                "seed {seed}: entry {} was sealed under generation {} after a device with generation {} left",
                e.seq,
                env.g,
                old.generation()
            );
            assert!(old.open(b"", &env).is_err());
        }
    }
    let rekeyed = history
        .iter()
        .skip_while(|ev| !matches!(ev, Event::Revoked { .. }))
        .any(|ev| matches!(ev, Event::Keyring { generation, .. } if *generation >= 2));
    assert!(rekeyed, "seed {seed}: no new keys after the revocation");
    eprintln!(
        "seed {seed}: {} entries, {} appended after the revocation, keys at generation {}",
        sealed.len(),
        after_revoke.iter().map(|(f, h)| h - f).sum::<u64>(),
        sim.pool.keyring().map_or(0, |k| k.generation)
    );
    // The device that joined during the run reads the whole log.
    let y = sim.root.path().join("y-reads");
    std::fs::create_dir_all(&y).unwrap();
    let joined = SealedLog::new(sim.pool.log(0), sim.keys["y"].clone());
    let at = journal::rebuild(&y.join("cluster.db"), "sim", &joined)
        .await
        .unwrap_or_else(|e| panic!("seed {seed}: the device that joined cannot read the log: {e}"));
    assert_eq!(at, sealed.len() as u64, "seed {seed}");
    let entries = {
        use cua_spaces_cluster::coordinator::journal::LogReader as _;
        let mut all = Vec::new();
        let mut after = 0;
        while let Some((page, head)) = joined.read(after).await.unwrap() {
            if page.is_empty() {
                break;
            }
            after = page.last().unwrap().seq;
            all.extend(page);
            if after >= head {
                break;
            }
        }
        all
    };
    assert!(
        epochs.windows(2).all(|w| w[0] <= w[1]),
        "seed {seed}: epochs went back"
    );
    // At most one result per job, and every result real.
    let produced = sim.produced.lock().unwrap().clone();
    let mut ended: HashSet<String> = HashSet::new();
    for e in &entries {
        if !e.terminal {
            continue;
        }
        assert!(
            ended.insert(e.job.clone()),
            "seed {seed}: job {} ended twice",
            e.job
        );
        let op: store::Op = serde_json::from_value(e.data.clone()).unwrap();
        if let store::Op::Change {
            change:
                store::Change::Complete {
                    attempt,
                    state,
                    stdout,
                    ..
                },
        } = op
        {
            assert_eq!(state, "succeeded", "seed {seed}: {}", e.job);
            let out = String::from_utf8_lossy(&stdout).to_string();
            assert_eq!(
                produced.get(&(e.job.clone(), attempt)),
                Some(&out),
                "seed {seed}: the result of {} attempt {attempt} was not produced by it",
                e.job
            );
        }
    }
    // No lost jobs.
    assert_eq!(
        ended.len(),
        n,
        "seed {seed}: {} of {n} jobs ended",
        ended.len()
    );
    assert_eq!(
        final_counts.get("succeeded").copied().unwrap_or(0),
        n as u64,
        "seed {seed}: {final_counts:?}"
    );
}

async fn run_seed(seed: u64) {
    let mut rng = StdRng::seed_from_u64(seed);
    let sim = Sim::new(3);
    let (n1, n2) = (120, 40);
    let n = n1 + n2;
    // The relay refuses appends past its log limit until the holder
    // snapshots (here after a few entries, so it happens under faults).
    sim.pool.limit_log(LOG_LIMIT);
    let names = ["a", "b", "c"];
    let mut who = 0usize;
    sim.keys_up().await;
    let mut coord = sim.take_over(names[who]).await;
    coord.submit("r", 0, jobs("r", n1)).await.unwrap();
    // Every machine keeps the keys in step, as the agents do (here: the
    // holder and the machine that joins).
    let upkeep_stop = CancellationToken::new();
    let upkeep = {
        let (stop, keys) = (upkeep_stop.clone(), sim.keys.clone());
        tokio::spawn(async move {
            while !stop.is_cancelled() {
                for k in keys.values() {
                    let _ = k.sync().await;
                }
                // Rarely, as the agents do (every 10 s, against jobs of
                // minutes): it is the log's refusal (`rekey`) that makes a
                // holder stop using keys a revoked device has, at once.
                tokio::time::sleep(Duration::from_millis(2_000)).await;
            }
        })
    };
    let mut revoked: Option<Arc<Bundle>> = None;
    let steps = rng.random_range(4..9);
    // Mid-run, the laptop that holds the keys is revoked and a new machine
    // signs in.
    let (revoke_at, join_at) = (rng.random_range(0..steps), rng.random_range(0..steps));
    // Holders that lost the lease and still run (paused processes that
    // woke up), by machine: a machine's next takeover replaces its own.
    let mut old: Vec<(usize, Arc<Coordinator>)> = Vec::new();
    let retire = |old: &mut Vec<(usize, Arc<Coordinator>)>, who: usize| {
        old.retain(|(w, c)| {
            if *w == who {
                c.stop();
                false
            } else {
                true
            }
        });
    };
    for step in 0..steps {
        tokio::time::sleep(Duration::from_millis(rng.random_range(100..700))).await;
        if step == revoke_at {
            // The old keys stop at once; the holder makes new ones without
            // the laptop.
            let x = &sim.keys["x"];
            revoked = Some(x.cached().expect("x has the keys"));
            sim.pool.revoke(&x.device());
            // More work right after it left (under the new keys only).
            coord.submit("r2", 0, jobs("r2", n2)).await.unwrap();
        }
        if step == join_at {
            // It registers; a machine that has the keys shares them.
            let _ = sim.keys["y"].sync().await;
        }
        match rng.random_range(0..6) {
            // The holder pauses past its lease; another takes over; the old
            // one wakes up still believing it coordinates (fenced on its
            // next append).
            0 | 1 => {
                old.push((who, coord));
                who = (who + 1) % names.len();
                retire(&mut old, who);
                coord = sim.take_over(names[who]).await;
            }
            // The holder crashes; another takes over.
            2 => {
                coord.stop();
                who = (who + 1) % names.len();
                retire(&mut old, who);
                coord = sim.take_over(names[who]).await;
            }
            // A partition between the holder and a host for a while.
            3 => {
                let h = format!("h{}", rng.random_range(0..3));
                let cut = sim.cut(names[who], &h);
                cut.store(true, Ordering::SeqCst);
                let ms = rng.random_range(300..2500);
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(ms)).await;
                    cut.store(false, Ordering::SeqCst);
                });
            }
            // Appends lost on the way, or their answers lost.
            4 => sim.pool.drop_requests(rng.random_range(1..4)),
            _ => sim.pool.drop_answers(rng.random_range(1..4)),
        }
    }

    // Heal everything; the last holder finishes the run.
    for c in sim.cuts.lock().unwrap().values() {
        c.store(false, Ordering::SeqCst);
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    let counts = loop {
        let counts = coord.store().call(|c| store::counts(c)).await.unwrap();
        let ended: u64 = counts
            .iter()
            .filter(|(k, _)| store::state::is_terminal(k))
            .map(|(_, v)| *v)
            .sum();
        if ended == n as u64 {
            break counts;
        }
        if tokio::time::Instant::now() >= deadline {
            let stuck = coord
                .store()
                .call(|c| store::jobs(c, "", "", 0, 100_000, false))
                .await
                .unwrap();
            let pool_id = coord
                .store()
                .call(|c| store::meta_get(c, "cluster_id"))
                .await
                .unwrap()
                .unwrap_or_default();
            let mut lines = Vec::new();
            for j in stuck
                .iter()
                .filter(|j| !store::state::is_terminal(&j.state))
            {
                let on_hosts: Vec<String> = sim
                    .hosts
                    .iter()
                    .filter_map(|(h, jobs)| {
                        let r = jobs.get(&format!("{pool_id}-{}", j.id))?;
                        Some(format!("{h}: attempt {} state {:?}", r.attempt, r.state))
                    })
                    .collect();
                lines.push(format!(
                    "job {} {} node {} attempt {}; hosts {on_hosts:?}",
                    j.id, j.state, j.node, j.attempt
                ));
            }
            panic!(
                "seed {seed}: not done: {counts:?} (holder fenced: {}); history tail {:?}\n{}",
                coord.is_fenced(),
                sim.pool.history().iter().rev().take(12).collect::<Vec<_>>(),
                lines.join("\n")
            );
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    // Old holders that woke up must have been fenced or stayed idle.
    for (_, o) in &old {
        o.stop();
    }
    coord.stop();
    // y has the keys by now (the upkeep shared them).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while sim.keys["y"].cached().is_none() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "seed {seed}: the new machine never got the keys"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    upkeep_stop.cancel();
    let _ = upkeep.await;
    // The full log made the holders snapshot, each under its own epoch.
    let entries = sim.pool.entries().len() as u64;
    let snapshots = sim.pool.snapshots();
    assert!(
        entries <= LOG_LIMIT || !snapshots.is_empty(),
        "seed {seed}: {entries} entries but no snapshot"
    );
    eprintln!(
        "seed {seed}: {entries} entries, {} snapshots",
        snapshots.len()
    );
    check(&sim, n, &counts, seed, revoked).await;
}

/// Entries the pool log takes past its last snapshot in the simulation.
const LOG_LIMIT: u64 = 40;

fn seeds() -> std::ops::Range<u64> {
    std::env::var("CUA_POOL_SIM_SEEDS")
        .ok()
        .and_then(|v| {
            let (a, b) = v.split_once("..")?;
            Some(a.parse().ok()?..b.parse().ok()?)
        })
        .unwrap_or(1..7)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_pool_survives_takeovers_partitions_and_lost_appends() {
    for seed in seeds() {
        run_seed(seed).await;
        eprintln!("seed {seed}: ok");
    }
}

/// A holder that lost its lease cannot record anything: not a dispatch,
/// not a result. Its jobs are taken over and finished by the next one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stale_holder_is_fenced_and_its_jobs_finish_elsewhere() {
    let sim = Sim::new(2);
    sim.keys_up().await;
    let a = sim.take_over("a").await;
    a.submit("r", 0, jobs("r", 40)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let b = sim.take_over("b").await;
    // a's next write is refused and a stops.
    let mut late = jobs("r", 1);
    late[0].key = "r2/0".into();
    let r = a.submit("r2", 0, late).await;
    assert!(r.is_err(), "a stale holder recorded a submit");
    assert!(a.is_fenced());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let c = b.store().call(|c| store::counts(c)).await.unwrap();
        if c.get("succeeded").copied() == Some(40) {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "{c:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        sim.pool.history().iter().any(|e| matches!(
            e,
            Event::Fenced {
                epoch: 1,
                current: 2
            }
        )),
        "a was never refused"
    );
    b.stop();
}
