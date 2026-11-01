// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A dedicated host's own Space (`kind = "machine"`) through the real job
//! runner ([`cua_cluster::NodeJobs`]), the machine executor
//! ([`cua_cluster::machine::MachineExecutor`]) and a real cua-spacesd (in
//! process): a member runs with its bundle, its run's data directory and
//! live output; a cancel or a timeout stops its whole process group; the
//! start barrier, progress (heartbeats, else output) and the self-fence
//! work through the member's work directory; a peer router rewrites a gang
//! member's peers; the removed native kind is refused.

#![cfg(unix)]

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use cua_cluster::machine::{HostSpacesd, MachineExecutor};
use cua_cluster::node::{NodeConfig, NodeJobs};
use cua_cluster::peers::{GangPeers, MemberNet, PeerRouter, RoutedPeers};
use cua_proto::env::v1 as pb;

fn config(dir: &std::path::Path) -> NodeConfig {
    NodeConfig {
        dir: dir.join("host/jobs"),
        millicpus_total: 4000,
        memory_total_mb: 8192,
        jobs_max: 0,
        min_available_mb: 0,
        retention: Duration::from_secs(3600),
        output_retention: Duration::from_secs(3600),
        boot_id: "b".into(),
    }
}

/// A member of run `run-1`.
fn member(id: &str, attempt: u32, script: &str) -> pb::HostJobSpec {
    member_of("run-1", id, attempt, script)
}

/// A member of `run`. Each test uses its own run: tests run side by side
/// in one process, and as root each run's user ends when the last of its
/// members on that executor does (killing what still runs as that user).
fn member_of(run: &str, id: &str, attempt: u32, script: &str) -> pb::HostJobSpec {
    pb::HostJobSpec {
        job_id: id.into(),
        attempt,
        kind: "machine".into(),
        argv: vec!["sh".into(), "-c".into(), script.into()],
        millicpus: 1000,
        memory_mb: 256,
        timeout_secs: 60,
        run_id: run.into(),
        max_output_bytes: 1 << 20,
        ..Default::default()
    }
}

async fn wait_end(jobs: &NodeJobs, id: &str, attempt: u32) -> cua_cluster::ledger::JobRecord {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(r) = jobs.get(id) {
            if r.attempt == attempt && r.state().is_terminal() {
                return r;
            }
        }
        assert!(Instant::now() < deadline, "job {id} did not end");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// One test's host at a time. As root on Linux every executor keeps its
/// members under the one shared root (`/var/lib/cua-jobs`), and a host's
/// sweep removes attempts its own ledger does not know, so two tests' hosts
/// side by side would remove each other's members. A real computer runs one.
async fn one_host_at_a_time() -> tokio::sync::MutexGuard<'static, ()> {
    static HOST: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    HOST.lock().await
}

fn machine(home: &std::path::Path, t: &common::Target) -> MachineExecutor {
    let at = HostSpacesd {
        url: t.url.clone(),
        token: t.token.clone(),
    };
    let exec = MachineExecutor::new(home, move || Ok(at.clone()));
    common::seed_uv(exec.dirs());
    exec
}

