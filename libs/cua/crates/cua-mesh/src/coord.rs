// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The node's connection to the coordinator: control messages and the
//! encrypted relay, over one WebSocket that reconnects on its own.
//!
//! The coordinator going away does not end sessions: peers keep their
//! WireGuard sessions and their last network map, so direct paths carry on
//! untouched and relayed traffic resumes when the connection comes back.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;

use crate::engine::Engine;
use crate::key::{NodeIdentity, PublicKey};
use crate::proto::{self, ClientMsg, Mode, NodeKind, ServerMsg};
use crate::{Error, Result};

/// Credentials for the coordinator connection, fetched before every
/// (re)connect: header name and value pairs.
pub type AuthFuture =
    Pin<Box<dyn Future<Output = std::result::Result<Vec<(String, String)>, String>> + Send>>;
/// Produces [`AuthFuture`]s.
pub type AuthProvider = Arc<dyn Fn() -> AuthFuture + Send + Sync>;

/// A static-mode pairing request a machine decides on: `(client key, ts,
/// proof)`; true accepts.
pub type PairVerifier = Arc<dyn Fn(&PublicKey, u64, &str) -> bool + Send + Sync>;

/// The control connection's state, for status and waiting.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ControlState {
    /// Connected and welcomed.
    pub connected: bool,
    /// Received at least one network map since start.
    pub has_netmap: bool,
    /// Account or static (once welcomed).
    pub mode: Option<Mode>,
    /// The coordinator's protocol version.
    pub version: Option<u32>,
    /// STUN servers it advertised.
    pub stun: Vec<String>,
    /// The last error, if the connection is down.
    pub last_error: Option<String>,
    /// Why the coordinator turned this node away (HTTP 401 or 403: its
    /// device was revoked or its account may no longer use the mesh),
    /// until it is welcomed again. Streams then fail at once with this
    /// reason instead of waiting on peers that dropped this node.
    pub denied: Option<String>,
    /// The relay answered that it has no mesh coordinator (it was turned
    /// off), until it is welcomed again.
    pub unsupported: bool,
    /// Connections made since start.
    pub connects: u64,
}

type PairWaiter = oneshot::Sender<std::result::Result<(), String>>;

pub(crate) struct Control {
    pub(crate) state: watch::Sender<ControlState>,
    out: Mutex<Option<mpsc::Sender<ClientMsg>>>,
    pair_waiters: Mutex<HashMap<String, Vec<PairWaiter>>>,
    endpoints: Mutex<Vec<std::net::SocketAddr>>,
}

pub(crate) struct ControlConfig {
    pub url: String,
    pub kind: NodeKind,
    pub name: String,
    pub auth: AuthProvider,
    pub pair_verifier: Option<PairVerifier>,
    /// Static mode, machines: peers this machine approved (filters netmaps).
    pub approved: Option<Arc<crate::node::Approved>>,
    /// Forgotten by the coordinator on disconnect.
    pub ephemeral: bool,
}

impl Control {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: watch::channel(ControlState::default()).0,
            out: Mutex::new(None),
            pair_waiters: Mutex::new(HashMap::new()),
            endpoints: Mutex::new(Vec::new()),
        })
    }

    pub(crate) fn send(&self, msg: ClientMsg) -> bool {
        match self.out.lock().expect("out").as_ref() {
            Some(tx) => tx.try_send(msg).is_ok(),
            None => false,
        }
    }

    pub(crate) fn set_endpoints(&self, endpoints: Vec<std::net::SocketAddr>) {
        let changed = {
            let mut e = self.endpoints.lock().expect("endpoints");
            let changed = *e != endpoints;
            *e = endpoints.clone();
            changed
        };
        if changed {
            self.send(ClientMsg::Endpoints { endpoints });
        }
    }

    pub(crate) async fn pair(
        &self,
        machine_id: &str,
        ts: u64,
        proof: String,
        timeout: Duration,
    ) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.pair_waiters
            .lock()
            .expect("pair")
            .entry(machine_id.to_owned())
            .or_default()
            .push(tx);
        if !self.send(ClientMsg::Pair {
            machine_id: machine_id.to_owned(),
            ts,
            proof,
        }) {
            return Err(Error::Unreachable(
                "not connected to the coordinator".into(),
            ));
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(Ok(()))) => Ok(()),
            Ok(Ok(Err(reason))) => Err(Error::Refused(reason)),
            _ => Err(Error::Unreachable(format!(
                "no pairing answer from {machine_id} within {timeout:?}"
            ))),
        }
    }

    fn finish_pair(&self, machine_id: &str, result: std::result::Result<(), String>) {
        if let Some(waiters) = self.pair_waiters.lock().expect("pair").remove(machine_id) {
            for w in waiters {
                let _ = w.send(result.clone());
            }
        }
    }
}

