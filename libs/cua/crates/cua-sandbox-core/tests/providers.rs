#![allow(deprecated)] // also exercises the deprecated `apply_pool` wrapper
//! Sandbox lifecycle over fake providers, plus a Direct-provider integration
//! test against the in-process cua-spacesd-client mock server. Nothing here launches
//! host processes or touches the real `~/.cua` (every test uses a temp
//! state dir).

use async_trait::async_trait;
use cua_fleet::testing::FakeFleet;
use cua_sandbox_core::{
    ConnectOptionsOverride, CreateOptions, Error, ForwardVia, InstanceStatus, LocalEndpoints,
    LocalInstance, LocalRuntime, LocalStartSpec, LocalSummary, PortTarget, Probe, ProviderKind,
    RuntimeError, RuntimeResult, SandboxState, Sandboxes, Status,
};
use cua_spacesd_client::testing::{MockAuth, MockGateway, MockServer};
use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

// ------------------------------------------------------------ fake runtime

/// In-memory local runtime. Guest ports map to whatever host ports the test
/// registers in `port_map` (real loopback listeners), or to a closed port.
#[derive(Default)]
struct FakeRuntime {
    instances: Mutex<HashMap<String, InstanceStatus>>,
    port_map: Mutex<BTreeMap<u16, u16>>,
    calls: Mutex<Vec<String>>,
    /// What the guest-side TCP view answers (`None`: no view).
    guest_listening: Mutex<Option<bool>>,
    envs: Mutex<Vec<BTreeMap<String, String>>>,
    specs: Mutex<Vec<LocalStartSpec>>,
    /// The instance's command exits right after start (a crashing CMD).
    exits: Mutex<bool>,
    /// `start` creates the instance, then fails with this (a start that
    /// dies half way, for example on a full disk).
    fail_start: Mutex<Option<RuntimeError>>,
    /// The image's `ai.cua.spacesd` declaration, reported by
    /// `start_resolved` (`None`: no image info at all).
    image_spacesd: Mutex<Option<Option<bool>>>,
    /// `start` creates the instance, then never returns (a boot a
    /// cancelled create cuts off).
    hang_start: Mutex<bool>,
}

impl FakeRuntime {
    fn map(&self, guest: u16, host: u16) {
        self.port_map.lock().unwrap().insert(guest, host);
    }

    fn endpoints(&self) -> LocalEndpoints {
        LocalEndpoints {
            host: "127.0.0.1".into(),
            ports: self.port_map.lock().unwrap().clone(),
            ..Default::default()
        }
    }

    fn log(&self, s: String) {
        self.calls.lock().unwrap().push(s);
    }
}

#[async_trait]
impl LocalRuntime for FakeRuntime {
    fn backend(&self) -> String {
        "fake".into()
    }

    async fn start(&self, spec: &LocalStartSpec) -> RuntimeResult<LocalInstance> {
        self.log(format!("start {} ports={:?}", spec.name, spec.ports));
        self.envs.lock().unwrap().push(spec.env.clone());
        self.specs.lock().unwrap().push(spec.clone());
        let after = if *self.exits.lock().unwrap() {
            InstanceStatus::Stopped
        } else {
            InstanceStatus::Running
        };
        self.instances
            .lock()
            .unwrap()
            .insert(spec.name.clone(), after);
        if let Some(e) = self.fail_start.lock().unwrap().clone() {
            return Err(e);
        }
        let hang = *self.hang_start.lock().unwrap();
        if hang {
            std::future::pending::<()>().await;
        }
        Ok(LocalInstance {
            name: spec.name.clone(),
            backend: "fake".into(),
            status: InstanceStatus::Running,
            endpoints: self.endpoints(),
        })
    }

    async fn start_resolved(
        &self,
        spec: &LocalStartSpec,
    ) -> RuntimeResult<(LocalInstance, Option<cua_sandbox_core::ImageInfo>)> {
        let inst = self.start(spec).await?;
        let info =
            (*self.image_spacesd.lock().unwrap()).map(|spacesd| cua_sandbox_core::ImageInfo {
                reference: spec.image.clone(),
                pinned_ref: format!("{}@sha256:0", spec.image),
                digest: "sha256:0".into(),
                variant: "rootfs".into(),
                arch: None,
                os: "linux".into(),
                emulated: false,
                spacesd,
            });
        Ok((inst, info))
    }

    async fn stop(&self, name: &str) -> RuntimeResult<()> {
        self.log(format!("stop {name}"));
        self.set(name, InstanceStatus::Stopped)
    }

    async fn suspend(&self, name: &str) -> RuntimeResult<()> {
        self.log(format!("suspend {name}"));
        self.set(name, InstanceStatus::Paused)
    }

    async fn resume(&self, name: &str) -> RuntimeResult<LocalInstance> {
        self.log(format!("resume {name}"));
        self.set(name, InstanceStatus::Running)?;
        Ok(LocalInstance {
            name: name.into(),
            backend: "fake".into(),
            status: InstanceStatus::Running,
            endpoints: self.endpoints(),
        })
    }

    async fn list(&self) -> RuntimeResult<Vec<LocalSummary>> {
        Ok(self
            .instances
            .lock()
            .unwrap()
            .iter()
            .map(|(n, s)| LocalSummary {
                name: n.clone(),
                backend: "fake".into(),
                status: s.clone(),
            })
            .collect())
    }

    async fn status(&self, name: &str) -> RuntimeResult<InstanceStatus> {
        self.instances
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .ok_or_else(|| RuntimeError::NotFound(name.into()))
    }

    async fn delete(&self, name: &str) -> RuntimeResult<()> {
        self.log(format!("delete {name}"));
        self.instances
            .lock()
            .unwrap()
            .remove(name)
            .map(|_| ())
            .ok_or_else(|| RuntimeError::NotFound(name.into()))
    }

    async fn endpoints(&self, _name: &str) -> RuntimeResult<LocalEndpoints> {
        Ok(self.endpoints())
    }

    async fn guest_tcp_listening(&self, _name: &str, _port: u16) -> RuntimeResult<Option<bool>> {
        Ok(*self.guest_listening.lock().unwrap())
    }
}

impl FakeRuntime {
    fn set(&self, name: &str, s: InstanceStatus) -> RuntimeResult<()> {
        match self.instances.lock().unwrap().get_mut(name) {
            Some(v) => {
                *v = s;
                Ok(())
            }
            None => Err(RuntimeError::NotFound(name.into())),
        }
    }
}

/// A tiny HTTP/1.1 responder (not a spacesd): answers every request
/// with `status` and echoes the request line in the body.
async fn http_responder(status: u16) -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let n = s.read(&mut buf).await.unwrap_or(0);
                let line = String::from_utf8_lossy(&buf[..n])
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_string();
                let resp = format!(
                    "HTTP/1.1 {status} X\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{line}",
                    line.len()
                );
                let _ = s.write_all(resp.as_bytes()).await;
            });
        }
    });
    port
}

