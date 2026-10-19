//! Opt-in: boots a QEMU guest from the linux disk twice, one VM
//! at a time, and checks outbound network from inside the guest: it works by
//! default and is cut with `restrict_network` (SDK `network="none"`), while
//! the SDK still reaches cua-spacesd through the loopback forward.
//!
//! ```text
//! CUA_VMM_TEST_QEMU=1 [CUA_VMM_TEST_DISK=~/.cache/cua-images/linux/arm64/disk.img] \
//!   cargo test -p cua-vmm --test qemu_egress -- --ignored --nocapture
//! ```
//!
//! Needs internet on the host. 4 GiB RAM, one VM at a time, state in a temp
//! dir; each VM is deleted on every path. The disk must ship the gRPC
//! cua-spacesd (`cua.env.v1`).

#![cfg(feature = "qemu")]

use std::path::{Path, PathBuf};
use std::time::Duration;

use cua_vmm::qemu::{QemuConfig, QemuRuntime};
use cua_vmm::{ImageSource, Probe, Runtime, StartSpec};

const SPACESD: u16 = 3211;

/// DNS lookup plus an HTTPS request (curl, else a raw TCP connect).
const EGRESS: &str = "getent hosts example.com && \
    if command -v curl >/dev/null; then curl -sSI --max-time 15 https://example.com | head -1; \
    else timeout 15 bash -c 'exec 3<>/dev/tcp/example.com/443' && echo tcp-ok; fi";

fn disk() -> Option<PathBuf> {
    let path = std::env::var_os("CUA_VMM_TEST_DISK")
        .map(PathBuf::from)
        .or_else(|| {
            let home = std::env::var_os("HOME")?;
            let arch = if cfg!(target_arch = "aarch64") {
                "arm64"
            } else {
                "amd64"
            };
            Some(PathBuf::from(home).join(format!(".cache/cua-images/linux/{arch}/disk.img")))
        })?;
    path.exists().then_some(path)
}

/// Boots one guest, runs [`EGRESS`] in it, and deletes it. Returns whether
/// the command succeeded, with its output.
async fn egress(disk: &Path, restrict: bool) -> Result<(bool, String), String> {
    let root = tempfile::tempdir().unwrap();
    let rt = QemuRuntime::new(QemuConfig {
        root: root.path().to_path_buf(),
        vnc: false,
        graceful_stop: Duration::from_secs(5),
        ..QemuConfig::default()
    });
    let token = format!("{:016x}{:08x}", rand_u64(), std::process::id());
    let name = format!(
        "cua-e2e-egress-{}-{}",
        if restrict { "none" } else { "default" },
        std::process::id()
    );
    let mut spec = StartSpec::new(
        &name,
        ImageSource::Disk {
            path: disk.to_path_buf(),
        },
    )
    .cpus(2)
    .memory_mb(4096)
    .env("CUA_ENV_TOKEN", &token)
    .probe(Probe::Tcp { port: SPACESD })
    .ready_timeout(Duration::from_secs(300));
    spec.restrict_network = restrict;

    let result = async {
        let inst = rt.start(&spec).await.map_err(|e| format!("start: {e}"))?;
        let port = *inst
            .endpoints
            .ports
            .get(&SPACESD)
            .ok_or("3211 not published")?;
        let url = format!("http://{}:{port}", inst.endpoints.host);
        let mut last = String::new();
        // Bounded: spacesd may restart once after cloud-init.
        for _ in 0..30 {
            match cua_spacesd_client::SpacesdClient::connect_url(&url, Some(token.clone())).await {
                Ok(env) => match env.run(EGRESS).await {
                    Ok(out) => {
                        let text = format!("{}{}", out.stdout_str(), out.stderr_str());
                        return Ok((out.success(), text));
                    }
                    Err(e) => last = format!("run: {e}"),
                },
                Err(e) => last = format!("connect: {e}"),
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
        Err(format!("spacesd never answered: {last}"))
    }
    .await;
    let _ = rt.stop(&name).await;
    let _ = rt.delete(&name).await;
    result
}

#[tokio::test]
#[ignore = "boots a 4 GiB QEMU VM twice; set CUA_VMM_TEST_QEMU=1 and run with --ignored"]
async fn guests_have_egress_by_default_and_none_cuts_it() {
    if std::env::var("CUA_VMM_TEST_QEMU").as_deref() != Ok("1") {
        eprintln!("skipped: set CUA_VMM_TEST_QEMU=1");
        return;
    }
    let Some(disk) = disk() else {
        panic!("linux disk not found; set CUA_VMM_TEST_DISK");
    };
    let (ok, out) = egress(&disk, false).await.unwrap();
    eprintln!("default network: ok={ok}\n{out}");
    assert!(ok, "default guest has no egress: {out}");

    let (ok, out) = egress(&disk, true).await.unwrap();
    eprintln!("network=none: ok={ok}\n{out}");
    assert!(!ok, "restricted guest reached the internet: {out}");
}

fn rand_u64() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish()
}
