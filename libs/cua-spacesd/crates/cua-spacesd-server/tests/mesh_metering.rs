// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Mesh sessions count against the relay allowance (CUA-1506), end to end
//! with real `cua-mesh` nodes: the relay as coordinator and meter, an
//! account machine's mesh node (`mesh_host`) in front of a local server,
//! and client devices on the mesh.
//!
//! - a direct mesh session is a live session that uses no relay hours;
//! - a relayed one uses relay hours too;
//! - a QUIC media flow over the mesh is a session (a desktop view);
//! - HTTPS and the mesh at once on one machine are one session;
//! - members of a paying team pool their mesh sessions;
//! - a machine cannot charge an account it has no relationship with.
// The relay's optional hosted features (Cargo feature `relay-hosted`).
#![cfg(feature = "relay-hosted")]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use cua_mesh::{Mesh, MeshConfig, NodeKind, Path};
use cua_relay::meter::{parse_allowances, MeterConfig, LINGER};
use cua_relay::oidc::testing::FakeIssuer;
use cua_relay::oidc::{OidcConfig, OidcValidator};
use cua_relay::server::{Relay, RelayConfig};
use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_util::sync::CancellationToken;

const ISSUER: &str = "https://auth.test/realms/cua";
const ADMIN: &str = "admin-secret";

