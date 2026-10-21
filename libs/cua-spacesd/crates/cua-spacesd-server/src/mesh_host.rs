// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The machine side of the mesh (`cua-spacesd join`).
//!
//! Next to its relay connection, a hosted machine runs a mesh node (MIT
//! `cua-mesh`) when the relay coordinates one. Peers reach the machine's
//! spacesd over WireGuard end to end, so the relay only ever relays
//! ciphertext for them:
//!
//! - **Account machines.** Every stream comes from a peer the relay's map
//!   vouched for (its account, email and role, the same facts a relay
//!   assertion carries). A small local proxy strips any client credential
//!   and adds an assertion this process signs with an in-process key the
//!   machine trusts next to the relay's, so spacesd's own host policy
//!   (owner, local allow list, sharing switch, view-only shares) decides
//!   exactly as it does for relayed calls. The assertion is only added when
//!   the request carries the per-forward session key its dialer chose, so
//!   another local user on the client machine cannot borrow the session.
//! - **Static machines.** Clients pair by proving they hold the machine's
//!   env token (checked here, never sent); the machine approves and
//!   remembers them, and filters the relay's map to its own approvals.
//!   Streams are spliced to spacesd unchanged: the env token still travels
//!   in each request, now inside WireGuard.
//! - **QUIC media.** Mesh UDP [`cua_mesh::MEDIA_UDP_PORT`] reaches the
//!   local QUIC media port, so QUIC streaming works over the mesh.
//! - **Metering (account machines).** The relay meters sessions on its
//!   HTTPS path itself; mesh traffic never passes it in the clear, so this
//!   node tells it which peers hold streams open (requests in flight,
//!   WebSockets, media flows), as a small snapshot sent when it changes and
//!   only to a relay that asks for it. The relay checks each peer against
//!   this machine's map and meters it as that peer's own account.
//!
//! A relay without a mesh (an older one) is simply not used: the machine
//! keeps serving over its relay connection and checks again later.
//!
//! The node runs unless [`cua_mesh::policy`] turns it off: `CUA_MESH=off`
//! in the service's environment keeps the machine on its relay connection
//! alone, and nothing is asked of the relay.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use cua_mesh::{Grant, IncomingStream, Mesh, MeshConfig, NodeKind, PeerUsage, Policy, PublicKey};
use http::{HeaderValue, Request, Response, StatusCode, Version};
use http_body_util::BodyExt as _;
use hyper::body::Incoming;
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio_util::sync::CancellationToken;

use cua_relay::assertion::{AssertionClaims, RelayKey, TrustedKeys, ASSERTION_HEADER};
use cua_relay::client::AccountLink;

/// The service name spacesd streams use.
pub const SPACESD_SERVICE: &str = "spacesd";
/// The request header carrying the dialer's per-forward session key.
pub const SESSION_HEADER: &str = "x-cua-mesh-session";

/// Client credential headers a mesh request never forwards in account mode
/// (the same set the relay strips).
const CLIENT_CREDENTIAL_HEADERS: &[&str] = &[
    "authorization",
    "x-cua-relay-authorization",
    "x-cua-env-authorization",
    "x-cua-device-session",
    ASSERTION_HEADER,
];

/// How often the streams peers hold open are checked and, when they
/// changed, reported to the relay.
pub const USAGE_EVERY: Duration = Duration::from_secs(1);
/// A media flow counts as open while its peer sent within this.
pub const MEDIA_ACTIVE: Duration = Duration::from_secs(10);

/// The streams each account peer holds open on this machine, for the
/// relay's allowance meter.
#[derive(Default)]
pub struct PeerStreams {
    peers: Mutex<HashMap<PublicKey, PeerOpen>>,
}

#[derive(Default)]
struct PeerOpen {
    user: String,
    open: u32,
    /// A WebSocket upgraded since the last snapshot.
    upgraded: bool,
}

/// One open stream of a peer; closes on drop.
pub struct StreamUse {
    streams: Arc<PeerStreams>,
    peer: PublicKey,
}