async fn echo_server() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            tokio::spawn(async move {
                let (mut r, mut w) = s.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    port
}

fn closed_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn quick() -> ConnectOptionsOverride {
    ConnectOptionsOverride {
        probe_timeout: Some(Duration::from_secs(3)),
    }
}

// ------------------------------------------------------ Fleet template, local

fn seed_template(fake: &FakeFleet, pool: &str, template: &str, vm: serde_json::Value) {
    fake.put_object(
        "pool",
        pool,
        pool,
        serde_json::json!({"apiVersion": "osgym.cua.ai/v1alpha1", "kind": "OSGymSandboxWarmPool",
            "metadata": {"namespace": pool, "name": pool},
            "spec": {"replicas": 1, "sandboxTemplateRef": {"name": template}}}),
    );
    fake.put_object(
        "template",
        pool,
        template,
        serde_json::json!({"apiVersion": "osgym.cua.ai/v1alpha1", "kind": "OSGymSandboxTemplate",
            "metadata": {"namespace": pool, "name": template},
            "spec": {"vmTemplate": vm}}),
    );
}

#[tokio::test]
async fn fleet_pool_runs_locally_with_its_template() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeFleet::new();
    seed_template(
        &fake,
        "cua-e2e-win",
        "win-tpl",
        serde_json::json!({
            "containerDiskImage": "public.ecr.aws/k5j5w0x5/cua-windows-2022:main-bac7daa3",
            "runtime": "kubevirt", "firmware": "efi", "cpuCores": 4, "memory": "8Gi",
            "services": [{"name": "server", "targetPort": 8000}, {"name": "mcp", "targetPort": 3000}],
            "probes": {"readinessProbe": {"tcpSocket": {"port": 8000}}}
        }),
    );
    let rt = Arc::new(FakeRuntime::default());
    let web = http_responder(200).await;
    rt.map(8000, web);
    let sbx = Sandboxes::builder()
        .local(rt.clone())
        .fleet(fake.client())
        .state_dir(dir.path())
        .build();

    // Pool name through `fleet.pool` (what `--local --pool` sends).
    let mut o = CreateOptions::new(ProviderKind::Local, "");
    o.fleet.pool = Some("cua-e2e-win".into());
    o.name = Some("cua-e2e-local-win".into());
    let sb = sbx.create(o).await.unwrap();
    assert_eq!(sb.provider(), ProviderKind::Local);
    let spec = rt.specs.lock().unwrap()[0].clone();
    assert_eq!(
        spec.image,
        "vm:public.ecr.aws/k5j5w0x5/cua-windows-2022:main-bac7daa3"
    );
    assert_eq!(spec.firmware.as_deref(), Some("efi"));
    assert_eq!((spec.cpus, spec.memory_mb), (2, 4096));
    assert_eq!(spec.ports, vec![3000, 3211, 8000]);
    assert_eq!(spec.probes.len(), 1);
    assert_eq!(spec.probes[0].port, 8000);
    // Only reads went to Fleet.
    assert!(fake.requests().iter().all(|r| r.method == "GET"));
    sb.delete().await.unwrap();

    // `pool:<template>` with no pool: the namespace-of-the-same-name template.
    seed_template(
        &fake,
        "ubuntu-tpl",
        "ubuntu-tpl",
        serde_json::json!({"containerDiskImage": "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:docker-latest",
            "runtime": "gvisor", "services": [{"name": "server", "targetPort": 8000}]}),
    );
    let _ = sbx
        .create(CreateOptions::new(ProviderKind::Local, "pool:ubuntu-tpl").name("cua-e2e-local-gv"))
        .await
        .unwrap();
    let spec = rt.specs.lock().unwrap()[1].clone();
    assert_eq!(
        spec.image,
        "container:public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:docker-latest"
    );
    assert_eq!(spec.firmware, None);

    // Unknown names are NotFound, not a Fleet create.
    let e = sbx
        .create(CreateOptions::new(ProviderKind::Local, "pool:nope"))
        .await
        .unwrap_err();
    assert!(matches!(e, Error::NotFound(_)), "{e}");
}

#[tokio::test]
async fn fleet_macos_template_is_unsupported_locally() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeFleet::new();
    seed_template(
        &fake,
        "cua-e2e-mac",
        "mac",
        serde_json::json!({"containerDiskImage": "127.0.0.1:5000/cua/macos-desktop-workspace:latest",
            "runtime": "macos"}),
    );
    let rt = Arc::new(FakeRuntime::default());
    let sbx = Sandboxes::builder()
        .local(rt.clone())
        .fleet(fake.client())
        .state_dir(dir.path())
        .build();
    let e = sbx
        .create(CreateOptions::new(ProviderKind::Local, "fleet:cua-e2e-mac"))
        .await
        .unwrap_err();
    assert!(matches!(e, Error::UnsupportedImage(_)), "{e}");
    assert_eq!(e.to_string(), cua_sandbox_core::MACOS_FLEET_UNSUPPORTED);
    assert!(rt.specs.lock().unwrap().is_empty(), "nothing was started");
}

// ------------------------------------------------------------------ local

#[tokio::test]
async fn local_lifecycle_state_and_readiness() {
    let dir = tempfile::tempdir().unwrap();
    let rt = Arc::new(FakeRuntime::default());
    let web = http_responder(204).await;
    rt.map(80, web);
    rt.map(3211, closed_port());
    let sbx = Sandboxes::builder()
        .local(rt.clone())
        .state_dir(dir.path())
        .build();

    let sb = sbx
        .create(
            CreateOptions::new(ProviderKind::Local, "ghcr.io/trycua/plain-ubuntu:24.04")
                .name("dev-box")
                .service("web", 80)
                .wait_for(Probe::Tcp(80))
                .wait_for(Probe::Http {
                    port: 80,
                    path: "/ready".into(),
                    status: Some(204),
                }),
        )
        .await
        .unwrap();
    assert_eq!(sb.runtime_type(), "fake");
    // env (3211) and the declared service port are published; no daemon is
    // probed during create.
    assert!(rt.calls.lock().unwrap()[0].contains("ports=[80, 3211]"));

    // State file in cua-sandbox's shape.
    match sbx.state().load("dev-box").unwrap() {
        SandboxState::Local(l) => {
            assert_eq!(l.runtime_type, "fake");
            assert_eq!(l.host, "127.0.0.1");
            assert_eq!(l.exposed_ports.unwrap()["80"], web);
            assert_eq!(l.image["registry"], "ghcr.io/trycua/plain-ubuntu:24.04");
        }
        other => panic!("{other:?}"),
    }

    // Ports / services / tunnel.
    assert_eq!(
        sb.port(80).unwrap(),
        PortTarget::Addr {
            host: "127.0.0.1".into(),
            port: web
        }
    );
    let r = sb
        .service("web")
        .unwrap()
        .request("GET", "/hello", None, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(r.status, 204);
    assert!(matches!(sb.service("nope"), Err(Error::InvalidArgument(_))));
    let echo = echo_server().await;
    rt.map(7000, echo);
    let sb = sbx.connect("dev-box").await.unwrap(); // picks up the new mapping
    let fwd = sb.tunnel().forward(7000).await.unwrap();
    let mut c = tokio::net::TcpStream::connect(fwd.local_addr.unwrap())
        .await
        .unwrap();
    c.write_all(b"ping").await.unwrap();
    let mut got = [0u8; 4];
    c.read_exact(&mut got).await.unwrap();
    assert_eq!(&got, b"ping");

    // No spacesd: a clean, typed error.
    let err = sb.spacesd_with(quick()).await.unwrap_err();
    assert!(matches!(err, Error::SpacesdNotAvailable { .. }), "{err:?}");
    // keep_alive is Fleet-only.
    assert!(matches!(
        sb.keep_alive(Duration::from_secs(60)).await.unwrap_err(),
        Error::Unsupported { .. }
    ));

    // Lifecycle by name.
    sbx.suspend("dev-box").await.unwrap();
    assert_eq!(sbx.get("dev-box").await.unwrap().status, Status::Suspended);
    sbx.resume("dev-box").await.unwrap();
    sbx.restart("dev-box").await.unwrap();
    assert_eq!(sb.status().await.unwrap(), Status::Running);
    let listed = sbx.list().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].provider, ProviderKind::Local);
    sb.delete().await.unwrap();
    assert!(sbx.state().load("dev-box").is_none());
    assert!(sbx.list().await.unwrap().is_empty());
    let calls = rt.calls.lock().unwrap().join(",");
    assert!(
        calls.contains("suspend dev-box")
            && calls.contains("stop dev-box")
            && calls.contains("delete dev-box")
    );
}

#[tokio::test]
async fn readiness_probe_timeout_cleans_up_ephemeral() {
    let dir = tempfile::tempdir().unwrap();
    let rt = Arc::new(FakeRuntime::default());
    rt.map(8080, closed_port());
    let sbx = Sandboxes::builder()
        .local(rt.clone())
        .state_dir(dir.path())
        .build();
    let mut o = CreateOptions::new(ProviderKind::Local, "img").wait_for(Probe::Tcp(8080));
    o.ready_timeout = Duration::from_millis(600);
    let err = sbx.create(o).await.unwrap_err();
    assert!(matches!(err, Error::Timeout(_)), "{err:?}");
    assert!(
        rt.instances.lock().unwrap().is_empty(),
        "ephemeral instance removed"
    );
    assert!(
        sbx.state().list_all().is_empty(),
        "ephemeral sandboxes write no state"
    );
}

#[tokio::test]
async fn ephemeral_sandboxes_hold_a_lease_until_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let rt = Arc::new(FakeRuntime::default());
    let sbx = Sandboxes::builder()
        .local(rt.clone())
        .state_dir(dir.path())
        .build();
    let sb = sbx
        .create(CreateOptions::new(ProviderKind::Local, "img"))
        .await
        .unwrap();
    let leases = sbx.state().leases();
    assert_eq!(leases.len(), 1);
    assert_eq!(leases[0].name, sb.name());
    assert_eq!(leases[0].pid, std::process::id());
    assert!(rt.specs.lock().unwrap()[0].ephemeral);
    sb.delete().await.unwrap();
    assert!(sbx.state().leases().is_empty());
    // A named sandbox takes no lease (it is never reaped).
    let named = sbx
        .create(CreateOptions::new(ProviderKind::Local, "img").name("keep"))
        .await
        .unwrap();
    assert!(sbx.state().leases().is_empty());
    assert!(!rt.specs.lock().unwrap()[1].ephemeral);
    named.delete().await.unwrap();
    // A daemon creating for a client records the client as the owner.
    let mut o = CreateOptions::new(ProviderKind::Local, "img");
    o.owner_pid = Some(4_000_000);
    let held = sbx.create(o).await.unwrap();
    assert_eq!(sbx.state().leases()[0].pid, 4_000_000);
    held.delete().await.unwrap();
}