fn alive(pid: i32) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        // SAFETY: signal 0 only checks the pid.
        let r = unsafe { libc::kill(pid, 0) };
        if r != 0 {
            return false;
        }
        if Instant::now() > deadline {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_member_runs_with_its_bundle_data_dir_and_live_output() {
    let _host = one_host_at_a_time().await;
    let t = common::target().await;
    let home = tempfile::tempdir().unwrap();
    let exec = machine(home.path(), &t);
    let dirs = exec.dirs().clone();
    let jobs = NodeJobs::open(config(home.path()), Arc::new(exec)).unwrap();
    let st = jobs.status();
    assert!(st.dedicated && st.kinds == ["machine"], "{st:?}");

    let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::fast(),
    ));
    let script = b"echo from-bundle; echo to-stderr 1>&2\n";
    let mut h = tar::Header::new_gnu();
    h.set_size(script.len() as u64);
    h.set_mode(0o644);
    h.set_cksum();
    tar.append_data(&mut h, "app/hello.sh", &script[..])
        .unwrap();
    let bundle = tar.into_inner().unwrap().finish().unwrap();

    let mut spec = member(
        "j1",
        1,
        "sh app/hello.sh; echo rank=$CUA_RANK world=$CUA_WORLD_SIZE run=$CUA_RUN_ID \
         attempt=$CUA_ATTEMPT kind=$CUA_SPACE_KIND; uv; echo cache=$UV_CACHE_DIR; \
         echo step-1 > \"$CUA_JOB_DATA/ckpt\"; echo result > \"$CUA_RESULT_PATH\"; sleep 1; \
         echo done",
    );
    spec.bundle = bundle;
    spec.result_path = "out.bin".into();
    let acks = jobs.run_batch(vec![spec], "t").await;
    assert_eq!(acks[0].outcome, "accepted", "{:?}", acks[0].reason);
    // Live output while it runs.
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let (chunk, size) = jobs.output("j1", 0).unwrap_or_default();
        if String::from_utf8_lossy(&chunk).contains("rank=0") {
            assert!(size >= chunk.len() as u64);
            assert_eq!(jobs.get("j1").unwrap().state().as_str(), "running");
            break;
        }
        assert!(Instant::now() < deadline, "no live output");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let r = wait_end(&jobs, "j1", 1).await;
    assert_eq!(r.state().as_str(), "succeeded", "{r:?}");
    let out = String::from_utf8_lossy(&r.stdout).into_owned();
    assert!(
        out.contains("from-bundle") && out.contains("to-stderr"),
        "{out}"
    );
    assert!(
        out.contains("rank=0 world=1 run=run-1 attempt=1 kind=machine"),
        "{out}"
    );
    // uv is first on PATH; the caches are the host's.
    assert!(out.contains("uv stand-in"), "{out}");
    assert!(
        out.contains(&format!("cache={}", dirs.cache_dir().join("uv").display())),
        "{out}"
    );
    assert_eq!(r.result, b"result\n");
    // Reading from an offset returns the rest.
    let (all, size) = jobs.output("j1", 0).unwrap();
    let (rest, _) = jobs.output("j1", 5).unwrap();
    assert_eq!(&all[5..], &rest[..]);
    assert_eq!(size as usize, all.len());
    // The work directory is gone with the attempt; the run's data stays,
    // and the next member of the run sees it.
    assert!(!dirs.work_dir("j1", 1).exists());
    let ckpt = dirs.run_data_dir("run-1").join("ckpt");
    assert_eq!(std::fs::read_to_string(&ckpt).unwrap(), "step-1\n");
    let acks = jobs
        .run_batch(vec![member("j2", 1, "cat \"$CUA_JOB_DATA/ckpt\"")], "t")
        .await;
    assert_eq!(acks[0].outcome, "accepted");
    let r = wait_end(&jobs, "j2", 1).await;
    assert_eq!(String::from_utf8_lossy(&r.stdout), "step-1\n");

    // The removed native kind is refused, saying why.
    let mut native = member("n", 1, "true");
    native.kind = "process".into();
    let acks = jobs.run_batch(vec![native], "t").await;
    assert_eq!(acks[0].outcome, "rejected");
    assert!(
        acks[0].reason.contains("every job runs in a Space"),
        "{}",
        acks[0].reason
    );
}

