// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The coordinator against fake hosts over real gRPC on loopback: each
//! host is a real job runner (ledger, fencing, capacity) whose Spaces are
//! fakes, in its own tokio runtime (killing the runtime is killing the
//! daemon), behind a TCP gate (closing the gate is a network partition).
//!
//! The chaos test runs 5,000 jobs while hosts are killed and restarted,
//! partitioned and healed, a host's disk is wiped, and the coordinator
//! itself is restarted, and checks that no job is lost and none completes
//! twice.

mod common;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::FakeExec;
use common::hosts::*;
use cua_cluster::JobSpec;
use cua_cluster::link::{GrpcLink, NodeLink, Target};
use cua_cluster::node::{NodeConfig, NodeJobs};
use cua_spaces_cluster::coordinator::durable::DurableSpec;
use cua_spaces_cluster::coordinator::store::{self, NewJob, NodeRow};
use cua_spaces_cluster::coordinator::{Coordinator, LinkFactory};

fn job(i: usize, argv: Vec<String>, max_attempts: u32) -> NewJob {
    NewJob {
        key: format!("k{i}"),
        spec: JobSpec {
            argv,
            millicpus: 1000,
            memory_mb: 256,
            timeout_secs: 60,
            max_output_bytes: 1024,
            ..Default::default()
        },
        max_attempts,
    }
}

async fn counts(c: &Coordinator) -> std::collections::BTreeMap<String, u64> {
    c.store().call(|c| store::counts(c)).await.unwrap()
}

