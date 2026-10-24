// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Gang rank traffic between one account's machines over the mesh
//! (CUA-1783), with the real relay as coordinator and a host's mesh node
//! (`mesh_host`):
//!
//! - another machine of the account reaches a `port:<n>` only while a
//!   running job registered `<n>` (and once it listens);
//! - an unregistered port, a port whose job ended, a client device, and a
//!   machine peer asking for spacesd are refused;
//! - the host's job runner gets outbound forwards on the control socket.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

#[cfg(unix)]
use cua_mesh::gang::ControlClient;
use cua_mesh::gang::PortRegistry;
use cua_mesh::{Mesh, MeshConfig, NodeKind};
use cua_relay::oidc::testing::FakeIssuer;
use cua_relay::oidc::{OidcConfig, OidcValidator};
use cua_relay::server::{Relay, RelayConfig};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio_util::sync::CancellationToken;

const ISSUER: &str = "https://auth.test/realms/cua";

struct World {
    base: String,
    issuer: FakeIssuer,
    stop: CancellationToken,
    dir: tempfile::TempDir,
}

impl Drop for World {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

impl World {
    async fn new() -> Self {
        let issuer = FakeIssuer::new(ISSUER);
        let dir = tempfile::tempdir().unwrap();
        let relay = Relay::new(RelayConfig {
            oidc: Some(Arc::new(OidcValidator::with_jwks(
                OidcConfig::new(ISSUER),
                issuer.jwks(),
            ))),
            device_enrollment: false,
            state_file: Some(dir.path().join("machines.json")),
            ..RelayConfig::default()
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let _ = relay.serve(listener, std::future::pending()).await;
        });
        Self {
            base,
            issuer,
            stop: CancellationToken::new(),
            dir,
        }
    }

    fn token(&self, sub: &str) -> String {
        self.issuer
            .token(sub, Some(&format!("{sub}@example.com")), "cua-relay", 600)
    }

    async fn register(&self, owner: &str, id: &str) -> String {
        let reg: Value = reqwest::Client::new()
            .post(format!("{}/v1/machines", self.base))
            .bearer_auth(self.token(owner))
            .json(&json!({"id": id, "name": id}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        reg["machine_token"].as_str().unwrap().to_owned()
    }

    /// A host: joins the relay, and runs its mesh node with gangs in
    /// `home`. Its local spacesd stand-in answers `ok` to anything.
    async fn host(&self, id: &str, token: &str, home: &std::path::Path, wait: Duration) {
        let local = spacesd_stand_in().await;
        let mut join = cua_relay::client::JoinConfig::new(
            self.base.replace("http://", "ws://"),
            token.to_owned(),
            id.to_owned(),
            local,
        );
        join.heartbeat = Duration::from_secs(2);
        let account = join.account.clone();
        tokio::spawn(cua_relay::client::run(join, self.stop.clone()));
        for _ in 0..300 {
            if account.owner().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(account.owner().is_some(), "relay join");
        let cfg = cua_spacesd_server::mesh_host::MeshHostConfig {
            relay_url: self.base.clone(),
            relay_token: token.to_owned(),
            relay_token_file: None,
            machine_id: id.to_owned(),
            dir: home.join("spacesd").join("mesh"),
            local,
            quic: None,
            account,
            env_token: Arc::new(|| None),
            name: id.to_owned(),
            policy: Arc::new(|| cua_mesh::Policy::Prefer),
            recheck: cua_spacesd_server::mesh_host::GATE_RECHECK,
            gang_home: Some(home.to_owned()),
            gang_wait: wait,
        };
        tokio::spawn(cua_spacesd_server::mesh_host::run(cfg, self.stop.clone()));
    }

    /// A plain mesh node: a machine (with its token) or `sub`'s client.
    async fn node(&self, kind: NodeKind, name: &str, token: &str) -> Mesh {
        let mut headers = vec![("authorization".into(), format!("Bearer {token}"))];
        if kind == NodeKind::Machine {
            headers.push(("x-cua-machine-id".into(), name.into()));
        }
        let mut cfg = MeshConfig::new(
            self.base.clone(),
            kind,
            Some(self.dir.path().join(format!("{name}.json"))),
            cua_mesh::static_auth(headers),
        );
        cfg.name = name.into();
        let m = Mesh::start(cfg).await.unwrap();
        m.wait_ready(Duration::from_secs(10)).await.unwrap();
        m
    }
}

async fn spacesd_stand_in() -> SocketAddr {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                let _ = s.read(&mut buf).await;
                let _ = s
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok")
                    .await;
            });
        }
    });
    addr
}

