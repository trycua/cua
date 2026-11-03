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
use tokio_tungstenite::tungstenite::protocol::CloseFrame;

use crate::engine::Engine;
use crate::key::{NodeIdentity, PublicKey};
use crate::proto::{self, ClientMsg, Mode, NodeKind, PeerUsage, ServerMsg};
use crate::reconnect::{self, Backoff};
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
    /// The [`ClientMsg::Usage`] version the coordinator meters (0: none).
    pub usage: u32,
}

/// A machine's stream usage: the snapshot to report, and the one this
/// connection last sent.
#[derive(Default)]
struct UsageOut {
    current: Vec<PeerUsage>,
    sent: Option<Vec<PeerUsage>>,
}

type PairWaiter = oneshot::Sender<std::result::Result<(), String>>;

pub(crate) struct Control {
    pub(crate) state: watch::Sender<ControlState>,
    out: Mutex<Option<mpsc::Sender<ClientMsg>>>,
    pair_waiters: Mutex<HashMap<String, Vec<PairWaiter>>>,
    endpoints: Mutex<Vec<std::net::SocketAddr>>,
    usage: Mutex<UsageOut>,
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
    /// Divides the reconnect wait (tests).
    pub reconnect_speedup: u32,
    /// The node identity file, when the identity is persistent: replaced by
    /// a new identity when other connections keep taking this one over (see
    /// [`MAX_TAKEOVERS`]).
    pub identity_path: Option<std::path::PathBuf>,
}

