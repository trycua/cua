// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A gang across two hosts whose ranks reach each other only through Cua
//! mesh forwards (CUA-1783), end to end on loopback: the real relay as
//! coordinator, each host's mesh node (`mesh_host`, with its port registry
//! and control socket) and each host's job executor for its own Space
//! (`kind = "machine"`: the members run through a real cua-spacesd) with
//! the mesh peer router. The coordinator's placed addresses are unroutable
//! (TEST-NET-1), so the ranks' ring exchange works only over the mesh.

#![cfg(unix)]

mod common;

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use cua_cluster::gang_mesh::MeshPeerRouter;
use cua_cluster::machine::{HostSpacesd, MachineExecutor};
use cua_cluster::{ExecEnd, JobExecutor as _, JobSpec};
use cua_relay::oidc::testing::FakeIssuer;
use cua_relay::oidc::{OidcConfig, OidcValidator};
use cua_relay::server::{Relay, RelayConfig};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

const ISSUER: &str = "https://auth.test/realms/cua";

/// Each rank listens on its own entry, then (like MLX's ring for two
/// ranks: rank 0 accepts first, rank 1 connects first) exchanges a line
/// with the other rank and a 4 MiB block both ways.
const RING: &str = r#"
import os, socket, sys, time, hashlib
rank = int(os.environ["CUA_RANK"])
peers = os.environ["CUA_PEERS"].split(",")
print("CUA_PEERS=" + os.environ["CUA_PEERS"], flush=True)
print("CUA_PEERS_PLACED=" + os.environ.get("CUA_PEERS_PLACED", ""), flush=True)
def addr(p):
    h, _, port = p.rpartition(":")
    return (h, int(port))
me, other = addr(peers[rank]), addr(peers[1 - rank])
srv = socket.socket(); srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
srv.bind(me); srv.listen(1)
def connect():
    for _ in range(100):
        try:
            return socket.create_connection(other, timeout=60)
        except OSError:
            time.sleep(0.2)
    sys.exit("could not connect to " + str(other))
if rank == 0:
    left, _ = srv.accept(); right = connect()
else:
    right = connect(); left, _ = srv.accept()
block = bytes((i * 31 + rank) % 251 for i in range(4 << 20))
right.sendall(b"hello from rank %d\n" % rank + block)
f = left.makefile("rb")
line = f.readline().decode().strip()
got = f.read(4 << 20)
want = bytes((i * 31 + (1 - rank)) % 251 for i in range(4 << 20))
print("got: " + line, flush=True)
print("block ok" if got == want else "block BAD", flush=True)
"#;

struct World {
    base: String,
    issuer: FakeIssuer,
    stop: CancellationToken,
    _state: tempfile::TempDir,
}

