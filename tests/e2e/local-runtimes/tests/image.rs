//! cua-image e2e: containerDisk pulls from the public Fleet registry booted
//! under QEMU, a local registry push/pull round trip, and the local builder
//! for both containers and VMs.
//!
//! * `CUA_E2E_FLEET_DISK=1` — pull the public Fleet containerDisk for the
//!   host arch (~1.7 GB arm64) and boot it under QEMU (hvf on Apple Silicon).
//! * `CUA_E2E_FLEET_DISK_X86=1` — pull the x86_64 containerDisk (~1.4 GB) and
//!   boot it under TCG (slow; waits for the serial boot log).
//! * `CUA_E2E_IMAGE=1` — local `registry:2` + container build + VM build.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use cua_e2e_local_runtimes::{Timer, gated, init_tracing, run_id};
use cua_image::builder::{BuildOptions, BuildStep, build_container, build_vm};
use cua_image::{ContainerDiskResolver, ImageCache, RegistryClient};
use cua_vmm::container::{ContainerConfig, ContainerRuntime};
use cua_vmm::qemu::{QemuConfig, QemuRuntime};
use cua_vmm::{
    Arch, ExecRequest, GuestExec, ImageSource, Probe, Runtime, SshAccess, SshExec, StartSpec,
};

const FLEET: &str = "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04";

fn cache() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap()).join(".cua/e2e-cache")
}

fn qemu_with(resolver: Arc<dyn cua_vmm::DiskResolver>) -> QemuRuntime {
    QemuRuntime::new(QemuConfig {
        root: cache().join("vmm-qemu"),
        resolver: Some(resolver),
        graceful_stop: Duration::from_secs(30),
        ..Default::default()
    })
}

fn serial_tail(path: &std::path::Path, n: usize) -> String {
    let s = std::fs::read_to_string(path).unwrap_or_default();
    let v: Vec<&str> = s.lines().rev().take(n).collect();
    v.into_iter().rev().collect::<Vec<_>>().join("\n")
}

