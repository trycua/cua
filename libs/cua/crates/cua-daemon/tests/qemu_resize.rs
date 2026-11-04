//! Opt-in: resizing a running QEMU VM on a real host (KVM on Linux, HVF on
//! a Mac). The VM stops and boots again from its own disk with the new
//! shape, on the host ports it had: a cua-spacesd client connected before
//! the resize answers again afterwards (no new endpoint to find), the new
//! vCPU count and memory are what the guest sees, and its files are kept.
//!
//! ```text
//! CUA_VMM_TEST_QEMU=1 CUA_VMM_TEST_DISK=<linux containerDisk qcow2> \
//!   cargo test -p cua-daemon --test qemu_resize -- --ignored --nocapture
//! ```
//!
//! One VM of at most 2 vCPUs and 1.5 GiB; state in a temp dir, deleted on
//! every path.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cua_daemon::guest_identity::SpacesdIdentity;
use cua_spacesd_client::SpacesdClient;
use cua_vmm::qemu::{QemuConfig, QemuRuntime};
use cua_vmm::{ImageSource, Probe, Resize, Runtime, StartSpec};

const SPACESD: u16 = 3211;

fn gate() -> Option<PathBuf> {
    if std::env::var("CUA_VMM_TEST_QEMU").as_deref() != Ok("1") {
        eprintln!("skipped: set CUA_VMM_TEST_QEMU=1");
        return None;
    }
    Some(PathBuf::from(
        std::env::var("CUA_VMM_TEST_DISK").expect("CUA_VMM_TEST_DISK"),
    ))
}

fn token() -> String {
    let mut b = [0u8; 16];
    rand::fill(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

async fn sh(c: &SpacesdClient, cmd: &str) -> Result<String, String> {
    let o = c.run(cmd).await.map_err(|e| format!("{cmd}: {e}"))?;
    Ok(o.stdout_str().trim().to_string())
}

async fn run(rt: &QemuRuntime, disk: &Path) -> Result<(), String> {
    let t = token();
    let spec = StartSpec::new(
        "rsz",
        ImageSource::Disk {
            path: disk.to_path_buf(),
        },
    )
    .cpus(1)
    .memory_mb(1024)
    .env("CUA_ENV_TOKEN", &t)
    .port(SPACESD)
    .probe(Probe::Tcp { port: SPACESD })
    .ready_timeout(Duration::from_secs(300));
    let before = rt.start(&spec).await.map_err(|e| format!("start: {e}"))?;
    let port = before.endpoints.ports[&SPACESD];
    let url = format!("http://{}:{port}", before.endpoints.host);
    // Held across the resize, as a caller holding the Space is.
    let held = SpacesdClient::connect_url(&url, Some(t.clone()))
        .await
        .map_err(|e| format!("connect: {e}"))?;
    if sh(&held, "nproc").await? != "1" {
        return Err("the VM did not boot with 1 vCPU".into());
    }
    sh(&held, "echo kept > /var/tmp/resize-marker && sync").await?;

    let shape = Resize {
        cpus: Some(2),
        memory_mb: Some(1536),
        probes: vec![Probe::Tcp { port: SPACESD }],
        ..Default::default()
    };
    let t0 = Instant::now();
    let after = rt
        .resize("rsz", &shape)
        .await
        .map_err(|e| format!("resize: {e}"))?;
    eprintln!("resize returned in {:.1}s", t0.elapsed().as_secs_f64());
    if after.endpoints.ports.get(&SPACESD) != Some(&port) {
        return Err(format!(
            "the resized VM moved from host port {port} to {:?}",
            after.endpoints.ports.get(&SPACESD)
        ));
    }
    // The held client reaches the new run (bounded: the guest's spacesd
    // comes up a few seconds after the port accepts).
    let deadline = Instant::now() + Duration::from_secs(120);
    let nproc = loop {
        match sh(&held, "nproc").await {
            Ok(n) => break n,
            Err(e) if Instant::now() < deadline => {
                eprintln!("waiting for the resized guest: {e}");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            Err(e) => return Err(format!("the held client never answered again: {e}")),
        }
    };
    eprintln!(
        "held client answered {:.1}s after the resize began",
        t0.elapsed().as_secs_f64()
    );
    if nproc != "2" {
        return Err(format!("nproc after the resize: {nproc}"));
    }
    let mem_mb: u64 = sh(&held, "free -m | awk '/Mem:/{print $2}'")
        .await?
        .parse()
        .map_err(|e| format!("free: {e}"))?;
    if !(1200..=1536).contains(&mem_mb) {
        return Err(format!("guest memory after the resize: {mem_mb} MiB"));
    }
    if sh(&held, "cat /var/tmp/resize-marker").await? != "kept" {
        return Err("a file written before the resize is gone".into());
    }
    Ok(())
}

#[tokio::test]
#[ignore = "boots a QEMU VM; set CUA_VMM_TEST_QEMU=1 and CUA_VMM_TEST_DISK, run with --ignored"]
async fn a_running_vm_resized_keeps_its_endpoint_and_files() {
    let Some(disk) = gate() else { return };
    let root = tempfile::tempdir().unwrap();
    let rt = QemuRuntime::new(QemuConfig {
        root: root.path().to_path_buf(),
        vnc: false,
        graceful_stop: Duration::from_secs(5),
        identity: Some(Arc::new(SpacesdIdentity)),
        ..QemuConfig::default()
    });
    let result = run(&rt, &disk).await;
    let _ = rt.delete("rsz").await;
    result.unwrap();
}
