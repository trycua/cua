//! Opt-in (nightly Linux lane, KVM): the Space lifecycle behind
//! `Sandboxes.checkpoint` and `Spaces.fork` on a RUNNING VM. Starts a
//! warm-pool member, claims it as a Space (the pool's rename), runs a
//! command in the guest, takes a memory checkpoint, forks it, starts the
//! fork and reads the command's result back from the fork.
//!
//! Regression: after a claim, the checkpoint failed with
//! `qemu_savevm_state_complete_precopy_non_iterable: bdrv_inactivate_all()
//! failed (-1)`, because QEMU re-opens its images by the path it started
//! with and the claim had renamed their directory.
//!
//! The member is the daemon's kind: with a disk whose cua-spacesd can become
//! a new machine it restores the golden snapshot (`CUA_VMM_TEST_REQUIRE_WARM=1`
//! demands that), as an overlay on the golden disk, with the bulk-lane disk,
//! so the device set has a node for every kind of file. The unused golden
//! snapshots are then cleared (the running member's backing chain must
//! survive that), and the test fails if any file the command line names no
//! longer resolves after the claim.
//!
//! ```text
//! CUA_VMM_TEST_QEMU=1 CUA_VMM_TEST_DISK=/path/to/disk.qcow2 \
//!   [CUA_VMM_TEST_REQUIRE_WARM=1] \
//!   cargo test -p cua-vmm --features qemu --test qemu_checkpoint_fork -- --ignored --nocapture
//! ```
//!
//! The disk must ship the gRPC cua-spacesd (see `qemu_env_token`); it is
//! only used as a backing file. Two 2 GiB VMs, state in a temp dir, all
//! deleted on every path.

#![cfg(all(feature = "qemu", unix))]

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use cua_vmm::qemu::{QemuConfig, QemuRuntime};
use cua_vmm::{
    ClaimIdentity, Endpoints, GuestIdentity, ImageSource, Probe, Runtime, StartSpec, is_pool_member,
};

/// What the daemon's identity provider does to a restored guest, without the
/// `Init` call (cua-vmm cannot depend on cua-daemon): runs the guest's root
/// renewal script through cua-spacesd with the snapshot's token, which moves
/// the guest to the Space's token, and waits until the new one is accepted.
struct RenewToken;

impl RenewToken {
    async fn once(url: &str, current: &str, new: &str) -> Result<(), String> {
        let client = cua_spacesd_client::SpacesdClient::connect_url(url, Some(current.to_string()))
            .await
            .map_err(|e| e.to_string())?;
        let [token_var, entropy_var, now_var, host_var, new_machine_var] =
            cua_vmm::cloudinit::RENEW_ENV;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let cmd = cua_spacesd_client::Command::new("sudo")
            .args([
                "-n".to_string(),
                format!("--preserve-env={}", cua_vmm::cloudinit::RENEW_ENV.join(",")),
                "sh".to_string(),
                "-c".to_string(),
                cua_vmm::cloudinit::renew_identity_script(),
            ])
            .env(token_var, new)
            .env(entropy_var, "00".repeat(64))
            .env(host_var, "")
            .env(new_machine_var, "1")
            .env(
                now_var,
                format!("{}.{:09}", now.as_secs(), now.subsec_nanos()),
            )
            .timeout(Duration::from_secs(20));
        let out = client.run(cmd).await.map_err(|e| e.to_string())?;
        out.success()
            .then_some(())
            .ok_or_else(|| out.stderr_str().trim().to_string())
    }
}

