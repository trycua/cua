//! Sandbox lifecycle over fake providers, plus a Direct-provider integration
//! test against the in-process cua-spacesd-client mock server. Nothing here launches
//! host processes or touches the real `~/.cua` (every test uses a temp
//! state dir).

use async_trait::async_trait;
use cua_sandbox_core::{
    ConnectOptionsOverride, CreateOptions, Error, ForwardVia, InstanceStatus, LocalEndpoints,
    LocalInstance, LocalRuntime, LocalStartSpec, LocalSummary, PortTarget, Probe, ProviderKind,
    RuntimeError, RuntimeResult, SandboxState, Sandboxes, Status,
};
use cua_spacesd_client::testing::{MockAuth, MockServer};
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
    // keep_alive is for providers with a lease.
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

// ----------------------------------------------------------------- direct

const IMAGE: &str = "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:main-38352d34";

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
        Error::CloudClosed
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

/// Records of Cua Cloud sandboxes (closed) stay listed, refuse to connect
/// with the closure message, and `delete` removes them.
#[tokio::test]
async fn cloud_records_are_listed_refused_and_deletable() {
    let dir = tempfile::tempdir().unwrap();
    let sbx = Sandboxes::builder().state_dir(dir.path()).build();
    sbx.state()
        .save_fleet_claim("old-claim", "cua-auto-x")
        .unwrap();
    let listed = sbx.list().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].provider, ProviderKind::Fleet);
    assert_eq!(listed[0].id, "cloud:old-claim");
    for e in [
        sbx.connect("old-claim").await.unwrap_err(),
        sbx.connect_ref(&"cloud:old-claim".parse().unwrap())
            .await
            .unwrap_err(),
        sbx.resume("old-claim").await.unwrap_err(),
        sbx.keep_alive("old-claim", Duration::from_secs(60))
            .await
            .unwrap_err(),
    ] {
        assert!(matches!(e, Error::CloudClosed), "{e:?}");
        assert!(e.to_string().contains("Cua Cloud has closed"), "{e}");
    }
    sbx.delete("old-claim").await.unwrap();
    assert!(sbx.list().await.unwrap().is_empty());
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

/// Readiness failure preserves an observed existing VM, while a newly created
/// VM still receives baseline cleanup. Both state and backend effects are local.
#[tokio::test]
async fn local_readiness_cleanup_distinguishes_existing_from_new() {
    for existing in [true, false] {
        let state = tempfile::tempdir().unwrap();
        let rt = Arc::new(FakeRuntime::default());
        let name = "cua-e2e-readiness";
        if existing {
            rt.instances
                .lock()
                .unwrap()
                .insert(name.into(), InstanceStatus::Running);
        }
        let sbx = Sandboxes::builder()
            .local(rt.clone())
            .state_dir(state.path())
            .build();
        let mut opts = CreateOptions::new(ProviderKind::Local, IMAGE)
            .name(name)
            .wait_for(Probe::Tcp(45678));
        opts.ready_timeout = Duration::from_millis(20);
        let err = sbx.create(opts).await.unwrap_err();
        assert!(matches!(err, Error::Timeout(_)), "{err:?}");
        assert_eq!(rt.instances.lock().unwrap().contains_key(name), existing);
        assert_eq!(sbx.state().load(name).is_some(), existing);
    }
}