#[tokio::test]
async fn a_failed_ephemeral_start_leaves_nothing_behind() {
    let dir = tempfile::tempdir().unwrap();
    let rt = Arc::new(FakeRuntime::default());
    *rt.fail_start.lock().unwrap() = Some(RuntimeError::InsufficientDisk(
        "not enough disk space to create VM x (run `cua cache prune`)".into(),
    ));
    let sbx = Sandboxes::builder()
        .local(rt.clone())
        .state_dir(dir.path())
        .build();
    let err = sbx
        .create(CreateOptions::new(ProviderKind::Local, "img"))
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::Runtime(RuntimeError::InsufficientDisk(_))),
        "{err:?}"
    );
    assert!(rt.instances.lock().unwrap().is_empty(), "instance removed");
    assert!(sbx.state().leases().is_empty(), "lease removed");
    assert!(
        rt.calls
            .lock()
            .unwrap()
            .iter()
            .any(|c| c.starts_with("delete cua-eph-"))
    );
}

#[tokio::test]
async fn readiness_fails_fast_when_the_instance_exits() {
    let dir = tempfile::tempdir().unwrap();
    let rt = Arc::new(FakeRuntime::default());
    rt.map(8080, closed_port());
    *rt.exits.lock().unwrap() = true;
    let sbx = Sandboxes::builder()
        .local(rt.clone())
        .state_dir(dir.path())
        .build();
    let mut o = CreateOptions::new(ProviderKind::Local, "img").wait_for(Probe::Tcp(8080));
    o.ready_timeout = Duration::from_secs(120);
    let started = std::time::Instant::now();
    let err = sbx.create(o).await.unwrap_err();
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "took {:?}",
        started.elapsed()
    );
    assert!(err.to_string().contains("exited"), "{err}");
    assert!(
        rt.instances.lock().unwrap().is_empty(),
        "ephemeral instance removed"
    );
}

/// A forwarder that accepts on the host and closes at once (docker-proxy or
/// slirp with nothing listening in the guest).
async fn accept_and_close() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((s, _)) = l.accept().await {
            drop(s);
        }
    });
    port
}

#[tokio::test]
async fn tcp_probe_asks_the_guest_side_first() {
    let dir = tempfile::tempdir().unwrap();
    let rt = Arc::new(FakeRuntime::default());
    // The host side accepts and holds the connection (a userland proxy),
    // but the guest does not listen yet: not ready.
    rt.map(22, echo_server().await);
    *rt.guest_listening.lock().unwrap() = Some(false);
    let sbx = Sandboxes::builder()
        .local(rt.clone())
        .state_dir(dir.path())
        .build();
    let mut o = CreateOptions::new(ProviderKind::Local, "img").wait_for(Probe::Tcp(22));
    o.ready_timeout = Duration::from_millis(800);
    let err = sbx.create(o.clone()).await.unwrap_err();
    assert!(matches!(err, Error::Timeout(_)), "{err:?}");

    *rt.guest_listening.lock().unwrap() = Some(true);
    let sb = sbx.create(o).await.unwrap();
    sb.delete().await.unwrap();
}

#[tokio::test]
async fn tcp_probe_host_fallback_rejects_accept_and_close() {
    let dir = tempfile::tempdir().unwrap();
    let rt = Arc::new(FakeRuntime::default());
    rt.map(22, accept_and_close().await);
    let sbx = Sandboxes::builder()
        .local(rt.clone())
        .state_dir(dir.path())
        .build();
    let mut o = CreateOptions::new(ProviderKind::Local, "img").wait_for(Probe::Tcp(22));
    o.ready_timeout = Duration::from_millis(1500);
    let err = sbx.create(o).await.unwrap_err();
    assert!(matches!(err, Error::Timeout(_)), "{err:?}");
}

#[tokio::test]
async fn local_env_attaches_when_driver_present() {
    let dir = tempfile::tempdir().unwrap();
    let env_srv = MockServer::start(MockAuth {
        token: Some("tok".into()),
        ..Default::default()
    })
    .await;
    let rt = Arc::new(FakeRuntime::default());
    rt.map(3211, env_srv.addr.port());
    let sbx = Sandboxes::builder()
        .local(rt.clone())
        .state_dir(dir.path())
        .build();
    let mut o = CreateOptions::new(ProviderKind::Local, "img").name("with-env");
    o.env_token = Some("tok".into());
    let sb = sbx.create(o).await.unwrap();
    let env = sb.spacesd().await.unwrap();
    assert_eq!(env.run("echo local").await.unwrap().stdout_str(), "local\n");
    // The token the client presents is the one the guest is started with.
    assert_eq!(
        rt.envs.lock().unwrap()[0]
            .get("CUA_ENV_TOKEN")
            .map(String::as_str),
        Some("tok")
    );

    // A non-env HTTP server on 3211 is not mistaken for the driver.
    let dir2 = tempfile::tempdir().unwrap();
    let rt2 = Arc::new(FakeRuntime::default());
    rt2.map(3211, http_responder(200).await);
    let sbx2 = Sandboxes::builder()
        .local(rt2)
        .state_dir(dir2.path())
        .build();
    let sb2 = sbx2
        .create(CreateOptions::new(ProviderKind::Local, "img"))
        .await
        .unwrap();
    assert!(matches!(
        sb2.spacesd_with(quick()).await.unwrap_err(),
        Error::SpacesdNotAvailable { .. }
    ));
}

#[tokio::test]
async fn local_sandboxes_mint_deliver_and_persist_an_env_token() {
    let dir = tempfile::tempdir().unwrap();
    // A driver that ignores the delivered token (bootstrap mode) still ends
    // up with the minted one: env() installs it with Init.
    let env_srv = MockServer::start(MockAuth::default()).await;
    let rt = Arc::new(FakeRuntime::default());
    rt.map(3211, env_srv.addr.port());
    let sbx = Sandboxes::builder()
        .local(rt.clone())
        .state_dir(dir.path())
        .build();
    let sb = sbx
        .create(CreateOptions::new(ProviderKind::Local, "img").name("cua-e2e-minted"))
        .await
        .unwrap();
    let minted = rt.envs.lock().unwrap()[0]
        .get("CUA_ENV_TOKEN")
        .cloned()
        .expect("a token is minted for a local Linux guest");
    assert_eq!(minted.len(), 32);
    assert!(minted.chars().all(|c| c.is_ascii_hexdigit()));
    // Kept with the sandbox record, owner-only.
    let path = dir.path().join("cua-e2e-minted.json");
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(raw["env_token"], serde_json::Value::String(minted.clone()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "state file mode {mode:o}");
    }
    let env = sb.spacesd().await.unwrap();
    assert_eq!(
        env.run("echo minted").await.unwrap().stdout_str(),
        "minted\n"
    );
    // Another handle (another process) reads the token from the record.
    let again = sbx.connect("cua-e2e-minted").await.unwrap();
    let env2 = again.spacesd().await.unwrap();
    assert_eq!(
        env2.run("echo again").await.unwrap().stdout_str(),
        "again\n"
    );
    // A status change rewrites the file and keeps it owner-only.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        cua_sandbox_core::StateStore::new(dir.path())
            .set_status("cua-e2e-minted", "running")
            .unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    // A caller's token wins; an explicit CUA_ENV_TOKEN in env wins over it.
    let mut o = CreateOptions::new(ProviderKind::Local, "img");
    o.env_token = Some("caller-token-0123456789".into());
    sbx.create(o).await.unwrap();
    let mut o = CreateOptions::new(ProviderKind::Local, "img");
    o.env_token = Some("caller-token-0123456789".into());
    o.env
        .insert("CUA_ENV_TOKEN".into(), "env-token-0123456789".into());
    sbx.create(o).await.unwrap();
    // Windows VMs have no delivery channel: nothing is minted.
    let mut o = CreateOptions::new(ProviderKind::Local, "img");
    o.os = "windows".into();
    sbx.create(o).await.unwrap();
    let envs = rt.envs.lock().unwrap();
    assert_eq!(
        envs[1].get("CUA_ENV_TOKEN").map(String::as_str),
        Some("caller-token-0123456789")
    );
    assert_eq!(
        envs[2].get("CUA_ENV_TOKEN").map(String::as_str),
        Some("env-token-0123456789")
    );
    assert_eq!(envs[3].get("CUA_ENV_TOKEN"), None);
}

