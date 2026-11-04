// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! End to end on loopback: a gang running a real Python program on two
//! hosts, each a real job runner in a throwaway cua home, served over gRPC
//! on 127.0.0.1 in a runtime of its own (shutting it down is the machine
//! going off: its daemon and the member die). The coordinator records into
//! a sealed pool log, as on the relay, and every request is the JSON a
//! client sends (`submit`, `run`, `job_output`, `run_result`).
//!
//! Each host's executor is a stand-in for a Space ([`StandIn`]): the
//! member's directories, environment and output log are the real ones
//! ([`cua_cluster::jobdir`], [`cua_cluster::member::member_env`]), and its
//! command runs as a local process where a host would run it in a Space
//! through that Space's cua-spacesd (`cua_spaces::host_jobs`, tested
//! against real Spaces elsewhere): this test is about the gang, not the
//! Space.
//!
//! The program prints its rank and peers, checkpoints every step into
//! `CUA_JOB_DATA`, and resumes from the checkpoint it finds. One host goes
//! off mid-run: the gang is placed again at size 1 on the host that is
//! left, resumes from that host's checkpoint, its live output is read while
//! it runs, and rank 0's result comes back.
//!
//! Needs `python3` on `PATH` (skipped without it).

#![cfg(unix)]

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use cua_cluster::jobdir::JobDirs;
use cua_cluster::link::{GrpcLink, JobsService, NodeLink, Target};
use cua_cluster::member::GuestPaths;
use cua_cluster::node::{ExecEnd, ExecOutcome, JobExecutor, NodeConfig, NodeJobs, Offers};
use cua_cluster::{CancellationToken, JobSpec};
use cua_proto::env::v1::host_jobs_service_server::HostJobsServiceServer;
use cua_spaces_cluster::coordinator::journal;
use cua_spaces_cluster::coordinator::requests::{self, Caller};
use cua_spaces_cluster::coordinator::store::NodeRow;
use cua_spaces_cluster::coordinator::{Config, Coordinator, LinkFactory};
use cua_spaces_cluster::pool::keys::{PoolKeys, SealedLog};
use cua_spaces_cluster::pool::memlog::MemPool;
use cua_spaces_cluster::pool::seal::PoolSecret;
use serde_json::{Value, json};

const TRAIN: &str = r#"
import os, sys, time
rank = int(os.environ["CUA_RANK"])
world = int(os.environ["CUA_WORLD_SIZE"])
data = os.environ["CUA_JOB_DATA"]
print(f"rank={rank} world={world} attempt={os.environ['CUA_ATTEMPT']} run={os.environ['CUA_RUN_ID']} peers={os.environ['CUA_PEERS']}", flush=True)
with open(os.path.join(os.environ["CUA_JOB_DIR"], "train.pid"), "w") as f:
    f.write(str(os.getpid()))
ckpt = os.path.join(data, "ckpt")
step = 0
if os.path.exists(ckpt):
    step = int(open(ckpt).read())
    print(f"resumed from step {step}", flush=True)
total = int(os.environ.get("STEPS", "40"))
while step < total:
    time.sleep(0.25)
    step += 1
    with open(ckpt + ".tmp", "w") as f:
        f.write(str(step))
    os.replace(ckpt + ".tmp", ckpt)
    print(f"step {step}", flush=True)
if rank == 0:
    with open(os.environ["CUA_RESULT_PATH"], "w") as f:
        f.write(f"adapter from rank 0 at step {step} (world {world})")
print("done", flush=True)
"#;

/// A member's Space, stood in for by a local process in its work
/// directory (the directories, environment and live output log are the
/// host's real ones).
struct StandIn {
    dirs: JobDirs,
}