/// The coordinator's connect URL (`ws(s)://<base>/v1/mesh/connect`).
pub fn connect_url(base: &str) -> Result<String> {
    let mut url = url::Url::parse(base).map_err(|e| Error::Protocol(format!("{base:?}: {e}")))?;
    let scheme = match url.scheme() {
        "https" | "wss" => "wss",
        "http" | "ws" => "ws",
        other => return Err(Error::Protocol(format!("unsupported scheme {other:?}"))),
    };
    url.set_scheme(scheme)
        .map_err(|_| Error::Protocol("cannot set scheme".into()))?;
    let path = url.path().trim_end_matches('/').to_owned();
    url.set_path(&format!("{path}{}", proto::CONNECT_PATH));
    Ok(url.to_string())
}

fn tls_connector() -> tokio_tungstenite::Connector {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let cfg = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_protocol_versions(rustls::DEFAULT_VERSIONS)
    .expect("the ring provider supports the default TLS versions")
    .with_root_certificates(roots)
    .with_no_client_auth();
    tokio_tungstenite::Connector::Rustls(Arc::new(cfg))
}

/// The reason, when `e` is the coordinator refusing this node's
/// credentials (HTTP 401 or 403) rather than a transient failure (a
/// restarting relay answers 502 or 503 and is retried as before).
fn denial(e: &Error) -> Option<String> {
    match e {
        Error::Refused(m) if m.starts_with("HTTP 401") || m.starts_with("HTTP 403") => {
            Some(m.clone())
        }
        _ => None,
    }
}

fn backoff(attempt: u32) -> Duration {
    let base = Duration::from_millis(250)
        .saturating_mul(1 << attempt.min(6))
        .min(Duration::from_secs(10));
    base.mul_f64(rand::random_range(0.5..=1.0))
}

pub(crate) async fn run(
    cfg: ControlConfig,
    control: Arc<Control>,
    engine: Arc<Engine>,
    identity: NodeIdentity,
    shutdown: tokio_util_cancel::Token,
) {
    let mut attempt = 0u32;
    loop {
        if shutdown.is_cancelled() {
            return;
        }
        let started = std::time::Instant::now();
        let result = session(&cfg, &control, &engine, &identity, &shutdown).await;
        engine.set_relay(None);
        *control.out.lock().expect("out") = None;
        control.state.send_modify(|s| {
            s.connected = false;
            s.last_error = result.as_ref().err().map(ToString::to_string);
            if let Some(reason) = result.as_ref().err().and_then(denial) {
                s.denied = Some(reason);
            }
            if matches!(result, Err(Error::Unsupported(_))) {
                s.unsupported = true;
            }
        });
        if let Err(e) = &result {
            tracing::info!(error = %e, "mesh coordinator connection ended");
            if matches!(e, Error::Refused(_)) {
                attempt = attempt.max(5);
            }
        }
        if started.elapsed() > Duration::from_secs(30) {
            attempt = 0;
        }
        let delay = backoff(attempt);
        attempt = attempt.saturating_add(1);
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            _ = shutdown.cancelled() => return,
        }
    }
}

