//! cua-image e2e: a local registry push/pull round trip and the local builder
//! for both containers and VMs.
//!
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
