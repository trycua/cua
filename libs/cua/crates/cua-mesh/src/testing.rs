// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! An in-process coordinator and relay for tests (never the real one, which
//! runs in Cua's relay service). It speaks the same protocol: the challenge, the network
//! map, relayed frames, pairing and STUN.
//!
//! - [`Mode::Account`]: every client peers with every machine (machines see
//!   clients with an owner grant).
//! - [`Mode::Static`]: clients pair with machines by proof, as in
//!   production.
//!
//! [`FakeCoordinator::stop`] and [`FakeCoordinator::restart`] simulate a
//! crash and a restart (the node registry survives, as it is persisted in
//! the real one); [`FakeCoordinator::set_relay`] drops relayed packets.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use futures_util::{SinkExt as _, StreamExt as _};
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

use crate::key::{PublicKey, SecretKey};
use crate::proto::{self, ClientMsg, Grant, Mode, NodeKind, PeerInfo, ServerMsg};

#[derive(Default)]
struct State {
    nodes: Mutex<HashMap<PublicKey, PeerInfo>>,
    conns: Mutex<HashMap<PublicKey, mpsc::UnboundedSender<WsMessage>>>,
    pairs: Mutex<HashSet<(PublicKey, PublicKey)>>,
    pending: Mutex<HashMap<String, (PublicKey, String)>>,
    relay_off: AtomicBool,
    relayed_bytes: AtomicU64,
    /// Everything relayed (the first 16 MiB), to check it is ciphertext.
    capture: Mutex<Vec<u8>>,
    /// Request headers (`name`, `value`) answered with HTTP 403, as the
    /// relay answers a revoked device.
    denied: Mutex<Vec<(String, String)>>,
    /// The relay's mesh is turned off: every connect gets HTTP 404.
    mesh_off: AtomicBool,
}

/// See the module docs.
pub struct FakeCoordinator {
    /// `http://127.0.0.1:<port>`.
    pub url: String,
    /// The STUN responder (`127.0.0.1:<port>`).
    pub stun: SocketAddr,
    mode: Mode,
    port: u16,
    state: Arc<State>,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl FakeCoordinator {
    /// Starts on loopback.
    pub async fn start(mode: Mode) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let stun = UdpSocket::bind("127.0.0.1:0").await.expect("stun");
        let stun_addr = stun.local_addr().expect("addr");
        let me = Self {
            url: format!("http://127.0.0.1:{port}"),
            stun: stun_addr,
            mode,
            port,
            state: Arc::default(),
            tasks: Mutex::new(Vec::new()),
        };
        me.spawn(listener);
        me.tasks
            .lock()
            .expect("tasks")
            .push(tokio::spawn(stun_loop(stun)));
        me
    }