// ------------------------------------------------------------------ fleet

const IMAGE: &str = "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:main-38352d34";

/// A fake Fleet whose (offline) registry knows `IMAGE` is a containerDisk.
fn fleet_fake() -> FakeFleet {
    let fake = FakeFleet::new();
    cua_fleet::testing::set_image_variant(IMAGE, cua_fleet::ImageVariant::ContainerDisk);
    fake
}

#[tokio::test]
async fn fleet_claim_on_existing_pool_reports_template_services() {
    let dir = tempfile::tempdir().unwrap();
    let fake = fleet_fake();
    let client = fake.client();
    let spec = cua_fleet::PoolSpec::new("cua-e2e-web-pool", IMAGE).services([("web", 8080u16)]);
    client.apply_pool(&spec).await.unwrap();
    let sbx = Sandboxes::builder()
        .fleet(client)
        .state_dir(dir.path())
        .build();
    let mut o = CreateOptions::new(ProviderKind::Fleet, "");
    o.fleet.pool = Some("cua-e2e-web-pool".into());
    let sb = sbx.create(o).await.unwrap();
    assert_eq!(
        sb.services(),
        &BTreeMap::from([("web".to_string(), 8080)]),
        "the pool template's services, not the default env:3211"
    );
    sb.delete().await.unwrap();
}

#[tokio::test]
async fn fleet_runtime_defaults_from_the_image_and_is_validated() {
    let dir = tempfile::tempdir().unwrap();
    let fake = fleet_fake();
    let sbx = Sandboxes::builder()
        .fleet(fake.client())
        .state_dir(dir.path())
        .build();
    let runtime_of = |name: &str| {
        fake.object("template", name, name).unwrap()["spec"]["vmTemplate"]["runtime"]
            .as_str()
            .map(str::to_string)
    };
    // The runtime follows the image's manifest (fixtures here), not its
    // tag: a rootfs runs on gVisor, a containerDisk boots on KubeVirt, even
    // when the tags say the opposite.
    let (rootfs, disk) = (
        "ghcr.io/trycua/cua-desktop-linux:latest-rootfs",
        "ghcr.io/trycua/cua-desktop-linux:docker-disk",
    );
    cua_fleet::testing::set_image_variant(rootfs, cua_fleet::ImageVariant::Rootfs);
    cua_fleet::testing::set_image_variant(disk, cua_fleet::ImageVariant::ContainerDisk);
    for (name, image, runtime) in [
        ("cua-e2e-rt-docker", rootfs, "gvisor"),
        ("cua-e2e-rt-disk", disk, "kubevirt"),
    ] {
        let sb = sbx
            .create(CreateOptions::new(ProviderKind::Fleet, image).name(name))
            .await
            .unwrap();
        // Managed pool: the template lives in the pool's namespace.
        let pool = sb.fleet_sandbox().unwrap().namespace.clone();
        assert_eq!(runtime_of(&pool).as_deref(), Some(runtime), "{image}");
        sb.delete().await.unwrap();
    }
    // An explicit runtime the image cannot run on is refused before any
    // pool exists.
    let mut o = CreateOptions::new(ProviderKind::Fleet, disk).name("cua-e2e-rt-crossed");
    o.fleet.runtime = Some(cua_fleet::RuntimeKind::Gvisor);
    let err = sbx.create(o).await.unwrap_err();
    assert!(err.to_string().contains("containerDisk"), "{err}");
    assert_eq!(
        fake.all_namespaces()
            .iter()
            .filter(|n| n.starts_with("cua-auto-"))
            .count(),
        2,
        "no pool for the refused pairing"
    );
    // An image whose manifest cannot be read falls back to its reference:
    // a `docker-` tag runs on gVisor.
    let sb = sbx
        .create(
            CreateOptions::new(ProviderKind::Fleet, "registry.example/unknown:docker-1")
                .name("cua-e2e-rt-unknown"),
        )
        .await
        .unwrap();
    let pool = sb.fleet_sandbox().unwrap().namespace.clone();
    assert_eq!(runtime_of(&pool).as_deref(), Some("gvisor"));
    sb.delete().await.unwrap();
}

/// A named cloud sandbox whose readiness never passes (the gateway's canned
/// answer is not the status the probe waits for).
fn never_ready(name: &str) -> CreateOptions {
    let mut o = CreateOptions::new(ProviderKind::Fleet, IMAGE)
        .name(name)
        .service("server", 8000)
        .wait_for(Probe::Http {
            port: 8000,
            path: "/status".into(),
            status: Some(599),
        });
    o.ready_timeout = Duration::from_millis(300);
    o
}

#[tokio::test]
async fn fleet_named_sandbox_that_fails_readiness_releases_its_claim() {
    let dir = tempfile::tempdir().unwrap();
    let fake = fleet_fake();
    let sbx = Sandboxes::builder()
        .fleet(fake.client())
        .state_dir(dir.path().join("sandboxes"))
        .build();
    let err = sbx
        .create(never_ready("cua-e2e-unready"))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Timeout(_)), "{err:?}");
    let pool = fake
        .all_namespaces()
        .into_iter()
        .find(|n| n.starts_with("cua-auto-"))
        .expect("a managed pool");
    assert!(
        !fake.exists("claim", &pool, "cua-e2e-unready"),
        "a named sandbox's claim is released when create fails"
    );
    assert!(
        sbx.state().load("cua-e2e-unready").is_none(),
        "no state file"
    );
}

#[tokio::test]
async fn fleet_named_sandbox_kept_on_failure_keeps_its_claim() {
    let dir = tempfile::tempdir().unwrap();
    let fake = fleet_fake();
    let sbx = Sandboxes::builder()
        .fleet(fake.client())
        .state_dir(dir.path().join("sandboxes"))
        .build();
    let mut o = never_ready("cua-e2e-kept");
    o.keep_on_failure = true;
    let err = sbx.create(o).await.unwrap_err();
    assert!(err.to_string().contains("cua-e2e-kept"), "{err}");
    let pool = fake
        .all_namespaces()
        .into_iter()
        .find(|n| n.starts_with("cua-auto-"))
        .expect("a managed pool");
    assert!(fake.exists("claim", &pool, "cua-e2e-kept"), "claim kept");
    match sbx.state().load("cua-e2e-kept") {
        Some(SandboxState::Fleet(f)) => assert_eq!(f.pool_name, pool),
        other => panic!("{other:?}"),
    }
    // The kept claim is addressable by its ref and deletes normally.
    let info = sbx.get("cua-e2e-kept").await.unwrap();
    assert_eq!(info.id, "cloud:cua-e2e-kept");
    sbx.delete("cua-e2e-kept").await.unwrap();
    assert!(!fake.exists("claim", &pool, "cua-e2e-kept"));
}

