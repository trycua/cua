// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The mesh end to end, in process: cua-relay as the coordinator and
//! encrypted relay, a machine joined with its machine token (account mode)
//! or a static token, the machine's mesh node in front of a real spacesd,
//! and a client device on the mesh calling spacesd through a loopback
//! forward. Covers identity (the relay's share rules arrive as the same
//! assertions spacesd already checks), the per-forward session key, static
//! pairing by env-token proof, a relay that only ever saw ciphertext, and
//! version negotiation with a relay that has no mesh.

mod common;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use cua_mesh::{Mesh, MeshConfig, NodeKind};
use cua_relay::oidc::testing::FakeIssuer;
use cua_relay::oidc::{OidcConfig, OidcValidator};
use cua_relay::server::{Relay, RelayConfig};
use cua_spacesd_client::{ConnectOptions, SpacesdClient, TransportPreference};
use tokio_util::sync::CancellationToken;

use common::*;

const ISSUER: &str = "https://auth.test/realms/cua";

async fn wait_until(what: &str, mut check: impl FnMut() -> bool) {
    for _ in 0..400 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for {what}");
}

async fn serve(relay: &Relay) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let serving = relay.clone();
    tokio::spawn(async move {
        let _ = serving.serve(listener, std::future::pending()).await;
    });
    addr
}