    fn spawn(&self, listener: TcpListener) {
        let state = self.state.clone();
        let mode = self.mode;
        let t = tokio::spawn(async move {
            // Connections live in this set, so aborting the listener task
            // (a simulated crash) drops every connection with it.
            let mut conns = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let Ok((tcp, _)) = accepted else { continue };
                        let state = state.clone();
                        conns.spawn(async move { serve(state, mode, tcp).await });
                    }
                    Some(_) = conns.join_next(), if !conns.is_empty() => {}
                }
            }
        });
        self.tasks.lock().expect("tasks").push(t);
    }

    /// Simulates a crash: the listener and every connection go away.
    pub fn stop(&self) {
        for t in self.tasks.lock().expect("tasks").drain(..) {
            t.abort();
        }
        self.state.conns.lock().expect("conns").clear();
    }

    /// Comes back on the same port with the same registry.
    pub async fn restart(&self) {
        self.stop();
        let mut listener = None;
        for _ in 0..50 {
            match TcpListener::bind(("127.0.0.1", self.port)).await {
                Ok(l) => {
                    listener = Some(l);
                    break;
                }
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(50)).await,
            }
        }
        self.spawn(listener.expect("rebind the coordinator port"));
        if let Ok(stun) = UdpSocket::bind(self.stun).await {
            self.tasks
                .lock()
                .expect("tasks")
                .push(tokio::spawn(stun_loop(stun)));
        }
    }

    /// Relays packets (true, the default) or drops them.
    pub fn set_relay(&self, on: bool) {
        self.state.relay_off.store(!on, Ordering::SeqCst);
    }

    /// Bytes relayed so far.
    pub fn relayed_bytes(&self) -> u64 {
        self.state.relayed_bytes.load(Ordering::Relaxed)
    }

    /// True when `needle` appears anywhere in what was relayed.
    pub fn relayed_contains(&self, needle: &[u8]) -> bool {
        let c = self.state.capture.lock().expect("capture");
        !needle.is_empty() && c.windows(needle.len()).any(|w| w == needle)
    }

    /// Revokes the node that connects with header `name: value` (its
    /// key is `node`): its connection is closed, every later connect gets
    /// HTTP 403, and its peers' maps drop it.
    pub fn deny(&self, name: &str, value: &str, node: &PublicKey) {
        self.state
            .denied
            .lock()
            .expect("denied")
            .push((name.to_ascii_lowercase(), value.to_owned()));
        self.state.nodes.lock().expect("nodes").remove(node);
        if let Some(tx) = self.state.conns.lock().expect("conns").remove(node) {
            let _ = tx.send(WsMessage::Close(None));
        }
        push_netmaps(&self.state, self.mode);
    }

    /// Turns the mesh off (`false`: every node is disconnected and each
    /// reconnect gets HTTP 404, as from a relay that turned its mesh off)
    /// or back on.
    pub fn set_mesh(&self, on: bool) {
        self.state.mesh_off.store(!on, Ordering::SeqCst);
        if !on {
            for (_, tx) in self.state.conns.lock().expect("conns").drain() {
                let _ = tx.send(WsMessage::Close(None));
            }
        }
    }

    /// Nodes connected right now.
    pub fn connected(&self) -> usize {
        self.state.conns.lock().expect("conns").len()
    }
}

impl Drop for FakeCoordinator {
    fn drop(&mut self) {
        self.stop();
    }
}

async fn stun_loop(sock: UdpSocket) {
    let mut buf = [0u8; 1500];
    loop {
        let Ok((n, src)) = sock.recv_from(&mut buf).await else {
            continue;
        };
        if let Some(tx) = crate::stun::parse_request(&buf[..n]) {
            let _ = sock.send_to(&crate::stun::response(tx, src), src).await;
        }
    }
}

fn peers_of(state: &State, mode: Mode, me: &PeerInfo) -> Vec<PeerInfo> {
    let nodes = state.nodes.lock().expect("nodes");
    let online = state.conns.lock().expect("conns");
    let pairs = state.pairs.lock().expect("pairs");
    nodes
        .values()
        .filter(|p| p.node_key != me.node_key && p.kind != me.kind)
        .filter(|p| match mode {
            Mode::Account => true,
            Mode::Static => {
                let (m, c) = match me.kind {
                    NodeKind::Machine => (me.node_key, p.node_key),
                    NodeKind::Client => (p.node_key, me.node_key),
                };
                pairs.contains(&(m, c))
            }
        })
        .map(|p| {
            let mut p = p.clone();
            p.online = online.contains_key(&p.node_key);
            if mode == Mode::Account && me.kind == NodeKind::Machine {
                p.grant = Some(Grant {
                    user: "user-1".into(),
                    account: "acct-1".into(),
                    email: Some("ada@example.com".into()),
                    name: Some("Ada".into()),
                    role: "owner".into(),
                    device: None,
                });
            }
            p
        })
        .collect()
}

fn push_netmaps(state: &State, mode: Mode) {
    let nodes: Vec<PeerInfo> = state
        .nodes
        .lock()
        .expect("nodes")
        .values()
        .cloned()
        .collect();
    let conns = state.conns.lock().expect("conns").clone();
    for n in nodes {
        if let Some(tx) = conns.get(&n.node_key) {
            let msg = ServerMsg::Netmap {
                peers: peers_of(state, mode, &n),
            };
            let _ = tx.send(WsMessage::Text(
                serde_json::to_string(&msg).expect("json").into(),
            ));
        }
    }
}