/// A cancel or a timeout stops the whole process group: a child the
/// command started in the background goes too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_and_timeout_stop_the_whole_group() {
    let _host = one_host_at_a_time().await;
    let t = common::target().await;
    let home = tempfile::tempdir().unwrap();
    let exec = machine(home.path(), &t);
    let dirs = exec.dirs().clone();
    let jobs = NodeJobs::open(config(home.path()), Arc::new(exec)).unwrap();
    let script = "sleep 300 & echo $! > \"$CUA_JOB_DATA/child-$CUA_JOB_ID.pid\"; wait";
    // The run's data outlives a test run on the shared root: no old pid.
    let pidfile = dirs.run_data_dir("run-cancel").join("child-c.pid");
    let _ = std::fs::remove_file(&pidfile);
    let acks = jobs
        .run_batch(vec![member_of("run-cancel", "c", 1, script)], "t")
        .await;
    assert_eq!(acks[0].outcome, "accepted");

    let deadline = Instant::now() + Duration::from_secs(20);
    while std::fs::read_to_string(&pidfile).map_or(true, |s| s.trim().is_empty()) {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let child: i32 = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(alive(child));
    jobs.cancel("c", 1, "test").await;
    let r = wait_end(&jobs, "c", 1).await;
    assert_eq!(r.state().as_str(), "canceled");
    assert!(!alive(child), "the background child outlived the cancel");

    let mut spec = member_of("run-cancel", "t", 1, script);
    spec.timeout_secs = 1;
    let acks = jobs.run_batch(vec![spec], "t").await;
    assert_eq!(acks[0].outcome, "accepted");
    let r = wait_end(&jobs, "t", 1).await;
    assert_eq!(r.state().as_str(), "failed");
    assert!(r.timed_out, "{r:?}");
    let child: i32 = std::fs::read_to_string(dirs.run_data_dir("run-cancel").join("child-t.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(!alive(child));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_non_zero_exit_fails_and_a_missing_program_is_127() {
    let _host = one_host_at_a_time().await;
    let t = common::target().await;
    let home = tempfile::tempdir().unwrap();
    let jobs = NodeJobs::open(config(home.path()), Arc::new(machine(home.path(), &t))).unwrap();
    jobs.run_batch(
        vec![member_of("run-exit", "f", 1, "echo oops; exit 3")],
        "t",
    )
    .await;
    let r = wait_end(&jobs, "f", 1).await;
    assert_eq!((r.state().as_str(), r.exit_code), ("failed", Some(3)));
    let mut spec = member_of("run-exit", "m", 1, "");
    spec.argv = vec!["/nonexistent/program".into()];
    jobs.run_batch(vec![spec], "t").await;
    let r = wait_end(&jobs, "m", 1).await;
    assert_eq!(r.state().as_str(), "failed", "{r:?}");
    assert_eq!(r.exit_code, Some(127), "{r:?}");
}

/// A member that asks for host services (`cua run --compat e2b` on a
/// dedicated cloud host) runs with what they give it (their environment,
/// with `{work}` expanded, and their files in its work directory), and is
/// refused before it starts when nothing started them: never run without
/// its credentials.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_member_with_services_gets_their_env_and_files_or_never_runs() {
    use cua_cluster::machine::MemberExtras;
    use cua_cluster::node::{ExecEnd, JobExecutor};
    let _host = one_host_at_a_time().await;
    let t = common::target().await;
    let home = tempfile::tempdir().unwrap();
    let exec = machine(home.path(), &t);
    let marker = home.path().join("ran");
    let mut spec = member_of(
        "run-services",
        "s1",
        1,
        &format!(
            "touch {m}; echo key=$E2B_API_KEY; echo cert=$SSL_CERT_FILE; cat \"$SSL_CERT_FILE\"",
            m = marker.display()
        ),
    );
    spec.services = vec!["compat:e2b".into()];
    let job = cua_cluster::JobSpec::from_pb(&spec);

    // Nothing started its services: refused, and its command never ran.
    let out = exec
        .run(&job, "cj-s1-1", cua_cluster::CancellationToken::new())
        .await;
    match &out.end {
        ExecEnd::Error(e) => assert!(e.contains("compat:e2b"), "{e}"),
        other => panic!("ran without its services: {other:?}"),
    }
    assert!(!marker.exists(), "the member ran without its services");

    // With them: their environment and files.
    let mut extras = MemberExtras::default();
    extras.env.insert("E2B_API_KEY".into(), "e2b_test".into());
    extras
        .env
        .insert("SSL_CERT_FILE".into(), "{work}/.cua-services/ca.pem".into());
    extras
        .files
        .push((".cua-services/ca.pem".into(), b"PEM-BYTES".to_vec()));
    let mut spec2 = spec.clone();
    spec2.attempt = 2;
    let job = cua_cluster::JobSpec::from_pb(&spec2);
    let out = exec
        .run_with(
            &job,
            "cj-s1-2",
            cua_cluster::CancellationToken::new(),
            Some(extras),
        )
        .await;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        matches!(out.end, ExecEnd::Exited(0)),
        "{:?} {text}",
        out.end
    );
    assert!(text.contains("key=e2b_test"), "{text}");
    assert!(text.contains("PEM-BYTES"), "{text}");
    assert!(
        text.contains("/.cua-services/ca.pem") && !text.contains("{work}"),
        "{text}"
    );

    // A service that names a path outside the work directory is refused.
    let mut bad = MemberExtras::default();
    bad.files.push(("../escape".into(), vec![]));
    let mut spec3 = spec.clone();
    spec3.attempt = 3;
    let out = exec
        .run_with(
            &cua_cluster::JobSpec::from_pb(&spec3),
            "cj-s1-3",
            cua_cluster::CancellationToken::new(),
            Some(bad),
        )
        .await;
    assert!(matches!(out.end, ExecEnd::Error(_)), "{:?}", out.end);
}