impl PeerStreams {
    /// A stream of `peer` (whose grant names `user`) opens.
    pub fn open(self: &Arc<Self>, peer: PublicKey, user: &str) -> StreamUse {
        let mut peers = self.peers.lock().expect("peer streams");
        let p = peers.entry(peer).or_default();
        p.user = user.to_owned();
        p.open += 1;
        StreamUse {
            streams: self.clone(),
            peer,
        }
    }

    /// `peer` upgraded a stream (a WebSocket): a session at once.
    pub fn upgraded(&self, peer: &PublicKey) {
        if let Some(p) = self.peers.lock().expect("peer streams").get_mut(peer) {
            p.upgraded = true;
        }
    }

    /// What to report now: every peer with a stream open or just upgraded,
    /// plus `media` peers (with their grants), sorted by key.
    pub fn snapshot(&self, media: &[(PublicKey, String)]) -> Vec<PeerUsage> {
        let mut out: HashMap<PublicKey, PeerUsage> = HashMap::new();
        {
            let mut peers = self.peers.lock().expect("peer streams");
            for (k, p) in peers.iter_mut() {
                if p.open > 0 || p.upgraded {
                    out.insert(
                        *k,
                        PeerUsage {
                            peer: *k,
                            user: p.user.clone(),
                            open: p.open,
                            live: std::mem::take(&mut p.upgraded),
                        },
                    );
                }
            }
            peers.retain(|_, p| p.open > 0);
        }
        for (k, user) in media {
            let e = out.entry(*k).or_insert_with(|| PeerUsage {
                peer: *k,
                user: user.clone(),
                open: 0,
                live: false,
            });
            // An active media flow is a held stream (a desktop view).
            e.open += 1;
            e.live = true;
        }
        let mut out: Vec<_> = out.into_values().filter(|p| p.open > 0).collect();
        out.sort_by_key(|p| p.peer);
        out
    }
}

impl Drop for StreamUse {
    fn drop(&mut self) {
        let mut peers = self.streams.peers.lock().expect("peer streams");
        if let Some(p) = peers.get_mut(&self.peer) {
            p.open = p.open.saturating_sub(1);
        }
    }
}

/// The machine's env token, read when a pairing proof arrives.
pub type EnvToken = Arc<dyn Fn() -> Option<String> + Send + Sync>;

/// The mesh policy now (re-read while the machine runs).
pub type PolicySource = Arc<dyn Fn() -> Policy + Send + Sync>;

/// How often a machine whose mesh is off checks whether it was turned on
/// (and a running node whether it was turned off). Local reads only.
pub const GATE_RECHECK: Duration = Duration::from_secs(30);

/// The in-process keys this process's mesh endpoints sign assertions with
/// (never leave the process). [`verify_local`] trusts them next to the
/// relay's keys.
static LOCAL_KEYS: LazyLock<Mutex<Vec<serde_json::Value>>> = LazyLock::new(Mutex::default);
static LOCAL_TRUSTED: LazyLock<TrustedKeys> = LazyLock::new(TrustedKeys::default);

fn trust_local(key: &RelayKey) -> Result<(), String> {
    let mut keys = LOCAL_KEYS.lock().expect("local keys");
    let jwks = key.jwks();
    keys.extend(jwks["keys"].as_array().cloned().unwrap_or_default());
    LOCAL_TRUSTED
        .set_jwks_json(&serde_json::json!({ "keys": *keys }).to_string())
        .map(|_| ())
}

/// Verifies an assertion this process's mesh endpoint signed (for an
/// account peer the relay's network map identified). `None` when no mesh
/// endpoint ever ran here.
pub fn verify_local(token: &str, machine_id: &str) -> Option<Result<AssertionClaims, String>> {
    if LOCAL_TRUSTED.is_empty() {
        return None;
    }
    Some(LOCAL_TRUSTED.verify(token, machine_id))
}

/// `bytes` bytes from the OS RNG, lowercase hex (assertion ids).
fn os_random_hex(bytes: usize) -> String {
    use rand::RngCore as _;
    let mut buf = vec![0u8; bytes];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    hex::encode(buf)
}