#[tokio::test]
async fn fleet_named_sandbox_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let fake = fleet_fake();
    let sbx = Sandboxes::builder()
        .fleet(fake.client())
        .state_dir(dir.path().join("sandboxes"))
        .build();
    let sb = sbx
        .create(
            CreateOptions::new(ProviderKind::Fleet, IMAGE)
                .name("cua-e2e-named")
                .service("server", 8000)
                .wait_for(Probe::Http {
                    port: 8000,
                    path: "/status".into(),
                    status: None,
                }),
        )
        .await
        .unwrap();
    assert_eq!(sb.provider(), ProviderKind::Fleet);
    let bound = sb.fleet_sandbox().unwrap().clone();
    assert_eq!(bound.claim, "cua-e2e-named");
    // A managed, autoscaled pool keyed by image and shape.
    let pool = bound.namespace.clone();
    assert!(pool.starts_with("cua-auto-"), "{pool}");
    let p = fake.object("pool", &pool, &pool).unwrap();
    assert_eq!(p["spec"]["autoscaling"]["minPoolSize"], 0);
    let t = fake.object("template", &pool, &pool).unwrap();
    let names: Vec<&str> = t["spec"]["vmTemplate"]["services"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["env", "server"]);
    assert!(
        t["spec"]["vmTemplate"].get("probes").is_none(),
        "no daemon readiness probe"
    );
    let claim = fake.object("claim", &pool, "cua-e2e-named").unwrap();
    assert_eq!(claim["spec"]["ttlSecondsAfterCreated"], 900);
    match sbx.state().load("cua-e2e-named").unwrap() {
        SandboxState::Fleet(f) => assert_eq!(f.pool_name, pool),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        sb.port(8000).unwrap(),
        PortTarget::Url(format!(
            "https://fleet.test/api/svc/{pool}/{}-server",
            bound.name
        ))
    );
    let fwd = sb.tunnel().forward(8000).await.unwrap();
    assert_eq!(fwd.via, ForwardVia::GatewayProxy);
    assert!(
        fwd.local_addr.is_some() && fwd.url.as_deref().unwrap().starts_with("http://127.0.0.1:")
    );
    fwd.close().await.unwrap();
    // Service requests go straight to the gateway with its bearer and the
    // claim header (the pipe itself is tested in tests/mcp.rs).
    let ep = sb.service("server").unwrap().endpoint().await.unwrap();
    assert_eq!(
        ep.url,
        format!("https://fleet.test/api/svc/{pool}/{}-server", bound.name)
    );
    assert!(
        ep.headers
            .iter()
            .any(|(k, v)| k == "x-cua-fleet-claim" && v == &bound.claim)
    );
    assert!(
        ep.headers
            .iter()
            .any(|(k, v)| k == "authorization" && v.starts_with("Bearer "))
    );
    // Dropping a named handle detaches: the claim outlives it (TTL).
    drop(sb);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(fake.exists("claim", &pool, "cua-e2e-named"));

    // Reconnect from state in a fresh manager, keep alive.
    let sbx2 = Sandboxes::builder()
        .fleet(fake.client())
        .state_dir(dir.path().join("sandboxes"))
        .build();
    let again = sbx2.connect("cua-e2e-named").await.unwrap();
    assert_eq!(again.fleet_sandbox().unwrap(), &bound);
    assert_eq!(
        again.services(),
        &BTreeMap::from([("env".to_string(), 3211), ("server".to_string(), 8000)]),
        "reattached services carry the template's ports"
    );
    sbx2.keep_alive("cua-e2e-named", Duration::from_secs(900))
        .await
        .unwrap();
    let claim = fake.object("claim", &pool, "cua-e2e-named").unwrap();
    assert!(claim["spec"]["lifecycle"]["shutdownTime"].is_string());
    // Fleet cannot suspend one claim, and a shared pool is never scaled:
    // suspend and restart are typed Unsupported; resume of a live claim
    // reattaches.
    let replicas = fake.object("pool", &pool, &pool).unwrap()["spec"]["replicas"].clone();
    for op in ["suspend", "restart"] {
        let err = match op {
            "suspend" => sbx2.suspend("cua-e2e-named").await,
            _ => sbx2.restart("cua-e2e-named").await,
        }
        .unwrap_err();
        assert!(
            matches!(
                err,
                Error::Unsupported {
                    provider: ProviderKind::Fleet,
                    ..
                }
            ) && err.to_string().contains("cannot suspend a single sandbox"),
            "{err:?}"
        );
    }
    assert!(fake.exists("claim", &pool, "cua-e2e-named"));
    sbx2.resume("cua-e2e-named").await.unwrap();
    assert_eq!(
        sbx2.get("cua-e2e-named").await.unwrap().status,
        Status::Running
    );
    let t = fake.object("template", &pool, &pool).unwrap();
    assert_eq!(
        t["spec"]["vmTemplate"]["services"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(fake.exists("claim", &pool, "cua-e2e-named"));
    assert_eq!(
        fake.object("pool", &pool, &pool).unwrap()["spec"]["replicas"],
        replicas,
        "never resized"
    );
    assert_eq!(
        fake.all_namespaces()
            .iter()
            .filter(|n| n.starts_with("cua-auto-"))
            .count(),
        1,
        "resume reuses the pool"
    );
    let info = sbx2.get("cua-e2e-named").await.unwrap();
    assert_eq!(info.provider, ProviderKind::Fleet);
    sbx2.delete("cua-e2e-named").await.unwrap();
    assert!(!fake.exists("claim", &pool, "cua-e2e-named"));
    assert!(sbx2.state().load("cua-e2e-named").is_none());
    assert!(
        fake.exists("pool", &pool, &pool),
        "managed pools are reused"
    );
    // The manager cache lives next to the state dir, not in ~/.cua.
    assert!(dir.path().join("fleet-pools.json").exists());
}

#[tokio::test]
async fn fleet_ephemeral_sandboxes_share_a_managed_pool_and_release_claims() {
    let dir = tempfile::tempdir().unwrap();
    let fake = fleet_fake();
    let sbx = Sandboxes::builder()
        .fleet(fake.client())
        .state_dir(dir.path().join("sandboxes"))
        .build();
    let sb = sbx
        .create(CreateOptions::new(ProviderKind::Fleet, IMAGE))
        .await
        .unwrap();
    assert!(sb.is_ephemeral());
    let pool = sb.fleet_sandbox().unwrap().namespace.clone();
    assert!(pool.starts_with("cua-auto-"), "{pool}");
    assert!(sbx.state().list_all().is_empty());
    let claim = sb.fleet_sandbox().unwrap().claim.clone();
    sb.delete().await.unwrap();
    assert!(!fake.exists("claim", &pool, &claim));
    assert!(
        fake.exists("pool", &pool, &pool),
        "the pool stays for reuse"
    );

    // Same image and shape: same pool. Dropping the handle releases.
    let sb2 = sbx
        .create(CreateOptions::new(ProviderKind::Fleet, IMAGE))
        .await
        .unwrap();
    assert_eq!(sb2.fleet_sandbox().unwrap().namespace, pool);
    let claim2 = sb2.fleet_sandbox().unwrap().claim.clone();
    let clone = sb2.clone();
    drop(sb2);
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(
        fake.exists("claim", &pool, &claim2),
        "a clone still holds it"
    );
    drop(clone);
    for _ in 0..100 {
        if !fake.exists("claim", &pool, &claim2) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!fake.exists("claim", &pool, &claim2));

    // Different shape: another pool.
    let mut o = CreateOptions::new(ProviderKind::Fleet, IMAGE);
    o.cpus = 4;
    o.fleet.warm = Some(true);
    o.fleet.max_pool_size = Some(3);
    let sb3 = sbx.create(o).await.unwrap();
    let pool3 = sb3.fleet_sandbox().unwrap().namespace.clone();
    assert_ne!(pool3, pool);
    let p3 = fake.object("pool", &pool3, &pool3).unwrap();
    assert_eq!(p3["spec"]["replicas"], 1);
    assert_eq!(p3["spec"]["autoscaling"]["maxPoolSize"], 3);
    sb3.delete().await.unwrap();
    assert_eq!(sbx.pools().unwrap().list().await.unwrap().len(), 2);
}

#[tokio::test]
async fn fleet_managed_detach_keeps_the_claim_and_windows_gets_efi() {
    let dir = tempfile::tempdir().unwrap();
    let fake = fleet_fake();
    let sbx = Sandboxes::builder()
        .fleet(fake.client())
        .state_dir(dir.path().join("sandboxes"))
        .build();
    let mut o = CreateOptions::new(ProviderKind::Fleet, IMAGE);
    o.os = "windows".into();
    let sb = sbx.create(o).await.unwrap();
    let b = sb.fleet_sandbox().unwrap().clone();
    let t = fake.object("template", &b.namespace, &b.namespace).unwrap();
    assert_eq!(t["spec"]["vmTemplate"]["firmware"], "efi");
    sb.detach();
    drop(sb);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        fake.exists("claim", &b.namespace, &b.claim),
        "detached, not released"
    );
}

#[tokio::test]
async fn fleet_probe_timeout_releases_resources() {
    let dir = tempfile::tempdir().unwrap();
    let fake = fleet_fake();
    fake.faults.lock().unwrap().service_status = Some(503);
    let sbx = Sandboxes::builder()
        .fleet(fake.client())
        .state_dir(dir.path().join("sandboxes"))
        .build();
    let mut o = CreateOptions::new(ProviderKind::Fleet, IMAGE).wait_for(Probe::Tcp(3211));
    o.ready_timeout = Duration::from_millis(500);
    let err = sbx.create(o).await.unwrap_err();
    assert!(matches!(err, Error::Timeout(_)), "{err:?}");
    let reqs = fake.requests();
    let claim_deletes = reqs
        .iter()
        .filter(|r| r.method == "DELETE" && r.path.contains("osgymsandboxclaims"))
        .count();
    let pool_deletes = reqs
        .iter()
        .filter(|r| r.method == "DELETE" && r.path.contains("osgymsandboxwarmpools"))
        .count();
    assert_eq!(claim_deletes, 1, "claim released after failed readiness");
    assert_eq!(pool_deletes, 0, "the managed pool stays for reuse");
}