async fn wait_until<F, Fut>(what: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..300 {
        if check().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out waiting for {what}");
}

struct World {
    relay: Relay,
    base: String,
    issuer: FakeIssuer,
    http: reqwest::Client,
    stop: CancellationToken,
    _dirs: Vec<tempfile::TempDir>,
}

impl World {
    async fn usage(&self, token: &str) -> Value {
        self.http
            .get(format!("{}/v1/usage", self.base))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    async fn mesh_status(&self) -> Value {
        self.http
            .get(format!("{}/relay/v1/mesh", self.base))
            .bearer_auth(ADMIN)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    fn token(&self, sub: &str) -> String {
        self.issuer
            .token(sub, Some(&format!("{sub}@example.com")), "cua-relay", 600)
    }

    async fn register(&self, owner: &str, id: &str, allow: &[&str]) -> String {
        let reg: Value = self
            .http
            .post(format!("{}/v1/machines", self.base))
            .bearer_auth(self.token(owner))
            .json(&json!({"id": id, "name": id}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if !allow.is_empty() {
            let allow: Vec<String> = allow.iter().map(|u| format!("{u}@example.com")).collect();
            let r = self
                .http
                .patch(format!("{}/v1/machines/{id}", self.base))
                .bearer_auth(self.token(owner))
                .json(&json!({ "allow": allow }))
                .send()
                .await
                .unwrap();
            assert!(r.status().is_success(), "share: {}", r.status());
        }
        reg["machine_token"].as_str().unwrap().to_owned()
    }

    /// Joins machine `id` to the relay and the mesh, in front of `local`
    /// (HTTP) and `media` (UDP).
    async fn join(&mut self, id: &str, token: &str, local: SocketAddr, media: SocketAddr) {
        let mut join = cua_relay::client::JoinConfig::new(
            self.base.replace("http://", "ws://"),
            token.to_owned(),
            id.to_owned(),
            local,
        );
        join.heartbeat = Duration::from_secs(2);
        let account = join.account.clone();
        tokio::spawn(cua_relay::client::run(join, self.stop.clone()));
        let link = account.clone();
        wait_until("relay join", || {
            let ok = link.owner().is_some();
            async move { ok }
        })
        .await;
        let dir = tempfile::tempdir().unwrap();
        let cfg = cua_spacesd_server::mesh_host::MeshHostConfig {
            relay_url: self.base.clone(),
            relay_token: token.to_owned(),
            relay_token_file: None,
            machine_id: id.to_owned(),
            dir: dir.path().to_owned(),
            local,
            quic: Some(media),
            media_udp_port: cua_mesh::MEDIA_UDP_PORT,
            account,
            env_token: Arc::new(|| None),
            name: id.to_owned(),
            policy: Arc::new(|| cua_mesh::Policy::Prefer),
            recheck: cua_spacesd_server::mesh_host::GATE_RECHECK,
            gang_home: None,
            gang_wait: cua_spacesd_server::mesh_host::GANG_WAIT,
        };
        self._dirs.push(dir);
        tokio::spawn(cua_spacesd_server::mesh_host::run(cfg, self.stop.clone()));
    }

    /// A client device of `sub` on the mesh (relayed only when `relay_only`).
    async fn client(&mut self, sub: &str, relay_only: bool) -> Mesh {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = MeshConfig::new(
            self.base.clone(),
            NodeKind::Client,
            Some(dir.path().join("client.json")),
            cua_mesh::static_auth(vec![(
                "x-cua-relay-authorization".into(),
                format!("Bearer {}", self.token(sub)),
            )]),
        );
        cfg.name = format!("{sub} laptop");
        cfg.relay_only = relay_only;
        self._dirs.push(dir);
        let mesh = Mesh::start(cfg).await.unwrap();
        mesh.wait_ready(Duration::from_secs(10)).await.unwrap();
        mesh
    }

    /// Moves the meter on: sessions held long enough become live, and an
    /// hour passes for every session that crosses the relay.
    fn hour(&self) {
        let now = cua_relay::assertion::now_secs();
        self.relay.meter().tick(Duration::ZERO, now);
        self.relay.meter().tick(Duration::from_secs(3600), now);
    }
}

async fn world() -> World {
    let issuer = FakeIssuer::new(ISSUER);
    let state = tempfile::tempdir().unwrap();
    // Bob and Carol are a paying team with one seat: one live session.
    let plans = state.path().join("plans.json");
    std::fs::write(
        &plans,
        json!({"teams": {"acme": {"seats": 1, "members": ["bob", "carol"]}}}).to_string(),
    )
    .unwrap();
    let relay = Relay::new(RelayConfig {
        oidc: Some(Arc::new(OidcValidator::with_jwks(
            OidcConfig::new(ISSUER),
            issuer.jwks(),
        ))),
        device_enrollment: false,
        admin_token: Some(ADMIN.into()),
        state_file: Some(state.path().join("machines.json")),
        meter: MeterConfig {
            allowances: parse_allowances("free=2/20,teams=1/100").unwrap(),
            plans_file: Some(plans),
            live_after: Duration::from_millis(200),
            ..MeterConfig::default()
        },
        ..RelayConfig::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let serving = relay.clone();
    tokio::spawn(async move {
        let _ = serving.serve(listener, std::future::pending()).await;
    });
    World {
        relay,
        base,
        issuer,
        http: reqwest::Client::new(),
        stop: CancellationToken::new(),
        _dirs: vec![state],
    }
}

/// The machine's local server: `/ws` holds a WebSocket open (a terminal, an
/// agent's stream); anything else answers at once.
async fn machine_server() -> SocketAddr {
    let app = axum::Router::new()
        .route(
            "/ws",
            axum::routing::get(|ws: axum::extract::ws::WebSocketUpgrade| async move {
                ws.on_upgrade(|mut socket| async move {
                    while let Some(Ok(m)) = socket.recv().await {
                        if socket.send(m).await.is_err() {
                            return;
                        }
                    }
                })
            }),
        )
        .fallback(|| async { "ok" });
    let local = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = local.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(local, app).await;
    });
    addr
}

/// A local UDP echo (the machine's QUIC media port).
async fn media_server() -> SocketAddr {
    let sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr = sock.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buf = [0u8; 2048];
        while let Ok((n, src)) = sock.recv_from(&mut buf).await {
            let _ = sock.send_to(&buf[..n], src).await;
        }
    });
    addr
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Opens `/ws` at `url` and checks it echoes.
async fn hold_ws(url: &str, bearer: Option<&str>) -> Ws {
    let mut req = url.into_client_request().unwrap();
    if let Some(t) = bearer {
        req.headers_mut()
            .insert("authorization", format!("Bearer {t}").parse().unwrap());
    }
    let (mut ws, _) = tokio_tungstenite::connect_async(req).await.expect("ws");
    ws.send(WsMessage::Text("ping".into())).await.unwrap();
    let echo = tokio::time::timeout(Duration::from_secs(10), ws.next())
        .await
        .expect("echo")
        .unwrap()
        .unwrap();
    assert_eq!(echo.into_text().unwrap(), "ping");
    ws
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mesh_sessions_count_against_the_allowance() {
    let mut w = world().await;
    let local = machine_server().await;
    let media = media_server().await;
    // Ada's machine, shared with Bob and Carol.
    let m1 = format!("mesh1{}", uuid::Uuid::new_v4().simple());
    let m1_token = w.register("ada", &m1, &["bob", "carol"]).await;
    w.join(&m1, &m1_token, local, media).await;
    let ada_token = w.token("ada");

    // --- Direct mesh: a live session, no relay hours. ---
    let ada = w.client("ada", false).await;
    let peer = ada
        .wait_machine(&m1, Duration::from_secs(15))
        .await
        .expect("Ada's machine is in her map");
    let fwd = ada
        .forward(
            peer.node_key,
            cua_spacesd_server::mesh_host::SPACESD_SERVICE,
            None,
        )
        .await
        .unwrap();
    // One call sets the path up; then wait for it to go direct, and for
    // the relayed handshake to fall out of the hours window.
    let r = w
        .http
        .get(format!("http://{}/probe", fwd.local))
        .send()
        .await
        .unwrap();
    assert_eq!(r.text().await.unwrap(), "ok");
    wait_until("a direct path", || {
        let direct = matches!(ada.path(&peer.node_key), Some(Path::Direct(_)));
        async move { direct }
    })
    .await;
    tokio::time::sleep(LINGER + Duration::from_secs(2)).await;
    // A short call is not a session.
    w.hour();
    let v = w.usage(&ada_token).await;
    assert_eq!(
        (v["live_sessions"].as_u64(), v["hours_used"].as_f64()),
        (Some(0), Some(0.0)),
        "{v}"
    );

    let mut ada_ws = hold_ws(&format!("ws://{}/ws", fwd.local), None).await;
    wait_until("Ada's mesh session is live", || {
        let w = &w;
        let t = ada_token.clone();
        async move {
            w.relay
                .meter()
                .tick(Duration::ZERO, cua_relay::assertion::now_secs());
            w.usage(&t).await["live_sessions"] == 1
        }
    })
    .await;
    w.hour();
    let v = w.usage(&ada_token).await;
    assert_eq!(v["live_sessions"], 1, "{v}");
    assert_eq!(
        v["hours_used"], 0.0,
        "a direct session uses no relay hours: {v}"
    );
    let status = w.mesh_status().await;
    assert!(status["metered_streams"].as_u64().unwrap() >= 1, "{status}");

    // --- HTTPS and the mesh at once on one machine: one session. ---
    let mut https_ws = hold_ws(
        &format!("{}/m/{m1}/ws", w.base.replace("http://", "ws://")),
        Some(&ada_token),
    )
    .await;
    w.hour();
    let v = w.usage(&ada_token).await;
    assert_eq!(
        v["live_sessions"], 1,
        "HTTPS + mesh on one machine count once: {v}"
    );
    assert_eq!(
        v["hours_used"], 1.0,
        "the HTTPS stream crosses the relay: {v}"
    );
    https_ws.close(None).await.unwrap();

    // --- Relayed mesh: a live session that uses relay hours. ---
    let bob_token = w.token("bob");
    let bob = w.client("bob", true).await;
    let peer_b = bob
        .wait_machine(&m1, Duration::from_secs(15))
        .await
        .expect("the shared machine is in Bob's map");
    let fwd_b = bob
        .forward(
            peer_b.node_key,
            cua_spacesd_server::mesh_host::SPACESD_SERVICE,
            None,
        )
        .await
        .unwrap();
    let mut bob_ws = hold_ws(&format!("ws://{}/ws", fwd_b.local), None).await;
    assert_eq!(bob.path(&peer_b.node_key), Some(Path::Relay));
    wait_until("Bob's relayed session is live", || {
        let w = &w;
        let t = bob_token.clone();
        async move {
            w.relay
                .meter()
                .tick(Duration::ZERO, cua_relay::assertion::now_secs());
            w.usage(&t).await["live_sessions"] == 1
        }
    })
    .await;
    // Keep traffic flowing through the relay while an hour passes.
    bob_ws
        .send(WsMessage::Text("still here".into()))
        .await
        .unwrap();
    let _ = bob_ws.next().await;
    wait_until("the relayed pair is attributed", || {
        let w = &w;
        let t = bob_token.clone();
        async move {
            let now = cua_relay::assertion::now_secs();
            w.relay.meter().tick(Duration::from_secs(60), now);
            w.usage(&t).await["my_hours_used"].as_f64().unwrap_or(0.0) > 0.0
        }
    })
    .await;
    let v = w.usage(&bob_token).await;
    assert_eq!(v["live_sessions"], 1, "{v}");
    assert_eq!(v["plan"], "teams", "{v}");
    assert!(v["hours_used"].as_f64().unwrap() > 0.0, "relayed: {v}");

    // --- A media flow (QUIC streaming) over the mesh; the team pools. ---
    let carol_token = w.token("carol");
    let carol = w.client("carol", false).await;
    let peer_c = carol
        .wait_machine(&m1, Duration::from_secs(15))
        .await
        .expect("the shared machine is in Carol's map");
    let fwd_c = carol
        .forward(
            peer_c.node_key,
            cua_spacesd_server::mesh_host::SPACESD_SERVICE,
            None,
        )
        .await
        .unwrap();
    let udp = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    udp.connect(fwd_c.local).await.unwrap();
    let media_task = tokio::spawn(async move {
        let mut buf = [0u8; 64];
        loop {
            let _ = udp.send(b"frame").await;
            let _ = tokio::time::timeout(Duration::from_millis(500), udp.recv(&mut buf)).await;
        }
    });
    wait_until("Carol's media session is live", || {
        let w = &w;
        let t = carol_token.clone();
        async move {
            w.relay
                .meter()
                .tick(Duration::ZERO, cua_relay::assertion::now_secs());
            w.usage(&t).await["live_sessions"] == 2
        }
    })
    .await;
    // Pooled: Bob and Carol share one seat's single live session.
    for t in [&bob_token, &carol_token] {
        let v = w.usage(t).await;
        assert_eq!(v["team"]["id"], "acme", "{v}");
        assert_eq!(
            (
                v["live_sessions"].as_u64(),
                v["live_sessions_limit"].as_u64()
            ),
            (Some(2), Some(1)),
            "{v}"
        );
        assert_eq!(v["soft_limit_reached"], true, "{v}");
        assert_eq!(v["reasons"], json!(["sessions"]), "{v}");
        assert!(
            v["upgrade_url"].as_str().unwrap().starts_with("https://"),
            "{v}"
        );
    }
    // Ada (outside the team) is unaffected.
    assert_eq!(w.usage(&ada_token).await["live_sessions"], 1);

    // --- A machine cannot charge accounts it has no relationship with. ---
    forged_reports(&w, &ada, &bob).await;
    let v = w.usage(&ada_token).await;
    assert_eq!(
        v["live_sessions"], 1,
        "a forged report changed nothing: {v}"
    );
    assert_eq!(w.usage(&bob_token).await["live_sessions"], 2);

    // Streams closing end the sessions (after the linger).
    media_task.abort();
    ada_ws.close(None).await.unwrap();
    bob_ws.close(None).await.unwrap();
    drop((ada_ws, bob_ws));
    wait_until("the mesh streams close", || {
        let w = &w;
        async move { w.mesh_status().await["metered_streams"] == 0 }
    })
    .await;
    w.stop.cancel();
}

/// Dan's machine (shared with Ada only) connects to the coordinator by
/// hand and reports streams for Bob (not in its map) and for Ada's key
/// under Bob's name: both are ignored.
async fn forged_reports(w: &World, ada: &Mesh, bob: &Mesh) {
    let m2 = format!("mesh2{}", uuid::Uuid::new_v4().simple());
    let token = w.register("dan", &m2, &["ada"]).await;
    let mut req = format!("{}/v1/mesh/connect", w.base.replace("http://", "ws://"))
        .into_client_request()
        .unwrap();
    req.headers_mut()
        .insert("authorization", format!("Bearer {token}").parse().unwrap());
    req.headers_mut()
        .insert(cua_mesh::proto::MACHINE_ID_HEADER, m2.parse().unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    let text = |m: WsMessage| m.into_text().unwrap().to_string();
    let challenge: cua_mesh::proto::ServerMsg =
        serde_json::from_str(&text(ws.next().await.unwrap().unwrap())).unwrap();
    let cua_mesh::proto::ServerMsg::Challenge {
        server_key, nonce, ..
    } = challenge
    else {
        panic!("{challenge:?}");
    };
    let node = cua_mesh::SecretKey::generate();
    let hello = cua_mesh::proto::ClientMsg::Hello {
        version: cua_mesh::VERSION,
        kind: NodeKind::Machine,
        node_key: node.public(),
        disco_key: cua_mesh::SecretKey::generate().public(),
        quic_pin: "00".into(),
        name: "dan box".into(),
        endpoints: vec![],
        proof: cua_mesh::proto::hello_proof(&node, &server_key, &nonce),
        ephemeral: true,
    };
    ws.send(WsMessage::Text(serde_json::to_string(&hello).unwrap()))
        .await
        .unwrap();
    let welcome: cua_mesh::proto::ServerMsg =
        serde_json::from_str(&text(ws.next().await.unwrap().unwrap())).unwrap();
    match welcome {
        cua_mesh::proto::ServerMsg::Welcome { usage, .. } => {
            assert_eq!(usage, cua_mesh::proto::USAGE_VERSION)
        }
        other => panic!("{other:?}"),
    }
    // Ada (shared) shows up in this machine's map; Bob does not.
    loop {
        let m = tokio::time::timeout(Duration::from_secs(10), ws.next())
            .await
            .expect("netmap")
            .unwrap()
            .unwrap();
        if let Ok(cua_mesh::proto::ServerMsg::Netmap { peers }) = serde_json::from_str(&text(m)) {
            if peers.iter().any(|p| p.node_key == ada.node_key()) {
                assert!(!peers.iter().any(|p| p.node_key == bob.node_key()));
                break;
            }
        }
    }
    let before = w.mesh_status().await;
    let report = cua_mesh::proto::ClientMsg::Usage {
        v: cua_mesh::proto::USAGE_VERSION,
        peers: vec![
            cua_mesh::PeerUsage {
                peer: bob.node_key(),
                user: "bob".into(),
                open: 3,
                live: true,
            },
            cua_mesh::PeerUsage {
                peer: ada.node_key(),
                user: "bob".into(),
                open: 3,
                live: true,
            },
        ],
    };
    ws.send(WsMessage::Text(serde_json::to_string(&report).unwrap()))
        .await
        .unwrap();
    // An unknown version is ignored outright.
    ws.send(WsMessage::Text(
        json!({"type": "usage", "v": 99, "peers": [{"peer": ada.node_key(), "user": "ada", "open": 9}]})
            .to_string()
            ,
    ))
    .await
    .unwrap();
    let rejected = |s: &Value| s["usage_rejected"].as_u64().unwrap();
    wait_until("the forged report is refused", || async {
        rejected(&w.mesh_status().await) > rejected(&before)
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let after = w.mesh_status().await;
    // Both entries ignored; the unknown version never counted.
    assert_eq!(rejected(&after) - rejected(&before), 2, "{after}");
    w.relay
        .meter()
        .tick(Duration::ZERO, cua_relay::assertion::now_secs());
    let _ = ws.close(None).await;
}