async fn post(
    addr: SocketAddr,
    path: &str,
    token: &str,
    body: serde_json::Value,
) -> serde_json::Value {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("http://{addr}{path}"))
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(full(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let response = http_client().request(request).await.unwrap();
    let bytes = body_bytes(response, 1 << 20).await;
    serde_json::from_slice(&bytes).unwrap()
}

async fn client_over(
    forward: SocketAddr,
    token: Option<String>,
    session: Option<&str>,
    transport: TransportPreference,
) -> SpacesdClient {
    let mut options = ConnectOptions::parse(&format!("http://{forward}"))
        .unwrap()
        .transport(transport)
        .probe(false);
    if let Some(t) = token {
        options = options.token(t);
    }
    if let Some(s) = session {
        options = options.header(cua_spacesd_server::mesh_host::SESSION_HEADER, s);
    }
    SpacesdClient::connect(options).await.expect("connect")
}

fn client_mesh(
    relay: SocketAddr,
    dir: &std::path::Path,
    headers: Vec<(String, String)>,
) -> MeshConfig {
    let mut cfg = MeshConfig::new(
        format!("http://{relay}"),
        NodeKind::Client,
        Some(dir.join("client.json")),
        cua_mesh::static_auth(headers),
    );
    cfg.name = "laptop".into();
    cfg
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn account_peers_reach_spacesd_end_to_end_with_their_identity() {
    let driver = target().await;
    let ctx = driver.local.as_ref().expect("in process").ctx.clone();
    let local: SocketAddr = driver.url.trim_start_matches("http://").parse().unwrap();
    let issuer = FakeIssuer::new(ISSUER);
    let relay = Relay::new(RelayConfig {
        oidc: Some(Arc::new(OidcValidator::with_jwks(
            OidcConfig::new(ISSUER),
            issuer.jwks(),
        ))),
        device_enrollment: false,
        ..RelayConfig::default()
    });
    let relay_addr = serve(&relay).await;
    let (_, info) = (0, {
        let r = http_client()
            .get(format!("http://{relay_addr}/v1/info").parse().unwrap())
            .await
            .unwrap();
        serde_json::from_slice::<serde_json::Value>(&body_bytes(r, 1 << 16).await).unwrap()
    });
    assert_eq!(info["mesh"]["version"], cua_mesh::VERSION);

    // Ada registers and joins her machine; Bob is not on its share list.
    let machine_id = format!("mesh{}", uuid::Uuid::new_v4().simple());
    let ada = issuer.token("ada", Some("ada@example.com"), "cua-relay", 600);
    let reply = post(
        relay_addr,
        "/v1/machines",
        &ada,
        serde_json::json!({"id": machine_id, "name": "Ada's box"}),
    )
    .await;
    let machine_token = reply["machine_token"].as_str().unwrap().to_owned();
    let dir = tempfile::tempdir().unwrap();
    let policy = dir.path().join("host.json");
    std::fs::write(&policy, r#"{"owner":"ada","allow":[]}"#).unwrap();
    let mut join = cua_relay::client::JoinConfig::new(
        format!("ws://{relay_addr}"),
        machine_token.clone(),
        machine_id.clone(),
        local,
    );
    join.heartbeat = Duration::from_secs(2);
    ctx.auth().set_external(Arc::new(
        cua_spacesd_server::relay_account::RelayAssertionAuth::new(
            machine_id.clone(),
            join.account.clone(),
            Some(policy),
        ),
    ));
    let shutdown = CancellationToken::new();
    let mesh_cfg = ctx.mesh_host_config(
        &format!("http://{relay_addr}"),
        &machine_token,
        None,
        &machine_id,
        join.account.clone(),
        local,
    );
    // These tests turn the mesh on (`CUA_MESH=prefer`); its default is off.
    let mesh_cfg = cua_spacesd_server::mesh_host::MeshHostConfig {
        policy: Arc::new(|| cua_mesh::Policy::Prefer),
        ..mesh_cfg
    };
    let link = join.account.clone();
    tokio::spawn(cua_relay::client::run(join, shutdown.clone()));
    wait_until("relay join", || link.owner().is_some()).await;
    tokio::spawn(cua_spacesd_server::mesh_host::run(
        mesh_cfg,
        shutdown.clone(),
    ));

    // Ada's laptop joins the mesh with her account token.
    let ada_mesh = Mesh::start(client_mesh(
        relay_addr,
        dir.path(),
        vec![("x-cua-relay-authorization".into(), format!("Bearer {ada}"))],
    ))
    .await
    .unwrap();
    ada_mesh.wait_ready(Duration::from_secs(10)).await.unwrap();
    let peer = ada_mesh
        .wait_machine(&machine_id, Duration::from_secs(15))
        .await
        .expect("Ada's machine is in her map");
    let session = "s3ss10n-k3y-for-this-forward";
    let fwd = ada_mesh
        .forward(
            peer.node_key,
            cua_spacesd_server::mesh_host::SPACESD_SERVICE,
            Some(session.into()),
        )
        .await
        .unwrap();

    for transport in TRANSPORTS {
        let c = client_over(fwd.local, None, Some(session), transport).await;
        let (code, out) = run_sh(&c, "echo over-the-mesh").await;
        assert_eq!(
            (code, out.as_slice()),
            (Some(0), &b"over-the-mesh\n"[..]),
            "{transport:?}"
        );
    }
    // Without the forward's session key there is no identity: refused.
    let c = client_over(fwd.local, None, None, TransportPreference::Native).await;
    assert!(c
        .system()
        .health(cua_proto::env::v1::HealthRequest {})
        .await
        .is_err());
    let c = client_over(fwd.local, None, Some("wrong"), TransportPreference::Native).await;
    assert!(c
        .system()
        .health(cua_proto::env::v1::HealthRequest {})
        .await
        .is_err());

    // Bob has an account but no share: the machine is not in his map.
    let bob = issuer.token("bob", Some("bob@example.com"), "cua-relay", 600);
    let bob_dir = tempfile::tempdir().unwrap();
    let bob_mesh = Mesh::start(client_mesh(
        relay_addr,
        bob_dir.path(),
        vec![("x-cua-relay-authorization".into(), format!("Bearer {bob}"))],
    ))
    .await
    .unwrap();
    bob_mesh.wait_ready(Duration::from_secs(10)).await.unwrap();
    assert!(bob_mesh
        .wait_machine(&machine_id, Duration::from_secs(3))
        .await
        .is_none());

    // Ada shares it with Bob: it appears in his map within a recompute.
    let request = http::Request::builder()
        .method("PATCH")
        .uri(format!("http://{relay_addr}/v1/machines/{machine_id}"))
        .header("authorization", format!("Bearer {ada}"))
        .header("content-type", "application/json")
        .body(full(
            serde_json::to_vec(&serde_json::json!({"allow": ["bob@example.com"]})).unwrap(),
        ))
        .unwrap();
    let response = http_client().request(request).await.unwrap();
    assert!(response.status().is_success(), "{}", response.status());
    let bob_peer = bob_mesh
        .wait_machine(&machine_id, Duration::from_secs(10))
        .await
        .expect("shared machine in Bob's map");
    let bob_fwd = bob_mesh
        .forward(bob_peer.node_key, "spacesd", Some("bob-session".into()))
        .await
        .unwrap();
    let c = client_over(
        bob_fwd.local,
        None,
        Some("bob-session"),
        TransportPreference::Native,
    )
    .await;
    let (code, out) = run_sh(&c, "echo bob").await;
    assert_eq!((code, out.as_slice()), (Some(0), &b"bob\n"[..]));

    // Stopping sharing cuts Bob off the mesh too.
    let reply = post(
        relay_addr,
        &format!("/v1/machines/{machine_id}/stop-sharing"),
        &machine_token,
        serde_json::json!({}),
    )
    .await;
    assert_eq!(reply["sharing"], false, "{reply}");
    wait_until("Bob's peer removed", || {
        bob_mesh.machine(&machine_id).is_none()
    })
    .await;
    shutdown.cancel();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn static_clients_pair_by_env_token_proof_and_work_through_the_relay() {
    let driver = target().await;
    let ctx = driver.local.as_ref().expect("in process").ctx.clone();
    let local: SocketAddr = driver.url.trim_start_matches("http://").parse().unwrap();
    let relay = Relay::new(RelayConfig {
        tokens: vec!["reg-token".into()],
        ..RelayConfig::default()
    });
    let relay_addr = serve(&relay).await;
    let machine_id = format!("stat{}", uuid::Uuid::new_v4().simple());
    let join = cua_relay::client::JoinConfig::new(
        format!("ws://{relay_addr}"),
        "reg-token".into(),
        machine_id.clone(),
        local,
    );
    let shutdown = CancellationToken::new();
    let mesh_cfg = ctx.mesh_host_config(
        &format!("http://{relay_addr}"),
        "reg-token",
        None,
        &machine_id,
        join.account.clone(),
        local,
    );
    let mesh_cfg = cua_spacesd_server::mesh_host::MeshHostConfig {
        policy: Arc::new(|| cua_mesh::Policy::Prefer),
        ..mesh_cfg
    };
    tokio::spawn(cua_relay::client::run(join, shutdown.clone()));
    tokio::spawn(cua_spacesd_server::mesh_host::run(
        mesh_cfg,
        shutdown.clone(),
    ));

    let dir = tempfile::tempdir().unwrap();
    let mut cfg = client_mesh(relay_addr, dir.path(), Vec::new());
    // Force every packet through the relay to check what it sees.
    cfg.relay_only = true;
    let mesh = Mesh::start(cfg).await.unwrap();
    mesh.wait_ready(Duration::from_secs(10)).await.unwrap();
    assert!(
        mesh.machine(&machine_id).is_none(),
        "nothing before pairing"
    );
    // Wait for the machine's own node to be on the mesh, then pair.
    let mut paired = Err(cua_mesh::Error::Unreachable(String::new()));
    for _ in 0..40 {
        paired = mesh.pair(&machine_id, "wrong-token").await;
        if matches!(&paired, Err(cua_mesh::Error::Refused(r)) if r.contains("did not accept")) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(
        matches!(&paired, Err(cua_mesh::Error::Refused(_))),
        "{paired:?}"
    );
    mesh.pair(&machine_id, &driver.token).await.unwrap();
    let peer = mesh
        .wait_machine(&machine_id, Duration::from_secs(10))
        .await
        .expect("paired");
    let fwd = mesh.forward(peer.node_key, "spacesd", None).await.unwrap();
    let c = client_over(
        fwd.local,
        Some(driver.token.clone()),
        None,
        TransportPreference::Native,
    )
    .await;
    let (code, out) = run_sh(&c, "echo CIPHERTEXT-ONLY-CHECK-$((6*7))").await;
    assert_eq!(
        (code, out.as_slice()),
        (Some(0), &b"CIPHERTEXT-ONLY-CHECK-42\n"[..])
    );
    assert!(
        relay
            .mesh()
            .relayed_bytes
            .load(std::sync::atomic::Ordering::Relaxed)
            > 0
    );
    shutdown.cancel();
}

/// p50 of `n` `Health` round trips, in milliseconds.
async fn health_p50(c: &SpacesdClient, n: usize) -> f64 {
    let mut v = Vec::with_capacity(n);
    for i in 0..n + 10 {
        let t = std::time::Instant::now();
        c.system()
            .health(cua_proto::env::v1::HealthRequest {})
            .await
            .expect("health");
        if i >= 10 {
            v.push(t.elapsed().as_secs_f64() * 1e3);
        }
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

/// Guard of the channel (CUA-1889 item 5): calls over the mesh go straight
/// to the machine (a direct path, as between two computers on one LAN) and
/// stay on it, with no added delay per call (a timer, a lost TCP_NODELAY or
/// a reconnect per call shows up as milliseconds). In process on loopback,
/// so the relay's network round trips are not part of it; the numbers on a
/// real LAN, against the relay, are libs/cua/bench/channel's baselines.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn calls_over_the_mesh_take_the_direct_path_without_added_delay() {
    let driver = target().await;
    let ctx = driver.local.as_ref().expect("in process").ctx.clone();
    let local: SocketAddr = driver.url.trim_start_matches("http://").parse().unwrap();
    let issuer = FakeIssuer::new(ISSUER);
    let relay = Relay::new(RelayConfig {
        oidc: Some(Arc::new(OidcValidator::with_jwks(
            OidcConfig::new(ISSUER),
            issuer.jwks(),
        ))),
        device_enrollment: false,
        ..RelayConfig::default()
    });
    let relay_addr = serve(&relay).await;
    let machine_id = format!("mesh{}", uuid::Uuid::new_v4().simple());
    let ada = issuer.token("ada", Some("ada@example.com"), "cua-relay", 600);
    let reply = post(
        relay_addr,
        "/v1/machines",
        &ada,
        serde_json::json!({"id": machine_id, "name": "Ada's box"}),
    )
    .await;
    let machine_token = reply["machine_token"].as_str().unwrap().to_owned();
    let dir = tempfile::tempdir().unwrap();
    let policy = dir.path().join("host.json");
    std::fs::write(&policy, r#"{"owner":"ada","allow":[]}"#).unwrap();
    let mut join = cua_relay::client::JoinConfig::new(
        format!("ws://{relay_addr}"),
        machine_token.clone(),
        machine_id.clone(),
        local,
    );
    join.heartbeat = Duration::from_secs(2);
    ctx.auth().set_external(Arc::new(
        cua_spacesd_server::relay_account::RelayAssertionAuth::new(
            machine_id.clone(),
            join.account.clone(),
            Some(policy),
        ),
    ));
    let shutdown = CancellationToken::new();
    let mesh_cfg = cua_spacesd_server::mesh_host::MeshHostConfig {
        policy: Arc::new(|| cua_mesh::Policy::Prefer),
        ..ctx.mesh_host_config(
            &format!("http://{relay_addr}"),
            &machine_token,
            None,
            &machine_id,
            join.account.clone(),
            local,
        )
    };
    let link = join.account.clone();
    tokio::spawn(cua_relay::client::run(join, shutdown.clone()));
    wait_until("relay join", || link.owner().is_some()).await;
    tokio::spawn(cua_spacesd_server::mesh_host::run(
        mesh_cfg,
        shutdown.clone(),
    ));
    let ada_mesh = Mesh::start(client_mesh(
        relay_addr,
        dir.path(),
        vec![("x-cua-relay-authorization".into(), format!("Bearer {ada}"))],
    ))
    .await
    .unwrap();
    ada_mesh.wait_ready(Duration::from_secs(10)).await.unwrap();
    let peer = ada_mesh
        .wait_machine(&machine_id, Duration::from_secs(15))
        .await
        .expect("Ada's machine is in her map");
    let path = ada_mesh.warm(&peer.node_key, Duration::from_secs(10)).await;
    assert!(
        matches!(path, Some(cua_mesh::Path::Direct(_))),
        "a direct path between two nodes on one network: {path:?}"
    );
    let fwd = ada_mesh
        .forward(
            peer.node_key,
            cua_spacesd_server::mesh_host::SPACESD_SERVICE,
            Some("s3ss10n".into()),
        )
        .await
        .unwrap();
    let mesh = client_over(
        fwd.local,
        None,
        Some("s3ss10n"),
        TransportPreference::Native,
    )
    .await;
    let relayed = Target {
        url: format!("http://{relay_addr}/m/{machine_id}"),
        token: String::new(),
        scratch: driver.scratch.clone(),
        local: None,
    }
    .client_with_token(TransportPreference::Native, Some(ada.clone()))
    .await;
    let over_mesh = health_p50(&mesh, 200).await;
    let over_relay = health_p50(&relayed, 200).await;
    eprintln!("health p50: mesh direct {over_mesh:.3} ms, relay HTTPS path {over_relay:.3} ms");
    assert!(
        matches!(
            ada_mesh.path(&peer.node_key),
            Some(cua_mesh::Path::Direct(_))
        ),
        "the calls stayed on the direct path"
    );
    // Loopback, debug builds on shared CI runners (userspace crypto is
    // slow unoptimized: about 4 ms here, like the in-process relay path):
    // generous, so only an added delay per call trips it.
    assert!(
        over_mesh < 25.0 && over_mesh < over_relay * 3.0,
        "mesh direct p50 {over_mesh:.3} ms (relay path {over_relay:.3} ms)"
    );
    shutdown.cancel();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_relay_without_a_mesh_is_detected() {
    let relay = Relay::new(RelayConfig {
        tokens: vec!["t".into()],
        mesh: cua_relay::mesh::MeshConfig {
            enabled: false,
            ..Default::default()
        },
        ..RelayConfig::default()
    });
    let addr = serve(&relay).await;
    assert!(
        !cua_spacesd_server::mesh_host::relay_has_mesh(&format!("http://{addr}"))
            .await
            .unwrap()
    );
    let relay = Relay::new(RelayConfig {
        tokens: vec!["t".into()],
        ..RelayConfig::default()
    });
    let addr = serve(&relay).await;
    assert!(
        cua_spacesd_server::mesh_host::relay_has_mesh(&format!("ws://{addr}"))
            .await
            .unwrap()
    );
}