#[tokio::test]
async fn fleet_explicit_pool_keeps_its_semantics() {
    let dir = tempfile::tempdir().unwrap();
    let fake = fleet_fake();
    let fleet = fake.client();
    fleet
        .apply_pool(&cua_fleet::PoolSpec::new("my-pool", IMAGE))
        .await
        .unwrap();
    let sbx = Sandboxes::builder()
        .fleet(fleet)
        .state_dir(dir.path().join("sandboxes"))
        .build();
    let mut o = CreateOptions::new(ProviderKind::Fleet, IMAGE).name("mine");
    o.fleet.pool = Some("my-pool".into());
    let sb = sbx.create(o).await.unwrap();
    assert_eq!(sb.fleet_sandbox().unwrap().namespace, "my-pool");
    let claim = fake.object("claim", "my-pool", "mine").unwrap();
    assert!(claim["spec"].get("ttlSecondsAfterCreated").is_none());
    // A user-owned pool is never scaled through one of its sandboxes.
    let replicas = fake.object("pool", "my-pool", "my-pool").unwrap()["spec"]["replicas"].clone();
    assert!(matches!(
        sbx.suspend("mine").await.unwrap_err(),
        Error::Unsupported { .. }
    ));
    assert_eq!(
        fake.object("pool", "my-pool", "my-pool").unwrap()["spec"]["replicas"],
        replicas
    );
    sb.delete().await.unwrap();
    assert!(fake.exists("pool", "my-pool", "my-pool"));
    assert!(!fake.exists("claim", "my-pool", "mine"));
}

#[tokio::test]
async fn fleet_env_through_gateway_and_missing_env_service() {
    let dir = tempfile::tempdir().unwrap();
    let fake = fleet_fake();
    // The fake binds claim X to sandbox `sbx-X`, so the gateway prefix is
    // known up front.
    let srv = MockServer::start(MockAuth {
        token: None,
        gateway: Some(MockGateway {
            prefix: "/api/svc/cua-e2e-gw/sbx-cua-e2e-gw-env".into(),
            bearer: "fake-fleet-token".into(),
            claim: "cua-e2e-gw".into(),
        }),
        prefix: None,
    })
    .await;
    let sbx = Sandboxes::builder()
        .fleet(fake.client_with_base(&srv.url()))
        .state_dir(dir.path().join("sandboxes"))
        .build();
    fake.client_with_base(&srv.url())
        .apply_pool(&cua_fleet::PoolSpec::new("cua-e2e-gw", IMAGE))
        .await
        .unwrap();
    let mut o = CreateOptions::new(ProviderKind::Fleet, IMAGE).name("cua-e2e-gw");
    o.fleet.pool = Some("cua-e2e-gw".into());
    let sb = sbx.create(o).await.unwrap();
    let env = sb.spacesd().await.unwrap();
    assert_eq!(env.transport(), cua_spacesd_client::Transport::GrpcWeb);
    assert_eq!(env.run("echo gw").await.unwrap().stdout_str(), "gw\n");

    // Without the tunnel capability, Fleet forwards a declared service's
    // port through a loopback proxy that attaches the gateway bearer and
    // claim (any HTTP client works, here gRPC-Web with no Fleet headers),
    // and says why for any other port.
    let fwd = sb.tunnel().forward(3211).await.unwrap();
    assert_eq!(fwd.via, ForwardVia::GatewayProxy);
    let url = fwd.url.clone().unwrap();
    assert!(url.starts_with("http://127.0.0.1:"), "{url}");
    let plain = cua_spacesd_client::ConnectOptions::parse(&url)
        .unwrap()
        .transport(cua_spacesd_client::TransportPreference::GrpcWeb);
    let via_proxy = cua_spacesd_client::SpacesdClient::connect(plain)
        .await
        .unwrap();
    assert_eq!(via_proxy.run("echo px").await.unwrap().stdout_str(), "px\n");
    fwd.close().await.unwrap();
    match sb.tunnel().forward(8080).await.unwrap_err() {
        Error::Unsupported { op, .. } => assert!(op.contains("tunnel.forward"), "{op}"),
        other => panic!("{other:?}"),
    }
    // With it, a real TCP forward over the spacesd's /tunnel WebSocket,
    // through the gateway prefix with the gateway bearer and claim.
    srv.state
        .advertise(&[cua_spacesd_client::TUNNEL_FORWARD_FEATURE]);
    let echo = echo_server().await;
    let fwd = sb.tunnel().forward(echo).await.unwrap();
    assert_eq!(fwd.via, ForwardVia::EnvTunnel);
    let addr = fwd.local_addr.unwrap();
    let conns: Vec<_> = (0..4u8)
        .map(|i| {
            tokio::spawn(async move {
                let mut c = tokio::net::TcpStream::connect(addr).await.unwrap();
                let msg = [b'a' + i; 32];
                c.write_all(&msg).await.unwrap();
                let mut got = [0u8; 32];
                tokio::time::timeout(Duration::from_secs(10), c.read_exact(&mut got))
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(got, msg);
            })
        })
        .collect();
    for c in conns {
        c.await.unwrap();
    }
    let attaches = srv.state.tunnel_attaches();
    assert_eq!(attaches.len(), 4);
    for a in &attaches {
        assert!(a.accepted, "{a:?}");
        assert!(
            a.path
                .starts_with("/api/svc/cua-e2e-gw/sbx-cua-e2e-gw-env/tunnel?ticket="),
            "{a:?}"
        );
        assert_eq!(a.authorization.as_deref(), Some("Bearer fake-fleet-token"));
        assert_eq!(a.claim.as_deref(), Some("cua-e2e-gw"));
    }
    fwd.close().await.unwrap();
    assert!(srv.state.open_forwards().is_empty());

    // Claiming from an existing pool whose image exposes only a legacy
    // service: no `env` service, so env() fails cleanly without probing.
    fake.client()
        .apply_pool(
            &cua_fleet::PoolSpec::new("cua-e2e-legacy", IMAGE).services([("server", 8000u16)]),
        )
        .await
        .unwrap();
    let mut o = CreateOptions::new(ProviderKind::Fleet, IMAGE).name("legacy-claim");
    o.fleet.pool = Some("cua-e2e-legacy".into());
    let sb2 = sbx.create(o).await.unwrap();
    assert_eq!(
        sb2.fleet_sandbox().unwrap().services,
        vec!["server".to_string()]
    );
    match sb2.spacesd().await.unwrap_err() {
        Error::SpacesdNotAvailable { reason, .. } => assert!(reason.contains("no `env` service")),
        other => panic!("{other:?}"),
    }
    sb2.delete().await.unwrap();
    assert!(
        fake.exists("pool", "cua-e2e-legacy", "cua-e2e-legacy"),
        "claiming never deletes a shared pool"
    );
}

// ----------------------------------------------------------------- direct