async fn wait_done(c: &Coordinator, total: u64, within: Duration) {
    let deadline = Instant::now() + within;
    loop {
        let n = counts(c).await;
        let ended: u64 = n
            .iter()
            .filter(|(k, _)| store::state::is_terminal(k))
            .map(|(_, v)| *v)
            .sum();
        if ended >= total {
            return;
        }
        if std::env::var_os("CUA_CLUSTER_TEST_PROGRESS").is_some() {
            let (q, f) = c.depth();
            println!("{n:?} queued={q} in-flight={f} {:?}", c.metrics());
        }
        assert!(
            Instant::now() < deadline,
            "not done: {n:?} {:?}",
            c.metrics()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Hosts die, partition, lose their disk; the coordinator restarts; every
/// one of 5,000 jobs completes exactly once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn five_thousand_jobs_survive_chaos_with_no_loss_and_no_duplicate_completion() {
    let root = tempfile::tempdir().unwrap();
    let exec = FakeExec::new();
    let mut hosts = Vec::new();
    for i in 0..4 {
        hosts.push(Host::new(&format!("h{i}"), root.path(), exec.clone(), 24).await);
    }
    let cdir = root.path().join("coord");
    let mut coord = Coordinator::start(config(&cdir), Some(links()))
        .await
        .unwrap();
    for h in &hosts {
        coord.add_node(h.row()).await.unwrap();
    }
    const N: usize = 5000;
    let jobs: Vec<NewJob> = (0..N)
        .map(|i| {
            let ms = 20 + (i * 7919) % 100;
            job(i, vec!["sleep".into(), ms.to_string()], 100)
        })
        .collect();
    let started = Instant::now();
    let ids = coord.submit("chaos", 0, jobs).await.unwrap();
    assert_eq!(ids.len(), N);
    // Submitting again is idempotent.
    let again = coord
        .submit(
            "chaos",
            0,
            (0..10)
                .map(|i| job(i, vec!["sleep".into(), "1".into()], 100))
                .collect(),
        )
        .await
        .unwrap();
    assert!(again.iter().all(|(_, existed)| *existed));

    tokio::time::sleep(Duration::from_millis(500)).await;
    hosts[1].kill(); // daemon killed mid-job
    tokio::time::sleep(Duration::from_millis(500)).await;
    hosts[2].gate.close(); // partition: it keeps running its jobs
    tokio::time::sleep(Duration::from_millis(500)).await;
    // The coordinator dies and comes back.
    coord.stop();
    drop(coord);
    tokio::time::sleep(Duration::from_millis(300)).await;
    coord = Coordinator::start(config(&cdir), Some(links()))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(700)).await;
    hosts[1].start();
    hosts[3].kill();
    // Its disk is wiped: a new ledger.
    std::fs::remove_dir_all(&hosts[3].dir).unwrap();
    tokio::time::sleep(Duration::from_millis(1000)).await;
    // Heal the partition: the same server, still running its jobs.
    let upstream = *hosts[2].gate.upstream.lock().unwrap();
    hosts[2].gate.open(upstream);
    hosts[3].start();

    wait_done(&coord, N as u64, Duration::from_secs(240)).await;
    let elapsed = started.elapsed();
    let n = counts(&coord).await;
    assert_eq!(n.get("succeeded").copied(), Some(N as u64), "{n:?}");
    assert_eq!(n.len(), 1, "every job succeeded: {n:?}");

    // Exactly once in the database; at least once on the hosts.
    let rows = coord
        .store()
        .call(|c| store::jobs(c, "chaos", "", 0, 100_000, false))
        .await
        .unwrap();
    assert_eq!(rows.len(), N);
    let cid = coord.cluster_id().to_string();
    let seen = exec.seen.lock().unwrap();
    let mut extra = 0u32;
    for r in &rows {
        assert_eq!(r.state, "succeeded");
        let ends = seen
            .ends
            .get(&format!("{cid}-{}", r.id))
            .copied()
            .unwrap_or(0);
        assert!(ends >= 1, "job {} completed on no host", r.id);
        extra += ends - 1;
    }
    let m = coord.metrics();
    println!(
        "chaos: {N} jobs in {elapsed:?}; extra executions {extra}; retries {}; duplicates suppressed {}; \
         node-down {}; reconciles {}; fences {}; rejected {}",
        m.retries, m.duplicates_suppressed, m.node_down, m.reconciles, m.fences, m.rejected
    );
    let mut per: HashMap<String, u32> = HashMap::new();
    for r in &rows {
        *per.entry(r.node.clone()).or_default() += 1;
    }
    println!("completions by host: {per:?}");
    drop(seen);
    coord.stop();
    for h in &mut hosts {
        h.kill();
    }
}

/// A coordinator killed right after dispatching comes back, reconciles
/// from the hosts, and finishes every job once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restarted_coordinator_reconciles_jobs_in_flight() {
    let root = tempfile::tempdir().unwrap();
    let exec = FakeExec::new();
    let mut h = Host::new("only", root.path(), exec.clone(), 50).await;
    let cdir = root.path().join("coord");
    let coord = Coordinator::start(config(&cdir), Some(links()))
        .await
        .unwrap();
    coord.add_node(h.row()).await.unwrap();
    coord
        .submit(
            "r",
            0,
            (0..200)
                .map(|i| job(i, vec!["sleep".into(), "300".into()], 5))
                .collect(),
        )
        .await
        .unwrap();
    // Wait until some are running, then kill the coordinator.
    let deadline = Instant::now() + Duration::from_secs(10);
    while coord.metrics().accepted < 20 {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    coord.stop();
    drop(coord);
    let coord = Coordinator::start(config(&cdir), Some(links()))
        .await
        .unwrap();
    wait_done(&coord, 200, Duration::from_secs(60)).await;
    let n = counts(&coord).await;
    assert_eq!(n.get("succeeded").copied(), Some(200), "{n:?}");
    // Nothing ran twice: the host kept running what it had, the new
    // coordinator read the results back.
    let seen = exec.seen.lock().unwrap();
    let total: u32 = seen.starts.values().sum();
    assert_eq!(total, 200, "every job ran once");
    drop(seen);
    coord.stop();
    h.kill();
}

/// Each run's concurrency limit holds; a job's command failure is a
/// completion (not retried); a job no host can run fails after its
/// attempts; a drained host gets no new jobs; a removed host's jobs run
/// elsewhere.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn limits_failures_drain_and_remove() {
    let root = tempfile::tempdir().unwrap();
    let exec = FakeExec::new();
    let mut a = Host::new("a", root.path(), exec.clone(), 8).await;
    let mut b = Host::new("b", root.path(), exec.clone(), 8).await;
    let coord = Coordinator::start(config(&root.path().join("c")), Some(links()))
        .await
        .unwrap();
    coord.add_node(a.row()).await.unwrap();
    coord.add_node(b.row()).await.unwrap();

    // Concurrency 3: never more than three of the run in flight.
    coord
        .submit(
            "limited",
            3,
            (0..30)
                .map(|i| job(i, vec!["sleep".into(), "50".into()], 3))
                .collect(),
        )
        .await
        .unwrap();
    let mut peak = 0;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let n = exec.seen.lock().unwrap().live.len();
        peak = peak.max(n);
        let done = coord
            .store()
            .call(|c| store::run_summary(c, "limited"))
            .await
            .unwrap()
            .unwrap()
            .done;
        if done {
            break;
        }
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(peak <= 3, "peak {peak}");

    coord
        .submit(
            "bad",
            0,
            vec![
                NewJob {
                    key: "exit".into(),
                    ..job(0, vec!["exit".into(), "7".into()], 3)
                },
                NewJob {
                    key: "nospace".into(),
                    ..job(1, vec!["nospace".into()], 3)
                },
            ],
        )
        .await
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let rows = loop {
        let rows = coord
            .store()
            .call(|c| store::jobs(c, "bad", "", 0, 10, true))
            .await
            .unwrap();
        if rows.iter().all(|r| store::state::is_terminal(&r.state)) {
            break rows;
        }
        assert!(Instant::now() < deadline, "{rows:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(
        (rows[0].state.as_str(), rows[0].exit_code, rows[0].attempt),
        ("failed", Some(7), 1)
    );
    assert_eq!(rows[1].state, "error");
    assert_eq!(rows[1].attempt, 3, "{:?}", rows[1]);
    assert!(
        rows[1].error.contains("gave up after 3"),
        "{}",
        rows[1].error
    );

    // Drain a: new jobs go to b only.
    coord.drain("a", true).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let ids = coord
        .submit(
            "drained",
            0,
            (0..10)
                .map(|i| job(100 + i, vec!["sleep".into(), "10".into()], 3))
                .collect(),
        )
        .await
        .unwrap();
    wait_done(&coord, 30 + 2 + 10, Duration::from_secs(30)).await;
    for (id, _) in ids {
        let j = coord
            .store()
            .call(move |c| store::job(c, id))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(j.node, "b");
    }
    // Remove b while it runs long jobs: they go to a (undrained).
    coord.drain("a", false).await.unwrap();
    coord.drain("b", false).await.unwrap();
    coord
        .submit(
            "moved",
            0,
            (0..4)
                .map(|i| job(200 + i, vec!["sleep".into(), "1500".into()], 3))
                .collect(),
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    coord.remove_node("b").await.unwrap();
    wait_done(&coord, 30 + 2 + 10 + 4, Duration::from_secs(30)).await;
    let rows = coord
        .store()
        .call(|c| store::jobs(c, "moved", "", 0, 10, false))
        .await
        .unwrap();
    assert!(
        rows.iter().all(|r| r.state == "succeeded" && r.node == "a"),
        "{rows:?}"
    );
    assert_eq!(coord.nodes().len(), 1);
    coord.stop();
    a.kill();
    b.kill();
}

/// The scheduler alone: jobs that end at once on four fake hosts with
/// room for 250 each, so what is measured is placement, dispatch,
/// acknowledgement, the feeds and the database (10,000 jobs;
/// `CUA_CLUSTER_BENCH_JOBS` for more).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_scheduler_sustains_hundreds_of_dispatches_per_second() {
    let n: usize = std::env::var("CUA_CLUSTER_BENCH_JOBS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10_000);
    let root = tempfile::tempdir().unwrap();
    let exec = FakeExec::new();
    let mut hosts = Vec::new();
    for i in 0..4 {
        hosts.push(Host::new(&format!("h{i}"), root.path(), exec.clone(), 250).await);
    }
    let coord = Coordinator::start(config(&root.path().join("c")), Some(links()))
        .await
        .unwrap();
    for h in &hosts {
        coord.add_node(h.row()).await.unwrap();
    }
    let started = Instant::now();
    for chunk in (0..n).collect::<Vec<_>>().chunks(10_000) {
        coord
            .submit(
                "bench",
                0,
                chunk
                    .iter()
                    .map(|i| job(*i, vec!["true".into()], 5))
                    .collect(),
            )
            .await
            .unwrap();
    }
    wait_done(&coord, n as u64, Duration::from_secs(600)).await;
    let wall = started.elapsed();
    let c = counts(&coord).await;
    assert_eq!(c.get("succeeded").copied(), Some(n as u64), "{c:?}");
    let m = coord.metrics();
    println!(
        "scheduler: {n} jobs on 4 hosts in {wall:?} ({:.0} jobs/s); dispatch call p50 {} ms p99 {} ms; \
         submit-to-end p50 {} ms p99 {} ms; rejected {}",
        n as f64 / wall.as_secs_f64(),
        m.dispatch_p50_ms,
        m.dispatch_p99_ms,
        m.e2e_p50_ms,
        m.e2e_p99_ms,
        m.rejected
    );
    coord.stop();
    for h in &mut hosts {
        h.kill();
    }
}

/// Elastic capacity (a provider in the coordinator's process, like Modal)
/// takes jobs only when no host has room, or when a run names it, and its
/// job limit holds.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn elastic_capacity_takes_the_overflow_and_named_runs() {
    let root = tempfile::tempdir().unwrap();
    let exec = FakeExec::new();
    let mut host = Host::new("mini", root.path(), exec.clone(), 2).await;
    let elastic_exec = FakeExec::new();
    let pdir = root.path().join("provider");
    let made: Arc<std::sync::OnceLock<Arc<dyn NodeLink>>> = Arc::default();
    let factory: LinkFactory = {
        let made = made.clone();
        let elastic_exec = elastic_exec.clone();
        Arc::new(move |row: &NodeRow| {
            if row.via == "provider" {
                let l = made.get_or_init(|| {
                    let jobs = NodeJobs::open(
                        NodeConfig {
                            dir: pdir.clone(),
                            millicpus_total: 64_000,
                            memory_total_mb: 1 << 20,
                            jobs_max: 3,
                            min_available_mb: 0,
                            retention: Duration::from_secs(3600),
                            output_retention: Duration::from_secs(3600),
                            boot_id: "p".into(),
                        },
                        Arc::new(elastic_exec.clone()),
                    )
                    .unwrap();
                    Arc::new(cua_cluster::link::LocalLink::new(jobs, "cloud", "linux"))
                });
                return Some(l.clone());
            }
            links()(row)
        })
    };
    let coord = Coordinator::start(config(&root.path().join("c")), Some(factory))
        .await
        .unwrap();
    coord.add_node(host.row()).await.unwrap();
    coord
        .add_node(NodeRow {
            name: "cloud".into(),
            via: "provider".into(),
            provider: "cloud".into(),
            state: "ready".into(),
            os: "linux".into(),
            max_jobs: 3,
            ..Default::default()
        })
        .await
        .unwrap();
    wait_for("both ready", Duration::from_secs(10), async || {
        (coord.nodes().iter().filter(|n| n.health == "ready").count() == 2).then_some(())
    })
    .await;
    // Two fit on the host at once (2 CPUs): the rest overflow to the
    // provider, never more than 3 there at once.
    coord
        .submit(
            "burst",
            0,
            (0..12)
                .map(|i| job(i, vec!["sleep".into(), "300".into()], 5))
                .collect(),
        )
        .await
        .unwrap();
    let mut peak = 0;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        peak = peak.max(elastic_exec.seen.lock().unwrap().live.len());
        let n = counts(&coord).await;
        if n.get("succeeded").copied() == Some(12) {
            break;
        }
        assert!(Instant::now() < deadline, "{n:?}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(peak <= 3, "peak {peak}");
    let rows = coord
        .store()
        .call(|c| store::jobs(c, "burst", "", 0, 100, false))
        .await
        .unwrap();
    let on_host = rows.iter().filter(|r| r.node == "mini").count();
    let on_cloud = rows.iter().filter(|r| r.node == "cloud").count();
    assert!(
        on_host >= 2 && on_cloud >= 1,
        "host {on_host}, cloud {on_cloud}"
    );
    // A lone job goes to the idle host, not the provider ...
    let lone = coord
        .submit(
            "lone",
            0,
            vec![NewJob {
                key: "lone".into(),
                ..job(100, vec!["true".into()], 5)
            }],
        )
        .await
        .unwrap()[0]
        .0;
    // ... unless the run names the provider.
    let mut named = job(101, vec!["true".into()], 5);
    named.key = "named".into();
    named.spec.on = "cloud".into();
    let named = coord.submit("named", 0, vec![named]).await.unwrap()[0].0;
    wait_done(&coord, 14, Duration::from_secs(30)).await;
    let node_of = async |id: i64| {
        coord
            .store()
            .call(move |c| store::job(c, id))
            .await
            .unwrap()
            .unwrap()
            .node
    };
    assert_eq!(node_of(lone).await, "mini");
    assert_eq!(node_of(named).await, "cloud");
    coord.stop();
    host.kill();
}

/// `--on aws`: a job pinned to a cloud (`cloud:aws`) runs only on hosts that
/// cloud supplied, never on your own machine or another cloud's host, and
/// one pinned to a cloud with no host yet waits and counts as that cloud's
/// demand (what the elastic pool grows).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_job_on_a_cloud_runs_only_on_that_clouds_hosts() {
    let root = tempfile::tempdir().unwrap();
    let mut mini = Host::new("mini", root.path(), FakeExec::new(), 8).await;
    let mut aws = Host::new("aws-1", root.path(), FakeExec::new(), 8).await;
    let mut hz = Host::new("hetzner-1", root.path(), FakeExec::new(), 8).await;
    let coord = Coordinator::start(config(&root.path().join("c")), Some(links()))
        .await
        .unwrap();
    coord.add_node(mini.row()).await.unwrap();
    coord
        .add_node(NodeRow {
            provider: "aws".into(),
            ..aws.row()
        })
        .await
        .unwrap();
    coord
        .add_node(NodeRow {
            provider: "hetzner".into(),
            ..hz.row()
        })
        .await
        .unwrap();
    wait_for("hosts up", Duration::from_secs(10), async || {
        (coord.nodes().iter().filter(|n| n.health == "ready").count() == 3).then_some(())
    })
    .await;
    let on = |i: usize, on: &str| {
        let mut j = job(i, vec!["true".into()], 5);
        j.spec.on = on.into();
        j
    };
    coord
        .submit("aws", 0, (0..6).map(|i| on(i, "cloud:aws")).collect())
        .await
        .unwrap();
    wait_done(&coord, 6, Duration::from_secs(30)).await;
    let rows = coord
        .store()
        .call(|c| store::jobs(c, "aws", "", 0, 100, false))
        .await
        .unwrap();
    assert_eq!(rows.len(), 6);
    assert!(rows.iter().all(|r| r.node == "aws-1"), "{rows:?}");
    // No gcp host: it waits (own machines are not where it goes), and the
    // elastic pool is told to add that cloud's machines.
    let waiting = coord
        .submit("gcp", 0, vec![on(50, "cloud:gcp")])
        .await
        .unwrap()[0]
        .0;
    // A job naming a machine is no demand for a new one.
    coord
        .submit("named", 0, vec![on(51, "no-such-host")])
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let state = coord
        .store()
        .call(move |c| store::job(c, waiting))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.state, "queued", "{state:?}");
    let demand = coord.pending_demand(Duration::ZERO);
    assert_eq!(demand.len(), 1, "{demand:?}");
    assert_eq!(demand[0]["provider"], "gcp");
    assert_eq!(demand[0]["count"], 1);
    // A gang (`cua run`, compat runs) that no computer of that cloud can
    // take is demand too, one machine per member it is short; one the
    // cloud's machines can take is not.
    let gang = |on: &str| cua_spaces_cluster::coordinator::gang::GangSpec {
        min: 1,
        max: 1,
        retries: 0,
        spec: JobSpec {
            on: on.into(),
            ..job(60, vec!["sleep".into(), "30".into()], 5).spec
        },
        policy: Default::default(),
    };
    coord
        .submit_gang("gang-gcp", gang("cloud:gcp"))
        .await
        .unwrap();
    wait_for(
        "the gcp gang is demand",
        Duration::from_secs(10),
        async || {
            let d = coord.pending_demand(Duration::ZERO);
            (d.len() == 1 && d[0]["count"] == 2).then_some(())
        },
    )
    .await;
    coord
        .submit_gang("gang-aws", gang("cloud:aws"))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let demand = coord.pending_demand(Duration::ZERO);
    assert_eq!(demand.len(), 1, "{demand:?}");
    assert_eq!(demand[0]["provider"], "gcp");
    assert_eq!(demand[0]["count"], 2, "{demand:?}");
    coord.stop();
    mini.kill();
    aws.kill();
    hz.kill();
}

