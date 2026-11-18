//! Opt-in: a job (and a plain SDK exec) that prints a lot, very fast, in a
//! real Space, finishes once with its complete output.
//!
//! Regression for a job that died with `rpc failed: h2 protocol error:
//! error reading a body from connection (ResourceExhausted)` (h2's flood
//! protection closing the stream of a command that wrote many tiny chunks)
//! and then ran again as an infrastructure failure:
//!
//! - the job runs once (one attempt, state `succeeded`, exit 0);
//! - its host log holds every line, once, in order (count and checksum);
//! - the same flood through the SDK path (`SpacesdClient::run` on the
//!   Space) returns the complete output.
//!
//! ```text
//! CUA_HOME=/tmp/some-throwaway-home CUA_E2E_JOB_FLOOD=1 \
//!   cargo test -p cua-daemon --test job_flood_live -- --ignored --nocapture
//! ```
//!
//! Needs Docker with the `runsc` runtime and network for the image
//! (`CUA_JOB_SPACES_IMAGE`, default `ghcr.io/trycua/linux:24.04-slim`); a
//! throwaway `CUA_HOME` (never `~/.cua`). `CUA_JOB_FLOOD_LINES` sets the
//! line count (default 2,000,000, about 50 MB).

#![cfg(all(unix, feature = "spaces"))]

use std::sync::Arc;
use std::time::{Duration, Instant};

use cua_cluster::node::{NodeConfig, NodeJobs};
use cua_daemon::{Runtime, RuntimeConfig, local::VmmLocal};
use cua_proto::env::v1 as pb;
use sha2::{Digest, Sha256};

/// Prints `n` numbered short lines as fast as it can, then 60 long
/// tracebacks on stderr (what a failing `pytest -q` prints).
fn flood_script(n: u64) -> String {
    format!(
        r#"
import sys
out = sys.stdout
for i in range({n}):
    out.write("line %08d\n" % i)
    if i % 4 == 0:
        out.flush()
out.flush()
for t in range(60):
    sys.stderr.write("Traceback (most recent call last):\n")
    for f in range(20):
        sys.stderr.write('  File "/app/test_%d.py", line %d, in test_%d\n    assert value == expected\n' % (t, f, f))
    sys.stderr.write("AssertionError: case %d\n" % t)
    sys.stderr.flush()
"#
    )
}

fn lines_of(text: &[u8]) -> (usize, String) {
    let text = String::from_utf8_lossy(text);
    let mut h = Sha256::new();
    let mut n = 0;
    for l in text.lines().filter(|l| l.starts_with("line ")) {
        h.update(l.as_bytes());
        h.update(b"\n");
        n += 1;
    }
    (
        n,
        h.finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
    )
}

fn expected(n: u64) -> (usize, String) {
    let mut h = Sha256::new();
    for i in 0..n {
        h.update(format!("line {i:08}\n").as_bytes());
    }
    (
        n as usize,
        h.finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "runs a gVisor container Space on this machine's Docker; set CUA_E2E_JOB_FLOOD=1 and a throwaway CUA_HOME"]
async fn a_job_that_floods_its_output_runs_once_with_every_line() {
    if std::env::var("CUA_E2E_JOB_FLOOD").as_deref() != Ok("1") {
        eprintln!("skipped: set CUA_E2E_JOB_FLOOD=1");
        return;
    }
    let home = std::path::PathBuf::from(
        std::env::var_os("CUA_HOME").expect("set CUA_HOME to a throwaway home"),
    );
    let real = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cua"));
    assert!(Some(&home) != real.as_ref(), "never the user's ~/.cua");
    let image = std::env::var("CUA_JOB_SPACES_IMAGE")
        .unwrap_or_else(|_| "ghcr.io/trycua/linux:24.04-slim".into());
    let n: u64 = std::env::var("CUA_JOB_FLOOD_LINES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2_000_000);
    let vmm = Arc::new(VmmLocal::default());
    let rt = Runtime::new(RuntimeConfig {
        state_dir: Some(home.join("sandboxes")),
        vmm: Some(vmm),
        spaces_home: Some(home.clone()),
        ..Default::default()
    })
    .unwrap();
    let exec = cua_spaces::host_jobs::SpacesExecutor::local(rt.spaces().clone()).await;
    let jobs = NodeJobs::open(
        NodeConfig {
            dir: home.join("host/jobs"),
            millicpus_total: 4000,
            memory_total_mb: 8192,
            jobs_max: 0,
            min_available_mb: 0,
            retention: Duration::from_secs(3600),
            output_retention: Duration::from_secs(3600),
            boot_id: "flood".into(),
        },
        Arc::new(exec),
    )
    .unwrap();

    // The job: one attempt, no result file.
    let job = pb::HostJobSpec {
        job_id: "flood-1".into(),
        attempt: 1,
        image: image.clone(),
        kind: "container".into(),
        argv: vec!["python3".into(), "-c".into(), flood_script(n)],
        millicpus: 1000,
        memory_mb: 1024,
        timeout_secs: 900,
        run_id: "flood-run".into(),
        max_output_bytes: 1 << 20,
        ..Default::default()
    };
    let acks = jobs.run_batch(vec![job], "t").await;
    assert_eq!(acks[0].outcome, "accepted", "{acks:?}");
    let deadline = Instant::now() + Duration::from_secs(900);
    let record = loop {
        if let Some(r) = jobs.get("flood-1")
            && r.state().is_terminal()
        {
            break r;
        }
        assert!(Instant::now() < deadline, "the job did not end");
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    assert_eq!(
        record.state().as_str(),
        "succeeded",
        "{} ({:?})",
        record.error,
        record.state()
    );
    assert_eq!(record.exit_code, Some(0));
    // The host's log, read the way a follower reads it: chunk by chunk.
    let mut log = Vec::new();
    loop {
        let (chunk, total) = jobs.output("flood-1", log.len() as u64).unwrap();
        log.extend_from_slice(&chunk);
        if chunk.is_empty() || log.len() as u64 >= total {
            break;
        }
    }
    let (count, sum) = lines_of(&log);
    let (want_count, want_sum) = expected(n);
    eprintln!(
        "job output: {} bytes, {count} lines, sha256 {sum}",
        log.len()
    );
    assert_eq!(count, want_count, "every line, once");
    assert_eq!(sum, want_sum, "in order, none repeated");
    assert_eq!(
        String::from_utf8_lossy(&log)
            .matches("Traceback (most recent call last):")
            .count(),
        60
    );

    // The SDK path: a flooding command run through the Space's client.
    let created = rt
        .spaces()
        .create(cua_spaces::SpaceCreate {
            image: Some(image),
            on: Some(cua_sandbox_core::placement::On::Local),
            kind: cua_sandbox_core::placement::Kind::Container,
            name: Some("flood-sdk".into()),
            wait: Some(true),
            ..Default::default()
        })
        .await
        .expect("create the SDK Space");
    let id = created.ready().expect("ready").id;
    let space = rt.spaces().space(&id).await.unwrap();
    let out = space
        .spacesd()
        .unwrap()
        .run(
            cua_spacesd_client::Command::new("python3")
                .arg("-c")
                .arg(flood_script(n)),
        )
        .await
        .expect("the SDK exec must complete");
    let (count, sum) = lines_of(&out.stdout);
    eprintln!(
        "sdk exec: {} bytes, {count} lines, sha256 {sum}",
        out.stdout.len()
    );
    assert!(out.status.success(), "{:?}", out.status);
    assert_eq!((count, sum), (want_count, want_sum));
    assert!(out.stderr_str().matches("Traceback").count() >= 60);
    let _ = rt.spaces().delete(&id).await;
}
