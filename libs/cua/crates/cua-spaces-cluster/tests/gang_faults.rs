// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Gang fault scenarios through the real coordinator and real host job
//! runners (in process), each named by its number in the jobs research
//! report's scenario list:
//!
//! - 5: lost machines use the infrastructure budget, not `retries`.
//! - 10: a host cut off from the coordinator while its member keeps
//!   running stops that member itself before the next attempt starts, so
//!   the stale attempt never overlaps (or checkpoints after) the new one.
//! - 11/12: a member that stops making progress (a network split between
//!   ranks, a SIGSTOP) ends its attempt as `stalled`, a code failure.
//! - 35: a start barrier that times out is an infrastructure failure, and
//!   no member starts its work before every member is ready.
//!
//! Plus the infrastructure budget running out, and a submit that reuses a
//! run id with another spec (a conflict).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use cua_cluster::JobSpec;
use cua_cluster::link::{LinkError, LocalLink, NodeLink};
use cua_cluster::node::{ExecEnd, ExecOutcome, JobExecutor, NodeConfig, NodeJobs, Offers};
use cua_proto::env::v1 as pb;
use cua_spaces_cluster::coordinator::requests::{self, Caller};
use cua_spaces_cluster::coordinator::store::NodeRow;
use cua_spaces_cluster::coordinator::{Config, CoordError, Coordinator, LinkFactory};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

/// One execution of a member.
#[derive(Clone, Debug)]
struct Ran {
    host: String,
    /// Its job's id on its host.
    job: String,
    run: String,
    attempt: u32,
    rank: u32,
    started: Instant,
    /// When it passed its start barrier and began its work.
    worked: Option<Instant>,
    ended: Option<Instant>,
    /// Checkpoints it "committed" (one per step of its work).
    commits: Vec<Instant>,
}

#[derive(Clone, Default)]
struct Ctl {
    attempt: u32,
    ready: bool,
    released: bool,
    last_progress_ms: i64,
    step: i64,
}

/// Runs `["work", "<ms>"]` like the cua jobs runtime would: it reports
/// ready (unless `UNREADY_RANK`/`UNREADY_ATTEMPTS` say otherwise), waits at
/// its start barrier when it has one, then works in 50 ms steps, beating
/// and checkpointing each step; `STALL_RANK`/`STALL_ATTEMPTS` stop the
/// beats after 3 steps while it keeps running (a SIGSTOP, a hung
/// collective). A member the test tells to fail ([`Pool::fail`]) exits 1
/// at its next step with the output it was given.
#[derive(Clone)]
struct Exec {
    host: String,
    ran: Arc<Mutex<Vec<Ran>>>,
    fail: Fails,
    kill: Arc<Mutex<CancellationToken>>,
    ctl: Arc<Mutex<HashMap<String, Ctl>>>,
}

fn picked(env: &dyn Fn(&str) -> String, who: &str, rank: u32, attempt: u32) -> bool {
    env(&format!("{who}_RANK")) == rank.to_string()
        && (env(&format!("{who}_ATTEMPTS")) == "*"
            || env(&format!("{who}_ATTEMPTS"))
                .split(',')
                .any(|a| a == attempt.to_string()))
}

/// What a GPU timeout looks like in an MLX rank's output (CUA-1826) ...
const METAL_TIMEOUT: &str = "    mx.eval(state, losses, n_tokens, grad_accum)
RuntimeError: [METAL] Command buffer execution failed: Caused GPU Timeout Error (00000002:kIOGPUCommandBufferCallbackErrorTimeout).
[ring] Socket 4 was closed by the peer
[ring] Too many send/recv errors. Failing pending tasks...
cua: mlx_lm.lora exited with status 1
";

/// ... and its peer's, whose ring broke because of it.
const RING_BROKE: &str = "[ring] Socket 4 was closed by the peer
[ring] Too many send/recv errors. Failing pending tasks...
    mx.eval(state, losses, n_tokens, grad_accum)
RuntimeError: [ring] connection to a peer was lost
cua: mlx_lm.lora exited with status 1
";

/// Members told to fail, by their job's id on their host: the output
/// each exits 1 with.
type Fails = Arc<Mutex<HashMap<String, String>>>;