/// A durable Space: placed by name, held by one client under a lease with
/// a fencing token, created again on another host (from scratch) when its
/// host goes down, reachable at its new address with the same lease, and
/// the copy on the dead host deleted when it comes back.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_durable_space_moves_when_its_host_dies_and_keeps_its_lease() {
    let root = tempfile::tempdir().unwrap();
    let exec = FakeExec::new();
    let mut hosts = vec![
        Host::new("a", root.path(), exec.clone(), 8).await,
        Host::new("b", root.path(), exec.clone(), 8).await,
    ];
    let coord = Coordinator::start(config(&root.path().join("c")), Some(links()))
        .await
        .unwrap();
    for h in &hosts {
        coord.add_node(h.row()).await.unwrap();
    }
    // Both up before placing.
    wait_for("hosts up", Duration::from_secs(10), async || {
        (coord.nodes().iter().filter(|n| n.health == "ready").count() == 2).then_some(())
    })
    .await;
    assert!(
        coord
            .durable_create("Bad Name", DurableSpec::default())
            .await
            .is_err()
    );
    let v = coord
        .durable_create("research", DurableSpec::default())
        .await
        .unwrap();
    assert_eq!((v.state.as_str(), v.generation), ("running", 1), "{v:?}");
    let first = v.node.clone();
    let i = hosts.iter().position(|h| h.name == first).unwrap();
    // Idempotent by name.
    assert_eq!(
        coord
            .durable_create("research", DurableSpec::default())
            .await
            .unwrap()
            .node,
        first
    );

    let lease = coord
        .durable_acquire("research", "agent-1", Duration::from_secs(60))
        .await
        .unwrap();
    assert_eq!(lease.lease_token, 1);
    assert!(lease.id.starts_with("direct:127.0.0.1:"), "{}", lease.id);
    let taken = coord
        .durable_acquire("research", "agent-2", Duration::from_secs(60))
        .await
        .unwrap_err();
    assert!(taken.to_string().contains("held by agent-1"), "{taken}");

    // Its host dies: the Space is created again on the other one.
    hosts[i].kill();
    let moved = wait_for("rescheduled", Duration::from_secs(30), async || {
        let v = coord.durable_get("research").await.unwrap();
        (v.generation == 2).then_some(v)
    })
    .await;
    assert_ne!(moved.node, first);
    assert_eq!(moved.restored, "from-scratch");
    // The holder keeps its lease and finds the new address with it.
    let now = coord.durable_connection("research", 1).await.unwrap();
    assert_ne!(now.token, lease.token, "a new Space on another host");
    assert_eq!(
        (now.node.as_str(), now.generation),
        (moved.node.as_str(), 2)
    );

    // The dead host comes back with the old copy: it is deleted.
    let old = hosts[i]
        .spaces
        .0
        .lock()
        .unwrap()
        .spaces
        .iter()
        .map(|s| s.local_space.clone())
        .find(|s| s.starts_with("local:research-"))
        .expect("the first copy is on the dead host");
    hosts[i].start();
    wait_for("stale copy deleted", Duration::from_secs(30), async || {
        hosts[i]
            .spaces
            .0
            .lock()
            .unwrap()
            .deleted
            .contains(&old)
            .then_some(())
    })
    .await;

    // Released, someone else holds it with a higher token; the old token
    // is fenced off.
    coord.durable_release("research", 1).await.unwrap();
    let next = coord
        .durable_acquire("research", "agent-2", Duration::from_secs(60))
        .await
        .unwrap();
    assert_eq!(next.lease_token, 2);
    let fenced = coord
        .durable_renew("research", 1, Duration::from_secs(60))
        .await
        .unwrap_err();
    assert!(fenced.to_string().contains("no longer held"), "{fenced}");

    // A coordinator restart changes nothing.
    coord.stop();
    drop(coord);
    let coord = Coordinator::start(config(&root.path().join("c")), Some(links()))
        .await
        .unwrap();
    let again = coord.durable_connection("research", 2).await.unwrap();
    assert_eq!(
        (again.generation, again.node.as_str()),
        (2, moved.node.as_str())
    );

    // Deleted: gone from the list and from its host.
    coord.durable_delete("research").await.unwrap();
    assert!(coord.durable_list().await.unwrap().is_empty());
    let j = hosts.iter().position(|h| h.name == moved.node).unwrap();
    assert!(
        hosts[j]
            .spaces
            .0
            .lock()
            .unwrap()
            .deleted
            .iter()
            .any(|d| d.starts_with("local:research-"))
    );
    coord.stop();
    for h in &mut hosts {
        h.kill();
    }
}