/// How the machine joins the mesh.
#[derive(Clone)]
pub struct MeshHostConfig {
    /// Relay base URL.
    pub relay_url: String,
    /// The machine token (account) or static registration token.
    pub relay_token: String,
    /// Re-read before every connect when set (token rotation).
    pub relay_token_file: Option<PathBuf>,
    /// The machine id.
    pub machine_id: String,
    /// Where the node identity and approvals live.
    pub dir: PathBuf,
    /// The local spacesd listener.
    pub local: SocketAddr,
    /// The local QUIC media port, if QUIC runs.
    pub quic: Option<SocketAddr>,
    /// Account details learned on the relay handshake.
    pub account: Arc<AccountLink>,
    /// The env token, for static-mode pairing proofs.
    pub env_token: EnvToken,
    /// Display name.
    pub name: String,
    /// Whether the mesh may run ([`cua_mesh::policy`]).
    pub policy: PolicySource,
    /// How often [`Self::policy`] is checked again ([`GATE_RECHECK`]).
    pub recheck: Duration,
}

impl std::fmt::Debug for MeshHostConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MeshHostConfig")
            .field("relay_url", &self.relay_url)
            .field("machine_id", &self.machine_id)
            .finish()
    }
}

/// Whether the relay at `url` coordinates a mesh this build speaks.
pub async fn relay_has_mesh(url: &str) -> Result<bool, String> {
    let info = format!("{}/v1/info", url.trim_end_matches('/'));
    let info = info
        .replacen("wss://", "https://", 1)
        .replacen("ws://", "http://", 1);
    let resp = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?
        .get(&info)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Ok(false);
    }
    let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    Ok(v.get("mesh")
        .and_then(|m| m.get("version"))
        .and_then(|v| v.as_u64())
        == Some(u64::from(cua_mesh::VERSION)))
}

/// Runs the machine's mesh node until `shutdown`: while the policy allows
/// it and the relay coordinates a mesh (re-checking every 10 minutes while
/// it does not), stopping it again when the policy turns it off.
pub async fn run(cfg: MeshHostConfig, shutdown: CancellationToken) {
    let ctx_key = Arc::new(RelayKey::generate());
    let mut trusted = false;
    loop {
        if !wait_until_allowed(&cfg, &shutdown).await {
            return;
        }
        let mesh = match start(&cfg).await {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(error = %e, "mesh unavailable; serving over the relay only");
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(600)) => continue,
                    _ = shutdown.cancelled() => return,
                }
            }
        };
        tracing::info!(addr = %mesh.addr(), "machine joined the mesh");
        if !trusted {
            match trust_local(&ctx_key) {
                Ok(()) => trusted = true,
                Err(e) => tracing::warn!(
                    error = %e,
                    "cannot trust the mesh key; account peers cannot authenticate"
                ),
            }
        }
        let streams = Arc::new(PeerStreams::default());
        let reporter = tokio::spawn(report_usage(
            mesh.clone(),
            streams.clone(),
            cfg.account.clone(),
        ));
        let ctx = Arc::new(Ctx {
            cfg: cfg.clone(),
            key: ctx_key.clone(),
            streams,
        });
        let mut gate = tokio::time::interval(cfg.recheck);
        gate.tick().await;
        let stopped = loop {
            let incoming = tokio::select! {
                i = mesh.accept() => i,
                _ = gate.tick() => {
                    if (cfg.policy)().uses_mesh() {
                        continue;
                    }
                    tracing::info!("the mesh was turned off; serving over the relay only");
                    break false;
                }
                _ = shutdown.cancelled() => break true,
            };
            let Some(incoming) = incoming else {
                break false;
            };
            let ctx = ctx.clone();
            tokio::spawn(async move { handle(ctx, incoming).await });
        };
        reporter.abort();
        mesh.shutdown();
        if stopped {
            return;
        }
    }
}