#[async_trait]
impl JobExecutor for StandIn {
    async fn run(&self, job: &JobSpec, _space: &str, cancel: CancellationToken) -> ExecOutcome {
        let mut out = ExecOutcome::ended(ExecEnd::Exited(0));
        if let Err(e) = self.dirs.prepare(job, false) {
            out.end = ExecEnd::Error(e);
            return out;
        }
        let work = self.dirs.work_dir(&job.job_id, job.attempt);
        let guest = GuestPaths {
            work: work.display().to_string(),
            data: self
                .dirs
                .run_data_dir(cua_cluster::jobdir::run_of(job))
                .display()
                .to_string(),
            cache: self.dirs.cache_dir().display().to_string(),
            tools: self.dirs.home().join("bin").display().to_string(),
            path: std::env::var("PATH").unwrap_or_default(),
            python_installs: None,
        };
        let env = cua_cluster::member::member_env(job, &guest, "container", None);
        let log = std::fs::File::create(self.dirs.output_log(&job.job_id, job.attempt)).unwrap();
        let mut child = tokio::process::Command::new(&job.argv[0])
            .args(&job.argv[1..])
            .current_dir(&work)
            .env_clear()
            .envs(env)
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        out.end = tokio::select! {
            s = child.wait() => ExecEnd::Exited(s.unwrap().code().unwrap_or(137)),
            _ = cancel.cancelled() => {
                let _ = child.kill().await;
                ExecEnd::Canceled
            }
        };
        if !job.result_path.is_empty() {
            match cua_cluster::member::read_result(&self.dirs, job) {
                Ok(r) => out.result = r,
                Err(e) => out.result_error = Some(e),
            }
        }
        out.stdout = self.dirs.output_tail(&job.job_id, job.attempt, 1 << 20);
        self.dirs.finish(job);
        out
    }

    async fn remove_space(&self, _space: &str) {}

    fn offers(&self) -> Offers {
        Offers {
            kinds: vec!["container".into()],
            runtimes: vec!["stand-in".into()],
        }
    }

    fn output(&self, job_id: &str, attempt: u32, from: u64) -> Option<(Vec<u8>, u64)> {
        self.dirs.output(job_id, attempt, from)
    }
}

/// A host: a job runner in its own runtime, served on loopback.
struct Host {
    name: String,
    home: std::path::PathBuf,
    port: u16,
    rt: Option<tokio::runtime::Runtime>,
}

impl Host {
    fn start(name: &str, root: &std::path::Path) -> Self {
        let home = root.join(name).join(".cua");
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let h = home.clone();
        let n = name.to_string();
        let (rt, port) = std::thread::spawn(move || {
            let port = rt.block_on(async move {
                let exec = StandIn {
                    dirs: JobDirs::new(&h),
                };
                let jobs = NodeJobs::open(
                    NodeConfig {
                        dir: h.join("host/jobs"),
                        millicpus_total: 4000,
                        memory_total_mb: 8192,
                        jobs_max: 0,
                        min_available_mb: 0,
                        retention: Duration::from_secs(3600),
                        output_retention: Duration::from_secs(3600),
                        boot_id: n,
                    },
                    Arc::new(exec),
                )
                .unwrap();
                // Cut off, a host stops its gang members within 2 s (plus
                // the kill grace): the gang is placed again after that.
                jobs.set_self_fence_after(Duration::from_secs(2));
                let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let port = l.local_addr().unwrap().port();
                tokio::spawn(
                    tonic::transport::Server::builder()
                        .add_service(HostJobsServiceServer::new(JobsService(jobs)))
                        .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(l)),
                );
                port
            });
            (rt, port)
        })
        .join()
        .unwrap();
        Self {
            name: name.into(),
            home,
            port,
            rt: Some(rt),
        }
    }

    /// The machine goes off: its daemon stops, and the job's process with
    /// it.
    fn kill(&mut self) {
        if let Some(rt) = self.rt.take() {
            rt.shutdown_background();
        }
    }

    fn row(&self) -> NodeRow {
        NodeRow {
            name: self.name.clone(),
            direct: format!("127.0.0.1:{}", self.port),
            token: "t".into(),
            via: "direct".into(),
            state: "ready".into(),
            os: std::env::consts::OS.into(),
            ..Default::default()
        }
    }
}

fn bundle() -> Vec<u8> {
    let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::default(),
    ));
    let mut h = tar::Header::new_gnu();
    h.set_size(TRAIN.len() as u64);
    h.set_mode(0o644);
    h.set_cksum();
    tar.append_data(&mut h, "train.py", TRAIN.as_bytes())
        .unwrap();
    tar.into_inner().unwrap().finish().unwrap()
}

fn has_python() -> bool {
    std::process::Command::new("python3")
        .arg("-c")
        .arg("pass")
        .status()
        .is_ok_and(|s| s.success())
}

async fn ask(c: &Coordinator, method: &str, params: Value) -> Value {
    requests::handle(c, method, params, &Caller::local())
        .await
        .unwrap_or_else(|e| panic!("{method}: {e}"))
}

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks the pid.
    unsafe { libc_kill(pid, 0) == 0 }
}

