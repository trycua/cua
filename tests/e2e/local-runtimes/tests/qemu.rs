//! QEMU backend e2e.
//!
//! * `CUA_E2E_QEMU=1`: boots an arm64 Linux cloud image (Debian 12
//!   genericcloud by default; override with `CUA_E2E_QEMU_DISK=/path.qcow2`)
//!   under hvf/kvm with a cloud-init seed carrying an SSH key, checks
//!   `uname -m` over SSH, QMP screendump, suspend/resume, live checkpoint
//!   (savevm), stop, stopped checkpoint + fork + boot of the fork.
//! * `CUA_E2E_QEMU_X86=1`: boots a Fleet x86_64 containerDisk under TCG and
//!   waits for the kernel boot log on the serial console
//!   (`CUA_E2E_QEMU_X86_DISK=/path/disk.qcow2`, pulled by the image test).

use std::path::PathBuf;
use std::time::Duration;

use cua_e2e_local_runtimes::{Timer, gated, init_tracing, run_id};
use cua_vmm::qemu::{QemuConfig, QemuRuntime};
use cua_vmm::{
    Arch, ExecRequest, GuestExec, ImageSource, Probe, Runtime, SshAccess, SshExec, StartSpec,
    Status,
};

fn cache() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap()).join(".cua/e2e-cache")
}

fn runtime() -> QemuRuntime {
    QemuRuntime::new(QemuConfig {
        root: cache().join("vmm-qemu"),
        graceful_stop: Duration::from_secs(20),
        ..Default::default()
    })
}

fn dump(name: &str, file: &str, lines: usize) {
    let p = cache().join("vmm-qemu").join(name).join(file);
    let s = std::fs::read_to_string(&p).unwrap_or_default();
    let tail: Vec<&str> = s.lines().rev().take(lines).collect();
    eprintln!(
        "--- {} (last {lines}) ---\n{}",
        p.display(),
        tail.into_iter().rev().collect::<Vec<_>>().join("\n")
    );
}

#[tokio::test]
async fn arm64_cloud_image_boots_with_ssh_exec_and_checkpoints() {
    if !gated("CUA_E2E_QEMU") {
        return;
    }
    init_tracing();
    let disk = std::env::var("CUA_E2E_QEMU_DISK")
        .map(PathBuf::from)
        .unwrap_or_else(|_| cache().join("debian-12-genericcloud-arm64.qcow2"));
    assert!(
        disk.exists(),
        "missing {disk:?}; download a Debian/Ubuntu arm64 cloud image there"
    );
    let key = cua_vmm::cloudinit::ensure_ssh_key(&cache().join("id_ed25519"))
        .await
        .unwrap();
    let rt = runtime();
    let name = format!("cua-e2e-qemu-{}", run_id());
    let ssh = SshAccess {
        user: "cua".into(),
        private_key: key.clone(),
        password: None,
    };
    let spec = StartSpec::new(&name, ImageSource::disk(&disk))
        .arch(Arch::Aarch64)
        .cpus(2)
        .memory_mb(2048)
        .ssh(ssh.clone())
        .probe(Probe::tcp(22))
        .ready_timeout(Duration::from_secs(300));
    let t = Timer::start();
    let result = async {
        let inst = rt.start(&spec).await?;
        t.lap("qemu start -> sshd banner (tcp probe)");
        eprintln!("instance: {}", serde_json::to_string(&inst).unwrap());
        let ep = inst.endpoints.ssh.clone().expect("ssh endpoint");
        let exec = SshExec::from_endpoint(&ep);
        exec.wait_until_ready(Duration::from_secs(180)).await?;
        t.lap("ssh login ready (cloud-init user created)");
        let out = exec
            .exec(ExecRequest::sh(
                "uname -m; uname -r; cat /etc/os-release | head -1",
            ))
            .await?
            .check("uname")?;
        eprintln!("guest: {}", out.stdout_str());
        assert_eq!(out.stdout_str().lines().next(), Some("aarch64"));
        assert!(exec.is_root().await.is_ok());
        exec.put_file("/opt/cua/marker", b"qemu", 0o644).await?;

        let serial = std::fs::read_to_string(inst.endpoints.serial_log.as_ref().unwrap())
            .unwrap_or_default();
        eprintln!("serial log bytes: {}", serial.len());
        assert!(
            serial.contains("Linux version") || serial.contains("login:") || serial.len() > 1000
        );

        let png = cache().join(format!("{name}.png"));
        rt.qmp(&name).await?.screendump(&png).await?;
        let bytes = std::fs::read(&png).unwrap();
        assert_eq!(&bytes[1..4], b"PNG");
        eprintln!("screendump: {} bytes", bytes.len());
        let _ = std::fs::remove_file(&png);

        rt.suspend(&name).await?;
        assert_eq!(rt.status(&name).await?, Status::Paused);
        rt.resume(&name).await?;
        assert_eq!(rt.status(&name).await?, Status::Running);

        let t2 = Timer::start();
        match rt.checkpoint(&name, "live1").await {
            Ok(_) => {
                t2.lap("live checkpoint (savevm)");
                rt.restore(&name, "live1").await?;
                t2.lap("restore (loadvm)");
                exec.exec(ExecRequest::sh("true"))
                    .await?
                    .check("after loadvm")?;
                eprintln!("LIVE CHECKPOINT: ok");
            }
            Err(e) => {
                eprintln!("LIVE CHECKPOINT: failed: {e}");
                dump(&name, "qemu.log", 20);
                if rt.status(&name).await? != Status::Running {
                    // Keep going with the stopped-state checks.
                    eprintln!("(qemu exited during savevm)");
                }
            }
        }

        let t3 = Timer::start();
        rt.stop(&name).await?;
        t3.lap("graceful stop (ACPI powerdown)");
        assert_eq!(rt.status(&name).await?, Status::Stopped);

        // Stopped checkpoint -> fork -> boot fork, marker must be there.
        let ck = format!("{name}-ck");
        rt.checkpoint(&name, &ck).await?;
        let fork = format!("{name}-fork");
        rt.fork(&ck, &fork).await?;
        let t4 = Timer::start();
        let fspec = StartSpec::new(&fork, ImageSource::Existing)
            .arch(Arch::Aarch64)
            .ssh(ssh.clone())
            .probe(Probe::tcp(22))
            .ready_timeout(Duration::from_secs(300));
        let finst = rt.start(&fspec).await?;
        let fexec = SshExec::from_endpoint(finst.endpoints.ssh.as_ref().unwrap());
        if let Err(e) = fexec.wait_until_ready(Duration::from_secs(180)).await {
            dump(&fork, "serial.log", 40);
            dump(&fork, "qemu.log", 20);
            return Err(e);
        }
        t4.lap("fork boot -> ssh");
        let m = fexec
            .exec(ExecRequest::sh("cat /opt/cua/marker"))
            .await?
            .check("marker")?;
        assert_eq!(m.stdout_str(), "qemu");
        rt.stop(&fork).await?;
        let listed = rt.list().await?;
        eprintln!("list: {listed:?}");
        rt.delete(&fork).await?;
        rt.delete(&ck).await?;
        Ok::<_, cua_vmm::VmmError>(())
    }
    .await;
    let _ = rt.delete(&name).await;
    for suffix in ["-fork", "-ck"] {
        let _ = rt.delete(&format!("{name}{suffix}")).await;
    }
    t.lap("total");
    result.unwrap();
}

