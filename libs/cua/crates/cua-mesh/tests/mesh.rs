// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Two real nodes (UDP sockets, WireGuard, QUIC) and an in-process
//! coordinator: streams, direct and relayed paths, pairing, UDP
//! forwarding, and outages (coordinator crash, relay loss, NAT rebinding).

use std::sync::Arc;
use std::time::Duration;

use cua_mesh::testing::FakeCoordinator;
use cua_mesh::{Error, Mesh, MeshConfig, Mode, NodeKind, Path, PublicKey, static_auth};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

const READY: Duration = Duration::from_secs(10);

async fn node(
    coord: &FakeCoordinator,
    kind: NodeKind,
    dir: &std::path::Path,
    name: &str,
    relay_only: bool,
) -> Mesh {
    let mut headers = Vec::new();
    if kind == NodeKind::Machine {
        headers.push(("x-cua-machine-id".to_owned(), format!("machine-{name}")));
    }
    let mut cfg = MeshConfig::new(
        coord.url.clone(),
        kind,
        Some(dir.join(format!("{name}.json"))),
        static_auth(headers),
    );
    cfg.name = name.to_owned();
    cfg.stun = vec![coord.stun.to_string()];
    cfg.relay_only = relay_only;
    cfg.approved_path = Some(dir.join(format!("{name}-approved.json")));
    let m = Mesh::start(cfg).await.expect("start");
    m.wait_ready(READY).await.expect("ready");
    m
}

/// Serves `echo` streams on a machine: every byte comes back.
fn serve_echo(machine: &Mesh) {
    let m = machine.clone();
    tokio::spawn(async move {
        while let Some(incoming) = m.accept().await {
            assert_eq!(incoming.header.service, "echo");
            tokio::spawn(async move {
                let (mut r, mut w) = tokio::io::split(incoming.stream);
                let _ = tokio::io::copy(&mut r, &mut w).await;
                let _ = w.shutdown().await;
            });
        }
    });
}

