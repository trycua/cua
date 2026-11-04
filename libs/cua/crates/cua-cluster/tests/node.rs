//! The host's job runner: idempotent re-delivery, attempt fencing,
//! accept/reject from its own capacity, the change feed, and recovery
//! after the runner restarts mid-job.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{FakeExec, spec};
use cua_cluster::node::{NodeConfig, NodeJobs};

fn config(dir: &std::path::Path, cpus: u32) -> NodeConfig {
    NodeConfig {
        dir: dir.to_path_buf(),
        millicpus_total: cpus * 1000,
        memory_total_mb: 64 * 1024,
        jobs_max: 0,
        min_available_mb: 0,
        retention: Duration::from_secs(3600),
        output_retention: Duration::from_secs(3600),
        boot_id: "boot".into(),
    }
}

async fn wait_state(jobs: &NodeJobs, id: &str, state: &str) -> cua_cluster::ledger::JobRecord {
    for _ in 0..500 {
        if let Some(r) = jobs.get(id)
            && r.state().as_str() == state
        {
            return r;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("{id} never became {state}: {:?}", jobs.get(id));
}

/// Delivering the same job and attempt again returns its record; it runs
/// once.
#[tokio::test]
async fn a_redelivered_job_runs_once() {
    let dir = tempfile::tempdir().unwrap();
    let exec = FakeExec::new();
    let jobs = NodeJobs::open(config(dir.path(), 4), Arc::new(exec.clone())).unwrap();
    let a = jobs
        .run_batch(vec![spec("j1", 1, &["echo", "hi"])], "s")
        .await;
    assert_eq!(a[0].outcome, "accepted");
    let done = wait_state(&jobs, "j1", "succeeded").await;
    assert_eq!(done.stdout, b"hi\n");
    let again = jobs
        .run_batch(vec![spec("j1", 1, &["echo", "hi"])], "s")
        .await;
    assert_eq!(again[0].outcome, "duplicate");
    let rec = again[0].job.as_ref().unwrap();
    assert_eq!(
        (rec.state.as_str(), rec.stdout.as_slice()),
        ("succeeded", &b"hi\n"[..])
    );
    assert_eq!(exec.starts("j1"), 1);
}

/// A lower attempt is stale; a higher one cancels the running one and runs.
#[tokio::test]
async fn attempts_fence_older_deliveries() {
    let dir = tempfile::tempdir().unwrap();
    let exec = FakeExec::new();
    let jobs = NodeJobs::open(config(dir.path(), 4), Arc::new(exec.clone())).unwrap();
    assert_eq!(
        jobs.run_batch(vec![spec("j", 2, &["hang"])], "s").await[0].outcome,
        "accepted"
    );
    let stale = jobs.run_batch(vec![spec("j", 1, &["hang"])], "s").await;
    assert_eq!(stale[0].outcome, "stale");
    assert!(stale[0].reason.contains("older"), "{}", stale[0].reason);
    assert_eq!(
        jobs.run_batch(vec![spec("j", 3, &["echo", "x"])], "s")
            .await[0]
            .outcome,
        "accepted"
    );
    let r = wait_state(&jobs, "j", "succeeded").await;
    assert_eq!(r.attempt, 3);
    // The superseded attempt was canceled and recorded nothing.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(jobs.get("j").unwrap().attempt, 3);
    assert_eq!(exec.seen.lock().unwrap().live.len(), 0);
    assert_eq!(jobs.status().jobs_running, 0);
}

/// A cancel of a job never seen records a fence: that attempt, arriving
/// late, is refused; the next one runs.
#[tokio::test]
async fn a_cancel_before_delivery_fences_the_attempt() {
    let dir = tempfile::tempdir().unwrap();
    let exec = FakeExec::new();
    let jobs = NodeJobs::open(config(dir.path(), 4), Arc::new(exec.clone())).unwrap();
    let t = jobs.cancel("late", 1, "it ran elsewhere").await;
    assert_eq!(t.state().as_str(), "canceled");
    let late = jobs.run_batch(vec![spec("late", 1, &["echo"])], "s").await;
    assert_eq!(late[0].outcome, "stale");
    assert_eq!(exec.starts("late"), 0);
    assert_eq!(
        jobs.run_batch(vec![spec("late", 2, &["echo"])], "s").await[0].outcome,
        "accepted"
    );
    wait_state(&jobs, "late", "succeeded").await;
    // A cancel of a running job stops it.
    jobs.run_batch(vec![spec("run", 1, &["hang"])], "s").await;
    wait_state(&jobs, "run", "running").await;
    jobs.cancel("run", 0, "owner").await;
    let c = wait_state(&jobs, "run", "canceled").await;
    assert_eq!(c.error, "owner");
}

/// A job that does not fit what is free is rejected, not queued.
#[tokio::test]
async fn jobs_beyond_capacity_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let jobs = NodeJobs::open(config(dir.path(), 2), Arc::new(FakeExec::new())).unwrap();
    let acks = jobs
        .run_batch(
            vec![
                spec("a", 1, &["hang"]),
                spec("b", 1, &["hang"]),
                spec("c", 1, &["hang"]),
            ],
            "s",
        )
        .await;
    let outcomes: Vec<&str> = acks.iter().map(|a| a.outcome.as_str()).collect();
    assert_eq!(outcomes, ["accepted", "accepted", "rejected"]);
    assert!(acks[2].reason.contains("CPU"), "{}", acks[2].reason);
    let st = jobs.status();
    assert_eq!((st.millicpus_free, st.jobs_running), (0, 2));
    let mut microvm = spec("m", 1, &["echo"]);
    microvm.kind = "microvm".into();
    let r = jobs.run_batch(vec![microvm], "s").await;
    assert!(
        r[0].reason.contains("does not create microvm"),
        "{}",
        r[0].reason
    );
    jobs.set_draining(true);
    jobs.cancel("a", 0, "").await;
    wait_state(&jobs, "a", "canceled").await;
    let r = jobs.run_batch(vec![spec("d", 1, &["echo"])], "s").await;
    assert!(r[0].reason.contains("draining"), "{}", r[0].reason);
}

/// Overcommit multiplies what jobs may reserve; the available-memory floor
/// still refuses work when the machine itself runs low.
#[tokio::test]
async fn overcommit_reserves_more_but_keeps_a_memory_floor() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(dir.path(), 2);
    cfg.apply_overcommit(1.0);
    assert_eq!((cfg.millicpus_total, cfg.min_available_mb), (2000, 0));
    cfg.apply_overcommit(2.0);
    assert_eq!(
        (cfg.millicpus_total, cfg.memory_total_mb),
        (4000, 128 * 1024)
    );
    assert!(cfg.min_available_mb >= 1024);
    let jobs = NodeJobs::open(cfg, Arc::new(FakeExec::new())).unwrap();
    let acks = jobs
        .run_batch(
            (0..5)
                .map(|i| spec(&format!("o{i}"), 1, &["hang"]))
                .collect(),
            "s",
        )
        .await;
    let accepted = acks.iter().filter(|a| a.outcome == "accepted").count();
    // Four 1-CPU jobs on two CPUs at 2x; the fifth does not fit.
    assert_eq!(accepted, 4, "{acks:?}");

    // A floor above what any machine has: everything is refused, saying why.
    let dir2 = tempfile::tempdir().unwrap();
    let mut low = config(dir2.path(), 2);
    low.min_available_mb = u64::MAX;
    let jobs = NodeJobs::open(low, Arc::new(FakeExec::new())).unwrap();
    let r = jobs.run_batch(vec![spec("x", 1, &["echo"])], "s").await;
    if cua_cluster::sysinfo::available_memory_mb().is_some() {
        assert_eq!(r[0].outcome, "rejected");
        assert!(r[0].reason.contains("low on memory"), "{}", r[0].reason);
    }
}