#[async_trait]
impl JobExecutor for Exec {
    async fn run(&self, job: &JobSpec, _space: &str, cancel: CancellationToken) -> ExecOutcome {
        let env = |k: &str| job.env.get(k).cloned().unwrap_or_default();
        let rank: u32 = env("CUA_RANK").parse().unwrap_or(0);
        let attempt: u32 = env("CUA_ATTEMPT").parse().unwrap_or(0);
        let idx = {
            let mut r = self.ran.lock().unwrap();
            r.push(Ran {
                host: self.host.clone(),
                job: job.job_id.clone(),
                run: env("CUA_RUN_ID"),
                attempt,
                rank,
                started: Instant::now(),
                worked: None,
                ended: None,
                commits: vec![],
            });
            r.len() - 1
        };
        let barrier = job.env.contains_key("CUA_BARRIER_TIMEOUT");
        let unready = picked(&env, "UNREADY", rank, attempt);
        let stall = picked(&env, "STALL", rank, attempt);
        let ms: u64 = job.argv.get(1).and_then(|a| a.parse().ok()).unwrap_or(300);
        let id = job.job_id.clone();
        self.ctl.lock().unwrap().insert(
            id.clone(),
            Ctl {
                attempt: job.attempt,
                ready: !unready,
                released: !barrier,
                last_progress_ms: 0,
                step: -1,
            },
        );
        let (ctl, ran, fail) = (self.ctl.clone(), self.ran.clone(), self.fail.clone());
        let work = async move {
            while !ctl.lock().unwrap().get(&id).is_some_and(|c| c.released) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            ran.lock().unwrap()[idx].worked = Some(Instant::now());
            let t0 = Instant::now();
            let mut step = 0i64;
            while t0.elapsed() < Duration::from_millis(ms) {
                if let Some(out) = fail.lock().unwrap().get(&id) {
                    return Some(out.clone());
                }
                step += 1;
                if !(stall && step > 3) {
                    if let Some(c) = ctl.lock().unwrap().get_mut(&id) {
                        c.step = step;
                        c.last_progress_ms = cua_cluster::model::now_ms();
                    }
                    ran.lock().unwrap()[idx].commits.push(Instant::now());
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            None
        };
        let kill = self.kill.lock().unwrap().clone();
        let mut output = String::new();
        let end = tokio::select! {
            _ = cancel.cancelled() => ExecEnd::Canceled,
            _ = kill.cancelled() => ExecEnd::Error("the machine went off".into()),
            failed = work => match failed {
                Some(out) => {
                    output = out;
                    ExecEnd::Exited(1)
                }
                None => ExecEnd::Exited(0),
            },
        };
        self.ctl.lock().unwrap().remove(&job.job_id);
        self.ran.lock().unwrap()[idx].ended = Some(Instant::now());
        let mut o = ExecOutcome::ended(end);
        o.stdout = output.into_bytes();
        o.result = format!("rank {rank} attempt {attempt}").into_bytes();
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

/// A host link every call of which fails while `down` (off or cut off).
struct HostLink {
    inner: LocalLink,
    down: Arc<AtomicBool>,
}

impl HostLink {
    fn check(&self) -> Result<(), LinkError> {
        if self.down.load(Ordering::SeqCst) {
            Err(LinkError::Unreachable("unreachable".into()))
        } else {
            Ok(())
        }
    }
}

#[async_trait]
impl NodeLink for HostLink {
    async fn run(&self, req: pb::RunHostJobsRequest) -> Result<pb::RunHostJobsResponse, LinkError> {
        self.check()?;
        self.inner.run(req).await
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

/// A host's self-fence time here.
const SELF_FENCE: Duration = Duration::from_secs(1);

/// Scenario 10's: longer than the coordinator takes to find the host lost
/// and requeue (1.2 s + 1 s), so only waiting for the fence keeps the
/// attempts apart.
const LONG_FENCE: Duration = Duration::from_secs(4);

struct Pool {
    _root: tempfile::TempDir,
    coord: Arc<Coordinator>,
    ran: Arc<Mutex<Vec<Ran>>>,
    fail: Fails,
    /// Each host's job runner, read past any cut.
    hosts: HashMap<String, Arc<NodeJobs>>,
    down: HashMap<String, Arc<AtomicBool>>,
    kills: HashMap<String, Arc<Mutex<CancellationToken>>>,
}

impl Pool {
    async fn new(hosts: usize) -> Self {
        Self::with_fence(hosts, SELF_FENCE).await
    }

    async fn with_fence(hosts: usize, fence: Duration) -> Self {
        Self::build(hosts, fence, |_| {}).await
    }

    async fn build(hosts: usize, fence: Duration, tune: fn(&mut Config)) -> Self {
        let root = tempfile::tempdir().unwrap();
        let ran: Arc<Mutex<Vec<Ran>>> = Arc::default();
        let fail: Fails = Arc::default();
        let mut runners = HashMap::new();
        let mut links: HashMap<String, Arc<dyn NodeLink>> = HashMap::new();
        let mut down = HashMap::new();
        let mut kills = HashMap::new();
        for i in 0..hosts {
            let name = format!("h{i}");
            let kill = Arc::new(Mutex::new(CancellationToken::new()));
            let jobs = NodeJobs::open(
                NodeConfig {
                    dir: root.path().join(&name),
                    millicpus_total: 4000,
                    memory_total_mb: 8192,
                    jobs_max: 0,
                    min_available_mb: 0,
                    retention: Duration::from_secs(3600),
                    output_retention: Duration::from_secs(3600),
                    boot_id: name.clone(),
                },
                Arc::new(Exec {
                    host: name.clone(),
                    ran: ran.clone(),
                    fail: fail.clone(),
                    kill: kill.clone(),
                    ctl: Arc::default(),
                }),
            )
            .unwrap();
            jobs.set_self_fence_after(fence);
            runners.insert(name.clone(), jobs.clone());
            let d = Arc::new(AtomicBool::new(false));
            links.insert(
                name.clone(),
                Arc::new(HostLink {
                    inner: LocalLink::new(jobs, name.clone(), "linux"),
                    down: d.clone(),
                }),
            );
            down.insert(name.clone(), d);
            kills.insert(name, kill);
        }
        let factory: LinkFactory = Arc::new(move |row: &NodeRow| links.get(&row.name).cloned());
        let mut cfg = Config::new(root.path().join("coord"));
        cfg.poll_wait = Duration::from_millis(200);
        cfg.node_down_after = Duration::from_millis(1200);
        cfg.ack_grace = Duration::from_millis(1500);
        cfg.run_grace = Duration::from_secs(5);
        tune(&mut cfg);
        let coord = Coordinator::start(cfg, Some(factory)).await.unwrap();
        for i in 0..hosts {
            coord
                .add_node(NodeRow {
                    name: format!("h{i}"),
                    via: "local".into(),
                    state: "ready".into(),
                    os: "linux".into(),
                    ..Default::default()
                })
                .await
                .unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while coord.nodes().iter().any(|n| n.health != "ready") {
            assert!(Instant::now() < deadline, "hosts never came up");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Self {
            _root: root,
            coord,
            ran,
            fail,
            hosts: runners,
            down,
            kills,
        }
    }

    async fn ask(&self, method: &str, params: Value) -> Value {
        requests::handle(&self.coord, method, params, &Caller::local())
            .await
            .unwrap_or_else(|e| panic!("{method}: {e}"))
    }

    /// The machine goes off: what ran there stops, and it answers nothing.
    fn power_off(&self, host: &str) {
        self.down[host].store(true, Ordering::SeqCst);
        let mut k = self.kills[host].lock().unwrap();
        k.cancel();
        *k = CancellationToken::new();
    }

    /// The machine is cut off from the coordinator: what runs there keeps
    /// running.
    fn cut(&self, host: &str, cut: bool) {
        self.down[host].store(cut, Ordering::SeqCst);
    }

    async fn wait_state(&self, run: &str, want: &[&str], secs: u64) -> Value {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            let v = self.ask("run", json!({ "run": run })).await;
            if want.contains(&v["state"].as_str().unwrap_or("")) {
                return v;
            }
            assert!(
                Instant::now() < deadline,
                "run {run} never got to {want:?}: {v:#}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// `r`'s command exits 1 at its next step, printing `out`.
    fn fail(&self, r: &Ran, out: &str) {
        self.fail.lock().unwrap().insert(r.job.clone(), out.into());
    }

    /// When `r`'s command ended by its host's clock, once its host has
    /// recorded that it did (as the coordinator reads it: the record's
    /// time less the Space's delete).
    async fn ended_ms(&self, r: &Ran) -> i64 {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(rec) = self.hosts[&r.host].get(&r.job)
                && rec.finished_at_ms > 0
            {
                return rec.finished_at_ms - i64::from(rec.delete_ms);
            }
            assert!(Instant::now() < deadline, "{r:?} never ended");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    fn ran(&self, run: &str) -> Vec<Ran> {
        self.ran
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.run == run)
            .cloned()
            .collect()
    }

    /// Waits until `n` members of `attempt` of `run` do their work.
    async fn working(&self, run: &str, attempt: u32, n: usize) -> Vec<Ran> {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let w: Vec<Ran> = self
                .ran(run)
                .into_iter()
                .filter(|r| r.attempt == attempt && r.worked.is_some() && r.ended.is_none())
                .collect();
            if w.len() >= n {
                return w;
            }
            assert!(
                Instant::now() < deadline,
                "{run} attempt {attempt}: {n} members never worked: {:?}",
                self.ran(run)
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

fn gang(run: &str, nodes: &str, ms: u64, extra: Value) -> Value {
    let mut v = json!({
        "run": run, "gang": true, "nodes": nodes, "kind": "machine",
        "command": ["work", ms.to_string()], "cpus": 1, "memory_mb": 256,
        "timeout_secs": 120, "result_path": "out.bin",
    });
    if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
        for (k, x) in e {
            o.insert(k.clone(), x.clone());
        }
    }
    v
}

fn member(v: &Value, attempt: u32, rank: u32) -> &Value {
    v["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["attempt"] == attempt && m["rank"] == rank)
        .unwrap_or_else(|| panic!("no member {attempt}/{rank}: {v:#}"))
}

/// Scenario 5: a gang with `retries: 0` survives three lost machines (they
/// use the infrastructure budget); with `infra_retries: 1`, two lost
/// machines end it `failed`, saying so.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scenario_5_lost_machines_do_not_use_user_retries() {
    let p = Pool::new(5).await;
    p.ask("submit", gang("s5", "1:1", 2500, json!({"retries": 0})))
        .await;
    for attempt in 1..=3 {
        let w = p.working("s5", attempt, 1).await;
        p.power_off(&w[0].host);
    }
    let v = p.wait_state("s5", &["succeeded"], 90).await;
    let g = &v["gang"];
    assert_eq!(g["attempt"], 4, "{v:#}");
    assert_eq!(
        (g["failures"].as_u64(), g["infra_failures"].as_u64()),
        (Some(0), Some(3))
    );
    assert_eq!(g["summary"], "attempt 4 · 3 lost computers");
    assert!(
        member(&v, 1, 0)["error"]
            .as_str()
            .unwrap()
            .contains("went down"),
        "{v:#}"
    );

    // A small budget runs out.
    p.ask(
        "submit",
        gang(
            "s5b",
            "1:1",
            2500,
            json!({"retries": 0, "infra_retries": 1}),
        ),
    )
    .await;
    for attempt in 1..=2 {
        let w = p.working("s5b", attempt, 1).await;
        p.power_off(&w[0].host);
    }
    let v = p.wait_state("s5b", &["failed"], 60).await;
    let err = v["gang"]["error"].as_str().unwrap();
    assert!(
        err.contains("went down") && err.contains("lost computers"),
        "{err}"
    );
    assert_eq!(v["gang"]["failures"], 0);
}

/// Scenario 10: the host of rank 1 is cut off from the coordinator while
/// its member keeps running. The host stops that member itself, and the
/// next attempt starts only after: the two attempts never overlap and the
/// stale one commits no checkpoint once the new one runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scenario_10_a_cut_off_host_fences_its_member_before_the_next_attempt() {
    let p = Pool::with_fence(3, LONG_FENCE).await;
    p.ask(
        "submit",
        gang(
            "s10",
            "2:2",
            6000,
            json!({"retries": 0, "barrier_timeout_secs": 30}),
        ),
    )
    .await;
    let w = p.working("s10", 1, 2).await;
    let victim = w.iter().find(|r| r.rank == 1).unwrap().host.clone();
    let cut_at = Instant::now();
    p.cut(&victim, true);
    // While it waits, the run says why.
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let v = p.ask("run", json!({"run": "s10"})).await;
        let w = v["gang"]["waiting"].as_str().unwrap_or("").to_string();
        if w.contains("must have stopped its member") {
            assert!(w.contains(&format!("{victim} (self-fence 5 s)")), "{w}");
            break;
        }
        assert!(Instant::now() < deadline, "no fence wait shown: {v:#}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let v = p.wait_state("s10", &["succeeded"], 60).await;
    assert_eq!(v["gang"]["attempt"], 2, "{v:#}");
    assert_eq!(v["gang"]["failures"], 0);
    assert_eq!(v["gang"]["infra_failures"], 1);
    let ran = p.ran("s10");
    let stale = ran
        .iter()
        .find(|r| r.attempt == 1 && r.host == victim)
        .unwrap();
    let stale_end = stale.ended.expect("the stale member ended");
    // Its host stopped it within its self-fence time (plus a check), not
    // at the end of its 6 s of work.
    assert!(
        stale_end - cut_at < LONG_FENCE + Duration::from_millis(1600),
        "{:?}",
        stale_end - cut_at
    );
    let next = ran
        .iter()
        .filter(|r| r.attempt == 2)
        .map(|r| r.started)
        .min()
        .unwrap();
    assert!(
        stale_end < next,
        "the stale member outlived the next attempt's start"
    );
    for r in ran.iter().filter(|r| r.attempt == 1) {
        assert!(
            r.commits.iter().all(|c| *c < next),
            "attempt 1 rank {} committed after attempt 2 started",
            r.rank
        );
    }
    // The coordinator saw it as a lost machine.
    let m = member(&v, 1, 1);
    assert_eq!(m["host"], victim.as_str());
    assert!(m["error"].as_str().unwrap().contains("went down"), "{m}");
    p.cut(&victim, false);
}

/// Scenarios 11 and 12: rank 1 stops making progress (it beats three steps,
/// then hangs as a SIGSTOP or a split between ranks would leave it). The
/// watchdog ends the attempt as stalled (a code failure); the next attempt
/// succeeds.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scenario_11_12_a_member_without_progress_stalls_its_attempt() {
    let p = Pool::new(2).await;
    p.ask(
        "submit",
        gang(
            "s11",
            "2:2",
            5000,
            json!({
                "retries": {"max_retries": 1, "initial_delay": 0.2, "max_delay": 0.2},
                "stall_timeout_secs": 1,
                "env": {"STALL_RANK": "1", "STALL_ATTEMPTS": "1"},
            }),
        ),
    )
    .await;
    let v = p.wait_state("s11", &["succeeded"], 60).await;
    assert_eq!(v["gang"]["attempt"], 2, "{v:#}");
    assert_eq!(v["gang"]["failures"], 1);
    assert_eq!(v["gang"]["infra_failures"], 0);
    let stalled = member(&v, 1, 1);
    assert_eq!(stalled["state"], "canceled");
    let err = stalled["error"].as_str().unwrap();
    assert!(
        err.starts_with("stalled: rank 1 on ") && err.contains("last step 3"),
        "{err}"
    );
    let other = member(&v, 1, 0)["error"].as_str().unwrap().to_string();
    assert!(
        other.starts_with("stopped with its gang: stalled: "),
        "{other}"
    );
    // Stopped about a second after its last beat, not after its 5 s of work.
    let a1 = p
        .ran("s11")
        .into_iter()
        .find(|r| r.attempt == 1 && r.rank == 1)
        .unwrap();
    let ran_for = a1.ended.unwrap() - a1.worked.unwrap();
    assert!(ran_for < Duration::from_millis(4000), "{ran_for:?}");
}

/// Scenario 35: rank 1 never gets ready. The barrier times out and ends the
/// attempt as an infrastructure failure (`retries: 0` still succeeds), and
/// rank 0 never started its work while it waited.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scenario_35_a_barrier_timeout_is_not_a_user_failure() {
    let p = Pool::new(2).await;
    p.ask(
        "submit",
        gang(
            "s35",
            "2:2",
            500,
            json!({
                "retries": 0,
                "barrier_timeout_secs": 2,
                "env": {"UNREADY_RANK": "1", "UNREADY_ATTEMPTS": "1"},
            }),
        ),
    )
    .await;
    let v = p.wait_state("s35", &["succeeded"], 60).await;
    assert_eq!(v["gang"]["attempt"], 2, "{v:#}");
    assert_eq!(v["gang"]["failures"], 0);
    assert_eq!(v["gang"]["infra_failures"], 1);
    for rank in 0..2 {
        let m = member(&v, 1, rank);
        assert_eq!(m["state"], "canceled");
        let err = m["error"].as_str().unwrap();
        assert!(
            err.starts_with("start barrier timed out: after 2 s, rank 1 on "),
            "{err}"
        );
    }
    let ran = p.ran("s35");
    // Held at the barrier: attempt 1 never worked.
    assert!(
        ran.iter()
            .filter(|r| r.attempt == 1)
            .all(|r| r.worked.is_none()),
        "{ran:?}"
    );
    // Attempt 2: nobody worked before everybody had started.
    let a2: Vec<&Ran> = ran.iter().filter(|r| r.attempt == 2).collect();
    assert_eq!(a2.len(), 2);
    let last_start = a2.iter().map(|r| r.started).max().unwrap();
    assert!(a2.iter().all(|r| r.worked.unwrap() >= last_start), "{a2:?}");
}

/// A run id reused with another spec is a conflict naming what differs;
/// the same spec attaches to the run.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_submit_with_a_reused_run_id_and_another_spec_is_a_conflict() {
    let p = Pool::new(2).await;
    let first = gang("g9", "1:2", 300, json!({"retries": 2}));
    p.ask("submit", first.clone()).await;
    let again = p.ask("submit", first).await;
    assert_eq!(again["existing"], 1);
    let other = gang(
        "g9",
        "1:1",
        300,
        json!({"retries": 2, "env": {"LR": "1e-3"}}),
    );
    let err = requests::handle(&p.coord, "submit", other.clone(), &Caller::local())
        .await
        .unwrap_err();
    assert!(matches!(err, CoordError::Conflict(_)), "{err}");
    let msg = err.to_string();
    assert!(
        msg.contains("max") && msg.contains("spec.env") && msg.contains("run g9 exists"),
        "{msg}"
    );
    assert_eq!(requests::status_of(&err), 409);
    let (status, body) = requests::answer(&p.coord, "submit", other, &Caller::local()).await;
    assert_eq!(status, 409, "{body}");
    p.wait_state("g9", &["succeeded"], 30).await;
}

/// CUA-1826: rank 1's GPU times out and rank 0's ring breaks because of
/// it 200 ms later, but the coordinator hears of rank 0 first (rank 1's
/// host is out of reach for a moment, as a host over a slow link is). The
/// attempt blames rank 1, the member that failed first by its host's
/// clock; and the other way round, rank 0.
///
/// Each step waits for the event before it rather than for a time, so a
/// slow machine cannot reorder them: the peer fails only once the first
/// one's host has recorded its end, and the cut host is reached again
/// only once the coordinator has the peer's failure. Being out of reach
/// for a moment is not being lost, so the coordinator's down detection
/// and the hosts' self-fence are kept out of the way.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_member_that_failed_first_is_blamed_not_the_one_heard_of_first() {
    let p = Pool::build(2, Duration::from_secs(600), |c| {
        c.node_down_after = Duration::from_secs(600);
    })
    .await;
    for (run, first) in [("f1826a", 1u32), ("f1826b", 0u32)] {
        p.ask("submit", gang(run, "2:2", 60_000, json!({"retries": 0})))
            .await;
        let w = p.working(run, 1, 2).await;
        let a = w.iter().find(|r| r.rank == first).unwrap();
        let b = w.iter().find(|r| r.rank != first).unwrap();
        let host = a.host.clone();
        // The coordinator does not hear from the first one's host until
        // after the other has failed too.
        p.cut(&host, true);
        p.fail(a, METAL_TIMEOUT);
        let a_ended = p.ended_ms(a).await;
        while cua_cluster::model::now_ms() <= a_ended + 200 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        p.fail(b, RING_BROKE);
        let b_ended = p.ended_ms(b).await;
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let v = p.ask("run", json!({ "run": run })).await;
            if member(&v, 1, 1 - first)["state"] == "failed" {
                assert_ne!(member(&v, 1, first)["state"], "failed", "{v:#}");
                break;
            }
            assert!(
                Instant::now() < deadline,
                "{run}: never heard of rank {}: {v:#}",
                1 - first
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        p.cut(&host, false);
        let v = p.wait_state(run, &["failed"], 60).await;
        let err = v["gang"]["error"].as_str().unwrap();
        assert!(
            err.starts_with(&format!(
                "rank {first} on {host} failed: exited with code 1"
            )),
            "{run}: {err}\n{v:#}"
        );
        assert!(err.contains("1 code failures, 0 retries allowed"), "{err}");
        let (ma, mb) = (member(&v, 1, first), member(&v, 1, 1 - first));
        assert_eq!(ma["peer_lost"], false, "{v:#}");
        assert_eq!(mb["peer_lost"], true, "{v:#}");
        // Their ends as their hosts recorded them, not as heard of.
        assert_eq!(ma["ended_ms"], a_ended, "{v:#}");
        assert_eq!(mb["ended_ms"], b_ended, "{v:#}");
        assert!(
            b_ended - a_ended >= 200,
            "{run}: ended {} ms apart",
            b_ended - a_ended
        );
    }
}