#[tokio::test]
async fn direct_provider_against_mock_env_server() {
    let dir = tempfile::tempdir().unwrap();
    let srv = MockServer::start(MockAuth {
        token: Some("direct-token".into()),
        ..Default::default()
    })
    .await;
    let sbx = Sandboxes::builder().state_dir(dir.path()).build();
    let sb = sbx
        .connect_url(&srv.url(), Some("direct-token".into()))
        .unwrap();
    assert_eq!(sb.provider(), ProviderKind::Direct);
    // Readiness is user-declared only; a TCP probe on the env port passes.
    sb.wait_ready(&[Probe::Tcp(srv.addr.port())], Duration::from_secs(5))
        .await
        .unwrap();

    let env = sb.spacesd().await.unwrap();
    assert_eq!(env.transport(), cua_spacesd_client::Transport::Native);
    assert_eq!(
        env.run("echo direct").await.unwrap().stdout_str(),
        "direct\n"
    );
    let body: Vec<u8> = (0..5_000_000u32).map(|i| (i % 251) as u8).collect();
    env.upload("/d.bin", body.clone(), Default::default())
        .await
        .unwrap();
    assert_eq!(env.download("/d.bin").await.unwrap().to_vec(), body);

    // Without the driver's tunnel capability, forwarding is unsupported
    // (never a raw TCP guess at the endpoint host).
    match sb.tunnel().forward(srv.addr.port()).await.unwrap_err() {
        Error::Unsupported { provider, op } => {
            assert_eq!(provider, ProviderKind::Direct);
            assert!(op.contains("tunnel.forward"), "{op}");
        }
        other => panic!("{other:?}"),
    }
    assert!(srv.state.tunnel_attaches().is_empty());

    // Through a tunnel.forward of the env port (over the driver's /tunnel
    // WebSocket), the driver still answers.
    srv.state
        .advertise(&[cua_spacesd_client::TUNNEL_FORWARD_FEATURE]);
    let fwd = sb.tunnel().forward(srv.addr.port()).await.unwrap();
    assert_eq!(fwd.via, ForwardVia::EnvTunnel);
    let via = sbx
        .connect_url(
            &fwd.local_addr.unwrap().to_string(),
            Some("direct-token".into()),
        )
        .unwrap();
    assert!(
        via.spacesd()
            .await
            .unwrap()
            .run("echo tunneled")
            .await
            .unwrap()
            .success()
    );
    assert!(srv.state.tunnel_attaches().iter().all(|a| a.accepted));
    assert!(!srv.state.tunnel_attaches().is_empty());
    fwd.close().await.unwrap();
    assert!(srv.state.open_forwards().is_empty());

    // Remembered by name.
    sbx.remember_direct("my-direct", &srv.url()).unwrap();
    let named = sbx.connect("my-direct").await.unwrap();
    assert_eq!(named.name(), "my-direct");
    assert_eq!(
        sbx.get("my-direct").await.unwrap().provider,
        ProviderKind::Direct
    );
    assert!(matches!(
        sbx.suspend("my-direct").await.unwrap_err(),
        Error::Unsupported { .. }
    ));
    sbx.delete("my-direct").await.unwrap();
    assert!(matches!(
        sbx.connect("my-direct").await.unwrap_err(),
        Error::NotFound(_)
    ));

    // Wrong token: an auth error, not "not available".
    let bad = sbx.connect_url(&srv.url(), Some("nope".into())).unwrap();
    assert!(matches!(
        bad.spacesd().await.unwrap_err(),
        Error::Env(cua_spacesd_client::Error::Unauthenticated(_))
    ));

    // A machine without the driver.
    let plain = sbx
        .connect_url(&format!("127.0.0.1:{}", http_responder(404).await), None)
        .unwrap();
    assert!(matches!(
        plain.spacesd_with(quick()).await.unwrap_err(),
        Error::SpacesdNotAvailable { .. }
    ));
}

#[tokio::test]
async fn unconfigured_providers() {
    let dir = tempfile::tempdir().unwrap();
    let sbx = Sandboxes::builder().state_dir(dir.path()).build();
    assert!(matches!(
        sbx.create(CreateOptions::new(ProviderKind::Fleet, IMAGE))
            .await
            .unwrap_err(),
        Error::ProviderNotConfigured(ProviderKind::Fleet)
    ));
    assert!(matches!(
        sbx.create(CreateOptions::new(ProviderKind::Local, IMAGE))
            .await
            .unwrap_err(),
        Error::ProviderNotConfigured(ProviderKind::Local)
    ));
    assert!(matches!(
        sbx.create(CreateOptions::new(ProviderKind::Direct, IMAGE))
            .await
            .unwrap_err(),
        Error::InvalidArgument(_)
    ));
}

// ------------------------------------------------ command, env, services

#[tokio::test]
async fn local_command_env_and_named_service_probes() {
    let dir = tempfile::tempdir().unwrap();
    let rt = Arc::new(FakeRuntime::default());
    let health = http_responder(200).await;
    rt.map(8765, health);
    let sbx = Sandboxes::builder()
        .local(rt.clone())
        .state_dir(dir.path())
        .build();
    let mut o = CreateOptions::new(ProviderKind::Local, "python:3.12-slim")
        .name("cua-e2e-cmd")
        .service("mcp", 8765)
        .command(["python", "-m", "srv", "--port", "8765"])
        .wait_for_service("mcp", Some("health"))
        .unwrap();
    o.env.insert("GREETING".into(), "hi".into());
    // An undeclared service cannot be probed.
    assert!(matches!(
        o.service_probe("nope", None),
        Err(Error::InvalidArgument(_))
    ));
    let sb = sbx.create(o).await.unwrap();
    let spec = rt.specs.lock().unwrap().last().cloned().unwrap();
    assert_eq!(
        spec.command,
        Some(vec![
            "python".to_string(),
            "-m".into(),
            "srv".into(),
            "--port".into(),
            "8765".into()
        ])
    );
    assert_eq!(spec.env.get("GREETING").map(String::as_str), Some("hi"));
    assert!(spec.ports.contains(&8765));
    assert_eq!(spec.probes.len(), 1);
    assert_eq!(spec.probes[0].port, 8765);
    assert_eq!(spec.probes[0].http_path.as_deref(), Some("/health"));
    // Portable info.
    assert_eq!(sb.id(), "local:cua-e2e-cmd");
    assert_eq!(sb.location(), "local");
    assert_eq!(sb.expires_at(), None);
    let d = sb.provider_details();
    assert_eq!(d.get("backend").map(String::as_str), Some("fake"));
    assert!(!d.contains_key("pool"));
    sb.delete().await.unwrap();
}

#[tokio::test]
async fn fleet_command_and_env_are_hashed_and_run_on_both_runtimes() {
    let dir = tempfile::tempdir().unwrap();
    let fake = fleet_fake();
    let sbx = Sandboxes::builder()
        .fleet(fake.client())
        .state_dir(dir.path().join("sandboxes"))
        .build();
    let rootfs = "docker.io/library/python:3.12-slim";
    cua_fleet::testing::set_image_variant(rootfs, cua_fleet::ImageVariant::Rootfs);
    let plain = sbx
        .create(CreateOptions::new(ProviderKind::Fleet, rootfs).service("mcp", 8765))
        .await
        .unwrap();
    let plain_pool = plain.fleet_sandbox().unwrap().namespace.clone();
    plain.delete().await.unwrap();

    let o = CreateOptions::new(ProviderKind::Fleet, rootfs)
        .service("mcp", 8765)
        .command(["python", "/srv.py"]);
    let key = o.fleet_pool_key().await.unwrap();
    assert_eq!(key.command, Some(vec!["python".into(), "/srv.py".into()]));
    let sb = sbx.create(o).await.unwrap();
    let pool = sb.fleet_sandbox().unwrap().namespace.clone();
    assert_ne!(pool, plain_pool, "the command is part of the pool key");
    let t = fake.object("template", &pool, &pool).unwrap();
    assert_eq!(
        t["spec"]["vmTemplate"]["command"],
        serde_json::json!(["python", "/srv.py"])
    );
    // Portable info: no Fleet words outside provider_details.
    assert_eq!(sb.location(), "cloud");
    assert!(sb.expires_at().is_some(), "a held claim has an expiry");
    let d = sb.provider_details();
    assert_eq!(d.get("pool"), Some(&pool));
    assert!(d.contains_key("claim") && d.contains_key("namespace"));
    sb.delete().await.unwrap();

    // A readiness probe becomes the pod's TCP readiness (same key again).
    let o = CreateOptions::new(ProviderKind::Fleet, rootfs)
        .service("mcp", 8765)
        .command(["python", "/srv.py"])
        .wait_for_service("mcp", None)
        .unwrap();
    assert_eq!(
        o.fleet_pool_key().await.unwrap().readiness_tcp_port,
        Some(8765)
    );

    // KubeVirt (containerDisk) images run a command too (processMode Run).
    let sb = sbx
        .create(CreateOptions::new(ProviderKind::Fleet, IMAGE).command(["/init"]))
        .await
        .unwrap();
    let pool = sb.fleet_sandbox().unwrap().namespace.clone();
    let vm = fake.object("template", &pool, &pool).unwrap()["spec"]["vmTemplate"].clone();
    assert_eq!(vm["command"], serde_json::json!(["/init"]));
    assert_eq!(vm["processMode"], "Run");
    sb.delete().await.unwrap();
    // env on the cloud: the template's env, run with processMode Run.
    let mut o = CreateOptions::new(ProviderKind::Fleet, rootfs);
    o.env.insert("K".into(), "v".into());
    let sb = sbx.create(o).await.unwrap();
    let pool = sb.fleet_sandbox().unwrap().namespace.clone();
    let vm = fake.object("template", &pool, &pool).unwrap()["spec"]["vmTemplate"].clone();
    assert_eq!(vm["env"], serde_json::json!({"K": "v"}));
    assert_eq!(vm["processMode"], "Run");
    sb.delete().await.unwrap();
    // An explicit pool whose template differs: a mismatch with the diff,
    // never silently ignored.
    let mut o = CreateOptions::new(ProviderKind::Fleet, "").command(["x"]);
    o.fleet.pool = Some(plain_pool.clone());
    let err = sbx.create(o).await.unwrap_err();
    assert!(
        matches!(
            &err,
            Error::Fleet(cua_fleet::Error::PoolSpecMismatch { diffs, .. })
                if diffs.len() == 1 && diffs[0].field == "command"
        ),
        "{err}"
    );
    // A managed pool's template is its key: `apply` is refused there.
    let mut o = CreateOptions::new(ProviderKind::Fleet, "").command(["x"]);
    o.fleet.pool = Some(plain_pool.clone());
    o.fleet.apply = true;
    assert!(matches!(
        sbx.create(o).await.unwrap_err(),
        Error::InvalidArgument(_)
    ));
}