/// A gang member's listener: echoes every connection.
async fn echo_on(port: u16) -> tokio::task::JoinHandle<()> {
    let l = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .unwrap();
    tokio::spawn(async move {
        while let Ok((s, _)) = l.accept().await {
            tokio::spawn(async move {
                let (mut r, mut w) = s.into_split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    })
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn in_map(m: &Mesh, key: &cua_mesh::PublicKey) {
    for _ in 0..200 {
        if m.peers().iter().any(|p| &p.node_key == key) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("peer never appeared");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gang_ports_open_only_to_the_accounts_machines_while_a_job_holds_them() {
    let w = World::new().await;
    let home = tempfile::tempdir().unwrap();
    let ta = w.register("ada", "ada-host-a001").await;
    let tb = w.register("ada", "ada-host-b001").await;
    w.host("ada-host-a001", &ta, home.path(), Duration::from_secs(2))
        .await;
    let b = w.node(NodeKind::Machine, "ada-host-b001", &tb).await;
    let a = b
        .wait_machine("ada-host-a001", Duration::from_secs(20))
        .await
        .expect("a in b's map");
    in_map(&b, &a.node_key).await;
    // A stand-in for host A as B sees it.
    let a_key = a.node_key;
    let registry = PortRegistry::new(cua_mesh::gang::ports_dir(home.path()));
    let port = free_port();
    let svc = cua_mesh::proto::port_service(port);

    let open = |svc: String| {
        let b = b.clone();
        async move {
            let mut s = match b.open_stream(&a_key, &svc, None).await {
                Ok(s) => s,
                Err(_) => return Vec::new(),
            };
            let _ = s.write_all(b"rank-1").await;
            let mut got = vec![0u8; 6];
            match tokio::time::timeout(Duration::from_secs(20), s.read_exact(&mut got)).await {
                Ok(Ok(_)) => got,
                Ok(Err(_)) => Vec::new(),
                Err(_) => panic!("no answer and no refusal on {svc}"),
            }
        }
    };

    // Nothing registered the port: refused (after the short wait), even
    // with something listening on it.
    let listener = echo_on(port).await;
    assert_eq!(open(svc.clone()).await, b"", "an unregistered port");

    // A running job registers it: the account's other machine gets through.
    let reg = registry.register(port, "run-1 attempt 1 rank 0").unwrap();
    assert_eq!(open(svc.clone()).await, b"rank-1");

    // A machine peer never reaches spacesd.
    let mut s = b
        .open_stream(&a_key, cua_spacesd_server::mesh_host::SPACESD_SERVICE, None)
        .await
        .unwrap();
    // The refusal may close the stream before this write lands (Windows
    // reports that as ConnectionReset); either way nothing is answered.
    let _ = s.write_all(b"GET / HTTP/1.1\r\nhost: x\r\n\r\n").await;
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), s.read_to_end(&mut buf)).await;
    assert!(buf.is_empty(), "a machine peer reached spacesd: {buf:?}");

    // The job ends: refused again.
    drop(reg);
    assert_eq!(open(svc.clone()).await, b"", "a port whose job ended");
    listener.abort();

    // A job that registers first and listens a moment later is waited for.
    let late = free_port();
    let reg = registry.register(late, "run-2 attempt 1 rank 0").unwrap();
    let opening = tokio::spawn(open(cua_mesh::proto::port_service(late)));
    tokio::time::sleep(Duration::from_millis(700)).await;
    let _listener = echo_on(late).await;
    assert_eq!(opening.await.unwrap(), b"rank-1");
    drop(reg);

    // A member in a Space registers the Space's address as its target:
    // the stream goes there (here a listener on another port stands in for
    // the Space), not to 127.0.0.1:<n>.
    let gang = free_port();
    let space = free_port();
    let _in_space = echo_on(space).await;
    let reg = registry
        .register_to(
            gang,
            "run-4 attempt 1 rank 1",
            Some(std::net::SocketAddr::from(([127, 0, 0, 1], space))),
        )
        .unwrap();
    assert_eq!(open(cua_mesh::proto::port_service(gang)).await, b"rank-1");
    drop(reg);

    // The account owner's own client device is not a machine: refused.
    let ada = w
        .node(NodeKind::Client, "ada laptop", &w.token("ada"))
        .await;
    let reg = registry.register(port, "run-3 attempt 1 rank 0").unwrap();
    let _listener = echo_on(port).await;
    let a_mesh = ada
        .wait_machine("ada-host-a001", Duration::from_secs(20))
        .await
        .expect("a in ada's map");
    let mut s = ada.open_stream(&a_mesh.node_key, &svc, None).await.unwrap();
    // Refused streams are closed by the host; the write may lose that race.
    let _ = s.write_all(b"rank-1").await;
    let mut got = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), s.read_to_end(&mut got)).await;
    assert!(got.is_empty(), "a client reached a gang port");
    drop(reg);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_hosts_job_runner_forwards_to_another_machines_gang_port() {
    let w = World::new().await;
    let home = tempfile::tempdir().unwrap();
    let ta = w.register("ada", "ada-host-a002").await;
    let tb = w.register("ada", "ada-host-b002").await;
    w.host("ada-host-a002", &ta, home.path(), Duration::from_secs(2))
        .await;
    // Machine B serves port:<n> with an echo (its own mesh node).
    let b = w.node(NodeKind::Machine, "ada-host-b002", &tb).await;
    let echo = b.clone();
    tokio::spawn(async move {
        while let Some(i) = echo.accept().await {
            tokio::spawn(async move {
                let (mut r, mut wr) = tokio::io::split(i.stream);
                let _ = tokio::io::copy(&mut r, &mut wr).await;
            });
        }
    });
    let sock = cua_mesh::gang::control_path(home.path());
    for _ in 0..200 {
        if sock.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let mut ctl = ControlClient::connect(&sock).await.expect("control socket");
    let f = ctl.forward("ada-host-b002", 47012).await.unwrap();
    assert!(["direct", "relayed"].contains(&f.path.as_str()), "{f:?}");
    let mut tcp = tokio::net::TcpStream::connect(f.local).await.unwrap();
    tcp.write_all(b"all_sum").await.unwrap();
    let mut got = [0u8; 7];
    tokio::time::timeout(Duration::from_secs(20), tcp.read_exact(&mut got))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&got, b"all_sum");
    // A forward a Space reaches listens on the address asked for (a
    // private one of this machine), never on a public one.
    let on = ctl
        .forward_on("ada-host-b002", 47012, Some("127.0.0.1".parse().unwrap()))
        .await
        .unwrap();
    assert_eq!(on.local.ip().to_string(), "127.0.0.1");
    let err = ctl
        .forward_on("ada-host-b002", 47012, Some("8.8.8.8".parse().unwrap()))
        .await
        .unwrap_err();
    assert!(err.contains("private address"), "{err}");
    // Bob's machine is not in A's map: no forward.
    let tc = w.register("bob", "bob-host-c002").await;
    let _c = w.node(NodeKind::Machine, "bob-host-c002", &tc).await;
    let err = ctl.forward("bob-host-c002", 47012).await.unwrap_err();
    assert!(err.contains("not on this machine's mesh"), "{err}");
}
