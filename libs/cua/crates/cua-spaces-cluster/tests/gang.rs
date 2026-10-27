// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Gang runs (`nodes=(min, max)`) and retries on failure through the real
//! coordinator and real host job runners (in process), driven by the pool
//! requests clients send (`submit`, `run`, `run_result`, `cancel_run`):
//! placement on distinct hosts with one attempt and one environment,
//! stopping the other members when one fails, running again (smaller, on
//! the hosts that ran it before) after a backoff, giving up after the
//! retries, waiting for enough hosts, and canceling.

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
use cua_spaces_cluster::coordinator::{Config, Coordinator, LinkFactory};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

/// One execution of a member on a host.
#[derive(Clone, Debug)]
struct Ran {
    host: String,
    run: String,
    attempt: u32,
    rank: u32,
    world: u32,
    peers: String,
    started: Instant,
}

/// Runs `["work", "<ms>"]`; a member fails (exit 1) when its rank is
/// `FAIL_RANK` and its gang attempt is in `FAIL_ATTEMPTS` (`1,2`, or `*`).
#[derive(Clone)]
struct Exec {
    host: String,
    ran: Arc<Mutex<Vec<Ran>>>,
    /// Killing the host stops whatever runs on it.
    kill: Arc<Mutex<CancellationToken>>,
    dedicated: bool,
}

#[async_trait]
impl JobExecutor for Exec {
    async fn run(&self, job: &JobSpec, _space: &str, cancel: CancellationToken) -> ExecOutcome {
        let env = |k: &str| job.env.get(k).cloned().unwrap_or_default();
        let rank: u32 = env("CUA_RANK").parse().unwrap_or(0);
        let attempt: u32 = env("CUA_ATTEMPT").parse().unwrap_or(job.attempt);
        self.ran.lock().unwrap().push(Ran {
            host: self.host.clone(),
            run: env("CUA_RUN_ID"),
            attempt,
            rank,
            world: env("CUA_WORLD_SIZE").parse().unwrap_or(1),
            peers: env("CUA_PEERS"),
            started: Instant::now(),
        });
        let ms: u64 = job.argv.get(1).and_then(|a| a.parse().ok()).unwrap_or(50);
        let kill = self.kill.lock().unwrap().clone();
        let fail = env("FAIL_RANK") == rank.to_string()
            && (env("FAIL_ATTEMPTS") == "*"
                || env("FAIL_ATTEMPTS")
                    .split(',')
                    .any(|a| a == attempt.to_string()));
        let sleep = if fail { ms / 4 } else { ms };
        tokio::select! {
            _ = cancel.cancelled() => return ExecOutcome::ended(ExecEnd::Canceled),
            _ = kill.cancelled() => return ExecOutcome::ended(ExecEnd::Error("killed".into())),
            _ = tokio::time::sleep(Duration::from_millis(sleep)) => {}
        }
        let mut o = ExecOutcome::ended(ExecEnd::Exited(if fail { 1 } else { 0 }));
        o.stdout = format!("rank {rank} on {}\n", self.host).into_bytes();
        if !job.result_path.is_empty() {
            o.result = format!("result of rank {rank} attempt {attempt}").into_bytes();
        }
        o.run_ms = sleep as u32;
        o
    }

    async fn remove_space(&self, _space: &str) {}

    fn offers(&self) -> Offers {
        let mut kinds = vec!["container".to_string()];
        if self.dedicated {
            kinds.push("machine".into());
        }
        Offers {
            kinds,
            runtimes: vec!["runc".into()],
        }
    }
}

/// A host the coordinator loses while `down` (its machine is off).
struct HostLink {
    inner: LocalLink,
    down: Arc<AtomicBool>,
}