/// Waits until the policy allows the mesh and the relay coordinates one
/// this build speaks. False when `shutdown` came first. While the policy
/// is off, nothing is asked of the relay.
async fn wait_until_allowed(cfg: &MeshHostConfig, shutdown: &CancellationToken) -> bool {
    let mut said = false;
    loop {
        let wait = if !(cfg.policy)().uses_mesh() {
            cfg.recheck
        } else {
            match relay_has_mesh(&cfg.relay_url).await {
                Ok(true) => return true,
                Ok(false) => {
                    if !said {
                        tracing::info!(
                            "the relay has no mesh coordinator; serving over the relay only"
                        );
                        said = true;
                    }
                }
                Err(e) => tracing::debug!(error = %e, "relay info"),
            }
            Duration::from_secs(600)
        };
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = shutdown.cancelled() => return false,
        }
    }
}

/// Reports the streams account peers hold open to the relay, whenever
/// they change ([`Mesh::report_usage`] sends nothing unchanged, and
/// nothing to a relay that does not meter).
async fn report_usage(mesh: Mesh, streams: Arc<PeerStreams>, account: Arc<AccountLink>) {
    let mut tick = tokio::time::interval(USAGE_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        if account.owner().is_none() {
            continue;
        }
        let active = mesh.udp_active_peers(MEDIA_ACTIVE);
        let media: Vec<(PublicKey, String)> = if active.is_empty() {
            Vec::new()
        } else {
            mesh.peers()
                .into_iter()
                .filter(|p| active.contains(&p.node_key))
                .filter_map(|p| p.grant.map(|g| (p.node_key, g.user)))
                .collect()
        };
        mesh.report_usage(streams.snapshot(&media));
    }
}

async fn start(cfg: &MeshHostConfig) -> cua_mesh::Result<Mesh> {
    let token = cfg.relay_token.clone();
    let token_file = cfg.relay_token_file.clone();
    let machine_id = cfg.machine_id.clone();
    let auth: cua_mesh::AuthProvider = Arc::new(move || {
        let token = token_file
            .as_deref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| token.clone());
        let id = machine_id.clone();
        Box::pin(async move {
            Ok(vec![
                ("authorization".to_owned(), format!("Bearer {token}")),
                (cua_mesh::proto::MACHINE_ID_HEADER.to_owned(), id),
            ])
        })
    });
    let mut mc = MeshConfig::new(
        cfg.relay_url.clone(),
        NodeKind::Machine,
        Some(cfg.dir.join("identity.json")),
        auth,
    );
    mc.name = cfg.name.clone();
    mc.approved_path = Some(cfg.dir.join("approved.json"));
    let env = cfg.env_token.clone();
    let id = cfg.machine_id.clone();
    mc.pair_verifier = Some(Arc::new(move |client, ts, proof| {
        env().is_some_and(|token| {
            cua_mesh::proto::verify_pair_proof(
                &token,
                &id,
                client,
                ts,
                proof,
                cua_mesh::proto::now_secs(),
            )
        })
    }));
    let mesh = Mesh::start(mc).await?;
    if let Some(quic) = cfg.quic {
        mesh.serve_udp(cua_mesh::MEDIA_UDP_PORT, quic)?;
    }
    Ok(mesh)
}

struct Ctx {
    cfg: MeshHostConfig,
    key: Arc<RelayKey>,
    streams: Arc<PeerStreams>,
}

async fn handle(ctx: Arc<Ctx>, incoming: IncomingStream) {
    if incoming.header.service != SPACESD_SERVICE {
        return;
    }
    let account_machine = ctx.cfg.account.owner().is_some();
    match (incoming.peer.grant.clone(), account_machine) {
        (Some(grant), true) => {
            let peer = incoming.peer.node_key;
            serve_with_identity(ctx, incoming.stream, peer, grant, incoming.header.session).await
        }
        (None, false) => splice(incoming.stream, ctx.cfg.local).await,
        // A peer without an identity on an account machine (or the other
        // way round) has nothing to do here.
        _ => tracing::warn!(peer = %incoming.peer.node_key.short(), "mesh stream refused"),
    }
}

