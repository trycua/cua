//! Container backend e2e (CUA_E2E_DOCKER=1).

use std::time::Duration;

use cua_e2e_local_runtimes::{Timer, gated, init_tracing, run_id};
use cua_vmm::container::{ContainerConfig, ContainerRuntime};
use cua_vmm::{ExecRequest, ImageSource, Isolation, Probe, Runtime, StartSpec, Status};

/// Small image with a real listener: nginx on :80.
const IMAGE: &str = "nginx:alpine";

#[tokio::test]
async fn gvisor_container_lifecycle_with_port_and_exec() {
    if !gated("CUA_E2E_DOCKER") {
        return;
    }
    init_tracing();
    let t = Timer::start();
    let rt = ContainerRuntime::connect(ContainerConfig {
        allow_install_runsc: std::env::var("CUA_E2E_ALLOW_INSTALL").is_ok(),
        ..Default::default()
    })
    .await
    .expect("engine");
    eprintln!(
        "engine: {:?}; runtimes: {:?}",
        rt.endpoint(),
        rt.runtimes().await.unwrap()
    );
    let name = format!("cua-e2e-ctr-{}", run_id());
    let spec = StartSpec::new(&name, ImageSource::oci(IMAGE))
        .probe(Probe::http(80, "/"))
        .ready_timeout(Duration::from_secs(120));
    let inst = rt.start(&spec).await.expect("start");
    t.lap("container start (pull+run+http probe)");
    eprintln!("instance: {}", serde_json::to_string(&inst).unwrap());
    assert_eq!(inst.status, Status::Running);
    let host_port = inst.endpoints.host_port(80).expect("port 80 published");
    assert!(host_port > 0);

    let exec = rt.guest_exec(&name).await.unwrap().expect("exec");
    let out = exec
        .exec(ExecRequest::sh("uname -a"))
        .await
        .unwrap()
        .check("uname")
        .unwrap();
    let uname = out.stdout_str();
    eprintln!("uname -a: {uname}");
    if inst.isolation == Isolation::Gvisor {
        // gVisor reports its own synthetic kernel ("4.19.0-gvisor" / older "4.4.0",
        // always dated Sun Jan 10 2016).
        assert!(
            uname.contains("gvisor") || uname.contains("Jan 10 15:06:54 PST 2016"),
            "expected gVisor kernel, got {uname}"
        );
        let dmesg = exec
            .exec(ExecRequest::sh("dmesg 2>/dev/null | head -3"))
            .await
            .unwrap();
        eprintln!("dmesg: {}", dmesg.stdout_str());
    } else {
        eprintln!("WARNING: engine has no runsc; ran with runc");
    }

    // stdin + put_file + exit codes.
    exec.put_file("/tmp/cua/hello.txt", b"hello from host", 0o644)
        .await
        .unwrap();
    let cat = exec
        .exec(ExecRequest::sh("cat /tmp/cua/hello.txt; exit 7"))
        .await
        .unwrap();
    assert_eq!(cat.exit_code, 7);
    assert_eq!(cat.stdout_str(), "hello from host");
    t.lap("exec checks");

    // Suspend / resume.
    rt.suspend(&name).await.unwrap();
    assert_eq!(rt.status(&name).await.unwrap(), Status::Paused);
    rt.resume(&name).await.unwrap();
    assert_eq!(rt.status(&name).await.unwrap(), Status::Running);

    // Checkpoint (docker commit) + fork, and whether gVisor rootfs writes survive commit.
    exec.exec(ExecRequest::sh("echo marker > /cua-marker"))
        .await
        .unwrap()
        .check("marker")
        .unwrap();
    let ck = format!("{name}-ck");
    rt.checkpoint(&name, &ck).await.unwrap();
    let forked = format!("{name}-fork");
    rt.fork(&ck, &forked).await.unwrap();
    let f = rt
        .start(
            &StartSpec::new(&forked, ImageSource::Existing)
                .port(80)
                .probe(Probe::http(80, "/")),
        )
        .await
        .unwrap();
    let fexec = rt.guest_exec(&forked).await.unwrap().unwrap();
    let m = fexec
        .exec(ExecRequest::sh("cat /cua-marker 2>&1"))
        .await
        .unwrap();
    eprintln!(
        "fork isolation {:?}; marker -> {:?}",
        f.isolation,
        m.stdout_str().trim()
    );
    assert_eq!(
        m.stdout_str().trim(),
        "marker",
        "checkpoint lost rootfs writes"
    );
    // The fork keeps serving: config (CMD/ENV/EXPOSE) survived the checkpoint.
    assert!(f.endpoints.host_port(80).is_some());
    t.lap("checkpoint+fork");

    let listed = rt.list().await.unwrap();
    assert!(listed.iter().any(|s| s.name == name));
    rt.stop(&name).await.unwrap();
    assert_eq!(rt.status(&name).await.unwrap(), Status::Stopped);
    rt.delete(&name).await.unwrap();
    rt.delete(&forked).await.unwrap();
    assert!(rt.status(&name).await.is_err());
    t.lap("total");
}

/// The public Fleet gVisor rootfs image (`docker-*` tags) under runsc.
/// Gated separately (`CUA_E2E_FLEET_ROOTFS=1`): ~1.1 GB pull.
#[tokio::test]
async fn fleet_gvisor_rootfs_image_runs_under_runsc() {
    if !gated("CUA_E2E_FLEET_ROOTFS") {
        return;
    }
    init_tracing();
    let reference = std::env::var("CUA_E2E_FLEET_ROOTFS_REF")
        .unwrap_or_else(|_| "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:docker-latest".into());
    let rt = ContainerRuntime::connect(ContainerConfig {
        require_gvisor: true,
        ..Default::default()
    })
    .await
    .expect("engine with runsc");
    let name = format!("cua-e2e-fleet-{}", run_id());
    let t = Timer::start();
    // Daemon-agnostic readiness: wait for the noVNC port the image serves,
    // not for any in-guest agent API.
    let spec = StartSpec::new(&name, ImageSource::oci(&reference))
        .arch(cua_vmm::Arch::host())
        .cpus(2)
        .memory_mb(4096)
        .port(8000)
        .probe(Probe::tcp(6080))
        .ready_timeout(Duration::from_secs(600));
    let res = async {
        let inst = rt.start(&spec).await?;
        t.lap("fleet rootfs: pull + start + tcp:6080 probe");
        eprintln!("instance: {}", serde_json::to_string(&inst).unwrap());
        assert_eq!(inst.isolation, Isolation::Gvisor);
        let exec = rt.guest_exec(&name).await?.unwrap();
        let out = exec
            .exec(ExecRequest::sh(
                "uname -a; cat /etc/os-release | head -2; ps -eo comm | sort -u | head -30",
            ))
            .await?;
        eprintln!("{}", out.stdout_str());
        assert!(out.stdout_str().contains("gvisor"));
        Ok::<_, cua_vmm::VmmError>(())
    }
    .await;
    let _ = rt.delete(&name).await;
    t.lap("total");
    res.unwrap();
}