unsafe extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, sig: i32) -> i32;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_gang_resumes_on_the_survivor_after_a_host_dies() {
    if !has_python() {
        eprintln!("skipped: no python3");
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let mut a = Host::start("mac-a", root.path());
    let mut b = Host::start("mac-b", root.path());

    // The coordinator, recording into a sealed pool log.
    let pool = MemPool::new();
    let dir = Arc::new(pool.keys("e2e"));
    let keys = PoolKeys::new(
        "e2e",
        common::TestDevice::new(),
        PoolSecret::generate(),
        dir,
    );
    keys.sync().await.unwrap();
    let epoch = pool.acquire("a").unwrap();
    let log = SealedLog::new(pool.log(epoch), keys);
    let cdir = root.path().join("coordinator");
    journal::rebuild(&cdir.join("cluster.db"), "e2e", &log)
        .await
        .unwrap();
    let mut cfg = Config::new(&cdir);
    cfg.poll_wait = Duration::from_millis(300);
    cfg.node_down_after = Duration::from_millis(1500);
    cfg.ack_grace = Duration::from_secs(2);
    let links: LinkFactory = Arc::new(|row: &NodeRow| {
        Some(Arc::new(
            GrpcLink::new(Target::Direct(row.direct.clone()), row.token.clone()).without_probe(),
        ) as Arc<dyn NodeLink>)
    });
    let coord = Coordinator::start_with(cfg, Some(links), Arc::new(log))
        .await
        .unwrap();
    coord.add_node(a.row()).await.unwrap();
    coord.add_node(b.row()).await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while coord.nodes().iter().any(|n| n.health != "ready") {
        assert!(Instant::now() < deadline, "hosts never came up");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let st = ask(&coord, "status", json!({})).await;
    for m in st["machines"].as_array().unwrap() {
        assert!(
            m["kinds"].as_array().unwrap().contains(&json!("container")),
            "{m}"
        );
        assert_eq!(m["dedicated"], false, "{m}");
        assert!(
            m["usd_per_hour"].is_number() && m["accelerators"].is_array(),
            "{m}"
        );
    }

    // The removed native kind is refused at submit, saying why.
    let e = requests::handle(
        &coord,
        "submit",
        json!({"run": "native", "kind": "process", "command": ["true"]}),
        &Caller::local(),
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(e.contains("every job runs in a Space"), "{e}");

    // A gang of 2 (at least 1) members, in whatever Space each host picks.
    use base64::Engine as _;
    let r = ask(
        &coord,
        "submit",
        json!({
            "run": "lora",
            "gang": true,
            "nodes": {"min": 1, "max": 2},
            "command": ["python3", "train.py"],
            "env": {"STEPS": "40"},
            "bundle": base64::engine::general_purpose::STANDARD.encode(bundle()),
            "cpus": 1,
            "memory_mb": 256,
            "timeout_secs": 120,
            "retries": 3,
            "result_path": "adapter.txt",
        }),
    )
    .await;
    assert_eq!(r["gang"], true);

    // Both ranks run and checkpoint.
    let deadline = Instant::now() + Duration::from_secs(30);
    let first = loop {
        let v = ask(&coord, "run", json!({"run": "lora"})).await;
        let ok = v["members"]
            .as_array()
            .is_some_and(|m| m.len() == 2 && m.iter().all(|m| m["state"] == "running"));
        if ok {
            let out0 = ask(&coord, "job_output", json!({"run": "lora", "rank": 0})).await;
            let out1 = ask(&coord, "job_output", json!({"run": "lora", "rank": 1})).await;
            let (t0, t1) = (
                out0["text"].as_str().unwrap(),
                out1["text"].as_str().unwrap(),
            );
            if t0.contains("step 4") && t1.contains("step 4") {
                eprintln!("attempt 1, rank 0 on {}:\n{t0}", out0["host"]);
                eprintln!("attempt 1, rank 1 on {}:\n{t1}", out1["host"]);
                assert!(t0.contains("rank=0 world=2 attempt=1 run=lora"), "{t0}");
                assert!(t1.contains("rank=1 world=2 attempt=1 run=lora"), "{t1}");
                break v;
            }
        }
        assert!(Instant::now() < deadline, "the gang never ran: {v:#}");
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    let peers_line = ask(&coord, "job_output", json!({"run": "lora", "rank": 0})).await["text"]
        .as_str()
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .to_string();
    assert_eq!(
        peers_line
            .split("peers=")
            .nth(1)
            .unwrap()
            .split(',')
            .count(),
        2
    );

    // Kill the host that runs rank 1: its process dies with it.
    let victim_name = first["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["rank"] == 1)
        .unwrap()["host"]
        .as_str()
        .unwrap()
        .to_string();
    let (victim, survivor) = if victim_name == a.name {
        (&mut a, &b)
    } else {
        (&mut b, &a)
    };
    let pid_file = std::fs::read_dir(victim.home.join("jobs"))
        .unwrap()
        .flatten()
        .map(|e| e.path().join("job").join("train.pid"))
        .find(|p| p.is_file())
        .unwrap();
    let pid: i32 = std::fs::read_to_string(&pid_file).unwrap().parse().unwrap();
    assert!(alive(pid));
    eprintln!("killing {} (rank 1, python pid {pid})", victim.name);
    victim.kill();
    let deadline = Instant::now() + Duration::from_secs(10);
    while alive(pid) {
        assert!(
            Instant::now() < deadline,
            "the job's process outlived its host"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // The gang runs again, at size 1, on the survivor, from its checkpoint;
    // its output is read live while it runs.
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut live = String::new();
    let mut from = 0u64;
    let mut reads = 0;
    loop {
        let v = ask(&coord, "run", json!({"run": "lora"})).await;
        // The member may not have reached its host yet: try again.
        if v["gang"]["attempt"] == 2
            && let Ok(out) = requests::handle(
                &coord,
                "job_output",
                json!({"run": "lora", "rank": 0, "from": from}),
                &Caller::local(),
            )
            .await
        {
            assert_eq!(out["attempt"], 2);
            assert_eq!(out["host"], survivor.name.as_str());
            let t = out["text"].as_str().unwrap();
            if !t.is_empty() {
                reads += 1;
                live.push_str(t);
            }
            from = out["next"].as_u64().unwrap();
            if out["done"] == true {
                break;
            }
        }
        assert!(Instant::now() < deadline, "attempt 2 did not finish: {v:#}");
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    eprintln!(
        "attempt 2 on {} (read live in {reads} chunks):\n{live}",
        survivor.name
    );
    assert!(reads >= 2, "the output was not read while it ran");
    assert!(live.contains("rank=0 world=1 attempt=2 run=lora"), "{live}");
    let resumed: u32 = live
        .lines()
        .find_map(|l| l.strip_prefix("resumed from step "))
        .expect("attempt 2 resumed from a checkpoint")
        .trim()
        .parse()
        .unwrap();
    assert!(resumed >= 4, "resumed from step {resumed}");
    assert!(live.contains("step 40") && live.contains("done"), "{live}");

    let v = ask(&coord, "run", json!({"run": "lora"})).await;
    let deadline = Instant::now() + Duration::from_secs(20);
    let v = if v["state"] == "succeeded" {
        v
    } else {
        loop {
            let v = ask(&coord, "run", json!({"run": "lora"})).await;
            if v["state"] == "succeeded" {
                break v;
            }
            assert!(Instant::now() < deadline, "{v:#}");
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    };
    eprintln!("run: {}", serde_json::to_string_pretty(&v).unwrap());
    assert_eq!(v["gang"]["attempt"], 2);
    assert_eq!(v["gang"]["size"], 1);
    // A lost machine uses the infrastructure budget, not `retries`.
    assert_eq!(v["gang"]["failures"], 0);
    assert_eq!(v["gang"]["infra_failures"], 1);
    assert_eq!(v["gang"]["summary"], "attempt 2 · 1 lost computer");
    let a2: Vec<&Value> = v["members"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["attempt"] == 2)
        .collect();
    assert_eq!(a2.len(), 1);
    assert_eq!(a2[0]["host"], survivor.name.as_str());
    let lost = v["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["attempt"] == 1 && m["host"] == victim_name.as_str())
        .unwrap();
    assert_eq!(lost["state"], "error");

    // The checkpoint is on the survivor, at the last step.
    let data = std::fs::read_dir(survivor.home.join("job-data"))
        .unwrap()
        .flatten()
        .next()
        .unwrap()
        .path();
    assert_eq!(std::fs::read_to_string(data.join("ckpt")).unwrap(), "40");

    // Rank 0's result.
    let res = ask(&coord, "run_result", json!({"run": "lora"})).await;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(res["result_b64"].as_str().unwrap())
        .unwrap();
    let text = String::from_utf8(bytes).unwrap();
    eprintln!("result: {text}");
    assert_eq!(text, "adapter from rank 0 at step 40 (world 1)");
    coord.stop();
    a.kill();
    b.kill();
}