/// Jobs accepted moments ago count against available memory before their
/// Spaces have allocated it: a burst cannot overshoot the floor.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_burst_counts_memory_its_spaces_have_not_taken_yet() {
    let dir = tempfile::tempdir().unwrap();
    let phys = cua_cluster::sysinfo::total_memory_mb().unwrap();
    let avail = cua_cluster::sysinfo::available_memory_mb().unwrap();
    let mut cfg = config(dir.path(), 64);
    // Reservations overcommitted 2x: a 1024 MiB job is expected to use 512.
    cfg.memory_total_mb = phys * 2;
    cfg.min_available_mb = avail.saturating_sub(3 * 512 + 256);
    let jobs = NodeJobs::open(cfg, Arc::new(FakeExec::new())).unwrap();
    let specs = (0..5)
        .map(|i| {
            let mut s = spec(&format!("b{i}"), 1, &["hang"]);
            s.memory_mb = 1024;
            s
        })
        .collect();
    let acks = jobs.run_batch(specs, "s").await;
    let outcomes: Vec<&str> = acks.iter().map(|a| a.outcome.as_str()).collect();
    assert_eq!(&outcomes[..3], ["accepted"; 3], "{acks:?}");
    assert_eq!(outcomes[3], "rejected", "{acks:?}");
    assert!(
        acks[3].reason.contains("still starting"),
        "{}",
        acks[3].reason
    );
    // The host reports itself full for a 1 GiB job, so a scheduler stops
    // sending work it would refuse.
    let free = jobs.status().memory_free_mb;
    assert!(free < 1024, "{free}");
}

