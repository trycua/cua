//! Opt-in: a stopped QEMU VM (its process gone, its memory given back)
//! boots again from its own disk, and a resize gives it another shape.
//! Boots ONE guest from the linux disk:
//!
//! ```text
//! CUA_VMM_TEST_QEMU=1 CUA_VMM_TEST_DISK=~/.cache/cua-images/linux/arm64/disk.img \
//!   cargo test -p cua-vmm --features qemu --test qemu_stop_start -- --ignored --nocapture
//! ```
//!
//! 2 GiB RAM (3 GiB after the resize), state in a temp dir; the VM is
//! deleted on every path.

#![cfg(all(feature = "qemu", unix))]

use std::path::PathBuf;
use std::time::Duration;

use cua_vmm::qemu::{QemuConfig, QemuRuntime};
use cua_vmm::{ImageSource, Probe, Resize, Runtime, StartSpec, Status};

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

fn url_of(inst: &cua_vmm::Instance) -> Result<String, String> {
    let port = *inst
        .endpoints
        .ports
        .get(&3211)
        .ok_or("3211 not published")?;
    Ok(format!("http://{}:{port}", inst.endpoints.host))
}

#[tokio::test]
#[ignore = "boots a QEMU VM; set CUA_VMM_TEST_QEMU=1 and CUA_VMM_TEST_DISK, run with --ignored"]
async fn a_stopped_vm_boots_again_from_its_disk_and_resizes() {
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
        graceful_stop: Duration::from_secs(10),
        ..QemuConfig::default()
    });
    // A token in the shape cua-spaces' provision_local makes (128-bit hex);
    // the guest's spacesd accepts no other.
    let token = format!(
        "{:016x}{:016x}",
        (std::process::id() as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64
    );
    let name = format!("cua-e2e-stopstart-{}", std::process::id());
    let spec = StartSpec::new(&name, ImageSource::Disk { path: disk })
        .cpus(2)
        .memory_mb(2048)
        .env("CUA_ENV_TOKEN", &token)
        .probe(Probe::Tcp { port: 3211 })
        .ready_timeout(Duration::from_secs(300));

    let result = async {
        let inst = rt.start(&spec).await.map_err(|e| format!("start: {e}"))?;
        let url = url_of(&inst)?;
        guest_run(&url, &token, "echo kept > /var/tmp/marker && sync").await?;

        // Stop: the process ends, so the memory goes back to the machine.
        rt.stop(&name).await.map_err(|e| format!("stop: {e}"))?;
        let st = rt.status(&name).await.map_err(|e| format!("status: {e}"))?;
        if st != Status::Stopped {
            return Err(format!("after stop the VM is {st:?}, not stopped"));
        }

        // Start (resume) a stopped VM: it boots from its own disk.
        let inst = rt.resume(&name).await.map_err(|e| format!("resume: {e}"))?;
        let url = url_of(&inst)?;
        let marker = guest_run(&url, &token, "cat /var/tmp/marker").await?;
        if marker.trim() != "kept" {
            return Err(format!("the disk was not kept: {marker:?}"));
        }

        // Resize: another memory size, booted from the same disk.
        let shape = Resize {
            memory_mb: Some(3072),
            ..Default::default()
        };
        let inst = rt
            .resize(&name, &shape)
            .await
            .map_err(|e| format!("resize: {e}"))?;
        let url = url_of(&inst)?;
        let mem = guest_run(&url, &token, "awk '/MemTotal/{print $2}' /proc/meminfo").await?;
        let kib: u64 = mem
            .trim()
            .parse()
            .map_err(|_| format!("meminfo: {mem:?}"))?;
        if !(2_700_000..=3_200_000).contains(&kib) {
            return Err(format!("after the resize the guest has {kib} KiB"));
        }
        let marker = guest_run(&url, &token, "cat /var/tmp/marker").await?;
        if marker.trim() != "kept" {
            return Err(format!("the resize lost the disk: {marker:?}"));
        }
        Ok::<(), String>(())
    }
    .await;
    let _ = rt.stop(&name).await;
    let _ = rt.delete(&name).await;
    if let Err(e) = result {
        panic!("{e}");
    }
}