/// On the account's machines (reached through the relay, no direct
/// address): a durable Space joins the relay as a machine of its own, is
/// found by its name, and moves to another machine when its machine dies.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_durable_space_on_relay_machines_moves_when_its_machine_dies() {
    let root = tempfile::tempdir().unwrap();
    let exec = FakeExec::new();
    let mut hosts = vec![
        Host::new("a", root.path(), exec.clone(), 8).await,
        Host::new("b", root.path(), exec.clone(), 8).await,
    ];
    // Reached through the relay: the rows carry a relay machine and no
    // direct address (the test reaches them at their port all the same).
    let ports: std::collections::HashMap<String, String> = hosts
        .iter()
        .map(|h| (h.name.clone(), h.row().direct))
        .collect();
    let relayed: LinkFactory = Arc::new(move |row: &NodeRow| {
        Some(Arc::new(
            GrpcLink::new(Target::Direct(ports[&row.name].clone()), "t".to_string())
                .without_probe(),
        ) as Arc<dyn NodeLink>)
    });
    let coord = Coordinator::start(config(&root.path().join("c")), Some(relayed))
        .await
        .unwrap();
    for h in &hosts {
        let mut row = h.row();
        row.direct.clear();
        row.via = "relay".into();
        row.relay_machine = format!("m-{}", h.name);
        coord.add_node(row).await.unwrap();
    }
    wait_for("hosts up", Duration::from_secs(10), async || {
        (coord.nodes().iter().filter(|n| n.health == "ready").count() == 2).then_some(())
    })
    .await;
    let v = coord
        .durable_create("research", DurableSpec::default())
        .await
        .unwrap();
    assert_eq!((v.state.as_str(), v.generation), ("running", 1), "{v:?}");
    assert!(v.space.starts_with("relay:space-"), "{}", v.space);
    assert!(v.token.is_empty());
    let first = v.node.clone();
    let i = hosts.iter().position(|h| h.name == first).unwrap();
    hosts[i].kill();
    let moved = wait_for("rescheduled", Duration::from_secs(30), async || {
        let v = coord.durable_get("research").await.unwrap();
        (v.generation == 2).then_some(v)
    })
    .await;
    assert_ne!(moved.node, first);
    assert!(moved.space.starts_with("relay:space-"), "{}", moved.space);
    assert_ne!(moved.space, v.space);
    coord.durable_delete("research").await.unwrap();
    coord.stop();
    for h in &mut hosts {
        h.kill();
    }
}