async fn splice(mut stream: cua_mesh::MeshStream, local: SocketAddr) {
    match tokio::net::TcpStream::connect(local).await {
        Ok(mut tcp) => {
            let _ = tcp.set_nodelay(true);
            let _ = tokio::io::copy_bidirectional(&mut stream, &mut tcp).await;
        }
        Err(e) => tracing::warn!(error = %e, %local, "cannot reach the local spacesd"),
    }
}

type BoxBody = http_body_util::combinators::BoxBody<bytes::Bytes, hyper::Error>;

fn plain(status: StatusCode, msg: &str) -> Response<BoxBody> {
    let mut r = Response::new(
        http_body_util::Full::new(bytes::Bytes::from(msg.to_owned()))
            .map_err(|never| match never {})
            .boxed(),
    );
    *r.status_mut() = status;
    r
}

/// Serves HTTP/1.1 and HTTP/2 (gRPC, WebSockets, files) on one mesh
/// stream, adding the peer's identity, and forwards to the local spacesd.
async fn serve_with_identity(
    ctx: Arc<Ctx>,
    stream: cua_mesh::MeshStream,
    peer: PublicKey,
    grant: Grant,
    session: Option<String>,
) {
    let h2: Arc<tokio::sync::OnceCell<hyper::client::conn::http2::SendRequest<Incoming>>> =
        Arc::default();
    let grant = Arc::new(grant);
    let service = hyper::service::service_fn(move |req: Request<Incoming>| {
        let ctx = ctx.clone();
        let grant = grant.clone();
        let session = session.clone();
        let h2 = h2.clone();
        async move {
            // Held until the response body ends (or the upgraded socket
            // closes), as the relay's HTTPS path meters its streams.
            let held = ctx.streams.open(peer, &grant.user);
            let streams = ctx.streams.clone();
            let resp = forward(ctx, grant, session, h2, req, held).await;
            if resp.status() == StatusCode::SWITCHING_PROTOCOLS {
                streams.upgraded(&peer);
            }
            Ok::<_, std::convert::Infallible>(resp)
        }
    });
    let builder = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new());
    if let Err(e) = builder
        .serve_connection_with_upgrades(TokioIo::new(stream), service)
        .await
    {
        tracing::debug!(error = %e, "mesh stream ended");
    }
}

fn mint(ctx: &Ctx, grant: &Grant) -> String {
    let now = cua_relay::assertion::now_secs();
    ctx.key.sign(&AssertionClaims {
        iss: "cua-mesh".into(),
        aud: ctx.cfg.machine_id.clone(),
        sub: grant.user.clone(),
        acct: grant.account.clone(),
        email: grant.email.clone(),
        name: grant.name.clone(),
        mid: ctx.cfg.machine_id.clone(),
        role: grant.role.clone(),
        scope: "env".into(),
        iat: now,
        exp: now + cua_relay::assertion::MAX_ASSERTION_TTL_SECS,
        jti: os_random_hex(16),
    })
}