/// The feed returns what changed after a position, and a long poll wakes
/// on the next change.
#[tokio::test]
async fn the_feed_long_polls_for_changes() {
    let dir = tempfile::tempdir().unwrap();
    let jobs = NodeJobs::open(config(dir.path(), 4), Arc::new(FakeExec::new())).unwrap();
    let first = jobs.list(0, 0, Duration::ZERO, true).await;
    assert!(first.jobs.is_empty());
    let pos = first.next_seq;
    let j2 = jobs.clone();
    let waiter = tokio::spawn(async move { j2.list(pos, 0, Duration::from_secs(10), true).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    jobs.run_batch(vec![spec("f", 1, &["sleep", "100"])], "s")
        .await;
    let got = tokio::time::timeout(Duration::from_secs(2), waiter)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.jobs[0].state, "running");
    let mut pos = got.next_seq;
    let done = loop {
        let r = jobs.list(pos, 0, Duration::from_secs(5), false).await;
        pos = r.next_seq;
        if let Some(j) = r.jobs.iter().find(|j| j.state == "succeeded") {
            break j.clone();
        }
    };
    assert_eq!(done.job_id, "f");
    assert!(done.seq > got.jobs[0].seq);
    // Nothing new: a short wait answers empty with the heartbeat.
    let idle = jobs.list(pos, 0, Duration::from_millis(50), false).await;
    assert!(idle.jobs.is_empty());
    assert!(idle.node.unwrap().host_time_ms > 0);
}

/// A scheduler cursor ahead of the ledger (numbering that once went
/// backwards) is served from the start with the real position, so the
/// scheduler can move its cursor back.
#[tokio::test]
async fn a_cursor_ahead_of_the_ledger_is_reset() {
    let dir = tempfile::tempdir().unwrap();
    let jobs = NodeJobs::open(config(dir.path(), 4), Arc::new(FakeExec::new())).unwrap();
    jobs.run_batch(vec![spec("g", 1, &["sleep", "100"])], "s")
        .await;
    let real = jobs.list(0, 0, Duration::ZERO, false).await;
    assert!(!real.jobs.is_empty());
    let stale = jobs
        .list(real.next_seq + 20, 0, Duration::ZERO, false)
        .await;
    assert!(!stale.jobs.is_empty(), "served from the start");
    assert_eq!(
        stale.next_seq, real.next_seq,
        "and says where the numbering is"
    );
}

/// The runner stops mid-job (the daemon was killed): opening it again ends
/// the job with `error`, removes its Space, keeps finished jobs and their
/// fences, and counts the restart in its epoch.
#[tokio::test]
async fn a_restart_mid_job_ends_it_with_error() {
    let dir = tempfile::tempdir().unwrap();
    let exec = FakeExec::new();
    let (d, e) = (dir.path().to_path_buf(), exec.clone());
    // The "process" runs on its own runtime, off this async thread.
    let ledger_id = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let id = rt.block_on(async move {
            let jobs = NodeJobs::open(config(&d, 4), Arc::new(e)).unwrap();
            jobs.run_batch(vec![spec("done", 1, &["echo", "ok"])], "s")
                .await;
            wait_state(&jobs, "done", "succeeded").await;
            jobs.run_batch(vec![spec("mid", 1, &["hang"])], "s").await;
            jobs.cancel("fenced", 4, "x").await;
            jobs.status().ledger_id
        });
        // The process dies: every task stops.
        rt.shutdown_background();
        id
    })
    .join()
    .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let jobs = NodeJobs::open(config(dir.path(), 4), Arc::new(exec.clone())).unwrap();
    let st = jobs.status();
    assert_eq!((st.epoch, st.ledger_id.as_str()), (2, ledger_id.as_str()));
    let mid = wait_state(&jobs, "mid", "error").await;
    assert!(mid.error.contains("restarted"), "{}", mid.error);
    assert_eq!(
        exec.seen.lock().unwrap().removed,
        vec![cua_cluster::model::space_name("mid", 1)]
    );
    assert_eq!(jobs.get("done").unwrap().stdout, b"ok\n");
    let late = jobs
        .run_batch(vec![spec("fenced", 3, &["echo"])], "s")
        .await;
    assert_eq!(late[0].outcome, "stale");
    // Redelivering the crashed attempt reports its error instead of running
    // it again; a new attempt runs.
    let again = jobs.run_batch(vec![spec("mid", 1, &["echo"])], "s").await;
    assert_eq!(again[0].outcome, "duplicate");
    assert_eq!(
        jobs.run_batch(vec![spec("mid", 2, &["echo"])], "s").await[0].outcome,
        "accepted"
    );
    wait_state(&jobs, "mid", "succeeded").await;
    assert_eq!(st.jobs_running, 0);
}