async fn session(
    cfg: &ControlConfig,
    control: &Arc<Control>,
    engine: &Arc<Engine>,
    identity: &NodeIdentity,
    shutdown: &tokio_util_cancel::Token,
) -> Result<()> {
    let url = connect_url(&cfg.url)?;
    let mut request = url
        .as_str()
        .into_client_request()
        .map_err(|e| Error::Protocol(e.to_string()))?;
    let headers = (cfg.auth)().await.map_err(Error::Refused)?;
    for (k, v) in headers {
        let name = http::HeaderName::from_bytes(k.as_bytes())
            .map_err(|e| Error::Protocol(e.to_string()))?;
        let mut value =
            http::HeaderValue::from_str(&v).map_err(|e| Error::Protocol(e.to_string()))?;
        value.set_sensitive(true);
        request.headers_mut().insert(name, value);
    }
    // An explicit TLS setup (ring, the web PKI roots): never the
    // process-wide default provider, which a host process may leave
    // ambiguous. No Nagle: relayed packets and control frames are small.
    let connect = tokio_tungstenite::connect_async_tls_with_config(
        request,
        None,
        true,
        Some(tls_connector()),
    );
    let (ws, _) = match tokio::time::timeout(Duration::from_secs(15), connect).await {
        Ok(Ok(v)) => v,
        Ok(Err(tokio_tungstenite::tungstenite::Error::Http(resp))) => {
            let status = resp.status();
            let body = resp
                .body()
                .as_ref()
                .map(|b| {
                    String::from_utf8_lossy(b)
                        .chars()
                        .take(200)
                        .collect::<String>()
                })
                .unwrap_or_default();
            return Err(if status.as_u16() == 404 {
                Error::Unsupported("the relay has no mesh coordinator".into())
            } else {
                Error::Refused(format!("HTTP {status}: {body}"))
            });
        }
        Ok(Err(e)) => return Err(Error::Unreachable(e.to_string())),
        Err(_) => return Err(Error::Unreachable("connect timed out".into())),
    };
    let (mut sink, mut stream) = ws.split();

    // Challenge, hello, welcome.
    let first = tokio::time::timeout(Duration::from_secs(10), stream.next())
        .await
        .map_err(|_| Error::Unreachable("no challenge".into()))?;
    let (server_key, nonce, server_version) = match first {
        Some(Ok(WsMessage::Text(t))) => match serde_json::from_str::<ServerMsg>(&t) {
            Ok(ServerMsg::Challenge {
                version,
                server_key,
                nonce,
            }) => (server_key, nonce, version),
            Ok(ServerMsg::Error { message }) => return Err(Error::Refused(message)),
            _ => return Err(Error::Protocol("expected a challenge".into())),
        },
        _ => return Err(Error::Protocol("expected a challenge".into())),
    };
    if server_version != proto::VERSION {
        return Err(Error::Unsupported(format!(
            "coordinator speaks mesh v{server_version}, this node v{}",
            proto::VERSION
        )));
    }
    let endpoints = control.endpoints.lock().expect("endpoints").clone();
    let hello = ClientMsg::Hello {
        version: proto::VERSION,
        kind: cfg.kind,
        node_key: identity.public(),
        disco_key: identity.disco.public(),
        quic_pin: identity.cert_pin(),
        name: cfg.name.clone(),
        endpoints,
        proof: proto::hello_proof(&identity.node, &server_key, &nonce),
        ephemeral: cfg.ephemeral,
    };
    sink.send(WsMessage::Text(
        serde_json::to_string(&hello).expect("json").into(),
    ))
    .await
    .map_err(|e| Error::Unreachable(e.to_string()))?;
    let welcome = tokio::time::timeout(Duration::from_secs(10), stream.next())
        .await
        .map_err(|_| Error::Unreachable("no welcome".into()))?;
    let (mode, stun, version) = match welcome {
        Some(Ok(WsMessage::Text(t))) => match serde_json::from_str::<ServerMsg>(&t) {
            Ok(ServerMsg::Welcome {
                version,
                addr,
                mode,
                stun,
            }) => {
                if addr != identity.addr() {
                    return Err(Error::Protocol(
                        "coordinator assigned a foreign address".into(),
                    ));
                }
                (mode, stun, version)
            }
            Ok(ServerMsg::Error { message }) => return Err(Error::Refused(message)),
            _ => return Err(Error::Protocol("expected a welcome".into())),
        },
        _ => return Err(Error::Protocol("expected a welcome".into())),
    };

    let (out_tx, mut out_rx) = mpsc::channel::<ClientMsg>(64);
    let (relay_tx, mut relay_rx) = mpsc::channel::<(PublicKey, Vec<u8>)>(2048);
    *control.out.lock().expect("out") = Some(out_tx.clone());
    engine.set_relay(Some(relay_tx));
    control.state.send_modify(|s| {
        s.connected = true;
        s.mode = Some(mode);
        s.version = Some(version);
        s.stun = stun;
        s.last_error = None;
        s.denied = None;
        s.unsupported = false;
        s.connects += 1;
    });
    tracing::info!(url = %cfg.url, ?mode, "mesh coordinator connected");

    let mut ping = tokio::time::interval(Duration::from_secs(15));
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_heard = tokio::time::Instant::now();
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => {
                let _ = sink.close().await;
                return Ok(());
            }
            msg = stream.next() => {
                last_heard = tokio::time::Instant::now();
                match msg {
                    Some(Ok(WsMessage::Binary(b))) => {
                        if let Some((proto::FRAME_RECV, src, packet)) = proto::parse_frame(&b) {
                            engine.on_relay(src, packet);
                        }
                    }
                    Some(Ok(WsMessage::Text(t))) => {
                        let Ok(msg) = serde_json::from_str::<ServerMsg>(&t) else { continue };
                        handle(cfg, control, engine, mode, msg, &out_tx);
                    }
                    Some(Ok(WsMessage::Ping(p))) => {
                        let _ = sink.send(WsMessage::Pong(p)).await;
                    }
                    Some(Ok(WsMessage::Close(_))) | None => {
                        return Err(Error::Unreachable("coordinator closed the connection".into()));
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => return Err(Error::Unreachable(e.to_string())),
                }
            }
            Some(msg) = out_rx.recv() => {
                let text = serde_json::to_string(&msg).expect("json");
                sink.send(WsMessage::Text(text.into())).await.map_err(|e| Error::Unreachable(e.to_string()))?;
            }
            Some((dst, packet)) = relay_rx.recv() => {
                let f = proto::frame(proto::FRAME_SEND, &dst, &packet);
                sink.feed(WsMessage::Binary(f.into())).await.map_err(|e| Error::Unreachable(e.to_string()))?;
                // Batch whatever else is queued, then flush once.
                while let Ok((dst, packet)) = relay_rx.try_recv() {
                    let f = proto::frame(proto::FRAME_SEND, &dst, &packet);
                    sink.feed(WsMessage::Binary(f.into())).await.map_err(|e| Error::Unreachable(e.to_string()))?;
                }
                sink.flush().await.map_err(|e| Error::Unreachable(e.to_string()))?;
            }
            _ = ping.tick() => {
                if last_heard.elapsed() > Duration::from_secs(40) {
                    return Err(Error::Unreachable("coordinator stopped answering".into()));
                }
                let text = serde_json::to_string(&ClientMsg::Ping).expect("json");
                sink.send(WsMessage::Text(text.into())).await.map_err(|e| Error::Unreachable(e.to_string()))?;
            }
        }
    }
}