fn may_relay(state: &State, mode: Mode, a: &PublicKey, b: &PublicKey) -> bool {
    let nodes = state.nodes.lock().expect("nodes");
    let (Some(x), Some(y)) = (nodes.get(a), nodes.get(b)) else {
        return false;
    };
    if x.kind == y.kind {
        return false;
    }
    match mode {
        Mode::Account => true,
        Mode::Static => {
            let (m, c) = if x.kind == NodeKind::Machine {
                (a, b)
            } else {
                (b, a)
            };
            state.pairs.lock().expect("pairs").contains(&(*m, *c))
        }
    }
}

#[allow(clippy::result_large_err)]
async fn serve(state: Arc<State>, mode: Mode, tcp: tokio::net::TcpStream) {
    let mut machine_id = None::<String>;
    let denied = state.denied.lock().expect("denied").clone();
    let mesh_off = state.mesh_off.load(Ordering::SeqCst);
    let ws = tokio_tungstenite::accept_hdr_async(tcp, |req: &Request, resp: Response| {
        machine_id = req
            .headers()
            .get(proto::MACHINE_ID_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let refused = denied.iter().any(|(k, v)| {
            req.headers()
                .get(k.as_str())
                .and_then(|h| h.to_str().ok())
                .is_some_and(|h| h == v)
        });
        if mesh_off {
            let mut off = tokio_tungstenite::tungstenite::handshake::server::ErrorResponse::new(
                Some("this relay has no mesh coordinator".into()),
            );
            *off.status_mut() = tokio_tungstenite::tungstenite::http::StatusCode::NOT_FOUND;
            return Err(off);
        }
        if refused {
            let mut deny = tokio_tungstenite::tungstenite::handshake::server::ErrorResponse::new(
                Some("this device is not enrolled for your account".into()),
            );
            *deny.status_mut() = tokio_tungstenite::tungstenite::http::StatusCode::FORBIDDEN;
            return Err(deny);
        }
        Ok(resp)
    })
    .await;
    let Ok(ws) = ws else { return };
    let (mut sink, mut stream) = ws.split();
    let server = SecretKey::generate();
    let nonce = hex::encode(rand::random::<[u8; 16]>());
    let challenge = ServerMsg::Challenge {
        version: proto::VERSION,
        server_key: server.public(),
        nonce: nonce.clone(),
    };
    if sink
        .send(WsMessage::Text(
            serde_json::to_string(&challenge).expect("json").into(),
        ))
        .await
        .is_err()
    {
        return;
    }
    let Some(Ok(WsMessage::Text(t))) = stream.next().await else {
        return;
    };
    let Ok(ClientMsg::Hello {
        kind,
        node_key,
        disco_key,
        quic_pin,
        name,
        endpoints,
        proof,
        version,
        ..
    }) = serde_json::from_str::<ClientMsg>(&t)
    else {
        return;
    };
    if !proto::verify_hello_proof(&server, &node_key, &nonce, &proof) {
        let _ = sink
            .send(WsMessage::Text(
                serde_json::to_string(&ServerMsg::Error {
                    message: "bad proof".into(),
                })
                .expect("json")
                .into(),
            ))
            .await;
        return;
    }
    let info = PeerInfo {
        node_key,
        disco_key,
        quic_pin,
        kind,
        machine_id: if kind == NodeKind::Machine {
            machine_id
        } else {
            None
        },
        name,
        endpoints,
        online: true,
        version,
        grant: None,
    };
    let welcome = ServerMsg::Welcome {
        version: proto::VERSION,
        addr: node_key.mesh_addr(),
        mode,
        stun: Vec::new(),
    };
    if sink
        .send(WsMessage::Text(
            serde_json::to_string(&welcome).expect("json").into(),
        ))
        .await
        .is_err()
    {
        return;
    }
    let (tx, mut rx) = mpsc::unbounded_channel::<WsMessage>();
    state.nodes.lock().expect("nodes").insert(node_key, info);
    state
        .conns
        .lock()
        .expect("conns")
        .insert(node_key, tx.clone());
    push_netmaps(&state, mode);
    let writer = tokio::spawn(async move {
        while let Some(m) = rx.recv().await {
            if sink.send(m).await.is_err() {
                return;
            }
        }
    });
    while let Some(Ok(msg)) = stream.next().await {
        match msg {
            WsMessage::Binary(b) => {
                let Some((proto::FRAME_SEND, dst, packet)) = proto::parse_frame(&b) else {
                    continue;
                };
                if state.relay_off.load(Ordering::SeqCst)
                    || !may_relay(&state, mode, &node_key, &dst)
                {
                    continue;
                }
                state
                    .relayed_bytes
                    .fetch_add(packet.len() as u64, Ordering::Relaxed);
                {
                    let mut c = state.capture.lock().expect("capture");
                    if c.len() < 16 << 20 {
                        c.extend_from_slice(packet);
                    }
                }
                let target = state.conns.lock().expect("conns").get(&dst).cloned();
                if let Some(t) = target {
                    let _ = t.send(WsMessage::Binary(
                        proto::frame(proto::FRAME_RECV, &node_key, packet).into(),
                    ));
                }
            }
            WsMessage::Text(t) => match serde_json::from_str::<ClientMsg>(&t) {
                Ok(ClientMsg::Endpoints { endpoints }) => {
                    if let Some(n) = state.nodes.lock().expect("nodes").get_mut(&node_key) {
                        n.endpoints = endpoints;
                    }
                    push_netmaps(&state, mode);
                }
                Ok(ClientMsg::Ping) => {
                    let _ = tx.send(WsMessage::Text(
                        serde_json::to_string(&ServerMsg::Pong)
                            .expect("json")
                            .into(),
                    ));
                }
                Ok(ClientMsg::Pair {
                    machine_id,
                    ts,
                    proof,
                }) => {
                    let machine = state
                        .nodes
                        .lock()
                        .expect("nodes")
                        .values()
                        .find(|n| n.machine_id.as_deref() == Some(machine_id.as_str()))
                        .map(|n| n.node_key);
                    let conn =
                        machine.and_then(|m| state.conns.lock().expect("conns").get(&m).cloned());
                    match conn {
                        Some(c) => {
                            let id = hex::encode(rand::random::<[u8; 8]>());
                            state
                                .pending
                                .lock()
                                .expect("pending")
                                .insert(id.clone(), (node_key, machine_id));
                            let _ = c.send(WsMessage::Text(
                                serde_json::to_string(&ServerMsg::PairRequest {
                                    request_id: id,
                                    client_key: node_key,
                                    ts,
                                    proof,
                                    name: String::new(),
                                })
                                .expect("json")
                                .into(),
                            ));
                        }
                        None => {
                            let _ = tx.send(WsMessage::Text(
                                serde_json::to_string(&ServerMsg::PairResult {
                                    machine_id,
                                    ok: false,
                                    reason: "machine offline".into(),
                                })
                                .expect("json")
                                .into(),
                            ));
                        }
                    }
                }
                Ok(ClientMsg::PairReply {
                    request_id,
                    accept,
                    reason,
                }) => {
                    let Some((client, machine_id)) =
                        state.pending.lock().expect("pending").remove(&request_id)
                    else {
                        continue;
                    };
                    if accept {
                        state
                            .pairs
                            .lock()
                            .expect("pairs")
                            .insert((node_key, client));
                        push_netmaps(&state, mode);
                    }
                    let c = state.conns.lock().expect("conns").get(&client).cloned();
                    if let Some(c) = c {
                        let _ = c.send(WsMessage::Text(
                            serde_json::to_string(&ServerMsg::PairResult {
                                machine_id,
                                ok: accept,
                                reason,
                            })
                            .expect("json")
                            .into(),
                        ));
                    }
                }
                _ => {}
            },
            WsMessage::Close(_) => break,
            _ => {}
        }
    }
    writer.abort();
    let mut conns = state.conns.lock().expect("conns");
    if conns.get(&node_key).is_some_and(|c| c.same_channel(&tx)) {
        conns.remove(&node_key);
    }
    drop(conns);
    push_netmaps(&state, mode);
}