/// A machine that takes over with an empty database rebuilds durable
/// Spaces from the pool log (found on real machines: the durable table did
/// not exist yet while the log was replayed, so no machine could take over
/// a pool that had one).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_holder_rebuilds_durable_spaces_from_the_log() {
    use cua_spaces_cluster::coordinator::journal;
    use cua_spaces_cluster::pool::memlog::MemPool;
    let root = tempfile::tempdir().unwrap();
    let mut host = Host::new("a", root.path(), FakeExec::new(), 8).await;
    let pool = MemPool::new();
    let e1 = pool.acquire("first").unwrap();
    let first = Coordinator::start_with(
        config(&root.path().join("first")),
        Some(links()),
        Arc::new(pool.log(e1)),
    )
    .await
    .unwrap();
    first.add_node(host.row()).await.unwrap();
    wait_for("host up", Duration::from_secs(10), async || {
        (first.nodes().iter().any(|n| n.health == "ready")).then_some(())
    })
    .await;
    first
        .durable_create("research", DurableSpec::default())
        .await
        .unwrap();
    first.stop();
    pool.expire();
    let e2 = pool.acquire("second").unwrap();
    let dir = root.path().join("second");
    let log = pool.log(e2);
    journal::rebuild(&dir.join("cluster.db"), "p", &log)
        .await
        .expect("a fresh database rebuilds a log with durable Spaces");
    let second = Coordinator::start_with(config(&dir), Some(links()), Arc::new(log))
        .await
        .unwrap();
    let v = second.durable_get("research").await.unwrap();
    assert_eq!((v.state.as_str(), v.generation), ("running", 1), "{v:?}");
    second.stop();
    host.kill();
}

