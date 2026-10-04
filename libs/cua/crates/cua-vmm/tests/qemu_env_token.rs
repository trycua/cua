//! Opt-in: boots ONE QEMU guest from the linux disk with an
//! SDK-style env token in `StartSpec::env` and checks the guest spacesd
//! accepts exactly that token (delivered through the cloud-init seed) and
//! refuses a wrong one.
//!
//! ```text
//! CUA_VMM_TEST_QEMU=1 [CUA_VMM_TEST_DISK=~/.cache/cua-images/linux/arm64/disk.img] \
//!   cargo test -p cua-vmm --test qemu_env_token -- --ignored --nocapture
//! ```
//!
//! 4 GiB RAM, one VM, state in a temp dir; the VM is deleted on every path.
//! The disk must ship the gRPC cua-spacesd (`cua.env.v1`); older
//! linux builds with the legacy WebSocket driver fail the connect
//! even though the token is delivered. Point `CUA_VMM_TEST_DISK` at a current
//! build (qcow2 or raw; it is only used as a backing file).

#![cfg(feature = "qemu")]

use std::path::PathBuf;
use std::time::Duration;

use cua_vmm::qemu::{QemuConfig, QemuRuntime};
use cua_vmm::{ImageSource, Probe, Runtime, StartSpec};

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

#[tokio::test]
#[ignore = "boots a 4 GiB QEMU VM; set CUA_VMM_TEST_QEMU=1 and run with --ignored"]
async fn guest_spacesd_accepts_the_sdk_token() {
    if std::env::var("CUA_VMM_TEST_QEMU").as_deref() != Ok("1") {
        eprintln!("skipped: set CUA_VMM_TEST_QEMU=1");
        return;
    }
    let Some(disk) = disk() else {
        panic!("linux disk not found; set CUA_VMM_TEST_DISK");
    };
    let root = tempfile::tempdir().unwrap();
    let rt = QemuRuntime::new(QemuConfig {
        root: root.path().to_path_buf(),
        vnc: false,
        graceful_stop: Duration::from_secs(5),
        ..QemuConfig::default()
    });
    // Same shape as cua-spaces' provision_local: a random 128-bit hex token.
    let token = format!("{:032x}", rand_u128());
    let name = format!("cua-e2e-envtok-{}", std::process::id());
    let spec = StartSpec::new(&name, ImageSource::Disk { path: disk })
        .cpus(2)
        .memory_mb(4096)
        .env("CUA_ENV_TOKEN", &token)
        .probe(Probe::Tcp {
            port: cua_proto_port(),
        })
        .ready_timeout(Duration::from_secs(300));

    let result = async {
        let inst = rt.start(&spec).await.map_err(|e| format!("start: {e}"))?;
        let port = *inst
            .endpoints
            .ports
            .get(&cua_proto_port())
            .ok_or("3211 not published")?;
        let url = format!("http://{}:{port}", inst.endpoints.host);
        eprintln!("guest spacesd at {url}");
        // The driver may restart once (cloud-init runcmd try-restart); retry.
        let mut last = String::new();
        for _ in 0..30 {
            match cua_spacesd_client::SpacesdClient::connect_url(&url, Some(token.clone())).await {
                Ok(env) => match env.run("cat /run/cua/env-token; id -un").await {
                    Ok(out) => {
                        let stdout = out.stdout_str();
                        eprintln!("guest: {stdout:?}");
                        if !stdout.contains(&token) {
                            return Err(format!("guest token file differs: {stdout:?}"));
                        }
                        // A wrong token is refused.
                        let wrong = cua_spacesd_client::SpacesdClient::connect_url(
                            &url,
                            Some("wrong".into()),
                        )
                        .await;
                        let refused = match wrong {
                            Err(e) => e.to_string(),
                            Ok(c) => match c.run("true").await {
                                Err(e) => e.to_string(),
                                Ok(_) => return Err("wrong token accepted".into()),
                            },
                        };
                        eprintln!("wrong token refused: {refused}");
                        return Ok(());
                    }
                    Err(e) => last = format!("run: {e}"),
                },
                Err(e) => last = format!("connect: {e}"),
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
        Err(format!("spacesd never accepted the token: {last}"))
    }
    .await;
    let serial = root.path().join(&name).join("serial.log");
    if result.is_err()
        && let Ok(log) = std::fs::read_to_string(&serial)
    {
        let tail: Vec<&str> = log.lines().rev().take(80).collect();
        eprintln!(
            "--- serial tail ---\n{}",
            tail.into_iter().rev().collect::<Vec<_>>().join("\n")
        );
    }
    let _ = rt.stop(&name).await;
    let _ = rt.delete(&name).await;
    if let Err(e) = result {
        panic!("{e}");
    }
}

fn cua_proto_port() -> u16 {
    3211
}

fn rand_u128() -> u128 {
    use std::hash::{BuildHasher, Hasher};
    let a = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(a);
    (u128::from(a) << 64) | u128::from(h.finish())
}