async fn wait_serial(
    path: &std::path::Path,
    needles: &[&str],
    timeout: Duration,
) -> Option<String> {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        let s = std::fs::read_to_string(path).unwrap_or_default();
        if let Some(n) = needles.iter().find(|n| s.contains(**n)) {
            return Some(n.to_string());
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    None
}

async fn boot_fleet(arch: Arch, reference: &str, boot_timeout: Duration) {
    init_tracing();
    let resolver = Arc::new(ContainerDiskResolver {
        client: RegistryClient::default(),
        cache: ImageCache::new(cache().join("images")),
    });
    let t = Timer::start();
    let disk = cua_vmm::DiskResolver::resolve(resolver.as_ref(), reference, arch)
        .await
        .expect("pull containerDisk");
    t.lap(&format!(
        "pull + extract containerDisk ({reference}, {arch})"
    ));
    eprintln!(
        "disk: {} ({} MB)",
        disk.display(),
        std::fs::metadata(&disk).unwrap().len() >> 20
    );
    let rt = qemu_with(resolver);
    let key = cua_vmm::cloudinit::ensure_ssh_key(&cache().join("id_ed25519"))
        .await
        .unwrap();
    let name = format!("cua-e2e-fleet-{}-{}", arch.oci(), run_id());
    let spec = StartSpec::new(&name, ImageSource::oci(reference))
        .arch(arch)
        .cpus(4)
        .memory_mb(4096)
        .ssh(SshAccess {
            user: "cua".into(),
            private_key: key,
            password: None,
        })
        .ready_timeout(Duration::from_secs(60));
    let t2 = Timer::start();
    let res = async {
        let inst = rt.start(&spec).await?;
        t2.lap("qemu running");
        eprintln!("isolation {:?}", inst.isolation);
        let log = inst.endpoints.serial_log.clone().unwrap();
        let kernel = wait_serial(&log, &["Linux version", "Booting Linux"], boot_timeout).await;
        t2.lap(&format!("serial kernel banner: {kernel:?}"));
        let login = wait_serial(
            &log,
            &["login:", "Reached target Multi-User", "Cloud-init v."],
            boot_timeout,
        )
        .await;
        t2.lap(&format!("serial userspace marker: {login:?}"));
        // SSH via the cloud-init seed if the image honours NoCloud.
        let ssh = SshExec::from_endpoint(inst.endpoints.ssh.as_ref().unwrap());
        match ssh
            .wait_until_ready(Duration::from_secs(if arch == Arch::host() {
                240
            } else {
                60
            }))
            .await
        {
            Ok(()) => {
                t2.lap("ssh ready");
                let out = ssh
                    .exec(ExecRequest::sh(
                        "uname -m; systemctl is-system-running 2>/dev/null; ss -ltn | head -20",
                    ))
                    .await?;
                eprintln!("guest:\n{}", out.stdout_str());
            }
            Err(e) => eprintln!("ssh not available on this image: {e}"),
        }
        let png = cache().join(format!("{name}.png"));
        if rt.qmp(&name).await?.screendump(&png).await.is_ok() {
            eprintln!(
                "screendump: {} ({} bytes)",
                png.display(),
                std::fs::metadata(&png).map(|m| m.len()).unwrap_or(0)
            );
        }
        eprintln!("--- serial tail ---\n{}", serial_tail(&log, 12));
        assert!(
            kernel.is_some() || login.is_some(),
            "no boot output on the serial console"
        );
        Ok::<_, cua_vmm::VmmError>(())
    }
    .await;
    let _ = rt.delete(&name).await;
    t.lap("total");
    res.unwrap();
}

#[tokio::test]
async fn fleet_containerdisk_boots_natively() {
    if !gated("CUA_E2E_FLEET_DISK") {
        return;
    }
    boot_fleet(
        Arch::host(),
        &format!("{FLEET}:latest"),
        Duration::from_secs(300),
    )
    .await;
}

#[tokio::test]
async fn fleet_containerdisk_x86_boots_under_tcg() {
    if !gated("CUA_E2E_FLEET_DISK_X86") {
        return;
    }
    let reference =
        std::env::var("CUA_E2E_FLEET_X86_REF").unwrap_or_else(|_| format!("{FLEET}:main-38352d34"));
    boot_fleet(Arch::X86_64, &reference, Duration::from_secs(1500)).await;
}

/// Local `registry:2` for push/pull round trips; removed on drop.
struct LocalRegistry {
    rt: ContainerRuntime,
    name: String,
    host: String,
}

impl LocalRegistry {
    async fn start() -> Self {
        // runc is fine for a throwaway registry; keep gVisor for the sandboxes.
        let rt = ContainerRuntime::connect(ContainerConfig {
            prefer_gvisor: false,
            ..Default::default()
        })
        .await
        .unwrap();
        let name = format!("cua-e2e-registry-{}", run_id());
        let inst = rt
            .start(
                &StartSpec::new(&name, ImageSource::oci("registry:2"))
                    .probe(Probe::http(5000, "/v2/"))
                    .ready_timeout(Duration::from_secs(120)),
            )
            .await
            .unwrap();
        let host = format!("127.0.0.1:{}", inst.endpoints.host_port(5000).unwrap());
        Self { rt, name, host }
    }
    fn client(&self) -> RegistryClient {
        RegistryClient::new(vec![self.host.clone()])
    }
    async fn stop(self) {
        let _ = self.rt.delete(&self.name).await;
    }
}

#[tokio::test]
async fn local_registry_builds_and_round_trips() {
    if !gated("CUA_E2E_IMAGE") {
        return;
    }
    init_tracing();
    let reg = LocalRegistry::start().await;
    let res = run_builds(&reg).await;
    reg.stop().await;
    res.unwrap();
}

async fn run_builds(reg: &LocalRegistry) -> Result<(), Box<dyn std::error::Error>> {
    let t = Timer::start();
    let work = cache().join(format!("build-{}", run_id()));
    let src = work.join("hello.txt");
    std::fs::create_dir_all(&work)?;
    std::fs::write(&src, b"from the host\n")?;
    let mut env = std::collections::BTreeMap::new();
    env.insert("CUA_BUILT".to_string(), "yes it's built".to_string());
    let steps = vec![
        BuildStep::Env(env),
        BuildStep::apt(["jq"]),
        BuildStep::run("echo \"$CUA_BUILT\" > /opt/cua-marker"),
        BuildStep::Copy {
            src: src.clone(),
            dst: "/opt/hello.txt".into(),
        },
        BuildStep::Expose(8080),
    ];

    // ── Container build (gVisor) → push → pull/unpack → run imported image ──
    let ctr = ContainerRuntime::connect(ContainerConfig::default()).await?;
    let ctr_ref = format!("{}/cua-e2e/rootfs:test", reg.host);
    let mut opts = BuildOptions::new(work.join("ctr"));
    opts.push = Some(ctr_ref.clone());
    let engine_tag = format!("cua-e2e-built-{}", run_id());
    // Push goes through RegistryClient::default(); make the local registry insecure for it.
    unsafe { std::env::set_var("CUA_INSECURE_REGISTRIES", &reg.host) };
    let out = build_container(
        &ctr,
        "debian:bookworm-slim",
        &steps,
        &opts,
        Some(&engine_tag),
    )
    .await?;
    eprintln!("container build timings: {:?}", out.timings);
    t.lap("container build + push");
    let (_, digest) = out.pushed.clone().unwrap();
    let client = reg.client();
    let images = ImageCache::new(work.join("cache"));
    let root = cua_image::rootfs::pull(&client, &images, &ctr_ref, Arch::host().oci()).await?;
    assert_eq!(
        std::fs::read_to_string(root.join("opt/cua-marker"))?.trim(),
        "yes it's built"
    );
    assert!(root.join("usr/bin/jq").exists());
    eprintln!("pulled rootfs {digest} unpacked at {}", root.display());
    let run_name = format!("cua-e2e-built-run-{}", run_id());
    ctr.start(
        &StartSpec::new(
            &run_name,
            ImageSource::oci(format!("cua-vmm/checkpoint:{engine_tag}")),
        )
        .command(["sleep", "300"]),
    )
    .await?;
    let e = ctr.guest_exec(&run_name).await?.unwrap();
    let o = e
        .exec(ExecRequest::sh(
            "cat /opt/hello.txt; jq --version; . /etc/profile.d/cua-env.sh; echo $CUA_BUILT",
        ))
        .await?;
    eprintln!("imported image exec: {}", o.stdout_str());
    assert!(o.stdout_str().contains("from the host"));
    let _ = ctr.delete(&run_name).await;
    let _ = ctr.delete_checkpoint(&engine_tag).await;
    t.lap("container image verified");

    // ── VM build (QEMU) → containerDisk → push (multi-arch index) → boot by ref ──
    let base = cache().join("debian-12-genericcloud-arm64.qcow2");
    if !base.exists() || Arch::host() != Arch::Aarch64 {
        eprintln!("skipping VM build: needs {base:?} on an arm64 host");
        return Ok(());
    }
    let resolver = Arc::new(ContainerDiskResolver {
        client: reg.client(),
        cache: ImageCache::new(work.join("cache")),
    });
    let qemu = qemu_with(resolver.clone());
    let mut vopts = BuildOptions::new(work.join("vm"));
    vopts.arch = Arch::Aarch64;
    vopts.cpus = 4;
    vopts.memory_mb = 2048;
    vopts.disk_size_gb = Some(4);
    let vm_steps = vec![
        BuildStep::apt(["jq"]),
        BuildStep::run("echo vm-built > /opt/cua-marker"),
    ];
    let out = build_vm(&qemu, &base, &vm_steps, &vopts).await?;
    eprintln!("vm build timings: {:?}", out.timings);
    t.lap("vm build");
    let image = out.image.unwrap();
    eprintln!(
        "containerDisk layer: {} MB (flattened qcow2 {} MB)",
        image.layers[0].0.size >> 20,
        std::fs::metadata(out.disk.as_ref().unwrap())?.len() >> 20
    );
    let vm_ref = format!("{}/cua-e2e/vm:test", reg.host);
    let index_digest = cua_image::push_multiarch(&client, &vm_ref, &[image]).await?;
    eprintln!("pushed multi-arch index {index_digest}");
    t.lap("vm push (index + arm64)");

    let key = cua_vmm::cloudinit::ensure_ssh_key(&cache().join("id_ed25519")).await?;
    let name = format!("cua-e2e-built-vm-{}", run_id());
    let res = async {
        let inst = qemu
            .start(
                &StartSpec::new(&name, ImageSource::oci(&vm_ref))
                    .arch(Arch::Aarch64)
                    .ssh(SshAccess {
                        user: "cua".into(),
                        private_key: key.clone(),
                        password: None,
                    })
                    .probe(Probe::tcp(22))
                    .ready_timeout(Duration::from_secs(300)),
            )
            .await?;
        let ssh = SshExec::from_endpoint(inst.endpoints.ssh.as_ref().unwrap());
        ssh.wait_until_ready(Duration::from_secs(240)).await?;
        let o = ssh
            .exec(ExecRequest::sh("cat /opt/cua-marker; jq --version"))
            .await?
            .check("verify")?;
        eprintln!("booted built image: {}", o.stdout_str());
        assert!(o.stdout_str().contains("vm-built"));
        Ok::<_, cua_vmm::VmmError>(())
    }
    .await;
    let _ = qemu.delete(&name).await;
    res?;
    t.lap("vm image pulled by ref + booted + verified");
    let _ = std::fs::remove_dir_all(&work);
    Ok(())
}