impl Control {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: watch::channel(ControlState::default()).0,
            out: Mutex::new(None),
            pair_waiters: Mutex::new(HashMap::new()),
            endpoints: Mutex::new(Vec::new()),
            usage: Mutex::new(UsageOut::default()),
        })
    }

    /// Sets the stream usage to report, and sends it when the coordinator
    /// meters it and it differs from what this connection last sent. A
    /// coordinator that does not meter is sent nothing.
    pub(crate) fn set_usage(&self, mut peers: Vec<PeerUsage>) {
        peers.truncate(proto::MAX_USAGE_PEERS);
        let mut usage = self.usage.lock().expect("usage");
        usage.current = peers;
        self.flush_usage(&mut usage);
    }

    /// A new connection was welcomed: what it last sent is gone.
    fn usage_reconnected(&self) {
        let mut usage = self.usage.lock().expect("usage");
        usage.sent = None;
        self.flush_usage(&mut usage);
    }

    fn flush_usage(&self, usage: &mut UsageOut) {
        let (connected, version) = {
            let s = self.state.borrow();
            (s.connected, s.usage)
        };
        if !connected || version < proto::USAGE_VERSION {
            return;
        }
        let unchanged = match &usage.sent {
            Some(sent) => *sent == usage.current,
            // A fresh connection starts with nothing open.
            None => usage.current.is_empty(),
        };
        if unchanged {
            return;
        }
        let msg = ClientMsg::Usage {
            v: proto::USAGE_VERSION,
            peers: usage.current.clone(),
        };
        // A full queue: tried again on the next change or snapshot.
        if self.send(msg) {
            usage.sent = Some(usage.current.clone());
        }
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

/// What the coordinator says when a node key is registered to another
/// owner (the account that was signed in before).
pub const KEY_OWNED_BY_ANOTHER: &str = "this node key belongs to another owner";

/// The reason, when `e` is the coordinator refusing this node's
/// credentials (HTTP 401 or 403) or its key (bound to another owner, for
/// life) rather than a transient failure (a restarting relay answers 502
/// or 503 and is retried as before).
fn denial(e: &Error) -> Option<String> {
    match e {
        Error::Refused(m)
            if m.starts_with("HTTP 401")
                || m.starts_with("HTTP 403")
                || m.contains(KEY_OWNED_BY_ANOTHER) =>
        {
            Some(m.clone())
        }
        _ => None,
    }
}

/// How one coordinator session ended, for the log and for the wait before
/// the next connect.
#[derive(Default)]
struct Ended {
    /// When the coordinator welcomed the node (never, if it did not).
    welcomed: Option<std::time::Instant>,
    /// The WebSocket close the coordinator sent: code and reason.
    close: Option<(u16, String)>,
    /// The last error message the coordinator sent before it ended.
    server_error: Option<String>,
    /// The coordinator ended the session (a close or a dropped
    /// connection after the welcome), as opposed to a failure on this side.
    by_relay: bool,
    /// The `Retry-After` of an HTTP 429 that refused the connection.
    http_retry_after: Option<Duration>,
}

/// The seconds of a `Retry-After` header (the delay form; a date is ignored),
/// capped so a bad header cannot park a node for long.
fn retry_after_header(headers: &http::HeaderMap) -> Option<Duration> {
    let secs: u64 = headers
        .get(http::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()?;
    Some(Duration::from_secs(secs.min(3600)))
}

impl Ended {
    /// Notes a close frame and returns the error that ends the session.
    fn closed(&mut self, frame: Option<CloseFrame>) -> Error {
        self.by_relay = true;
        match frame {
            Some(f) => {
                let (code, reason) = (u16::from(f.code), f.reason.to_string());
                self.close = Some((code, reason.clone()));
                Error::Unreachable(format!(
                    "coordinator closed the connection ({code}: {reason})"
                ))
            }
            None => Error::Unreachable("coordinator closed the connection".into()),
        }
    }

    fn close_code(&self) -> Option<u16> {
        self.close.as_ref().map(|(c, _)| *c)
    }

    fn retry_after(&self) -> Option<Duration> {
        if let Some(wait) = self.http_retry_after {
            return Some(wait);
        }
        let (code, reason) = self.close.as_ref()?;
        (*code == proto::CLOSE_RATE_LIMITED)
            .then(|| proto::parse_retry_after(reason))
            .flatten()
            .map(Duration::from_secs)
    }
}

/// Consecutive sessions the coordinator ended with "another connection
/// took over" before this node stops reconnecting.
const MAX_TAKEOVERS: u32 = 3;

const TAKEN_OVER: &str = "another copy of Spaces keeps taking over this computer's connection \
    (two copies share one identity). Quit the other copy, then reopen Spaces here.";

/// Removes the identity file at `path` if it still holds `identity`.
fn retire_identity(path: &std::path::Path, identity: &NodeIdentity) {
    let ours = std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| NodeIdentity::from_json(&raw).ok())
        .is_some_and(|on_disk| on_disk.public() == identity.public());
    if ours && let Err(e) = std::fs::remove_file(path) {
        tracing::warn!(error = %e, "could not retire the mesh identity");
    }
}

pub(crate) async fn run(
    cfg: ControlConfig,
    control: Arc<Control>,
    engine: Arc<Engine>,
    identity: NodeIdentity,
    shutdown: tokio_util_cancel::Token,
) {
    let mut backoff = Backoff::default();
    let mut takeovers = 0u32;
    loop {
        if shutdown.is_cancelled() {
            return;
        }
        let mut ended = Ended::default();
        let result = session(&cfg, &control, &engine, &identity, &shutdown, &mut ended).await;
        engine.set_relay(None);
        *control.out.lock().expect("out") = None;
        if shutdown.is_cancelled() {
            return;
        }
        // Only the time since the welcome is a healthy session.
        let lived = ended.welcomed.map_or(Duration::ZERO, |t| t.elapsed());
        if ended.close_code() == Some(proto::CLOSE_REPLACED) {
            takeovers += 1;
        } else {
            takeovers = 0;
        }
        let gave_up = takeovers >= MAX_TAKEOVERS;
        control.state.send_modify(|s| {
            s.connected = false;
            s.last_error = result.as_ref().err().map(ToString::to_string);
            if let Some(reason) = result.as_ref().err().and_then(denial) {
                s.denied = Some(reason);
            }
            if gave_up {
                s.denied = Some(TAKEN_OVER.into());
            }
            if matches!(result, Err(Error::Unsupported(_))) {
                s.unsupported = true;
            }
        });
        if matches!(result, Err(Error::Refused(_))) {
            backoff.at_least(5);
        }
        let unit = rand::random_range(0.0..=1.0);
        let mut delay = backoff.next(lived, unit);
        // A relay close, or any drop after the welcome (a crashed relay
        // resets the socket without a close frame): all nodes lose it at once.
        let by_relay = ended.by_relay || (ended.welcomed.is_some() && result.is_err());
        if by_relay {
            delay += reconnect::spread(rand::random_range(0.0..=1.0));
        }
        if let Some(wait) = ended.retry_after() {
            delay = delay.max(wait);
        }
        let close = ended.close.as_ref();
        tracing::warn!(
            error = result.as_ref().err().map(ToString::to_string),
            close_code = close.map(|(c, _)| *c),
            close_reason = close.map(|(_, r)| r.as_str()),
            server_error = ended.server_error.as_deref(),
            welcomed = ended.welcomed.is_some(),
            lived_ms = lived.as_millis() as u64,
            by_relay,
            takeovers,
            retry_in_ms = delay.as_millis() as u64,
            "mesh coordinator session ended"
        );
        if gave_up {
            tracing::error!("{TAKEN_OVER} Not reconnecting.");
            // A copy of this identity file on another computer (a cloned
            // disk or image) would keep fighting for the key, so the next
            // node on this computer starts with a new one.
            if let Some(path) = &cfg.identity_path {
                retire_identity(path, &identity);
            }
            return;
        }
        tokio::select! {
            _ = tokio::time::sleep(delay / cfg.reconnect_speedup.max(1)) => {}
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
    ended: &mut Ended,
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
    // The version in the relay's logs: who loops is then one grep. A
    // credential provider may set its own.
    if !request.headers().contains_key(http::header::USER_AGENT) {
        request.headers_mut().insert(
            http::header::USER_AGENT,
            http::HeaderValue::from_static(concat!("cua-mesh/", env!("CARGO_PKG_VERSION"))),
        );
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
            if status.as_u16() == 429 {
                // Too many connections: wait what the relay says, at least.
                ended.http_retry_after = retry_after_header(resp.headers());
            }
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
            Ok(ServerMsg::Error { message }) => {
                ended.server_error = Some(message.clone());
                return Err(Error::Refused(message));
            }
            _ => return Err(Error::Protocol("expected a challenge".into())),
        },
        Some(Ok(WsMessage::Close(frame))) => return Err(ended.closed(frame)),
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
    let (mode, stun, version, usage) = match welcome {
        Some(Ok(WsMessage::Text(t))) => match serde_json::from_str::<ServerMsg>(&t) {
            Ok(ServerMsg::Welcome {
                version,
                addr,
                mode,
                stun,
                usage,
            }) => {
                if addr != identity.addr() {
                    return Err(Error::Protocol(
                        "coordinator assigned a foreign address".into(),
                    ));
                }
                (mode, stun, version, usage)
            }
            Ok(ServerMsg::Error { message }) => {
                ended.server_error = Some(message.clone());
                return Err(Error::Refused(message));
            }
            _ => return Err(Error::Protocol("expected a welcome".into())),
        },
        Some(Ok(WsMessage::Close(frame))) => return Err(ended.closed(frame)),
        _ => return Err(Error::Protocol("expected a welcome".into())),
    };

    ended.welcomed = Some(std::time::Instant::now());
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
        s.usage = usage;
    });
    tracing::info!(url = %cfg.url, ?mode, "mesh coordinator connected");
    control.usage_reconnected();

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
                        handle(cfg, control, engine, mode, msg, &out_tx, ended);
                    }
                    Some(Ok(WsMessage::Ping(p))) => {
                        let _ = sink.send(WsMessage::Pong(p)).await;
                    }
                    Some(Ok(WsMessage::Close(frame))) => return Err(ended.closed(frame)),
                    None => return Err(ended.closed(None)),
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
    ended: &mut Ended,
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
                    "the computer did not accept the proof".into()
                },
            });
        }
        ServerMsg::PairResult {
            machine_id,
            ok,
            reason,
        } => control.finish_pair(&machine_id, if ok { Ok(()) } else { Err(reason) }),
        ServerMsg::Pong | ServerMsg::Challenge { .. } | ServerMsg::Welcome { .. } => {}
        ServerMsg::Error { message } => {
            tracing::warn!(%message, "mesh coordinator error");
            ended.server_error = Some(message);
        }
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
    use super::*;

    fn frame(code: u16, reason: &str) -> Option<CloseFrame> {
        Some(CloseFrame {
            code: code.into(),
            reason: reason.into(),
        })
    }

    #[test]
    fn a_close_frame_is_recorded_with_its_reason() {
        let mut ended = Ended::default();
        let e = ended.closed(frame(proto::CLOSE_REPLACED, "replaced by a newer session"));
        assert!(ended.by_relay);
        assert_eq!(ended.close_code(), Some(proto::CLOSE_REPLACED));
        let text = e.to_string();
        assert!(text.contains("4001") && text.contains("replaced by a newer session"));
        // A bare close or a dropped connection has no reason but is the relay's.
        let mut bare = Ended::default();
        bare.closed(None);
        assert!(bare.by_relay && bare.close.is_none());
    }

    #[test]
    fn an_http_429_retry_after_is_honoured_and_capped() {
        let mut h = http::HeaderMap::new();
        assert_eq!(retry_after_header(&h), None);
        h.insert(http::header::RETRY_AFTER, "42".parse().unwrap());
        assert_eq!(retry_after_header(&h), Some(Duration::from_secs(42)));
        h.insert(http::header::RETRY_AFTER, "999999".parse().unwrap());
        assert_eq!(retry_after_header(&h), Some(Duration::from_secs(3600)));
        h.insert(
            http::header::RETRY_AFTER,
            "Wed, 21 Oct 2026 07:28:00 GMT".parse().unwrap(),
        );
        assert_eq!(retry_after_header(&h), None);
        let ended = Ended {
            http_retry_after: Some(Duration::from_secs(9)),
            ..Ended::default()
        };
        assert_eq!(ended.retry_after(), Some(Duration::from_secs(9)));
    }

    #[test]
    fn a_node_that_gave_up_retires_its_own_identity_file_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("client.json");
        let mine = NodeIdentity::load_or_create(&path).unwrap();
        // Someone else's identity on disk is left alone.
        let other = NodeIdentity::generate().unwrap();
        retire_identity(&path, &other);
        assert!(path.exists());
        retire_identity(&path, &mine);
        assert!(!path.exists());
        // The next start gets a new key.
        assert_ne!(
            NodeIdentity::load_or_create(&path).unwrap().public(),
            mine.public()
        );
    }

    #[test]
    fn a_rate_limit_close_says_how_long_to_wait() {
        let mut ended = Ended::default();
        ended.closed(frame(
            proto::CLOSE_RATE_LIMITED,
            &proto::retry_after_reason(37),
        ));
        assert_eq!(ended.retry_after(), Some(Duration::from_secs(37)));
        // Another code never carries a wait.
        let mut other = Ended::default();
        other.closed(frame(proto::CLOSE_DEVICE, "retry after 5 s"));
        assert_eq!(other.retry_after(), None);
        assert_eq!(proto::parse_retry_after("nothing here"), None);
    }

    fn usage(peer: PublicKey, open: u32) -> Vec<PeerUsage> {
        vec![PeerUsage {
            peer,
            user: "ada".into(),
            open,
            live: false,
        }]
    }

    #[test]
    fn usage_goes_only_to_a_coordinator_that_meters_it_and_only_when_it_changed() {
        let control = Control::new();
        let (tx, mut rx) = mpsc::channel(8);
        *control.out.lock().unwrap() = Some(tx);
        let peer = crate::key::SecretKey::generate().public();
        // An older coordinator (or a static one): nothing is ever sent.
        control.state.send_modify(|s| {
            s.connected = true;
            s.usage = 0;
        });
        control.usage_reconnected();
        control.set_usage(usage(peer, 1));
        assert!(rx.try_recv().is_err());
        // One that meters: the snapshot on welcome, then only changes.
        control
            .state
            .send_modify(|s| s.usage = proto::USAGE_VERSION);
        control.usage_reconnected();
        assert!(
            matches!(rx.try_recv(), Ok(ClientMsg::Usage { v: 1, peers }) if peers[0].open == 1)
        );
        control.set_usage(usage(peer, 1));
        assert!(rx.try_recv().is_err());
        control.set_usage(usage(peer, 2));
        assert!(matches!(rx.try_recv(), Ok(ClientMsg::Usage { peers, .. }) if peers[0].open == 2));
        control.set_usage(Vec::new());
        assert!(matches!(rx.try_recv(), Ok(ClientMsg::Usage { peers, .. }) if peers.is_empty()));
        // Disconnected: kept, and sent again on the next welcome.
        control.state.send_modify(|s| s.connected = false);
        control.set_usage(usage(peer, 3));
        assert!(rx.try_recv().is_err());
        control.state.send_modify(|s| s.connected = true);
        control.usage_reconnected();
        assert!(matches!(rx.try_recv(), Ok(ClientMsg::Usage { peers, .. }) if peers[0].open == 3));
        // A fresh connection with nothing open sends nothing.
        control.set_usage(Vec::new());
        let _ = rx.try_recv();
        control.usage_reconnected();
        assert!(rx.try_recv().is_err());
    }

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
