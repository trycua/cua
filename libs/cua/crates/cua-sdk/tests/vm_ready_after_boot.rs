//! Opt-in KVM/QEMU lane: a cold-booted VM Space is ready the moment create
//! returns, and stays usable. Cloud-init's final stage runs a second or two
//! after cua-spacesd first answers; a seed that restarted the driver there
//! aborted every call in flight (`Aborted`, `SendRequest`, `exit -1`).
//!
//! For each of `CUA_TEST_VM_READY_CREATES` (default 1) cold VMs, started
//! `CUA_TEST_VM_READY_CONCURRENCY` (default 1) at a time, the test runs
//! commands back to back from the instant create returns, over a fresh
//! connection for every second one, until at least 50 commands and 15
//! seconds have passed. Not one may fail and none is retried. The first
//! command also checks that the guest's cloud-init has finished
//! (`/run/cloud-init/result.json`): "ready" means boot will not touch the
//! driver again.
//!
//! ```text
//! CUA_TEST_VM_READY=1 [CUA_TEST_VM_IMAGE=ghcr.io/trycua/linux:24.04-disk] \
//!   [CUA_TEST_VM_READY_CREATES=20] [CUA_TEST_VM_READY_CONCURRENCY=16] \
//!   cargo test -p cua-sdk --test vm_ready_after_boot -- --ignored --nocapture
//! ```
//!
//! Needs `/dev/kvm` and QEMU. The state lives in a temp dir (an isolated
//! `CUA_HOME`); every VM is deleted on every path.

use std::sync::Arc;
use std::time::{Duration, Instant};

use cua_sdk::{Cua, CuaConfig, SandboxCreateOptions};
use futures_util::FutureExt;

const COMMANDS: usize = 50;
const WINDOW: Duration = Duration::from_secs(15);

fn env_or<T: std::str::FromStr>(k: &str, d: T) -> T {
    std::env::var(k)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(d)
}

/// Creates one cold VM Space and hammers it; returns create latency and
/// the number of commands run.
async fn create_and_hammer(
    cua: Arc<Cua>,
    image: String,
    name: String,
) -> Result<(Duration, usize), String> {
    let mut o = SandboxCreateOptions::new("local", &image);
    o.kind = Some("vm".into());
    o.name = Some(name.clone());
    o.cpus = Some(2);
    o.memory_mb = Some(1024);
    o.ready_timeout_ms = Some(300_000);
    let started = Instant::now();
    let created = cua.sandboxes().create(o).await;
    let outcome = std::panic::AssertUnwindSafe(async {
        let sb = created.map_err(|e| format!("{name}: create: {e}"))?;
        let create = started.elapsed();
        let t0 = Instant::now();
        let mut shared = sb
            .spacesd(Some(10_000))
            .await
            .map_err(|e| format!("{name}: connect: {e}"))?;
        let settled = shared
            .sh(
                "test -e /run/cloud-init/result.json && echo settled || echo booting".into(),
                Some(10_000),
            )
            .await
            .map_err(|e| format!("{name}: first command: {e}"))?;
        if settled.stdout != b"settled\n" {
            return Err(format!(
                "{name}: create returned while the guest's cloud-init was still running"
            ));
        }
        let mut n = 0usize;
        while n < COMMANDS || t0.elapsed() < WINDOW {
            if n % 2 == 1 {
                shared = sb.spacesd(Some(10_000)).await.map_err(|e| {
                    format!("{name}: command {n} at {:?}: connect: {e}", t0.elapsed())
                })?;
            }
            let out = shared
                .sh(format!("echo ok-{n}"), Some(10_000))
                .await
                .map_err(|e| format!("{name}: command {n} at {:?}: {e}", t0.elapsed()))?;
            if !out.exit.success || out.stdout != format!("ok-{n}\n").as_bytes() {
                return Err(format!(
                    "{name}: command {n} at {:?}: exit {:?}, stdout {:?}",
                    t0.elapsed(),
                    out.exit,
                    String::from_utf8_lossy(&out.stdout)
                ));
            }
            n += 1;
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Ok((create, n))
    })
    .catch_unwind()
    .await;
    let _ = cua.sandboxes().delete(name).await;
    match outcome {
        Ok(r) => r,
        Err(p) => std::panic::resume_unwind(p),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "boots QEMU VMs; set CUA_TEST_VM_READY=1 and run with --ignored"]
async fn a_cold_vm_space_is_usable_the_moment_create_returns() {
    if std::env::var("CUA_TEST_VM_READY").as_deref() != Ok("1") {
        eprintln!("skipped: set CUA_TEST_VM_READY=1 (boots QEMU VMs)");
        return;
    }
    let image = std::env::var("CUA_TEST_VM_IMAGE")
        .unwrap_or_else(|_| "ghcr.io/trycua/linux:24.04-disk".into());
    let creates: usize = env_or("CUA_TEST_VM_READY_CREATES", 1);
    let concurrency: usize = env_or("CUA_TEST_VM_READY_CONCURRENCY", 1).max(1);
    let dirs = tempfile::tempdir().unwrap();
    let cua = Cua::embedded(CuaConfig {
        state_dir: Some(dirs.path().join("sandboxes").display().to_string()),
        spaces_home: Some(dirs.path().join("cua").display().to_string()),
        ..Default::default()
    })
    .unwrap();
    let run = format!("{:x}", std::process::id());
    let mut failures = Vec::new();
    let mut done = 0usize;
    let mut slowest = Duration::ZERO;
    let mut ran = 0usize;
    while done < creates {
        let wave = concurrency.min(creates - done);
        let tasks: Vec<_> = (0..wave)
            .map(|i| {
                let name = format!("cua-e2e-ready-{run}-{}", done + i);
                tokio::spawn(create_and_hammer(cua.clone(), image.clone(), name))
            })
            .collect();
        for t in tasks {
            match t.await.unwrap() {
                Ok((create, n)) => {
                    slowest = slowest.max(create);
                    ran += n;
                }
                Err(e) => failures.push(e),
            }
        }
        done += wave;
    }
    eprintln!(
        "{creates} cold creates ({concurrency} at a time): {ran} commands, slowest create {slowest:?}, {} failures",
        failures.len()
    );
    assert!(failures.is_empty(), "failures:\n{}", failures.join("\n"));
}