#[tokio::test]
async fn a_named_pool_is_checked_against_the_given_fields_or_applied() {
    let dir = tempfile::tempdir().unwrap();
    let fake = fleet_fake();
    let fleet = fake.client();
    let sbx = Sandboxes::builder()
        .fleet(fleet.clone())
        .state_dir(dir.path().join("sandboxes"))
        .build();
    let rootfs = "docker.io/library/python:3.12-slim";
    cua_fleet::testing::set_image_variant(rootfs, cua_fleet::ImageVariant::Rootfs);
    let spec = cua_fleet::SandboxSpec {
        command: Some(vec!["python".into(), "/srv.py".into()]),
        services: [("mcp".to_string(), 8765)].into(),
        cpu: Some(2),
        ..cua_fleet::SandboxSpec::new(rootfs)
    };
    fleet
        .apply("cua-e2e-named", &spec, &cua_fleet::PoolOptions::default())
        .await
        .unwrap();

    // Matching fields (the image resolves to the template's) claim.
    let mut o = CreateOptions::new(ProviderKind::Fleet, rootfs)
        .service("mcp", 8765)
        .command(["python", "/srv.py"]);
    o.fleet.pool = Some("cua-e2e-named".into());
    let sb = sbx.create(o).await.unwrap();
    assert_eq!(sb.fleet_sandbox().unwrap().namespace, "cua-e2e-named");
    sb.delete().await.unwrap();

    // The default shape (cpus 2 / 4 GiB) is not compared unless given.
    let mut o = CreateOptions::new(ProviderKind::Fleet, "");
    o.cpus = 8;
    o.fleet.pool = Some("cua-e2e-named".into());
    sbx.create(o).await.unwrap().delete().await.unwrap();
    let mut o = CreateOptions::new(ProviderKind::Fleet, "");
    o.cpus = 8;
    o.fleet.cpus_given = true;
    o.fleet.pool = Some("cua-e2e-named".into());
    let err = sbx.create(o).await.unwrap_err().to_string();
    assert!(err.contains("cpu: pool has 2, requested 8"), "{err}");

    // apply=True reconciles the template, then claims.
    let mut o = CreateOptions::new(ProviderKind::Fleet, "").command(["python", "/v2.py"]);
    o.cpus = 4;
    o.fleet.cpus_given = true;
    o.fleet.pool = Some("cua-e2e-named".into());
    o.fleet.apply = true;
    sbx.create(o).await.unwrap().delete().await.unwrap();
    let t = fake
        .object("template", "cua-e2e-named", "cua-e2e-named")
        .unwrap();
    assert_eq!(t["spec"]["vmTemplate"]["command"][1], "/v2.py");
    assert_eq!(t["spec"]["vmTemplate"]["cpuCores"], 4);
    assert_eq!(t["spec"]["vmTemplate"]["services"][0]["targetPort"], 8765);
}

// ------------------------------------------------ spacesd readiness on create

#[tokio::test]
async fn create_waits_for_a_declared_spacesd_only() {
    let dir = tempfile::tempdir().unwrap();
    let env_srv = MockServer::start(MockAuth::default()).await;
    let rt = Arc::new(FakeRuntime::default());
    let sbx = Sandboxes::builder()
        .local(rt.clone())
        .state_dir(dir.path())
        .build();

    // Declared and answering: create returns once Health answers, and the
    // first guest call works at once.
    rt.map(3211, env_srv.addr.port());
    *rt.image_spacesd.lock().unwrap() = Some(Some(true));
    let sb = sbx
        .create(CreateOptions::new(ProviderKind::Local, "img").name("cua-e2e-ready"))
        .await
        .unwrap();
    let env = sb.spacesd().await.unwrap();
    assert_eq!(env.run("echo up").await.unwrap().stdout_str(), "up\n");

    // Declared but never answering: create fails within the budget with a
    // readable timeout (and removes an ephemeral sandbox).
    rt.map(3211, closed_port());
    let mut o = CreateOptions::new(ProviderKind::Local, "img");
    o.ready_timeout = Duration::from_secs(2);
    let started = std::time::Instant::now();
    let err = sbx.create(o).await.unwrap_err();
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "{:?}",
        started.elapsed()
    );
    match &err {
        Error::Timeout(m) => {
            assert!(m.contains("cua-spacesd did not answer"), "{m}");
            assert!(!m.contains("ChannelError"), "{m}");
        }
        other => panic!("unexpected {other:?}"),
    }

    // Not waited on: an image that does not declare spacesd, one that does
    // not say, and a command that replaces the entrypoint.
    for spacesd in [Some(false), None] {
        *rt.image_spacesd.lock().unwrap() = Some(spacesd);
        let mut o = CreateOptions::new(ProviderKind::Local, "img");
        o.ready_timeout = Duration::from_secs(2);
        sbx.create(o).await.unwrap();
    }
    *rt.image_spacesd.lock().unwrap() = Some(Some(true));
    let mut o = CreateOptions::new(ProviderKind::Local, "img");
    o.ready_timeout = Duration::from_secs(2);
    o.command = Some(vec!["sleep".into(), "infinity".into()]);
    sbx.create(o).await.unwrap();
}

// ------------------------------------------------------------ cancel

/// A cancelled local create deletes the instance it started and its
/// state; a create under a name already in use leaves that instance alone;
/// a GPU option reaches the runtime.
#[tokio::test]
async fn a_cancelled_local_create_deletes_only_what_it_made() {
    let dir = tempfile::tempdir().unwrap();
    let rt = Arc::new(FakeRuntime::default());
    *rt.hang_start.lock().unwrap() = true;
    let sbx = Sandboxes::builder()
        .local(rt.clone())
        .state_dir(dir.path())
        .build();
    let cancel = cua_sandbox_core::CancellationToken::new();
    let mut o = CreateOptions::new(ProviderKind::Local, "ghcr.io/x/linux:1");
    o.name = Some("cancel-me".into());
    o.gpu = Some("paravirtual".into());
    let create = sbx.create_cancellable(o, cancel.clone());
    let fire = async {
        while rt.instances.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        cancel.cancel();
    };
    let (r, ()) = tokio::join!(create, fire);
    match r {
        Err(Error::Cancelled(m)) => assert_eq!(m, "removed cancel-me"),
        other => panic!("{:?}", other.map(|s| s.id())),
    }
    assert!(rt.instances.lock().unwrap().is_empty());
    assert!(
        rt.calls
            .lock()
            .unwrap()
            .contains(&"delete cancel-me".to_string())
    );
    assert_eq!(
        rt.specs.lock().unwrap()[0].gpu.as_deref(),
        Some("paravirtual"),
        "the GPU option reaches the runtime"
    );
    // An ephemeral create gets its name up front and is cleaned up too.
    let cancel = cua_sandbox_core::CancellationToken::new();
    let create = sbx.create_cancellable(
        CreateOptions::new(ProviderKind::Local, "ghcr.io/x/linux:1"),
        cancel.clone(),
    );
    let fire = async {
        while rt.instances.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        cancel.cancel();
    };
    let (r, ()) = tokio::join!(create, fire);
    assert!(matches!(r, Err(Error::Cancelled(m)) if m.starts_with("removed cua-eph-")));
    assert!(rt.instances.lock().unwrap().is_empty());
    // A name in use: the running instance is never deleted.
    rt.instances
        .lock()
        .unwrap()
        .insert("theirs".into(), InstanceStatus::Running);
    let cancel = cua_sandbox_core::CancellationToken::new();
    let mut o = CreateOptions::new(ProviderKind::Local, "ghcr.io/x/linux:1");
    o.name = Some("theirs".into());
    let create = sbx.create_cancellable(o, cancel.clone());
    let fire = async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel.cancel();
    };
    let (r, ()) = tokio::join!(create, fire);
    assert!(matches!(r, Err(Error::Cancelled(m)) if m.contains("existed before")));
    assert!(rt.instances.lock().unwrap().contains_key("theirs"));
}