/// Jobs that fit a free host are placed even behind thousands queued for
/// a full one (no head-of-line blocking): a shape that found no host is
/// passed over for the rest of the round.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn jobs_for_a_free_host_are_not_stuck_behind_a_full_one() {
    let root = tempfile::tempdir().unwrap();
    let mut busy = Host::new("busy", root.path(), FakeExec::new(), 1).await;
    let mut free = Host::new("free", root.path(), FakeExec::new(), 4).await;
    let coord = Coordinator::start(config(&root.path().join("c")), Some(links()))
        .await
        .unwrap();
    coord.add_node(busy.row()).await.unwrap();
    coord.add_node(free.row()).await.unwrap();
    wait_for("both ready", Duration::from_secs(10), async || {
        (coord.nodes().iter().filter(|n| n.health == "ready").count() == 2).then_some(())
    })
    .await;
    let backlog: Vec<NewJob> = (0..3000)
        .map(|i| {
            let mut j = job(i, vec!["sleep".into(), "20000".into()], 5);
            j.spec.on = "busy".into();
            j
        })
        .collect();
    coord.submit("backlog", 0, backlog).await.unwrap();
    let quick: Vec<NewJob> = (0..4)
        .map(|i| {
            let mut j = job(10_000 + i, vec!["true".into()], 5);
            j.spec.on = "free".into();
            j
        })
        .collect();
    coord.submit("quick", 0, quick).await.unwrap();
    wait_for(
        "the free host's jobs",
        Duration::from_secs(10),
        async || {
            let rows = coord
                .store()
                .call(|c| store::jobs(c, "quick", "", 0, 10, false))
                .await
                .unwrap();
            (rows.iter().filter(|r| r.state == "succeeded").count() == 4).then_some(())
        },
    )
    .await;
    coord.stop();
    busy.kill();
    free.kill();
}