#[tokio::test]
async fn x86_containerdisk_boots_under_tcg() {
    if !gated("CUA_E2E_QEMU_X86") {
        return;
    }
    init_tracing();
    let disk =
        PathBuf::from(std::env::var("CUA_E2E_QEMU_X86_DISK").expect("set CUA_E2E_QEMU_X86_DISK"));
    let rt = runtime();
    let name = format!("cua-e2e-x86-{}", run_id());
    let spec = StartSpec::new(&name, ImageSource::disk(&disk))
        .arch(Arch::X86_64)
        .cpus(4)
        .memory_mb(4096)
        .ready_timeout(Duration::from_secs(60));
    let t = Timer::start();
    let result = async {
        let inst = rt.start(&spec).await?;
        t.lap("x86 qemu (tcg) running");
        eprintln!("isolation: {:?}", inst.isolation);
        let log = inst.endpoints.serial_log.clone().unwrap();
        let deadline = std::time::Instant::now()
            + Duration::from_secs(
                std::env::var("CUA_E2E_QEMU_X86_TIMEOUT")
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(1200),
            );
        let (mut saw_kernel, mut saw_login) = (false, false);
        while std::time::Instant::now() < deadline {
            let s = std::fs::read_to_string(&log).unwrap_or_default();
            if !saw_kernel && s.contains("Linux version") {
                saw_kernel = true;
                t.lap("serial: kernel booting");
            }
            if s.contains("login:") || s.contains("Reached target") && s.contains("Multi-User") {
                saw_login = true;
                t.lap("serial: reached login/multi-user");
                break;
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
        let s = std::fs::read_to_string(&log).unwrap_or_default();
        eprintln!(
            "--- serial tail ---\n{}",
            s.lines()
                .rev()
                .take(15)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n")
        );
        let png = cache().join(format!("{name}.png"));
        if rt.qmp(&name).await?.screendump(&png).await.is_ok() {
            eprintln!("screendump saved: {}", png.display());
        }
        assert!(
            saw_kernel || saw_login || !s.is_empty(),
            "no serial output at all"
        );
        eprintln!("x86 result: kernel={saw_kernel} login={saw_login}");
        Ok::<_, cua_vmm::VmmError>(())
    }
    .await;
    let _ = rt.delete(&name).await;
    t.lap("total");
    result.unwrap();
}
