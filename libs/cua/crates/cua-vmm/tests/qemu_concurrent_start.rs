//! Opt-in (nightly Linux lane, KVM): many VMs starting at once in ONE
//! runtime (one daemon) all come up, each with its own host ports.
//!
//! Regression: concurrent starts picked the same VNC display (the probe
//! bound and released the port, so two starts both saw it free) and QEMU
//! failed with `-vnc 127.0.0.1:7: Failed to find an available port:
//! Address already in use`. This shape (many workers creating and forking
//! at once) is the E2B migration workload.
//!
//! ```text
//! CUA_VMM_TEST_QEMU=1 CUA_VMM_TEST_DISK=/path/to/disk.qcow2 \
//!   cargo test -p cua-vmm --features qemu --test qemu_concurrent_start -- --ignored --nocapture
//! ```
//!
//! Eight 1 GiB VMs plus four forks, state in a temp dir, all deleted on
//! every path. VNC is on so the display allocation is exercised.

#![cfg(all(feature = "qemu", unix))]

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cua_vmm::qemu::{QemuConfig, QemuRuntime};
use cua_vmm::{
    ClaimIdentity, Endpoints, GuestIdentity, ImageSource, Instance, Probe, Runtime, StartSpec,
};

/// Enables memory checkpoints/forks of running VMs (as the daemon does);
/// the guest needs no renewal, the test is about the host side.
struct KeepToken;

#[async_trait]
impl GuestIdentity for KeepToken {
    async fn renew(
        &self,
        _endpoints: &Endpoints,
        _current: &str,
        _identity: &ClaimIdentity,
        _new_machine: bool,
        _timeout: Duration,
    ) -> cua_vmm::Result<()> {
        Ok(())
    }
}

fn spec(name: &str, disk: Option<&PathBuf>) -> StartSpec {
    let image = match disk {
        Some(path) => ImageSource::Disk { path: path.clone() },
        None => ImageSource::Existing,
    };
    StartSpec::new(name, image)
        .cpus(1)
        .memory_mb(1024)
        .env("CUA_ENV_TOKEN", format!("{:032x}", std::process::id()))
        .probe(Probe::Tcp { port: 3211 })
        .ready_timeout(Duration::from_secs(300))
}

/// Every instance's published host ports and VNC port, all distinct.
fn assert_distinct(instances: &[Instance]) {
    let mut seen = HashSet::new();
    for i in instances {
        let mut ports: Vec<u16> = i.endpoints.ports.values().copied().collect();
        ports.extend(i.endpoints.vnc.as_ref().map(|v| v.port));
        for p in ports {
            assert!(seen.insert(p), "host port {p} handed to two VMs");
        }
    }
}

#[tokio::test]
#[ignore = "boots QEMU VMs; set CUA_VMM_TEST_QEMU=1 and CUA_VMM_TEST_DISK, run with --ignored"]
async fn concurrent_starts_and_forks_all_come_up() {
    if std::env::var("CUA_VMM_TEST_QEMU").as_deref() != Ok("1") {
        eprintln!("skipped: set CUA_VMM_TEST_QEMU=1");
        return;
    }
    let disk = std::env::var_os("CUA_VMM_TEST_DISK")
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .expect("set CUA_VMM_TEST_DISK to a current linux disk");
    let root = tempfile::tempdir().unwrap();
    let rt = Arc::new(QemuRuntime::new(QemuConfig {
        root: root.path().to_path_buf(),
        vnc: true,
        identity: Some(Arc::new(KeepToken)),
        graceful_stop: Duration::from_secs(5),
        ..QemuConfig::default()
    }));
    let pid = std::process::id();
    let names: Vec<String> = (0..8).map(|i| format!("cua-e2e-cs{pid}-{i}")).collect();
    let forks: Vec<String> = (0..4).map(|i| format!("cua-e2e-cf{pid}-{i}")).collect();
    let ck = format!("cua-e2e-ck{pid}");

    let result = async {
        let starts: Vec<_> = names
            .iter()
            .map(|n| {
                let (rt, s) = (rt.clone(), spec(n, Some(&disk)));
                tokio::spawn(async move { rt.start(&s).await })
            })
            .collect();
        let mut up = Vec::new();
        for (n, h) in names.iter().zip(starts) {
            up.push(
                h.await
                    .map_err(|e| format!("start {n}: {e}"))?
                    .map_err(|e| format!("start {n}: {e}"))?,
            );
        }
        assert_distinct(&up);

        rt.checkpoint(&names[0], &ck)
            .await
            .map_err(|e| format!("checkpoint: {e}"))?;
        let child_starts: Vec<_> = forks
            .iter()
            .map(|f| {
                let (rt, ck, f2, s) = (rt.clone(), ck.clone(), f.clone(), spec(f, None));
                tokio::spawn(async move {
                    rt.fork(&ck, &f2).await?;
                    rt.start(&s).await
                })
            })
            .collect();
        for (f, h) in forks.iter().zip(child_starts) {
            let i = h
                .await
                .map_err(|e| format!("fork {f}: {e}"))?
                .map_err(|e| format!("fork {f}: {e}"))?;
            up.push(i);
        }
        assert_distinct(&up);
        Ok::<_, String>(())
    }
    .await;

    for name in forks.iter().chain(std::iter::once(&ck)).chain(names.iter()) {
        let _ = rt.stop(name).await;
        let _ = rt.delete(name).await;
    }
    if let Err(e) = result {
        panic!("{e}");
    }
}