async fn wait_peer(m: &Mesh, key: &PublicKey) {
    for _ in 0..100 {
        if m.peers().iter().any(|p| &p.node_key == key) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("peer never appeared");
}

async fn wait_path(m: &Mesh, key: &PublicKey, want_direct: bool, within: Duration) -> Path {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let p = m.path(key).expect("peer");
        if matches!(p, Path::Direct(_)) == want_direct {
            return p;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "path stayed {p} (wanted direct={want_direct})"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn roundtrip(stream: &mut cua_mesh::MeshStream, n: usize) {
    let data: Vec<u8> = (0..n).map(|i| (i * 7) as u8).collect();
    let (mut r, mut w) = tokio::io::split(stream);
    let writer = async {
        w.write_all(&data).await.unwrap();
        w.flush().await.unwrap();
    };
    let mut got = vec![0u8; n];
    let reader = async { r.read_exact(&mut got).await.unwrap() };
    tokio::time::timeout(Duration::from_secs(30), async {
        tokio::join!(writer, reader)
    })
    .await
    .expect("roundtrip timed out");
    assert_eq!(got, data);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn streams_go_direct_and_carry_data() {
    let dir = tempfile::tempdir().unwrap();
    let coord = FakeCoordinator::start(Mode::Account).await;
    let machine = node(&coord, NodeKind::Machine, dir.path(), "m", false).await;
    let client = node(&coord, NodeKind::Client, dir.path(), "c", false).await;
    serve_echo(&machine);
    wait_peer(&client, &machine.node_key()).await;
    let info = client.machine("machine-m").expect("machine in the map");
    assert_eq!(info.addr(), machine.addr());

    let mut s = client
        .open_stream(&machine.node_key(), "echo", None)
        .await
        .unwrap();
    roundtrip(&mut s, 64 * 1024).await;
    // Discovery moves it off the relay.
    wait_path(&client, &machine.node_key(), true, Duration::from_secs(10)).await;
    wait_path(&machine, &client.node_key(), true, Duration::from_secs(10)).await;
    roundtrip(&mut s, 4 << 20).await;
    let st = client.status();
    let peer = st
        .peers
        .iter()
        .find(|p| p.node_key == machine.node_key())
        .unwrap();
    assert!(peer.tx_direct > 4 << 20, "{peer:?}");
    // The machine sees who the client is.
    let seen = machine
        .peers()
        .into_iter()
        .find(|p| p.node_key == client.node_key())
        .unwrap();
    assert_eq!(seen.grant.unwrap().account, "acct-1");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_relay_carries_only_ciphertext_when_direct_fails() {
    let dir = tempfile::tempdir().unwrap();
    let coord = FakeCoordinator::start(Mode::Account).await;
    let machine = node(&coord, NodeKind::Machine, dir.path(), "m", true).await;
    let client = node(&coord, NodeKind::Client, dir.path(), "c", true).await;
    serve_echo(&machine);
    wait_peer(&client, &machine.node_key()).await;
    let mut s = client
        .open_stream(&machine.node_key(), "echo", None)
        .await
        .unwrap();
    let marker = b"PLAINTEXT-MARKER-0123456789".repeat(1000);
    let (mut r, mut w) = tokio::io::split(&mut s);
    w.write_all(&marker).await.unwrap();
    let mut got = vec![0u8; marker.len()];
    tokio::time::timeout(Duration::from_secs(20), r.read_exact(&mut got))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got, marker);
    assert_eq!(client.path(&machine.node_key()), Some(Path::Relay));
    assert!(coord.relayed_bytes() > marker.len() as u64);
    // The relay carried every byte, and never a byte of plaintext.
    assert!(!coord.relayed_contains(b"PLAINTEXT-MARKER"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn direct_sessions_survive_a_coordinator_crash() {
    let dir = tempfile::tempdir().unwrap();
    let coord = FakeCoordinator::start(Mode::Account).await;
    let machine = node(&coord, NodeKind::Machine, dir.path(), "m", false).await;
    let client = node(&coord, NodeKind::Client, dir.path(), "c", false).await;
    serve_echo(&machine);
    wait_peer(&client, &machine.node_key()).await;
    let mut s = client
        .open_stream(&machine.node_key(), "echo", None)
        .await
        .unwrap();
    roundtrip(&mut s, 1024).await;
    wait_path(&client, &machine.node_key(), true, Duration::from_secs(10)).await;

    coord.stop();
    // The open stream keeps working, and new streams open, all direct.
    for _ in 0..5 {
        roundtrip(&mut s, 256 * 1024).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let mut s2 = client
        .open_stream(&machine.node_key(), "echo", None)
        .await
        .unwrap();
    roundtrip(&mut s2, 1024).await;
    assert!(!client.control_state().connected);

    coord.restart().await;
    client.wait_ready(READY).await.unwrap();
    roundtrip(&mut s, 1024).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn relayed_sessions_resume_after_a_relay_restart() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
    let dir = tempfile::tempdir().unwrap();
    let coord = FakeCoordinator::start(Mode::Account).await;
    let machine = node(&coord, NodeKind::Machine, dir.path(), "m", true).await;
    let client = node(&coord, NodeKind::Client, dir.path(), "c", true).await;
    serve_echo(&machine);
    wait_peer(&client, &machine.node_key()).await;
    let mut s = client
        .open_stream(&machine.node_key(), "echo", None)
        .await
        .unwrap();
    roundtrip(&mut s, 1024).await;

    coord.stop();
    tokio::time::sleep(Duration::from_secs(3)).await;
    // Written while the relay is gone: QUIC retransmits once it is back.
    let pending = tokio::spawn(async move {
        roundtrip(&mut s, 64 * 1024).await;
        s
    });
    tokio::time::sleep(Duration::from_secs(2)).await;
    coord.restart().await;
    let mut s = tokio::time::timeout(Duration::from_secs(40), pending)
        .await
        .expect("stream resumed")
        .unwrap();
    roundtrip(&mut s, 1024).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_nat_rebinding_falls_back_and_finds_a_new_direct_path() {
    let dir = tempfile::tempdir().unwrap();
    let coord = FakeCoordinator::start(Mode::Account).await;
    let machine = node(&coord, NodeKind::Machine, dir.path(), "m", false).await;
    let client = node(&coord, NodeKind::Client, dir.path(), "c", false).await;
    serve_echo(&machine);
    wait_peer(&client, &machine.node_key()).await;
    let mut s = client
        .open_stream(&machine.node_key(), "echo", None)
        .await
        .unwrap();
    roundtrip(&mut s, 1024).await;
    let before = wait_path(&client, &machine.node_key(), true, Duration::from_secs(10)).await;

    // The client moves to a new port, as after a network change.
    client.rebind().await.unwrap();
    roundtrip(&mut s, 128 * 1024).await;
    let after = wait_path(&machine, &client.node_key(), true, Duration::from_secs(15)).await;
    let Path::Direct(before) = before else {
        unreachable!()
    };
    let _ = before;
    let Path::Direct(after) = after else {
        unreachable!()
    };
    let (v4, _) = client
        .status()
        .endpoints
        .first()
        .map(|e| (e.port(), ()))
        .unwrap_or((0, ()));
    assert_eq!(
        after.port(),
        v4,
        "the machine follows the client's new port"
    );
    roundtrip(&mut s, 128 * 1024).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn static_mode_pairs_by_proof_only() {
    let dir = tempfile::tempdir().unwrap();
    let coord = FakeCoordinator::start(Mode::Static).await;
    let mut cfg = MeshConfig::new(
        coord.url.clone(),
        NodeKind::Machine,
        Some(dir.path().join("m.json")),
        static_auth(vec![("x-cua-machine-id".into(), "machine-m".into())]),
    );
    cfg.approved_path = Some(dir.path().join("approved.json"));
    cfg.pair_verifier = Some(Arc::new(|client, ts, proof| {
        cua_mesh::proto::verify_pair_proof(
            "the-env-token",
            "machine-m",
            client,
            ts,
            proof,
            cua_mesh::proto::now_secs(),
        )
    }));
    let machine = Mesh::start(cfg).await.unwrap();
    machine.wait_ready(READY).await.unwrap();
    serve_echo(&machine);
    let client = node(&coord, NodeKind::Client, dir.path(), "c", false).await;
    assert!(client.machine("machine-m").is_none(), "unpaired: no peers");

    let wrong = client.pair("machine-m", "not-the-token").await;
    assert!(matches!(wrong, Err(Error::Refused(_))), "{wrong:?}");
    assert!(client.machine("machine-m").is_none());

    client.pair("machine-m", "the-env-token").await.unwrap();
    let info = client
        .wait_machine("machine-m", Duration::from_secs(5))
        .await
        .expect("paired machine in the map");
    let mut s = client
        .open_stream(&info.node_key, "echo", None)
        .await
        .unwrap();
    roundtrip(&mut s, 32 * 1024).await;
    // The approval persisted on the machine.
    let saved = std::fs::read_to_string(dir.path().join("approved.json")).unwrap();
    assert!(saved.contains(&client.node_key().to_base64()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn loopback_forwards_carry_tcp_and_udp() {
    let dir = tempfile::tempdir().unwrap();
    let coord = FakeCoordinator::start(Mode::Account).await;
    let machine = node(&coord, NodeKind::Machine, dir.path(), "m", false).await;
    let client = node(&coord, NodeKind::Client, dir.path(), "c", false).await;
    serve_echo(&machine);
    // A local UDP echo service on the machine, served on the media port.
    let udp_echo = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let echo_addr = udp_echo.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buf = [0u8; 2048];
        loop {
            let (n, src) = udp_echo.recv_from(&mut buf).await.unwrap();
            udp_echo.send_to(&buf[..n], src).await.unwrap();
        }
    });
    machine
        .serve_udp(cua_mesh::MEDIA_UDP_PORT, echo_addr)
        .unwrap();
    wait_peer(&client, &machine.node_key()).await;

    let fwd = client
        .forward(machine.node_key(), "echo", None)
        .await
        .unwrap();
    let mut tcp = tokio::net::TcpStream::connect(fwd.local).await.unwrap();
    tcp.write_all(b"hello over tcp").await.unwrap();
    let mut got = [0u8; 14];
    tcp.read_exact(&mut got).await.unwrap();
    assert_eq!(&got, b"hello over tcp");

    let udp = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let mut ok = false;
    for _ in 0..20 {
        udp.send_to(b"hello over udp", fwd.local).await.unwrap();
        let mut buf = [0u8; 64];
        if let Ok(Ok((n, _))) =
            tokio::time::timeout(Duration::from_millis(500), udp.recv_from(&mut buf)).await
        {
            assert_eq!(&buf[..n], b"hello over udp");
            ok = true;
            break;
        }
    }
    assert!(ok, "udp echo over the mesh");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn removed_peers_are_cut() {
    let dir = tempfile::tempdir().unwrap();
    let coord = FakeCoordinator::start(Mode::Static).await;
    let mut cfg = MeshConfig::new(
        coord.url.clone(),
        NodeKind::Machine,
        Some(dir.path().join("m.json")),
        static_auth(vec![("x-cua-machine-id".into(), "machine-m".into())]),
    );
    cfg.approved_path = Some(dir.path().join("approved.json"));
    cfg.pair_verifier = Some(Arc::new(|_, _, _| true));
    let machine = Mesh::start(cfg).await.unwrap();
    machine.wait_ready(READY).await.unwrap();
    serve_echo(&machine);
    let client = node(&coord, NodeKind::Client, dir.path(), "c", false).await;
    client.pair("machine-m", "x").await.unwrap();
    let info = client
        .wait_machine("machine-m", Duration::from_secs(5))
        .await
        .unwrap();
    let mut s = client
        .open_stream(&info.node_key, "echo", None)
        .await
        .unwrap();
    roundtrip(&mut s, 1024).await;
    // The machine revokes the client; the next map drops it and its
    // connection ends.
    assert!(machine.revoke(&client.node_key()));
    coord.restart().await; // a fresh map is pushed on reconnect
    machine.wait_ready(READY).await.unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(
        !machine
            .peers()
            .iter()
            .any(|p| p.node_key == client.node_key())
    );
    let (mut r, mut w) = tokio::io::split(&mut s);
    let _ = w.write_all(b"after revoke").await;
    let mut buf = [0u8; 12];
    let res = tokio::time::timeout(Duration::from_secs(10), r.read_exact(&mut buf)).await;
    assert!(!matches!(res, Ok(Ok(_))), "a revoked client is cut");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_revoked_device_fails_fast_with_the_reason() {
    let dir = tempfile::tempdir().unwrap();
    let coord = FakeCoordinator::start(Mode::Account).await;
    let machine = node(&coord, NodeKind::Machine, dir.path(), "m", false).await;
    let mut cfg = MeshConfig::new(
        coord.url.clone(),
        NodeKind::Client,
        Some(dir.path().join("c.json")),
        static_auth(vec![("x-test-device".into(), "dev-c".into())]),
    );
    cfg.name = "c".into();
    let client = Mesh::start(cfg).await.unwrap();
    client.wait_ready(READY).await.unwrap();
    serve_echo(&machine);
    wait_peer(&client, &machine.node_key()).await;
    let mut s = client
        .open_stream(&machine.node_key(), "echo", None)
        .await
        .unwrap();
    roundtrip(&mut s, 1024).await;

    // The relay revokes the device: its coordinator connection is closed
    // and every reconnect is refused with 403.
    coord.deny("x-test-device", "dev-c", &client.node_key());
    let t = std::time::Instant::now();
    while client.control_state().denied.is_none() {
        assert!(t.elapsed() < Duration::from_secs(10), "no denial seen");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // New streams fail at once, naming the refusal, instead of waiting
    // ~20 s on a machine that already dropped this device.
    let t = std::time::Instant::now();
    let err = client
        .open_stream(&machine.node_key(), "echo", None)
        .await
        .expect_err("a revoked device opens no stream");
    assert!(
        t.elapsed() < Duration::from_secs(1),
        "took {:?}",
        t.elapsed()
    );
    assert!(
        matches!(&err, Error::Refused(m) if m.contains("403")),
        "{err}"
    );
    // The open stream ends too.
    let (mut r, mut w) = tokio::io::split(&mut s);
    let _ = w.write_all(b"after revoke").await;
    let mut buf = [0u8; 12];
    let res = tokio::time::timeout(Duration::from_secs(3), r.read_exact(&mut buf)).await;
    assert!(
        matches!(res, Ok(Err(_))),
        "the open stream is closed: {res:?}"
    );

    // A new node for the same device (the reconnect after the cut) is
    // refused at once with the reason, not after the ready timeout.
    let t = std::time::Instant::now();
    let Err(err) = cua_mesh::dialer::Dialer::start(
        &coord.url,
        static_auth(vec![("x-test-device".into(), "dev-c".into())]),
        None,
        Duration::from_secs(10),
    )
    .await
    else {
        panic!("a revoked device's new node is refused");
    };
    assert!(
        t.elapsed() < Duration::from_secs(3),
        "took {:?}",
        t.elapsed()
    );
    assert!(
        matches!(&err, Error::Refused(m) if m.contains("403")),
        "{err}"
    );
}

/// A machine missing from the coordinator's map (an older cua-spacesd, or
/// one with the mesh off) is reported at once, so `prefer` falls back to
/// the relay's HTTPS path without waiting out the timeout; a machine that
/// is on the mesh is found.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_machine_off_the_mesh_is_reported_without_waiting() {
    let coord = FakeCoordinator::start(Mode::Account).await;
    let dir = tempfile::tempdir().unwrap();
    let machine = node(&coord, NodeKind::Machine, dir.path(), "m1", false).await;
    let client = node(&coord, NodeKind::Client, dir.path(), "c1", false).await;
    wait_peer(&client, &machine.node_key()).await;

    let started = std::time::Instant::now();
    let none = client
        .wait_machine_in_map("machine-not-on-mesh", Duration::from_secs(5))
        .await;
    assert!(none.is_none());
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "waited {:?} for a machine the map does not have",
        started.elapsed()
    );
    let found = client
        .wait_machine_in_map("machine-m1", Duration::from_secs(5))
        .await
        .expect("a machine on the mesh is found");
    assert_eq!(found.node_key, machine.node_key());
}

/// Two machines of one account (CUA-1783): each sees the other with the
/// machine grant, and one forwards a loopback port to a `port:<n>` service
/// on the other in both directions at once (a gang's ring), direct and
/// relayed. Without machine peering they never see each other.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn machines_of_one_account_forward_ports_to_each_other() {
    let dir = tempfile::tempdir().unwrap();
    let coord = FakeCoordinator::start(Mode::Account).await;
    let a = node(&coord, NodeKind::Machine, dir.path(), "a", false).await;
    let b = node(&coord, NodeKind::Machine, dir.path(), "b", false).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(a.machine("machine-b").is_none(), "no machine peering yet");

    coord.set_machine_peering(true);
    let b_info = a
        .wait_machine("machine-b", READY)
        .await
        .expect("b in a's map");
    let grant = b_info.grant.expect("a machine peer carries a grant");
    assert_eq!(grant.role, cua_mesh::proto::ROLE_MACHINE);
    let a_info = b
        .wait_machine("machine-a", READY)
        .await
        .expect("a in b's map");
    assert_eq!(a_info.node_key, a.node_key());

    // Each side serves `port:<n>` streams with an echo.
    for m in [&a, &b] {
        let m = m.clone();
        tokio::spawn(async move {
            while let Some(i) = m.accept().await {
                assert_eq!(
                    cua_mesh::proto::parse_port_service(&i.header.service),
                    Some(47012)
                );
                tokio::spawn(async move {
                    let (mut r, mut w) = tokio::io::split(i.stream);
                    let _ = tokio::io::copy(&mut r, &mut w).await;
                    let _ = w.shutdown().await;
                });
            }
        });
    }
    let service = cua_mesh::proto::port_service(47012);
    let ab = a.forward(b.node_key(), &service, None).await.unwrap();
    let ba = b.forward(a.node_key(), &service, None).await.unwrap();
    let echo = |local: std::net::SocketAddr, n: usize| async move {
        let mut tcp = tokio::net::TcpStream::connect(local).await.unwrap();
        let data: Vec<u8> = (0..n).map(|i| (i * 13) as u8).collect();
        let (mut r, mut w) = tcp.split();
        let mut got = vec![0u8; n];
        tokio::time::timeout(Duration::from_secs(30), async {
            tokio::join!(async { w.write_all(&data).await.unwrap() }, async {
                r.read_exact(&mut got).await.unwrap()
            })
        })
        .await
        .expect("echo through the forward");
        assert_eq!(got, data);
    };
    tokio::join!(echo(ab.local, 1 << 20), echo(ba.local, 1 << 20));
    wait_path(&a, &b.node_key(), true, Duration::from_secs(10)).await;
    wait_path(&b, &a.node_key(), true, Duration::from_secs(10)).await;
    tokio::join!(echo(ab.local, 4 << 20), echo(ba.local, 4 << 20));

    // Machine peering off again: the peers leave both maps.
    coord.set_machine_peering(false);
    for _ in 0..100 {
        if a.machine("machine-b").is_none() && b.machine("machine-a").is_none() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("machines stayed peers after peering was turned off");
}

/// The control socket (CUA-1783): a job runner on machine A asks A's
/// mesh node for a forward to a port on machine B, uses it, and the
/// forward ends when the runner hangs up. An unknown machine is an error.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_control_socket_makes_forwards_that_live_with_the_connection() {
    let dir = tempfile::tempdir().unwrap();
    let coord = FakeCoordinator::start(Mode::Account).await;
    coord.set_machine_peering(true);
    let a = node(&coord, NodeKind::Machine, dir.path(), "a", false).await;
    let b = node(&coord, NodeKind::Machine, dir.path(), "b", false).await;
    let echo = b.clone();
    tokio::spawn(async move {
        while let Some(i) = echo.accept().await {
            assert_eq!(i.header.service, "port:47012");
            tokio::spawn(async move {
                let (mut r, mut w) = tokio::io::split(i.stream);
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    let sock = dir.path().join("host").join(cua_mesh::gang::CONTROL_SOCKET);
    let server = cua_mesh::gang::serve_control(a.clone(), &sock).unwrap();
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&sock).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    let mut ctl = cua_mesh::gang::ControlClient::connect(&sock).await.unwrap();
    let f = ctl.forward("machine-b", 47012).await.unwrap();
    assert!(f.local.ip().is_loopback());
    assert_eq!(f.path, "direct", "loopback peers find a direct path");
    assert_eq!(f.peer, "b");
    let mut tcp = tokio::net::TcpStream::connect(f.local).await.unwrap();
    tcp.write_all(b"ring").await.unwrap();
    let mut got = [0u8; 4];
    tokio::time::timeout(Duration::from_secs(20), tcp.read_exact(&mut got))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&got, b"ring");
    let err = ctl.forward("machine-nope", 47012).await.unwrap_err();
    assert!(err.contains("not on this machine's mesh"), "{err}");

    // The runner hangs up: its forwards stop listening.
    drop(ctl);
    let mut gone = false;
    for _ in 0..50 {
        if tokio::net::TcpStream::connect(f.local).await.is_err() {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(gone, "the forward outlived its control connection");
    server.abort();
}

/// Bulk rank traffic (CUA-1783): 256 MiB one way through a machine's
/// loopback forward, spliced on the other machine to a local TCP sink.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_forward_carries_bulk_traffic_to_a_local_port() {
    let dir = tempfile::tempdir().unwrap();
    let coord = FakeCoordinator::start(Mode::Account).await;
    coord.set_machine_peering(true);
    let a = node(&coord, NodeKind::Machine, dir.path(), "a", false).await;
    let b = node(&coord, NodeKind::Machine, dir.path(), "b", false).await;
    // B's "gang member": counts bytes, then answers one byte.
    let sink = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let sink_addr = sink.local_addr().unwrap();
    const N: usize = 256 << 20;
    let counted = tokio::spawn(async move {
        let (mut s, _) = sink.accept().await.unwrap();
        let mut buf = vec![0u8; 1 << 20];
        let mut got = 0usize;
        while got < N {
            let n = s.read(&mut buf).await.unwrap();
            assert!(n > 0, "the stream ended after {got} bytes");
            got += n;
        }
        s.write_all(b"k").await.unwrap();
        got
    });
    let mb = b.clone();
    tokio::spawn(async move {
        while let Some(i) = mb.accept().await {
            tokio::spawn(async move {
                let mut tcp = tokio::net::TcpStream::connect(sink_addr).await.unwrap();
                let mut st = i.stream;
                let _ = tokio::io::copy_bidirectional(&mut st, &mut tcp).await;
            });
        }
    });
    b.machine("machine-a");
    let f = a
        .forward(
            a.wait_machine("machine-b", READY).await.unwrap().node_key,
            "port:1",
            None,
        )
        .await
        .unwrap();
    let mut tcp = tokio::net::TcpStream::connect(f.local).await.unwrap();
    let block = vec![0u8; 1 << 20];
    let t = std::time::Instant::now();
    for _ in 0..(N >> 20) {
        tcp.write_all(&block).await.unwrap();
    }
    let mut k = [0u8; 1];
    tokio::time::timeout(Duration::from_secs(120), tcp.read_exact(&mut k))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(counted.await.unwrap(), N);
    println!("256 MiB in {:?}", t.elapsed());
}
