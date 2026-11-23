//! Opt-in: a cluster job member in a real Space on this machine's container
//! engine (a gVisor container on a Linux host with Docker and runsc),
//! through the host's real job runner ([`cua_cluster::NodeJobs`]) and
//! executor ([`cua_spaces::host_jobs::SpacesExecutor`]):
//!
//! - a gang member (a world of one, with a start barrier) runs a Python job
//!   from its bundle through `uv` (mounted from the host, not in the
//!   image); its output streams into the host's log while it runs; it waits
//!   at its barrier until released, heartbeats, writes into the run's data
//!   directory on the host, and is stopped by its own host once its
//!   scheduler goes silent (self-fence): the command is killed and its
//!   Space deleted;
//! - a plain member writes its result file, which comes back from the
//!   mounted work directory; its Space is deleted after it.
//!
//! ```text
//! CUA_HOME=/tmp/some-throwaway-home CUA_JOB_SPACES_LIVE=1 \
//!   cargo test -p cua-daemon --test job_spaces_live -- --ignored --nocapture
//! ```
//!
//! Needs Docker with the `runsc` runtime, network for the image, `uv` and a
//! Python; a throwaway `CUA_HOME` (never `~/.cua`). Image
//! `CUA_JOB_SPACES_IMAGE` (default `ghcr.io/trycua/linux:24.04-slim`).

#![cfg(all(unix, feature = "spaces"))]

use std::sync::Arc;
use std::time::{Duration, Instant};

use cua_cluster::node::{NodeConfig, NodeJobs};
use cua_daemon::{Runtime, RuntimeConfig, local::VmmLocal};
use cua_proto::env::v1 as pb;

const JOB: &str = r#"
import json, os, pathlib, sys, time
rank = int(os.environ["CUA_RANK"])
print(f"python {sys.version.split()[0]} rank={rank} kind={os.environ['CUA_SPACE_KIND']} "
      f"peers={os.environ['CUA_PEERS']} data={os.environ['CUA_JOB_DATA']}", flush=True)
print("uname:", os.uname().release, flush=True)
barrier = pathlib.Path(os.environ["CUA_BARRIER"])
(barrier / "ready").write_text("ready")
print("ready; waiting at the barrier", flush=True)
while not (barrier / "go").exists():
    time.sleep(0.1)
print("released", flush=True)
data = pathlib.Path(os.environ["CUA_JOB_DATA"])
beat = pathlib.Path(os.environ["CUA_HEARTBEAT"])
for step in range(1, 1000):
    (data / "ckpt").write_text(str(step))
    tmp = beat.with_suffix(".tmp")
    tmp.write_text(json.dumps({"step": step}))
    tmp.replace(beat)
    print(f"step {step}", flush=True)
    time.sleep(0.5)
"#;

fn bundle(files: &[(&str, &str)]) -> Vec<u8> {
    let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::fast(),
    ));
    for (name, body) in files {
        let mut h = tar::Header::new_gnu();
        h.set_size(body.len() as u64);
        h.set_mode(0o644);
        h.set_cksum();
        tar.append_data(&mut h, name, body.as_bytes()).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap()
}

fn spec(id: &str, image: &str, argv: &[&str]) -> pb::HostJobSpec {
    pb::HostJobSpec {
        job_id: id.into(),
        attempt: 1,
        image: image.into(),
        kind: "container".into(),
        argv: argv.iter().map(|s| s.to_string()).collect(),
        millicpus: 1000,
        memory_mb: 2048,
        timeout_secs: 900,
        run_id: "live-run".into(),
        max_output_bytes: 1 << 20,
        ..Default::default()
    }
}