/// `auto` work (it names no machine) on a pool of only this machine, which
/// does not provide Spaces ("pinned": it takes the work that names it),
/// runs here instead of waiting for ever; with a machine that takes any
/// work, it goes there.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn auto_work_runs_on_a_lone_machine_that_does_not_provide_spaces() {
    let root = tempfile::tempdir().unwrap();
    let mut here = Host::new("local", root.path(), FakeExec::new(), 2).await;
    let coord = Coordinator::start(config(&root.path().join("c")), Some(links()))
        .await
        .unwrap();
    coord
        .add_node(NodeRow {
            state: "pinned".into(),
            ..here.row()
        })
        .await
        .unwrap();
    wait_for("up", Duration::from_secs(10), async || {
        coord
            .nodes()
            .iter()
            .any(|n| n.health == "own runs only")
            .then_some(())
    })
    .await;
    coord
        .submit("auto", 0, vec![job(0, vec!["true".into()], 5)])
        .await
        .unwrap();
    wait_for("the auto job", Duration::from_secs(10), async || {
        let rows = coord
            .store()
            .call(|c| store::jobs(c, "auto", "", 0, 10, false))
            .await
            .unwrap();
        rows.iter()
            .find(|r| r.state == "succeeded")
            .map(|r| r.node.clone())
    })
    .await;
    // Another machine that takes any work: unnamed work goes there.
    let mut other = Host::new("other", root.path(), FakeExec::new(), 2).await;
    coord.add_node(other.row()).await.unwrap();
    wait_for("the other ready", Duration::from_secs(10), async || {
        (coord.nodes().iter().filter(|n| n.health == "ready").count() == 1).then_some(())
    })
    .await;
    coord
        .submit("later", 0, vec![job(1, vec!["true".into()], 5)])
        .await
        .unwrap();
    let node = wait_for("the later job", Duration::from_secs(10), async || {
        let rows = coord
            .store()
            .call(|c| store::jobs(c, "later", "", 0, 10, false))
            .await
            .unwrap();
        rows.iter()
            .find(|r| r.state == "succeeded")
            .map(|r| r.node.clone())
    })
    .await;
    assert_eq!(node, "other");
    coord.stop();
    here.kill();
    other.kill();
}