/// Job failures keep their kind: a non-zero exit is `failed`, a Space that
/// does not start is `error`.
#[tokio::test]
async fn failures_keep_their_kind() {
    let dir = tempfile::tempdir().unwrap();
    let jobs = NodeJobs::open(config(dir.path(), 4), Arc::new(FakeExec::new())).unwrap();
    jobs.run_batch(
        vec![spec("x", 1, &["exit", "3"]), spec("n", 1, &["nospace"])],
        "s",
    )
    .await;
    let x = wait_state(&jobs, "x", "failed").await;
    assert_eq!(x.exit_code, Some(3));
    let n = wait_state(&jobs, "n", "error").await;
    assert!(n.error.contains("did not start"));
}

/// A runtime installed while the host runs (an external runtime provider)
/// is advertised and admitted without reopening the runner.
#[tokio::test]
async fn a_runtime_installed_while_running_is_offered() {
    let dir = tempfile::tempdir().unwrap();
    let exec = FakeExec::new();
    let jobs = NodeJobs::open(config(dir.path(), 4), Arc::new(exec.clone())).unwrap();
    let mut job = spec("ext", 1, &["echo", "hi"]);
    job.runtime = "acme".into();
    let refused = jobs.run_batch(vec![job.clone()], "s").await;
    assert_eq!(refused[0].outcome, "rejected");
    assert!(
        refused[0]
            .reason
            .contains("does not offer the acme runtime"),
        "{}",
        refused[0].reason
    );
    assert!(!jobs.status().runtimes.contains(&"acme".to_string()));

    exec.runtimes.lock().unwrap().push("acme".into());
    assert!(jobs.status().runtimes.contains(&"acme".to_string()));
    job.attempt = 2;
    assert_eq!(jobs.run_batch(vec![job], "s").await[0].outcome, "accepted");
    wait_state(&jobs, "ext", "succeeded").await;
}