/// A host that routes gang peers (a mesh forward) rewrites `CUA_PEERS`
/// before the member starts; the coordinator's list stays visible.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_peer_router_rewrites_a_gang_members_peers() {
    let _host = one_host_at_a_time().await;
    /// Counts the member's forwards still held.
    struct Held(Arc<std::sync::atomic::AtomicUsize>);
    impl Drop for Held {
        fn drop(&mut self) {
            self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
    type Seen = std::sync::Mutex<Vec<(GangPeers, MemberNet)>>;
    struct Loopback(Seen, Arc<std::sync::atomic::AtomicUsize>);
    #[async_trait::async_trait]
    impl PeerRouter for Loopback {
        async fn route(
            &self,
            g: &GangPeers,
            net: MemberNet,
        ) -> Result<Option<RoutedPeers>, String> {
            self.0.lock().unwrap().push((g.clone(), net));
            self.1.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(Some(RoutedPeers {
                peers: (0..g.peers.len())
                    .map(|r| format!("127.0.0.1:{}", 40000 + r))
                    .collect(),
                notes: vec!["peer rank 0 via mesh (direct)".into()],
                hold: Some(Box::new(Held(self.1.clone()))),
            }))
        }
    }
    let t = common::target().await;
    let home = tempfile::tempdir().unwrap();
    let router = Arc::new(Loopback(Default::default(), Default::default()));
    let exec = machine(home.path(), &t).with_peer_router(router.clone());
    let jobs = NodeJobs::open(config(home.path()), Arc::new(exec)).unwrap();
    let mut spec = member_of("run-peers", "g", 1, "echo \"$CUA_PEERS|$CUA_PEERS_PLACED\"");
    for (k, v) in [
        ("CUA_RUN_ID", "run-1"),
        ("CUA_ATTEMPT", "2"),
        ("CUA_RANK", "1"),
        ("CUA_PEERS", "10.0.0.1:47000,10.0.0.2:47000"),
        ("CUA_PEER_MACHINES", "m-a,m-b"),
    ] {
        spec.env.insert(k.into(), v.into());
    }
    jobs.run_batch(vec![spec], "t").await;
    let r = wait_end(&jobs, "g", 1).await;
    assert_eq!(
        String::from_utf8_lossy(&r.stdout),
        "cua: peer rank 0 via mesh (direct)\n\
         127.0.0.1:40000,127.0.0.1:40001|10.0.0.1:47000,10.0.0.2:47000\n"
    );
    // What routed the member was held while it ran, and let go after.
    assert_eq!(router.1.load(std::sync::atomic::Ordering::SeqCst), 0);
    let seen = router.0.lock().unwrap().clone();
    assert_eq!(
        seen,
        [(
            GangPeers {
                run: "run-1".into(),
                attempt: 2,
                rank: 1,
                peers: vec!["10.0.0.1:47000".into(), "10.0.0.2:47000".into()],
                machines: vec!["m-a".into(), "m-b".into()],
                names: Vec::new(),
            },
            MemberNet::machine()
        )]
    );

    // Direct routing (PyTorch's dynamic ports) keeps the placed addresses
    // in the machine's own Space, without asking the router.
    let mut direct = member_of("run-peers", "d", 1, "echo \"$CUA_PEERS\"");
    for (k, v) in [
        ("CUA_RANK", "0"),
        ("CUA_PEERS", "10.0.0.1:47000,10.0.0.2:47000"),
        ("CUA_PEER_ROUTE", "direct"),
    ] {
        direct.env.insert(k.into(), v.into());
    }
    jobs.run_batch(vec![direct], "t").await;
    let r = wait_end(&jobs, "d", 1).await;
    assert_eq!(
        String::from_utf8_lossy(&r.stdout),
        "cua: peer rank 1 direct at 10.0.0.2:47000\n10.0.0.1:47000,10.0.0.2:47000\n"
    );
    assert_eq!(router.0.lock().unwrap().len(), 1);
}

/// The start barrier, progress and the self-fence, through the real runner
/// with real processes: a gang member reports ready and waits for `go`,
/// reports progress, and is stopped by its own host once its scheduler
/// stops calling the feed; a job that is not a gang member keeps running.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_gang_member_waits_at_its_barrier_reports_progress_and_is_fenced_when_cut_off() {
    let _host = one_host_at_a_time().await;
    let t = common::target().await;
    let home = tempfile::tempdir().unwrap();
    let jobs = NodeJobs::open(config(home.path()), Arc::new(machine(home.path(), &t))).unwrap();
    jobs.set_self_fence_after(Duration::from_secs(2));
    // Gone within the wait, a check and the kill grace after the last call.
    assert_eq!(jobs.status().self_fence_after_ms, 2_000 + 1_000 + 15_000);
    // The host advertises that it acts on `HostJobSpec.services`.
    assert!(jobs.status().job_services);
    let mut gm = member_of(
        "run-barrier",
        "m",
        1,
        r#"test -d "$CUA_BARRIER" || exit 9
echo r > "$CUA_BARRIER/.r" && mv "$CUA_BARRIER/.r" "$CUA_BARRIER/ready"
while [ ! -f "$CUA_BARRIER/go" ]; do sleep 0.05; done
echo released
echo '{"step": 7}' > "$CUA_HEARTBEAT.tmp" && mv "$CUA_HEARTBEAT.tmp" "$CUA_HEARTBEAT"
sleep 60"#,
    );
    for (k, v) in [
        ("CUA_RANK", "0"),
        ("CUA_WORLD_SIZE", "1"),
        ("CUA_PEERS", "127.0.0.1:47000"),
        ("CUA_BARRIER_TIMEOUT", "60"),
    ] {
        gm.env.insert(k.into(), v.into());
    }
    // A member that never heartbeats is as live as its output.
    let plain = member_of("run-barrier", "p", 1, "echo started; sleep 60");
    let acks = jobs.run_batch(vec![gm, plain], "t").await;
    assert!(acks.iter().all(|a| a.outcome == "accepted"), "{acks:?}");

    let progress = |jobs: Arc<NodeJobs>, id: &'static str| async move {
        jobs.list(0, 100, Duration::ZERO, false)
            .await
            .progress
            .into_iter()
            .find(|p| p.job_id == id)
    };
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if progress(jobs.clone(), "m").await.is_some_and(|p| p.ready) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the member never got ready: {:?}\n{}",
            jobs.get("m"),
            String::from_utf8_lossy(&jobs.output("m", 0).unwrap_or_default().0)
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let p = progress(jobs.clone(), "m").await.unwrap();
    assert!(
        p.ready && !p.released && p.last_progress_ms == 0 && p.step == -1,
        "{p:?}"
    );
    // Releasing an attempt that does not run there does nothing.
    jobs.release(
        None,
        &[pb::HostJobRef {
            job_id: "m".into(),
            attempt: 2,
        }],
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!progress(jobs.clone(), "m").await.unwrap().released);
    jobs.release(
        None,
        &[pb::HostJobRef {
            job_id: "m".into(),
            attempt: 1,
        }],
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    let p = loop {
        let p = progress(jobs.clone(), "m").await.unwrap();
        if p.step == 7 {
            break p;
        }
        assert!(
            Instant::now() < deadline,
            "no progress after the release: {p:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert!(p.released && p.last_progress_ms > 0, "{p:?}");
    let p = progress(jobs.clone(), "p").await.expect("output liveness");
    assert!(p.last_progress_ms > 0 && p.step == -1, "{p:?}");

    // Its scheduler goes silent: the host stops the gang member itself.
    let t0 = Instant::now();
    let r = wait_end(&jobs, "m", 1).await;
    assert_eq!(r.state(), cua_cluster::HostJobState::Canceled);
    assert!(r.error.starts_with("fenced: "), "{}", r.error);
    assert!(t0.elapsed() < Duration::from_secs(10), "{:?}", t0.elapsed());
    // The plain job is not a gang member: it still runs.
    let p = jobs.get("p").unwrap();
    assert_eq!(p.state(), cua_cluster::HostJobState::Running);
    jobs.cancel("p", 1, "done").await;
    wait_end(&jobs, "p", 1).await;
}
