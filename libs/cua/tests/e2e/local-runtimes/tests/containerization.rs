//! Apple Containerization microVM e2e (CUA_E2E_APPLE_CONTAINER=1, macOS 26
//! on Apple silicon).
//!
//! Installs cua's own pinned `container` into `$CUA_HOME` when missing and
//! starts its services there (it refuses when another install of
//! `container` holds the per-user services), boots the Spaces image
//! `ghcr.io/trycua/linux:24.04-slim` as its own VM, waits for cua-spacesd,
//! checks it accepts the Space's token, runs commands, stops and starts it
//! again, and deletes it. `CUA_E2E_APPLE_IMAGE` overrides the image.

use std::time::{Duration, Instant};

use cua_e2e_local_runtimes::{Timer, gated, init_tracing, run_id};
use cua_vmm::containerization::ContainerizationRuntime;
use cua_vmm::{ExecRequest, ImageSource, Probe, Runtime, StartSpec, Status, VmmError};

const IMAGE: &str = "ghcr.io/trycua/linux:24.04-slim";

/// cua-spacesd at `port` answers an authenticated call with `token`.
async fn spacesd_accepts(host: &str, port: u16, token: &str) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let r = async {
            let c = cua_spacesd_client::SpacesdClient::connect_url(
                &format!("http://{host}:{port}"),
                Some(token.to_string()),
            )
            .await?;
            c.health().await?;
            c.metrics().await
        }
        .await;
        match r {
            Ok(_) => return,
            Err(e) if Instant::now() < deadline => {
                eprintln!("spacesd not ready yet: {e}");
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            Err(e) => panic!("cua-spacesd on {port} never accepted the token: {e}"),
        }
    }
}

/// Removes the test's VM when the test fails midway.
struct Cleanup {
    bin: Option<std::path::PathBuf>,
    name: String,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Some(b) = &self.bin {
            let _ = std::process::Command::new(b)
                .args(["delete", "--force", &self.name])
                .output();
        }
    }
}

async fn sh(rt: &ContainerizationRuntime, name: &str, script: &str) -> String {
    let exec = rt.guest_exec(name).await.unwrap().expect("exec");
    exec.exec(ExecRequest::sh(script).timeout(Duration::from_secs(60)))
        .await
        .unwrap()
        .check(script)
        .unwrap()
        .stdout_str()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn apple_container_space_lifecycle() {
    if !gated("CUA_E2E_APPLE_CONTAINER") {
        return;
    }
    init_tracing();
    let rt = ContainerizationRuntime::with_defaults();
    let t = Timer::start();
    rt.ensure_ready().await.expect("install + services");
    t.lap("containerization: installed and services up");
    let name = format!("e2e-acz-{}", run_id());
    let token = "e2e-token-0123456789abcdef";
    let _cleanup = Cleanup {
        bin: rt.bin(),
        name: name.clone(),
    };
    let image = std::env::var("CUA_E2E_APPLE_IMAGE").unwrap_or_else(|_| IMAGE.into());
    let spec = StartSpec::new(&name, ImageSource::oci(image))
        .cpus(2)
        .memory_mb(2048)
        .port(3211)
        .env("CUA_ENV_TOKEN", token)
        .probe(Probe::tcp(3211))
        .ready_timeout(Duration::from_secs(600));

    // 1. Create: pull (first run), boot its own VM, cua-spacesd listening.
    let t = Timer::start();
    let inst = rt.start(&spec).await.expect("start");
    t.lap("microVM: create to cua-spacesd listening");
    assert_eq!(inst.status, Status::Running);
    let port = inst.endpoints.host_port(3211).expect("3211 served");
    spacesd_accepts(&inst.endpoints.host, port, token).await;
    t.lap("microVM: cua-spacesd accepts its token");

    // 2. Its own kernel, and commands through `container exec`.
    let t = Timer::start();
    let out = sh(&rt, &name, "uname -r; hostname; id -u").await;
    t.lap("microVM: exec");
    eprintln!("guest: {out}");
    let host_kernel = std::process::Command::new("uname")
        .arg("-r")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    assert!(
        !out.lines()
            .next()
            .unwrap_or_default()
            .contains(&host_kernel),
        "the guest runs its own Linux kernel"
    );
    assert_eq!(
        rt.guest_tcp_listening(&name, 3211).await.unwrap(),
        Some(true)
    );
    assert!(rt.list().await.unwrap().iter().any(|s| s.name == name));

    // 3. Stop and start again (no pause in `container`).
    let t = Timer::start();
    rt.stop(&name).await.expect("stop");
    t.lap("microVM: stop");
    assert_eq!(rt.status(&name).await.unwrap(), Status::Stopped);
    let back = rt.resume(&name).await.expect("start again");
    assert_eq!(back.status, Status::Running);
    spacesd_accepts(
        &back.endpoints.host,
        back.endpoints.host_port(3211).unwrap(),
        token,
    )
    .await;
    t.lap("microVM: started again, cua-spacesd accepts its token");

    // 4. Delete.
    let t = Timer::start();
    rt.delete(&name).await.expect("delete");
    t.lap("microVM: delete");
    assert!(matches!(rt.status(&name).await, Err(VmmError::NotFound(_))));
}