/// Two accounts that share a host: each sees, cancels and fences only its
/// own jobs, even when they pick the same job ids.
#[tokio::test]
async fn accounts_on_one_host_never_see_or_touch_each_others_jobs() {
    let dir = tempfile::tempdir().unwrap();
    let exec = FakeExec::new();
    let jobs = NodeJobs::open(config(dir.path(), 4), Arc::new(exec.clone())).unwrap();
    let a = jobs
        .run_batch_for("ada", vec![spec("p-1", 1, &["sleep", "300"])], "pool")
        .await;
    assert_eq!(a[0].outcome, "accepted");
    assert_eq!(a[0].job_id, "p-1", "ids come back as the caller sent them");
    // Bob uses the same id: a different job, not a duplicate or a fence.
    let b = jobs
        .run_batch_for("bob", vec![spec("p-1", 1, &["echo", "bob"])], "pool")
        .await;
    assert_eq!(b[0].outcome, "accepted");
    for _ in 0..500 {
        if jobs.get_for("bob", "p-1", true).unwrap().state == "succeeded" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(jobs.get_for("bob", "p-1", true).unwrap().stdout, b"bob\n");
    // Bob cannot cancel (or fence with a higher attempt) Ada's job.
    jobs.cancel_for("bob", "p-1", 9, "mine").await;
    assert_eq!(
        jobs.get_for("ada", "p-1", false).unwrap().state,
        "running",
        "Ada's job runs on"
    );
    // Each one's feed lists only its own jobs.
    let feed = jobs
        .list_for("bob", 0, 100, Duration::from_secs(2), true)
        .await;
    assert!(feed.jobs.iter().all(|j| j.job_id == "p-1"));

    let all = jobs.list(0, 100, Duration::ZERO, false).await;
    assert!(all.jobs.iter().any(|j| j.job_id == "ada/p-1"));
    assert!(all.jobs.iter().any(|j| j.job_id == "bob/p-1"));
    let ada_feed = jobs.list_for("ada", 0, 100, Duration::ZERO, false).await;
    assert_eq!(ada_feed.jobs.len(), 1);
    assert!(jobs.get_for("carol", "p-1", false).is_none());
    jobs.cancel_for("ada", "p-1", 1, "done").await;
    for _ in 0..500 {
        if jobs.get_for("ada", "p-1", false).unwrap().state == "canceled" {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("Ada could not cancel her own job");
}

/// Reclaiming comes before refusing: capacity held by what nobody uses
/// (idle warm members, expired Spaces) is freed, then the job is judged.
#[tokio::test]
async fn idle_capacity_is_reclaimed_before_a_job_is_refused() {
    use cua_cluster::node::External;
    use std::sync::atomic::{AtomicU32, Ordering::SeqCst};

    let dir = tempfile::tempdir().unwrap();
    let jobs = NodeJobs::open(config(dir.path(), 2), Arc::new(FakeExec::new())).unwrap();
    // Two CPUs held outside the runner, by Spaces nobody uses.
    let held = Arc::new(AtomicU32::new(2000));
    let h = held.clone();
    jobs.set_external(move || External {
        millicpus: h.load(SeqCst),
        memory_mb: 0,
        running: Default::default(),
    });
    // Nothing can be reclaimed yet: refused for CPU, as before.
    let acks = jobs.run_batch(vec![spec("a", 1, &["echo"])], "s").await;
    assert_eq!(acks[0].outcome, "rejected");
    assert!(acks[0].reason.contains("CPU"), "{}", acks[0].reason);

    let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
    let (h, a) = (held.clone(), asked.clone());
    jobs.set_reclaim(move |millicpus, memory_mb| {
        let (h, a) = (h.clone(), a.clone());
        async move {
            a.lock().unwrap().push((millicpus, memory_mb));
            h.store(0, SeqCst);
        }
    });
    let acks = jobs.run_batch(vec![spec("b", 1, &["echo"])], "s").await;
    assert_eq!(acks[0].outcome, "accepted", "{}", acks[0].reason);
    assert_eq!(*asked.lock().unwrap(), [(1000, 0)], "short by one CPU");

    // Room to spare: nothing is reclaimed.
    held.store(0, SeqCst);
    asked.lock().unwrap().clear();
    let acks = jobs.run_batch(vec![spec("c", 1, &["echo"])], "s").await;
    assert_eq!(acks[0].outcome, "accepted");
    assert!(asked.lock().unwrap().is_empty());

    // A reclaim that frees nothing leaves the refusal in place.
    held.store(2000, SeqCst);
    jobs.set_reclaim(|_, _| async {});
    let acks = jobs.run_batch(vec![spec("d", 1, &["hang"])], "s").await;
    assert_eq!(acks[0].outcome, "rejected");
}