async fn forward(
    ctx: Arc<Ctx>,
    grant: Arc<Grant>,
    session: Option<String>,
    h2: Arc<tokio::sync::OnceCell<hyper::client::conn::http2::SendRequest<Incoming>>>,
    mut req: Request<Incoming>,
    held: StreamUse,
) -> Response<BoxBody> {
    let presented = req
        .headers()
        .get(SESSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let headers = req.headers_mut();
    for name in CLIENT_CREDENTIAL_HEADERS {
        headers.remove(*name);
    }
    headers.remove(SESSION_HEADER);
    let authentic = match (&session, &presented) {
        (Some(want), Some(got)) => {
            use subtle::ConstantTimeEq as _;
            want.len() == got.len() && bool::from(want.as_bytes().ct_eq(got.as_bytes()))
        }
        _ => false,
    };
    if authentic {
        if let Ok(v) = HeaderValue::from_str(&mint(&ctx, &grant)) {
            headers.insert(ASSERTION_HEADER, v);
        }
    }
    let local = ctx.cfg.local;
    if req.version() == Version::HTTP_2 {
        let sender = h2
            .get_or_try_init(|| async {
                let tcp = tokio::net::TcpStream::connect(local)
                    .await
                    .map_err(|e| e.to_string())?;
                let _ = tcp.set_nodelay(true);
                let (sender, conn) =
                    hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(tcp))
                        .await
                        .map_err(|e| e.to_string())?;
                tokio::spawn(async move {
                    let _ = conn.await;
                });
                Ok::<_, String>(sender)
            })
            .await;
        let mut sender = match sender {
            Ok(s) => s.clone(),
            Err(e) => return plain(StatusCode::BAD_GATEWAY, &format!("local spacesd: {e}")),
        };
        return match sender.send_request(req).await {
            Ok(resp) => resp.map(|b| holding(b, held)),
            Err(e) => plain(StatusCode::BAD_GATEWAY, &format!("local spacesd: {e}")),
        };
    }
    // HTTP/1.1 (WebSockets, files, the viewer): one local connection per
    // request, upgrades spliced through.
    let tcp = match tokio::net::TcpStream::connect(local).await {
        Ok(t) => t,
        Err(e) => return plain(StatusCode::BAD_GATEWAY, &format!("local spacesd: {e}")),
    };
    let _ = tcp.set_nodelay(true);
    let (mut sender, conn) = match hyper::client::conn::http1::handshake(TokioIo::new(tcp)).await {
        Ok(p) => p,
        Err(e) => return plain(StatusCode::BAD_GATEWAY, &format!("local spacesd: {e}")),
    };
    tokio::spawn(async move {
        let _ = conn.with_upgrades().await;
    });
    let is_upgrade = req.headers().contains_key(http::header::UPGRADE);
    let client_upgrade = is_upgrade.then(|| hyper::upgrade::on(&mut req));
    let mut resp = match sender.send_request(req).await {
        Ok(r) => r,
        Err(e) => return plain(StatusCode::BAD_GATEWAY, &format!("local spacesd: {e}")),
    };
    if resp.status() == StatusCode::SWITCHING_PROTOCOLS {
        if let Some(client_upgrade) = client_upgrade {
            let local_upgrade = hyper::upgrade::on(&mut resp);
            tokio::spawn(async move {
                // The upgraded socket is the stream now.
                let _held = held;
                if let Ok((a, b)) = tokio::try_join!(client_upgrade, local_upgrade) {
                    let mut a = TokioIo::new(a);
                    let mut b = TokioIo::new(b);
                    let _ = tokio::io::copy_bidirectional(&mut a, &mut b).await;
                }
            });
            return resp.map(|b| b.boxed());
        }
    }
    resp.map(|b| holding(b, held))
}

/// `body`, keeping `held` open until it ends or is dropped.
fn holding(body: Incoming, held: StreamUse) -> BoxBody {
    body.map_frame(move |frame| {
        let _ = &held;
        frame
    })
    .boxed()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshots_count_open_streams_upgrades_and_media_per_peer() {
        let streams = Arc::new(PeerStreams::default());
        let (a, b) = (
            cua_mesh::SecretKey::generate().public(),
            cua_mesh::SecretKey::generate().public(),
        );
        assert!(streams.snapshot(&[]).is_empty());
        let s1 = streams.open(a, "ada");
        let s2 = streams.open(a, "ada");
        streams.upgraded(&a);
        let snap = streams.snapshot(&[(b, "bob".into())]);
        let ada = snap.iter().find(|p| p.peer == a).unwrap();
        assert_eq!((ada.user.as_str(), ada.open, ada.live), ("ada", 2, true));
        let bob = snap.iter().find(|p| p.peer == b).unwrap();
        assert_eq!((bob.user.as_str(), bob.open, bob.live), ("bob", 1, true));
        // The upgrade is reported once; closed streams leave.
        drop(s1);
        let snap = streams.snapshot(&[]);
        assert_eq!(snap.len(), 1);
        assert_eq!((snap[0].open, snap[0].live), (1, false));
        drop(s2);
        assert!(streams.snapshot(&[]).is_empty());
        assert!(streams.peers.lock().unwrap().is_empty());
    }
}