impl Drop for World {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

impl World {
    async fn new() -> Self {
        let issuer = FakeIssuer::new(ISSUER);
        let state = tempfile::tempdir().unwrap();
        let relay = Relay::new(RelayConfig {
            oidc: Some(Arc::new(OidcValidator::with_jwks(
                OidcConfig::new(ISSUER),
                issuer.jwks(),
            ))),
            device_enrollment: false,
            state_file: Some(state.path().join("machines.json")),
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
            _state: state,
        }
    }

    /// Registers host `id` for `owner`, joins it to the relay and runs its
    /// mesh node with gangs in `home`.
    async fn host(&self, owner: &str, id: &str, home: &Path) {
        let token = self.issuer.token(
            owner,
            Some(&format!("{owner}@example.com")),
            "cua-relay",
            600,
        );
        let reg: Value = reqwest::Client::new()
            .post(format!("{}/v1/machines", self.base))
            .bearer_auth(token)
            .json(&json!({"id": id, "name": id}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let machine_token = reg["machine_token"].as_str().unwrap().to_owned();
        let local: SocketAddr = "127.0.0.1:9".parse().unwrap();
        let mut join = cua_relay::client::JoinConfig::new(
            self.base.replace("http://", "ws://"),
            machine_token.clone(),
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
            relay_token: machine_token,
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
            gang_wait: cua_spacesd_server::mesh_host::GANG_WAIT,
        };
        tokio::spawn(cua_spacesd_server::mesh_host::run(cfg, self.stop.clone()));
        let sock = cua_mesh::gang::control_path(home);
        for _ in 0..300 {
            if sock.exists() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("{id}: no mesh control socket");
    }
}

fn member(rank: u32, peers: &str, machines: &str) -> JobSpec {
    let mut env = std::collections::BTreeMap::new();
    for (k, v) in [
        ("CUA_RUN_ID", "ring-1".to_string()),
        ("CUA_ATTEMPT", "1".into()),
        ("CUA_RANK", rank.to_string()),
        ("CUA_WORLD_SIZE", "2".into()),
        ("CUA_PEERS", peers.into()),
        ("CUA_PEER_MACHINES", machines.into()),
    ] {
        env.insert(k.to_string(), v);
    }
    JobSpec {
        job_id: format!("ada/ring-1-{rank}"),
        attempt: 1,
        kind: "machine".into(),
        argv: vec!["python3".into(), "-c".into(), RING.into()],
        env,
        timeout_secs: 120,
        max_output_bytes: 1 << 20,
        run_id: "ada/ring-1".into(),
        ..Default::default()
    }
}

/// A host's executor for its own Space, through the in-process cua-spacesd
/// at `t`, with the mesh peer router of `home`.
fn machine(home: &Path, t: &common::Target) -> MachineExecutor {
    let at = HostSpacesd {
        url: t.url.clone(),
        token: t.token.clone(),
    };
    let exec = MachineExecutor::new(home, move || Ok(at.clone()))
        .with_peer_router(Arc::new(MeshPeerRouter::new(home)));
    common::seed_uv(exec.dirs());
    exec
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_hosts_gang_ranks_reach_each_other_only_over_the_mesh() {
    let w = World::new().await;
    let (home_a, home_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    w.host("ada", "ada-host-ring-a", home_a.path()).await;
    w.host("ada", "ada-host-ring-b", home_b.path()).await;

    // Placed addresses nobody can reach; distinct ports, since both
    // "hosts" share this machine's loopback.
    let peers = format!("192.0.2.1:{},192.0.2.2:{}", free_port(), free_port());
    let machines = "ada-host-ring-a,ada-host-ring-b";
    let t = common::target().await;
    let (ea, eb) = (machine(home_a.path(), &t), machine(home_b.path(), &t));
    let (mut ja, mut jb) = (member(0, &peers, machines), member(1, &peers, machines));
    // The two "hosts" share this machine's users too: as root each runs its
    // member as the run's own user and ends that user when its member ends,
    // which would kill the other host's member. Real hosts are separate
    // computers, so each host here gets its own run user (the run id names it).
    ja.run_id = "ada/ring-1-host-a".into();
    jb.run_id = "ada/ring-1-host-b".into();
    let (sa, sb) = (
        cua_cluster::model::space_name(&ja.job_id, 1),
        cua_cluster::model::space_name(&jb.job_id, 1),
    );
    let (ra, rb) = tokio::time::timeout(Duration::from_secs(180), async {
        tokio::join!(
            ea.run(&ja, &sa, CancellationToken::new()),
            eb.run(&jb, &sb, CancellationToken::new())
        )
    })
    .await
    .expect("the gang did not finish");
    let (oa, ob) = (
        String::from_utf8_lossy(&ra.stdout).into_owned(),
        String::from_utf8_lossy(&rb.stdout).into_owned(),
    );
    println!("rank 0:\n{oa}\nrank 1:\n{ob}");
    assert!(
        matches!(ra.end, ExecEnd::Exited(0)),
        "rank 0: {:?}\n{oa}",
        ra.end
    );
    assert!(
        matches!(rb.end, ExecEnd::Exited(0)),
        "rank 1: {:?}\n{ob}",
        rb.end
    );
    assert!(oa.contains("got: hello from rank 1\nblock ok"), "{oa}");
    assert!(ob.contains("got: hello from rank 0\nblock ok"), "{ob}");
    // Each member says how its peer is reached, sees loopback forwards and
    // binds its own entry on loopback; the placed list stays visible.
    assert!(
        oa.contains("cua: peer rank 1 via mesh (direct) to ada-host-ring-b"),
        "{oa}"
    );
    assert!(
        ob.contains("cua: peer rank 0 via mesh (direct) to ada-host-ring-a"),
        "{ob}"
    );
    for (out, rank) in [(&oa, 0), (&ob, 1)] {
        let line = out
            .lines()
            .find_map(|l| l.strip_prefix("CUA_PEERS="))
            .unwrap();
        let routed: Vec<&str> = line.split(',').collect();
        assert!(routed.iter().all(|p| p.starts_with("127.0.0.1:")), "{line}");
        let own_port = peers
            .split(',')
            .nth(rank)
            .unwrap()
            .rsplit(':')
            .next()
            .unwrap();
        assert_eq!(routed[rank], format!("127.0.0.1:{own_port}"));
        assert!(out.contains(&format!("CUA_PEERS_PLACED={peers}")), "{out}");
    }
    // The members' ports are unregistered once they end.
    for (home, rank) in [(home_a.path(), 0), (home_b.path(), 1)] {
        let port: u16 = peers
            .split(',')
            .nth(rank)
            .unwrap()
            .rsplit(':')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        let reg = cua_mesh::gang::PortRegistry::new(cua_mesh::gang::ports_dir(home));
        assert_eq!(reg.registered(port), None);
    }
}

/// Without a mesh node on the host (the mesh off), members in the
/// machine's own Space keep the placed addresses and say so.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_the_mesh_members_keep_their_placed_addresses() {
    let home = tempfile::tempdir().unwrap();
    let p = free_port();
    let peers = format!("127.0.0.1:{p},192.0.2.2:{}", free_port());
    let t = common::target().await;
    let exec = machine(home.path(), &t);
    let mut job = member(0, &peers, "ada-host-x,ada-host-y");
    // Its own run (so its own run user): the other test runs alongside and
    // ends ada/ring-1's user when its gang is done.
    job.run_id = "ada/no-mesh-1".into();
    job.job_id = "ada/no-mesh-1-0".into();
    job.env.insert("CUA_RUN_ID".into(), "no-mesh-1".into());
    job.argv = vec!["sh".into(), "-c".into(), "echo \"$CUA_PEERS\"".into()];
    let space = cua_cluster::model::space_name(&job.job_id, 1);
    let r = exec.run(&job, &space, CancellationToken::new()).await;
    let out = String::from_utf8_lossy(&r.stdout);
    assert!(
        out.contains("cua: rank traffic uses the placed addresses: the mesh is off"),
        "{out}"
    );
    assert!(
        out.contains("cua: peer rank 1 via placed address (direct, no mesh) to ada-host-y port "),
        "{out}"
    );
    // The notes never show a peer's IP; only the job's own CUA_PEERS line does.
    assert_eq!(out.matches("192.0.2.2").count(), 1, "{out}");
    assert!(out.ends_with(&format!("{peers}\n")), "{out}");
}