async fn wait_end(jobs: &NodeJobs, id: &str) -> cua_cluster::ledger::JobRecord {
    let deadline = Instant::now() + Duration::from_secs(900);
    loop {
        if let Some(r) = jobs.get(id)
            && r.state().is_terminal()
        {
            return r;
        }
        assert!(Instant::now() < deadline, "job {id} did not end");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

fn containers(prefix: &str) -> Vec<String> {
    let out = std::process::Command::new("docker")
        .args(["ps", "-a", "--format", "{{.Names}} {{.Image}}"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.starts_with(prefix))
        .map(str::to_string)
        .collect()
}

fn runtime_of(name: &str) -> String {
    let out = std::process::Command::new("docker")
        .args(["inspect", "-f", "{{.HostConfig.Runtime}}", name])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "runs gVisor container Spaces on this machine's Docker; set CUA_JOB_SPACES_LIVE=1 and a throwaway CUA_HOME"]
async fn a_member_runs_in_a_gvisor_space_with_live_output_barrier_heartbeat_and_self_fence() {
    if std::env::var("CUA_JOB_SPACES_LIVE").as_deref() != Ok("1") {
        eprintln!("skipped: set CUA_JOB_SPACES_LIVE=1");
        return;
    }
    let home = std::path::PathBuf::from(
        std::env::var_os("CUA_HOME").expect("set CUA_HOME to a throwaway home"),
    );
    let real = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cua"));
    assert!(Some(&home) != real.as_ref(), "never the user's ~/.cua");
    let image = std::env::var("CUA_JOB_SPACES_IMAGE")
        .unwrap_or_else(|_| "ghcr.io/trycua/linux:24.04-slim".into());
    let vmm = Arc::new(VmmLocal::default());
    let rt = Runtime::new(RuntimeConfig {
        state_dir: Some(home.join("sandboxes")),
        vmm: Some(vmm),
        spaces_home: Some(home.clone()),
        ..Default::default()
    })
    .unwrap();
    let exec = cua_spaces::host_jobs::SpacesExecutor::local(rt.spaces().clone()).await;
    let dirs = exec.dirs().clone();
    let offers = cua_cluster::node::JobExecutor::offers(&exec);
    eprintln!("offers: {offers:?}");
    assert!(
        offers.runtimes.iter().any(|r| r == "gvisor"),
        "Docker has no runsc: {offers:?}"
    );
    let jobs = NodeJobs::open(
        NodeConfig {
            dir: home.join("host/jobs"),
            millicpus_total: 4000,
            memory_total_mb: 8192,
            jobs_max: 0,
            min_available_mb: 0,
            retention: Duration::from_secs(3600),
            output_retention: Duration::from_secs(3600),
            boot_id: "live".into(),
        },
        Arc::new(exec),
    )
    .unwrap();
    jobs.set_self_fence_after(Duration::from_secs(5));

    // A gang member (a world of one) with a start barrier.
    let mut member = spec(
        "live-m",
        &image,
        &[
            "uv",
            "run",
            "--no-project",
            "--python",
            "3.12",
            "python",
            "job.py",
        ],
    );
    member.bundle = bundle(&[("job.py", JOB)]);
    for (k, v) in [
        ("CUA_RUN_ID", "live-run"),
        ("CUA_ATTEMPT", "1"),
        ("CUA_RANK", "0"),
        ("CUA_WORLD_SIZE", "1"),
        ("CUA_PEERS", "10.9.9.9:47010"),
        ("CUA_BARRIER_TIMEOUT", "900"),
    ] {
        member.env.insert(k.into(), v.into());
    }
    let space = cua_cluster::model::space_name("live-m", 1);
    let t0 = Instant::now();
    let acks = jobs.run_batch(vec![member], "t").await;
    assert_eq!(acks[0].outcome, "accepted", "{acks:?}");
    let feed = |jobs: Arc<NodeJobs>| async move {
        jobs.list(0, 100, Duration::ZERO, false)
            .await
            .progress
            .into_iter()
            .find(|p| p.job_id == "live-m")
    };
    // Ready at its barrier (the feed keeps its scheduler alive meanwhile);
    // its output streams into the host's log while it waits.
    let deadline = Instant::now() + Duration::from_secs(900);
    loop {
        if feed(jobs.clone()).await.is_some_and(|p| p.ready) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "never ready; output:\n{}",
            String::from_utf8_lossy(&jobs.output("live-m", 0).unwrap_or_default().0)
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    eprintln!("ready after {:?}", t0.elapsed());
    let (live, _) = jobs.output("live-m", 0).unwrap();
    let live = String::from_utf8_lossy(&live).into_owned();
    eprintln!("live output so far:\n{live}");
    assert!(
        live.contains("rank=0 kind=container") && live.contains("ready; waiting"),
        "{live}"
    );
    // A world of one listens at its own address in its Space.
    assert!(!live.contains("peers=10.9.9.9"), "{live}");
    assert!(live.contains("data=/cua/data"), "{live}");
    // It runs in a gVisor container.
    let running = containers(&space);
    assert_eq!(running.len(), 1, "{running:?}");
    assert_eq!(runtime_of(&space), "runsc", "{running:?}");
    let p = feed(jobs.clone()).await.unwrap();
    assert!(p.ready && !p.released, "{p:?}");
    // Released: it heartbeats and checkpoints into the run's data on the
    // host.
    jobs.release(
        None,
        &[pb::HostJobRef {
            job_id: "live-m".into(),
            attempt: 1,
        }],
    );
    let deadline = Instant::now() + Duration::from_secs(120);
    let p = loop {
        let p = feed(jobs.clone()).await.unwrap();
        if p.step >= 3 {
            break p;
        }
        assert!(Instant::now() < deadline, "no heartbeat: {p:?}");
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    assert!(p.released && p.last_progress_ms > 0, "{p:?}");
    let ckpt = dirs.run_data_dir("live-run").join("ckpt");
    let step: u32 = std::fs::read_to_string(&ckpt)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(step >= 3, "checkpoint on the host at step {step}");
    // Its scheduler goes silent: the host stops it and deletes its Space.
    let silent = Instant::now();
    let r = wait_end(&jobs, "live-m").await;
    eprintln!(
        "fenced after {:?} of silence: {} ({:?})",
        silent.elapsed(),
        r.error,
        r.state()
    );
    assert_eq!(r.state(), cua_cluster::HostJobState::Canceled);
    assert!(r.error.starts_with("fenced: "), "{}", r.error);
    let out = String::from_utf8_lossy(&jobs.output("live-m", 0).unwrap().0).into_owned();
    assert!(out.contains("released") && out.contains("step 3"), "{out}");
    assert!(containers(&space).is_empty(), "the member's Space is gone");

    // A plain member's result file comes back from its mounted work dir.
    let mut plain = spec(
        "live-p",
        &image,
        &[
            "sh",
            "-c",
            "echo from-$CUA_SPACE_KIND; uv --version; echo result > \"$CUA_RESULT_PATH\"",
        ],
    );
    plain.result_path = "out/result.txt".into();
    plain.bundle = bundle(&[("out/.keep", "")]);
    let acks = jobs.run_batch(vec![plain], "t").await;
    assert_eq!(acks[0].outcome, "accepted", "{acks:?}");
    let r = wait_end(&jobs, "live-p").await;
    let out = String::from_utf8_lossy(&r.stdout).into_owned();
    eprintln!("plain member: {:?}\n{out}", r.state());
    assert_eq!(r.state().as_str(), "succeeded", "{r:?}\n{out}");
    assert!(
        out.contains("from-container") && out.contains("uv 0."),
        "{out}"
    );
    assert_eq!(r.result, b"result\n");
    assert!(containers(&cua_cluster::model::space_name("live-p", 1)).is_empty());
}