impl HostLink {
    fn check(&self) -> Result<(), LinkError> {
        if self.down.load(Ordering::SeqCst) {
            Err(LinkError::Unreachable("the machine is off".into()))
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

struct Pool {
    _root: tempfile::TempDir,
    coord: Arc<Coordinator>,
    ran: Arc<Mutex<Vec<Ran>>>,
    down: HashMap<String, Arc<AtomicBool>>,
    kills: HashMap<String, Arc<Mutex<CancellationToken>>>,
}

impl Pool {
    /// `hosts` hosts; the first `dedicated` of them are dedicated (they offer
    /// their own Space, `kind = "machine"`).
    async fn new(hosts: usize, dedicated: usize) -> Self {
        let root = tempfile::tempdir().unwrap();
        let ran: Arc<Mutex<Vec<Ran>>> = Arc::default();
        let mut links: HashMap<String, Arc<dyn NodeLink>> = HashMap::new();
        let mut down = HashMap::new();
        let mut kills = HashMap::new();
        for i in 0..hosts {
            let name = format!("h{i}");
            let kill = Arc::new(Mutex::new(CancellationToken::new()));
            let exec = Exec {
                host: name.clone(),
                ran: ran.clone(),
                kill: kill.clone(),
                dedicated: i < dedicated,
            };
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
                Arc::new(exec),
            )
            .unwrap();
            // A host cut off stops its gang members within 2 s (the
            // coordinator waits that long before placing them again).
            jobs.set_self_fence_after(Duration::from_secs(1));
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
        // Every host heard from before work arrives.
        let deadline = Instant::now() + Duration::from_secs(10);
        while coord.nodes().iter().any(|n| n.health != "ready") {
            assert!(Instant::now() < deadline, "hosts never came up");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Self {
            _root: root,
            coord,
            ran,
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
    fn kill(&self, host: &str) {
        self.down[host].store(true, Ordering::SeqCst);
        let mut k = self.kills[host].lock().unwrap();
        k.cancel();
        *k = CancellationToken::new();
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

    fn ran(&self, run: &str) -> Vec<Ran> {
        self.ran
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.run == run)
            .cloned()
            .collect()
    }
}

fn gang(run: &str, nodes: &str, ms: u64, extra: Value) -> Value {
    let mut v = json!({
        "run": run,
        "gang": true,
        "nodes": nodes,
        "command": ["work", ms.to_string()],
        "cpus": 1,
        "memory_mb": 256,
        "timeout_secs": 60,
        "result_path": "out.bin",
    });
    if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
        for (k, x) in e {
            o.insert(k.clone(), x.clone());
        }
    }
    v
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_gang_runs_once_per_host_with_one_attempt_and_returns_rank_0s_result() {
    let p = Pool::new(3, 0).await;
    let r = p.ask("submit", gang("g1", "2:3", 300, json!({}))).await;
    assert_eq!(r["gang"], true);
    assert_eq!(r["nodes"], json!({"min": 2, "max": 3}));
    // A second submit of the run is the same run.
    let again = p.ask("submit", gang("g1", "2:3", 300, json!({}))).await;
    assert_eq!(again["existing"], 1);
    let v = p.wait_state("g1", &["succeeded"], 30).await;
    assert_eq!(v["gang"]["attempt"], 1);
    assert_eq!(v["gang"]["size"], 3);
    let ran = p.ran("g1");
    assert_eq!(ran.len(), 3, "{ran:?}");
    let mut hosts: Vec<&str> = ran.iter().map(|r| r.host.as_str()).collect();
    hosts.sort();
    hosts.dedup();
    assert_eq!(hosts.len(), 3, "members share a host: {ran:?}");
    let mut ranks: Vec<u32> = ran.iter().map(|r| r.rank).collect();
    ranks.sort();
    assert_eq!(ranks, [0, 1, 2]);
    assert!(ran.iter().all(|r| r.attempt == 1 && r.world == 3));
    // One rank table: three peers on one port.
    assert!(ran.windows(2).all(|w| w[0].peers == w[1].peers));
    let peers: Vec<&str> = ran[0].peers.split(',').collect();
    assert_eq!(peers.len(), 3);
    let port: Vec<&str> = peers
        .iter()
        .map(|p| p.rsplit(':').next().unwrap())
        .collect();
    assert!(port.windows(2).all(|w| w[0] == w[1]));
    // The members as `run` shows them, with their hosts and a cost.
    let members = v["members"].as_array().unwrap();
    assert_eq!(members.len(), 3);
    assert!(
        members
            .iter()
            .all(|m| m["state"] == "succeeded" && m["attempt"] == 1)
    );
    assert!(v["cost_usd"].as_f64().is_some() && v["machine_seconds"].as_f64().unwrap() > 0.0);
    // The run's result is rank 0's.
    let res = p.ask("run_result", json!({"run": "g1"})).await;
    assert_eq!(res["state"], "succeeded");
    assert_eq!(res["done"], true);
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(res["result_b64"].as_str().unwrap())
        .unwrap();
    assert_eq!(bytes, b"result of rank 0 attempt 1");
    // `runs` lists it with its gang.
    let runs = p.ask("runs", json!({})).await;
    let listed = runs["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["run"] == "g1")
        .unwrap()
        .clone();
    assert_eq!(listed["state"], "succeeded");
    assert_eq!(listed["gang"]["size"], 3);
}

/// One member fails: the others of its attempt are stopped at once, and
/// the gang runs again as attempt 2 on the same hosts (they hold its
/// checkpoints), after its backoff.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_member_stops_the_others_and_the_gang_runs_again_on_the_same_hosts() {
    let p = Pool::new(4, 0).await;
    p.ask(
        "submit",
        gang(
            "g2",
            "2:2",
            4000,
            json!({
                "env": {"FAIL_RANK": "1", "FAIL_ATTEMPTS": "1"},
                "retries": {"max_retries": 3, "initial_delay": 2, "backoff_coefficient": 2, "max_delay": 60},
            }),
        ),
    )
    .await;
    let v = p.wait_state("g2", &["succeeded"], 60).await;
    assert_eq!(v["gang"]["attempt"], 2, "{v:#}");
    assert_eq!(v["gang"]["failures"], 1);
    let ran = p.ran("g2");
    let a1: Vec<&Ran> = ran.iter().filter(|r| r.attempt == 1).collect();
    let a2: Vec<&Ran> = ran.iter().filter(|r| r.attempt == 2).collect();
    assert_eq!((a1.len(), a2.len()), (2, 2), "{ran:?}");
    let mut h1: Vec<&str> = a1.iter().map(|r| r.host.as_str()).collect();
    let mut h2: Vec<&str> = a2.iter().map(|r| r.host.as_str()).collect();
    h1.sort();
    h2.sort();
    assert_eq!(h1, h2, "attempt 2 went elsewhere than attempt 1");
    // Rank 0 of attempt 1 (a 4 s job) was stopped when rank 1 failed
    // (after 1 s), not left to finish: attempt 2 started within its
    // backoff (at most 2 s, jittered) of the failure.
    let first = a1.iter().map(|r| r.started).min().unwrap();
    let second = a2.iter().map(|r| r.started).min().unwrap();
    let gap = second - first;
    assert!(
        gap >= Duration::from_millis(900) && gap < Duration::from_millis(3600),
        "{gap:?}"
    );
    let members = v["members"].as_array().unwrap();
    let a1_states: Vec<&str> = members
        .iter()
        .filter(|m| m["attempt"] == 1)
        .map(|m| m["state"].as_str().unwrap())
        .collect();
    assert!(
        a1_states.contains(&"failed") && a1_states.contains(&"canceled"),
        "{a1_states:?}"
    );
}

/// A host is lost mid-run: the gang (nodes 1:2) runs again at the size that
/// fits, on the host that is left, and its result comes back.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lost_host_requeues_the_gang_smaller_on_the_survivor() {
    let p = Pool::new(2, 0).await;
    p.ask("submit", gang("g3", "1:2", 3000, json!({}))).await;
    let deadline = Instant::now() + Duration::from_secs(10);
    while p.ran("g3").len() < 2 {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let victim = p
        .ran("g3")
        .iter()
        .find(|r| r.rank == 1)
        .unwrap()
        .host
        .clone();
    p.kill(&victim);
    let v = p.wait_state("g3", &["succeeded"], 60).await;
    assert_eq!(v["gang"]["attempt"], 2, "{v:#}");
    assert_eq!(v["gang"]["size"], 1);
    let a2: Vec<Ran> = p.ran("g3").into_iter().filter(|r| r.attempt == 2).collect();
    assert_eq!(a2.len(), 1);
    assert_ne!(a2[0].host, victim);
    assert_eq!((a2[0].rank, a2[0].world), (0, 1));
    let res = p.ask("run_result", json!({"run": "g3"})).await;
    assert_eq!(res["state"], "succeeded");
    // The lost member is recorded as such.
    let lost = v["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["attempt"] == 1 && m["host"] == victim.as_str())
        .unwrap()
        .clone();
    assert_eq!(lost["state"], "error");
    assert!(
        lost["error"].as_str().unwrap().contains("went down"),
        "{lost}"
    );
}

/// A gang that fails more often than it may retry ends `failed`, saying
/// why.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_gang_that_keeps_failing_ends_failed_after_its_retries() {
    let p = Pool::new(2, 0).await;
    p.ask(
        "submit",
        gang(
            "g4",
            "2:2",
            400,
            json!({"retries": 1, "env": {"FAIL_RANK": "0", "FAIL_ATTEMPTS": "*"}}),
        ),
    )
    .await;
    let v = p.wait_state("g4", &["failed"], 60).await;
    assert_eq!(v["gang"]["attempt"], 2, "{v:#}");
    let err = v["gang"]["error"].as_str().unwrap();
    assert!(err.contains("rank 0") && err.contains("failed"), "{err}");
    let res = p.ask("run_result", json!({"run": "g4"})).await;
    assert_eq!(
        (res["state"].as_str(), res["done"].as_bool()),
        (Some("failed"), Some(true))
    );
    assert!(res.get("result_b64").is_none());
}

/// Too few hosts: the gang waits and says why; canceled, it ends at once.
/// A running gang canceled stops its members and ends `canceled`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_gang_waits_for_enough_hosts_and_cancels() {
    let p = Pool::new(2, 1).await;
    // A machine Space: only h0 (dedicated) offers one, and this gang needs two.
    p.ask("submit", gang("g5", "2:2", 300, json!({"kind": "machine"})))
        .await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let v = p.ask("run", json!({"run": "g5"})).await;
    assert_eq!(v["state"], "queued");
    let why = v["gang"]["waiting"].as_str().unwrap();
    assert!(
        why.contains("needs 2 computers") && why.contains("1 can"),
        "{why}"
    );
    p.ask("cancel_run", json!({"run": "g5"})).await;
    p.wait_state("g5", &["canceled"], 10).await;
    assert!(p.ran("g5").is_empty());

    // One dedicated host is enough for nodes 1:2: it runs there only.
    p.ask("submit", gang("g6", "1:2", 300, json!({"kind": "machine"})))
        .await;
    p.wait_state("g6", &["succeeded"], 30).await;
    let ran = p.ran("g6");
    assert_eq!(ran.len(), 1);
    assert_eq!(ran[0].host, "h0");

    // A running gang, canceled.
    p.ask("submit", gang("g7", "2:2", 30_000, json!({}))).await;
    let deadline = Instant::now() + Duration::from_secs(10);
    while p.ran("g7").len() < 2 {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    p.ask("cancel_run", json!({"run": "g7"})).await;
    let v = p.wait_state("g7", &["canceled"], 10).await;
    assert!(
        v["members"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| m["state"] == "canceled"),
        "{v:#}"
    );
}

/// A job (not a gang) that fails runs again `retries` times, after 2 s,
/// then 4 s; one that succeeds on a retry ends `succeeded`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn jobs_run_again_after_a_failure_with_a_backoff() {
    let p = Pool::new(1, 0).await;
    let submit = |run: &str, fail: &str| {
        json!({
            "run": run,
            "command": ["work", "100"],
            "retries": 2,
            "env": {"CUA_RUN_ID": run, "FAIL_RANK": "0", "FAIL_ATTEMPTS": fail},
        })
    };
    p.ask("submit", submit("j1", "*")).await;
    // Attempt numbers count host deliveries: 1, 2 and 3 fail.
    p.ask("submit", submit("j2", "1")).await;
    let v = p.wait_state("j1", &["failed"], 30).await;
    let ran = p.ran("j1");
    assert_eq!(ran.len(), 3, "{ran:?}");
    let gaps: Vec<Duration> = ran
        .windows(2)
        .map(|w| w[1].started - w[0].started)
        .collect();
    assert!(gaps[0] >= Duration::from_millis(1900), "{gaps:?}");
    assert!(gaps[1] >= Duration::from_millis(3900), "{gaps:?}");
    assert_eq!(v["members"][0]["attempt"], 3);
    let v = p.wait_state("j2", &["succeeded"], 30).await;
    assert_eq!(p.ran("j2").len(), 2);
    assert_eq!(v["retries"], 1);
}