fn handle(
    cfg: &ControlConfig,
    control: &Arc<Control>,
    engine: &Arc<Engine>,
    mode: Mode,
    msg: ServerMsg,
    out: &mpsc::Sender<ClientMsg>,
) {
    match msg {
        ServerMsg::Netmap { peers } => {
            let peers = match (&cfg.approved, mode, cfg.kind) {
                // A static-mode machine only ever talks to clients it
                // approved itself: the coordinator alone cannot add one.
                (Some(approved), Mode::Static, NodeKind::Machine) => peers
                    .into_iter()
                    .filter(|p| approved.contains(&p.node_key))
                    .collect(),
                _ => peers,
            };
            engine.set_peers(peers);
            control.state.send_modify(|s| s.has_netmap = true);
        }
        ServerMsg::PairRequest {
            request_id,
            client_key,
            ts,
            proof,
            name,
        } => {
            let accept = cfg
                .pair_verifier
                .as_ref()
                .is_some_and(|v| v(&client_key, ts, &proof));
            if accept && let Some(approved) = &cfg.approved {
                approved.add(client_key);
            }
            tracing::info!(client = %client_key.short(), %name, accept, "mesh pairing request");
            let _ = out.try_send(ClientMsg::PairReply {
                request_id,
                accept,
                reason: if accept {
                    String::new()
                } else {
                    "the machine did not accept the proof".into()
                },
            });
        }
        ServerMsg::PairResult {
            machine_id,
            ok,
            reason,
        } => control.finish_pair(&machine_id, if ok { Ok(()) } else { Err(reason) }),
        ServerMsg::Pong | ServerMsg::Challenge { .. } | ServerMsg::Welcome { .. } => {}
        ServerMsg::Error { message } => tracing::warn!(%message, "mesh coordinator error"),
    }
}

/// A minimal cancellation token (no extra dependency).
pub(crate) mod tokio_util_cancel {
    use std::sync::Arc;
    use tokio::sync::watch;

    #[derive(Clone)]
    pub(crate) struct Token(Arc<watch::Sender<bool>>);

    impl Token {
        pub(crate) fn new() -> Self {
            Self(Arc::new(watch::channel(false).0))
        }
        pub(crate) fn cancel(&self) {
            self.0.send_replace(true);
        }
        pub(crate) fn is_cancelled(&self) -> bool {
            *self.0.borrow()
        }
        pub(crate) async fn cancelled(&self) {
            let mut rx = self.0.subscribe();
            let _ = rx.wait_for(|v| *v).await;
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn connect_urls() {
        assert_eq!(
            super::connect_url("https://relay.example").unwrap(),
            "wss://relay.example/v1/mesh/connect"
        );
        assert_eq!(
            super::connect_url("http://127.0.0.1:8080/").unwrap(),
            "ws://127.0.0.1:8080/v1/mesh/connect"
        );
        assert!(super::connect_url("ftp://x").is_err());
    }
}
