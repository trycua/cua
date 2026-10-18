//! Lume backend e2e (`CUA_E2E_LUME=1`), against the running `lume serve`.
//!
//! Never touches VMs it did not create: every VM here is `cua-e2e-*` and is
//! deleted at the end. macOS images (~30 GB+) are deliberately not pulled.
//! `CUA_E2E_LUME_LINUX=1` additionally boots the Debian arm64 cloud image as a
//! Lume Linux VM (raw disk + cloud-init seed via `lume run --mount`).

use std::path::PathBuf;
use std::time::Duration;

use cua_e2e_local_runtimes::{Timer, gated, init_tracing, run_id};
use cua_vmm::lume::{LumeConfig, LumeRuntime};
use cua_vmm::{
    ExecRequest, GuestExec, ImageSource, Probe, Runtime, SshAccess, SshExec, StartSpec, Status,
    VmmError,
};

fn cache() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap()).join(".cua/e2e-cache")
}

fn runtime() -> LumeRuntime {
    LumeRuntime::new(LumeConfig {
        root: cache().join("vmm-lume"),
        ..Default::default()
    })
}

#[tokio::test]
async fn lume_client_lists_and_reports_proper_errors() {
    if !gated("CUA_E2E_LUME") {
        return;
    }
    init_tracing();
    let rt = runtime();
    rt.ensure_serving()
        .await
        .expect("lume serve reachable (or startable)");
    let status = rt.client().host_status().await.unwrap();
    eprintln!("lume host status: {status}");
    let vms = rt.list().await.unwrap();
    eprintln!(
        "existing VMs (read-only): {:?}",
        vms.iter().map(|v| (&v.name, &v.status)).collect::<Vec<_>>()
    );

    let ghost = format!("cua-e2e-missing-{}", run_id());
    assert!(rt.client().get(&ghost).await.unwrap().is_none());
    assert!(matches!(
        rt.status(&ghost).await,
        Err(VmmError::NotFound(_))
    ));
    assert!(matches!(
        rt.fork(&ghost, &format!("{ghost}-clone")).await,
        Err(VmmError::NotFound(_))
    ));
    assert!(matches!(
        rt.delete(&ghost).await,
        Err(VmmError::NotFound(_))
    ));
    assert!(matches!(rt.stop(&ghost).await, Err(VmmError::NotFound(_))));
    let err = rt
        .client()
        .clone_vm(&ghost, &format!("{ghost}-2"))
        .await
        .unwrap_err();
    eprintln!("clone of missing VM -> {err}");
    assert!(matches!(err, VmmError::NotFound(_)));
    assert!(matches!(
        rt.start(&StartSpec::new(&ghost, ImageSource::Existing))
            .await,
        Err(VmmError::NotFound(_))
    ));
}

#[tokio::test]
async fn lume_linux_vm_from_cloud_image() {
    if !gated("CUA_E2E_LUME_LINUX") {
        return;
    }
    init_tracing();
    let disk = cache().join("debian-12-genericcloud-arm64.qcow2");
    assert!(disk.exists(), "missing {disk:?}");
    let key = cua_vmm::cloudinit::ensure_ssh_key(&cache().join("id_ed25519"))
        .await
        .unwrap();
    let rt = runtime();
    let name = format!("cua-e2e-lume-{}", run_id());
    let spec = StartSpec::new(&name, ImageSource::disk(&disk))
        .cpus(2)
        .memory_mb(2048)
        .ssh(SshAccess {
            user: "cua".into(),
            private_key: key,
            password: None,
        })
        .probe(Probe::tcp(22))
        .ready_timeout(Duration::from_secs(240));
    let t = Timer::start();
    let res = async {
        let inst = rt.start(&spec).await?;
        t.lap("lume linux: convert + create + run + ip + tcp:22");
        eprintln!("instance: {}", serde_json::to_string(&inst).unwrap());
        // Diagnostics: how far `lume serve`'s ipAddress lags the guest being
        // reachable (bounded: 60 polls x 1 s).
        let lag = Timer::start();
        let mut api_ip = None;
        for _ in 0..60 {
            if let Some(vm) = rt.client().get(&name).await?
                && let Some(ip) = vm.ip()
            {
                api_ip = Some(ip.to_string());
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        eprintln!(
            "LUME_DIAG guest_ip={} api_ip={api_ip:?} api_lag_after_ready={:.1}s",
            inst.endpoints.host,
            lag.lap("api ip")
        );
        if let Some(ip) = &api_ip {
            assert_eq!(ip, &inst.endpoints.host, "lease/ARP and lume serve agree");
        }
        let ssh = SshExec::from_endpoint(inst.endpoints.ssh.as_ref().unwrap());
        ssh.wait_until_ready(Duration::from_secs(180)).await?;
        t.lap("ssh ready");
        let out = ssh
            .exec(ExecRequest::sh("uname -m; uname -r"))
            .await?
            .check("uname")?;
        eprintln!("guest: {}", out.stdout_str());
        assert!(out.stdout_str().starts_with("aarch64"));
        rt.stop(&name).await?;
        t.lap("stop");
        assert_eq!(rt.status(&name).await?, Status::Stopped);
        Ok::<_, VmmError>(())
    }
    .await;
    let _ = rt.delete(&name).await;
    t.lap("total");
    res.unwrap();
}