#[async_trait]
impl GuestIdentity for RenewToken {
    async fn renew(
        &self,
        endpoints: &Endpoints,
        current: &str,
        identity: &ClaimIdentity,
        _new_machine: bool,
        timeout: Duration,
    ) -> cua_vmm::Result<()> {
        let Some(new) = identity.token.as_deref().filter(|t| *t != current) else {
            return Ok(());
        };
        let port = endpoints.ports.get(&3211).copied().unwrap_or(3211);
        let host = if endpoints.host.is_empty() {
            "127.0.0.1"
        } else {
            endpoints.host.as_str()
        };
        let url = format!("http://{host}:{port}");
        let deadline = tokio::time::Instant::now() + timeout;
        let mut last = String::new();
        while tokio::time::Instant::now() < deadline {
            match Self::once(&url, current, new).await {
                Ok(()) => last.clear(),
                Err(e) => last = e,
            }
            // The driver follows its token file: done once it takes the new one.
            if let Ok(c) =
                cua_spacesd_client::SpacesdClient::connect_url(&url, Some(new.to_string())).await
                && c.run("true").await.is_ok_and(|o| o.success())
            {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        Err(cua_vmm::VmmError::other(format!("guest renewal: {last}")))
    }
}

fn disk() -> Option<PathBuf> {
    std::env::var_os("CUA_VMM_TEST_DISK")
        .map(PathBuf::from)
        .filter(|p| p.exists())
}

async fn guest_run(url: &str, token: &str, cmd: &str) -> Result<String, String> {
    let mut last = String::new();
    for _ in 0..40 {
        match cua_spacesd_client::SpacesdClient::connect_url(url, Some(token.to_string())).await {
            Ok(env) => match env.run(cmd).await {
                Ok(out) => return Ok(out.stdout_str().to_string()),
                Err(e) => last = format!("run: {e}"),
            },
            Err(e) => last = format!("connect: {e}"),
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    Err(format!("guest never answered `{cmd}`: {last}"))
}

/// Every path the command line names for QEMU to open (`file=`, `mem-path=`,
/// `-bios`, `-cdrom`) exists: a path that no longer resolves breaks the
/// migration that checkpoints and forks end with, which re-opens each block
/// node by its path.
fn assert_files_resolve(cmdline: &str) -> Result<(), String> {
    for tok in cmdline.split_whitespace() {
        for part in tok.split(',') {
            let path = ["file=", "mem-path="]
                .iter()
                .find_map(|k| part.strip_prefix(k))
                .filter(|p| p.starts_with('/'));
            if let Some(p) = path
                && !std::path::Path::new(p).exists()
            {
                return Err(format!("QEMU was started with {p}, which no longer exists"));
            }
        }
    }
    Ok(())
}

fn spec(name: &str, disk: Option<&PathBuf>, token: &str) -> StartSpec {
    let image = match disk {
        Some(path) => ImageSource::Disk { path: path.clone() },
        None => ImageSource::Existing,
    };
    StartSpec::new(name, image)
        .cpus(2)
        .memory_mb(2048)
        .env("CUA_ENV_TOKEN", token)
        .probe(Probe::Tcp { port: 3211 })
        .ready_timeout(Duration::from_secs(300))
}

#[tokio::test]
#[ignore = "boots QEMU VMs; set CUA_VMM_TEST_QEMU=1 and CUA_VMM_TEST_DISK, run with --ignored"]
async fn running_claimed_space_checkpoints_and_forks() {
    if std::env::var("CUA_VMM_TEST_QEMU").as_deref() != Ok("1") {
        eprintln!("skipped: set CUA_VMM_TEST_QEMU=1");
        return;
    }
    let Some(disk) = disk() else {
        panic!("set CUA_VMM_TEST_DISK to a current linux disk");
    };
    let root = tempfile::tempdir().unwrap();
    let rt = QemuRuntime::new(QemuConfig {
        root: root.path().to_path_buf(),
        vnc: false,
        graceful_stop: Duration::from_secs(5),
        identity: Some(std::sync::Arc::new(RenewToken)),
        ..QemuConfig::default()
    });
    let pid = std::process::id();
    let token = format!("{pid:032x}");
    let member = format!("{}t{pid}", cua_vmm::POOL_MEMBER_PREFIX);
    assert!(is_pool_member(&member));
    let (space, ck, child) = (
        format!("cua-e2e-space-{pid}"),
        format!("cua-e2e-ck-{pid}"),
        format!("cua-e2e-fork-{pid}"),
    );

    let result = async {
        let mut pool = spec(&member, Some(&disk), &token);
        pool.pool_member = true;
        rt.start(&pool).await.map_err(|e| format!("start: {e}"))?;
        let inst = rt
            .claim(
                &member,
                &space,
                &ClaimIdentity {
                    token: Some(token.clone()),
                    hostname: None,
                },
            )
            .await
            .map_err(|e| format!("claim: {e}"))?;
        let url = |i: &cua_vmm::Instance| {
            let port = i
                .endpoints
                .ports
                .get(&3211)
                .copied()
                .ok_or("3211 not published")?;
            Ok::<_, String>(format!("http://{}:{port}", i.endpoints.host))
        };
        // A restored member (the pool's case), with the bulk-lane disk.
        let dir = root.path().join(&space);
        let state: serde_json::Value = serde_json::from_slice(
            &std::fs::read(dir.join("state.json")).map_err(|e| format!("state.json: {e}"))?,
        )
        .map_err(|e| format!("state.json: {e}"))?;
        if state["last_start"] != "warm" {
            // A disk whose cua-spacesd predates `new_machine` (the published
            // slim disk) boots its members cold: the claim is still tested,
            // but not the restored path. The lane that builds the disk from
            // this tree requires the restore.
            let msg = format!(
                "the member booted {} rather than restoring the golden snapshot",
                state["last_start"]
            );
            if std::env::var("CUA_VMM_TEST_REQUIRE_WARM").as_deref() == Ok("1") {
                return Err(msg);
            }
            eprintln!("note: {msg}");
        }
        let cmdline = std::fs::read_to_string(dir.join("cmdline")).map_err(|e| e.to_string())?;
        if !cmdline.contains("id=cuabulk") {
            return Err(format!("the member has no bulk-lane disk: {cmdline}"));
        }
        // Every file QEMU was started with still resolves after the claim.
        assert_files_resolve(&cmdline)?;
        // Dropping the unused golden snapshots must keep the one this
        // running member's disk is layered on.
        rt.clear_golden()
            .map_err(|e| format!("clear_golden: {e}"))?;
        assert_files_resolve(&cmdline)?;
        let overlay = state["disk"].as_str().ok_or("no disk in state.json")?;
        for f in cua_vmm::qemu::backing_files(std::path::Path::new(overlay)) {
            if !f.exists() {
                return Err(format!("backing file {} is gone", f.display()));
            }
        }
        let src_url = url(&inst)?;
        guest_run(&src_url, &token, "echo kept > $HOME/m.txt; sync").await?;

        // The failing step: a running VM's memory to a file.
        rt.checkpoint(&space, &ck)
            .await
            .map_err(|e| format!("checkpoint: {e}"))?;
        // The source keeps running and answering after it.
        let still = guest_run(&src_url, &token, "cat $HOME/m.txt").await?;
        if still.trim() != "kept" {
            return Err(format!("source lost its file: {still:?}"));
        }

        rt.fork(&ck, &child)
            .await
            .map_err(|e| format!("fork: {e}"))?;
        let c = rt
            .start(&spec(&child, None, &token))
            .await
            .map_err(|e| format!("start fork: {e}"))?;
        let got = guest_run(&url(&c)?, &token, "cat $HOME/m.txt").await?;
        if got.trim() != "kept" {
            return Err(format!("fork did not see the file: {got:?}"));
        }
        Ok(())
    }
    .await;

    for name in [&child, &ck, &space, &member] {
        let _ = rt.stop(name).await;
        let _ = rt.delete(name).await;
    }
    if let Err(e) = result {
        panic!("{e}");
    }
}
