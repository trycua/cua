// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The relay: machines register over WebSocket with a relay token; clients
//! reach them at `/m/<machine-id>/…` (path mode, prefix stripped) or
//! `<machine-id>.<domain>` (host mode). Every client request becomes one
//! yamux stream carrying HTTP/1.1 (including WebSocket upgrades) or h2
//! (gRPC) to the machine's spacesd.
//!
//! Two registration modes share one relay:
//! - **Static relay tokens** (self-hosting): machines register with a
//!   shared secret and clients present the env token end to end; the relay
//!   forwards it unmodified and never stores it, but it does parse it in
//!   transit like every request it proxies (TLS terminates in front of the
//!   relay). See the crate README's "What the relay can see".
//! - **Accounts** (cua.ai): machines are registered by a signed-in user
//!   (`POST /v1/machines`) and join with the machine token they got back.
//!   Clients present their account token; the relay checks ownership /
//!   allowlist / sharing, strips the client's credentials and forwards a
//!   short-lived relay-signed principal assertion instead (see
//!   [`crate::assertion`]). The machine directory API lives in
//!   [`crate::api`].

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use axum::body::Body;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode, Uri, Version};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use futures_util::io::{AsyncReadExt as _, AsyncWriteExt as _};
use hyper_util::rt::{TokioExecutor, TokioIo};
use serde::Serialize;
use tokio::sync::mpsc;
use tokio_util::compat::FuturesAsyncReadCompatExt as _;
use tokio_util::sync::CancellationToken;

use crate::assertion::{self, AssertionClaims, RelayKey, ASSERTION_HEADER};
use crate::devices::{DevicePolicy, DeviceStore, DEVICE_SESSION_HEADER};
use crate::directory::{Directory, MACHINE_TOKEN_PREFIX};
use crate::mux::{self, Counted, Counters, MuxHandle};
use crate::oidc::{Identity, OidcValidator};
use crate::ws::WsIo;
use crate::{valid_machine_id, CONNECT_PATH, HEARTBEAT_BYTE, MACHINE_ID_HEADER, VERSION_HEADER};

/// Header a client may use for its account token instead of
/// `authorization` (so an env token can travel in `authorization`).
pub const RELAY_AUTHORIZATION_HEADER: &str = "x-cua-relay-authorization";
/// How long a client stays in a machine's presence list after its last
/// stream closed.
const PRESENCE_LINGER: Duration = Duration::from_secs(30);

/// Relay configuration.
#[derive(Debug, Clone)]
pub struct RelayConfig {
    /// Accepted machine-registration tokens.
    pub tokens: Vec<String>,
    /// Maximum machines connected per registration token.
    pub max_machines_per_token: usize,
    /// Maximum concurrent client streams per machine.
    pub max_streams_per_machine: u32,
    /// A machine without a heartbeat for this long is dropped.
    pub machine_idle_timeout: Duration,
    /// Host-mode domain: `<id>.<domain>` routes to machine `<id>`.
    pub host_domain: Option<String>,
    /// Token for `/relay/v1/machines`.
    pub admin_token: Option<String>,
    /// Refuse client requests that carry no spacesd credential
    /// (bearer header, ticket or signed URL). The relay cannot validate it;
    /// this only stops anonymous scanning from reaching machines.
    pub require_client_credentials: bool,
    /// Account mode: validates cua.ai account tokens. `None` disables the
    /// machine directory API and account machines.
    pub oidc: Option<Arc<OidcValidator>>,
    /// Where the machine directory is persisted (in memory when unset).
    pub state_file: Option<PathBuf>,
    /// Assertion signing key (a fresh one per process when unset).
    pub signing_key: Option<Arc<RelayKey>>,
    /// Public base URL (`https://relay.cua.ai`): assertion issuer and the
    /// machine URLs in the directory. Derived from `Host` when unset.
    pub public_url: Option<String>,
    /// Maximum machines one account may register.
    pub max_machines_per_account: usize,
    /// Account mode: client devices must be enrolled (with a second factor)
    /// to list or reach machines. See [`crate::devices`].
    pub device_enrollment: bool,
    /// Enrollment lifetime, migration grace period, bootstrap window and
    /// session lifetime.
    pub device_policy: DevicePolicy,
    /// Where devices and the audit log are persisted (default: next to the
    /// state file, `<state>.devices.json`; in memory without one).
    pub devices_file: Option<PathBuf>,
    /// Largest number of concurrent client streams across *every* machine
    /// at once (S7): a memory budget independent of how many machines are
    /// connected, enforced with backpressure (a new stream is refused, not
    /// buffered, once the budget is spent). Bytes are bounded per machine
    /// by `machine_window_bytes` × connected machines; this bounds them
    /// across all machines at once regardless of how many are connected, so
    /// one user's worth of machines and slow readers cannot, by itself,
    /// exhaust the pod and drop every other machine's connection with it.
    pub max_global_streams: u64,
    /// yamux's own per-machine-connection stream cap (a hard ceiling under
    /// `max_streams_per_machine`, which is the friendlier, app-level check
    /// in `proxy()`). Must satisfy `machine_window_bytes >= machine_max_streams * 256 KiB`.
    pub machine_max_streams: usize,
    /// yamux's per-machine connection receive window: the most one
    /// machine's connection buffers across all of its streams (S7).
    pub machine_window_bytes: usize,
}

impl Default for RelayConfig {
    fn default() -> Self {
        Self {
            tokens: Vec::new(),
            max_machines_per_token: 16,
            // Down from an earlier 1000 (S7): yamux's own per-connection
            // cap (machine_max_streams) is now the smaller crate::mux::
            // MAX_STREAMS, so this app-level check (which answers with a
            // friendlier error) should not be looser than it.
            max_streams_per_machine: crate::mux::MAX_STREAMS as u32,
            machine_idle_timeout: Duration::from_secs(90),
            host_domain: None,
            admin_token: None,
            require_client_credentials: true,
            oidc: None,
            state_file: None,
            signing_key: None,
            public_url: None,
            max_machines_per_account: 32,
            device_enrollment: true,
            device_policy: DevicePolicy::default(),
            devices_file: None,
            // 4096 streams: at a default 256 KiB yamux starting window this
            // is at most ~1 GiB actively buffered relay-wide even if every
            // stream's window grew to the per-machine ceiling at once,
            // comfortably inside a 1 GiB pod alongside the rest of the
            // process. Tune to the deployed pod's actual memory limit.
            max_global_streams: 4096,
            machine_max_streams: crate::mux::MAX_STREAMS,
            machine_window_bytes: crate::mux::DEFAULT_WINDOW_BYTES,
        }
    }
}

/// Who registered a connected machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Registrant {
    /// A static relay token (its index).
    Static(usize),
    /// A machine token from the account directory; the owning account.
    Account(String),
}

/// One client (account user) using a machine.
#[derive(Debug, Clone)]
pub(crate) struct ClientPresence {
    pub email: Option<String>,
    pub name: Option<String>,
    pub streams: u32,
    pub since: u64,
    pub last_seen: Instant,
}

pub(crate) struct Machine {
    pub(crate) id: String,
    pub(crate) registrant: Registrant,
    pub(crate) version: String,
    pub(crate) connected_at: SystemTime,
    generation: u64,
    handle: MuxHandle,
    counters: Arc<Counters>,
    active: AtomicU32,
    last_heartbeat: Mutex<Instant>,
    pub(crate) abort: tokio::task::AbortHandle,
    /// Cancelled to cut every client stream ("stop sharing").
    clients_cut: Mutex<CancellationToken>,
    /// Account users with open (or recently closed) streams.
    pub(crate) clients: Mutex<HashMap<String, ClientPresence>>,
}

impl Machine {
    /// Cuts every client stream and forgets presence.
    pub(crate) fn cut_clients(&self) {
        let old = std::mem::take(&mut *self.clients_cut.lock().expect("cut"));
        old.cancel();
        self.clients.lock().expect("clients").clear();
    }

    fn cut_token(&self) -> CancellationToken {
        self.clients_cut.lock().expect("cut").clone()
    }

    fn client_enter(&self, who: &Identity) {
        let mut clients = self.clients.lock().expect("clients");
        let entry = clients
            .entry(who.user.clone())
            .or_insert_with(|| ClientPresence {
                email: who.email.clone(),
                name: who.name.clone(),
                streams: 0,
                since: assertion::now_secs(),
                last_seen: Instant::now(),
            });
        entry.streams += 1;
        entry.last_seen = Instant::now();
    }

    fn client_leave(&self, user: &str) {
        if let Some(entry) = self.clients.lock().expect("clients").get_mut(user) {
            entry.streams = entry.streams.saturating_sub(1);
            entry.last_seen = Instant::now();
        }
    }

    /// Clients with open streams or a stream closed within the linger.
    pub(crate) fn presence(&self) -> Vec<(String, ClientPresence)> {
        let mut clients = self.clients.lock().expect("clients");
        clients.retain(|_, c| c.streams > 0 || c.last_seen.elapsed() < PRESENCE_LINGER);
        let mut rows: Vec<_> = clients
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        rows
    }
}

/// Relay state.
#[derive(Clone)]
pub struct Relay {
    pub(crate) config: Arc<RelayConfig>,
    pub(crate) machines: Arc<Mutex<HashMap<String, Arc<Machine>>>>,
    generation: Arc<AtomicU64>,
    pub(crate) directory: Arc<Directory>,
    pub(crate) devices: Arc<DeviceStore>,
    pub(crate) key: Arc<RelayKey>,
    /// Open client streams across every machine (S7): the global memory
    /// budget's live counter.
    pub(crate) global_streams: Arc<AtomicU64>,
}

/// One row of `/relay/v1/machines`.
#[derive(Debug, Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct MachineStatus {
    /// Machine id.
    pub id: String,
    /// Reported spacesd version.
    pub version: String,
    /// Seconds since the Unix epoch.
    pub connected_at: u64,
    /// Open client streams.
    pub active_streams: u32,
    /// Bytes from clients to the machine.
    pub bytes_to_machine: u64,
    /// Bytes from the machine to clients.
    pub bytes_from_machine: u64,
}

impl Relay {
    /// Creates a relay. Panics if the configured state file is unreadable;
    /// use [`Relay::try_new`] to handle that.
    pub fn new(config: RelayConfig) -> Self {
        Self::try_new(config).expect("relay state file")
    }

    /// Creates a relay, loading the machine directory from the state file.
    pub fn try_new(config: RelayConfig) -> std::io::Result<Self> {
        let directory = match &config.state_file {
            Some(path) => Directory::open(path.clone(), config.max_machines_per_account)?,
            None => Directory::in_memory(config.max_machines_per_account),
        };
        let devices_file = config.devices_file.clone().or_else(|| {
            config
                .state_file
                .as_ref()
                .map(|p| p.with_extension("devices.json"))
        });
        let devices = match devices_file {
            Some(path) => DeviceStore::open(path, config.device_policy)?,
            None => DeviceStore::in_memory(config.device_policy),
        };
        let key = config
            .signing_key
            .clone()
            .unwrap_or_else(|| Arc::new(RelayKey::generate()));
        Ok(Self {
            config: Arc::new(config),
            machines: Arc::default(),
            generation: Arc::default(),
            directory: Arc::new(directory),
            devices: Arc::new(devices),
            key,
            global_streams: Arc::default(),
        })
    }

    /// The HTTP router.
    pub fn router(&self) -> Router {
        Router::new()
            .route("/healthz", get(|| async { StatusCode::NO_CONTENT }))
            .route(CONNECT_PATH, get(machine_connect))
            .route("/relay/v1/machines", get(machine_list))
            .merge(crate::api::routes())
            .merge(crate::device_api::routes())
            .fallback(proxy)
            .with_state(self.clone())
    }

    /// The relay's assertion signing key.
    pub fn signing_key(&self) -> &RelayKey {
        &self.key
    }

    /// The public base URL for a request (config, else `Host`).
    pub(crate) fn public_url(&self, headers: &HeaderMap) -> String {
        if let Some(url) = &self.config.public_url {
            return url.trim_end_matches('/').to_owned();
        }
        let host = headers
            .get(header::HOST)
            .and_then(|h| h.to_str().ok())
            .unwrap_or("localhost");
        let proto = headers
            .get("x-forwarded-proto")
            .and_then(|h| h.to_str().ok())
            .filter(|p| *p == "https" || *p == "http")
            .unwrap_or("http");
        format!("{proto}://{host}")
    }

    /// The client URL for machine `id`: its own subdomain when a host-mode
    /// domain is configured (S6: a separate origin per machine), else the
    /// shared origin's `/m/<id>` path.
    pub(crate) fn machine_url(&self, headers: &HeaderMap, id: &str) -> String {
        let Some(domain) = &self.config.host_domain else {
            return format!("{}/m/{id}", self.public_url(headers));
        };
        let scheme = self
            .config
            .public_url
            .as_deref()
            .and_then(|u| u.split_once("://"))
            .map(|(s, _)| s.to_owned())
            .unwrap_or_else(|| {
                let https = headers
                    .get("x-forwarded-proto")
                    .and_then(|h| h.to_str().ok())
                    == Some("https");
                if https { "https" } else { "http" }.to_owned()
            });
        format!("{scheme}://{id}.{domain}")
    }

    /// The connected machine `id`, if any.
    pub(crate) fn machine(&self, id: &str) -> Option<Arc<Machine>> {
        self.machines.lock().expect("machines").get(id).cloned()
    }

    /// Drops machine `id` and cuts its clients.
    pub(crate) fn drop_machine(&self, id: &str) {
        if let Some(m) = self.machines.lock().expect("machines").remove(id) {
            m.cut_clients();
            m.abort.abort();
        }
    }

    /// Connected machine ids.
    pub fn machine_ids(&self) -> Vec<String> {
        let mut ids: Vec<_> = self
            .machines
            .lock()
            .expect("machines")
            .keys()
            .cloned()
            .collect();
        ids.sort();
        ids
    }

    /// Status rows.
    pub fn status(&self) -> Vec<MachineStatus> {
        let mut rows: Vec<_> = self
            .machines
            .lock()
            .expect("machines")
            .values()
            .map(|m| MachineStatus {
                id: m.id.clone(),
                version: m.version.clone(),
                connected_at: m
                    .connected_at
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
                active_streams: m.active.load(Ordering::SeqCst),
                bytes_to_machine: m.counters.to_machine.load(Ordering::Relaxed),
                bytes_from_machine: m.counters.from_machine.load(Ordering::Relaxed),
            })
            .collect();
        rows.sort_by(|a, b| a.id.cmp(&b.id));
        rows
    }

    /// Drops every machine session (as if the relay process died).
    pub fn disconnect_all(&self) {
        for machine in self
            .machines
            .lock()
            .expect("machines")
            .drain()
            .map(|(_, m)| m)
        {
            machine.abort.abort();
        }
    }

    /// Serves on `listener` until the future `shutdown` resolves.
    pub async fn serve(
        self,
        listener: tokio::net::TcpListener,
        shutdown: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> std::io::Result<()> {
        axum::serve(
            listener,
            self.router()
                .into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown)
        .await
    }

    fn token_index(&self, headers: &HeaderMap) -> Option<usize> {
        let presented = bearer(headers, header::AUTHORIZATION.as_str())?;
        self.config.tokens.iter().position(|t| {
            use subtle::ConstantTimeEq as _;
            t.len() == presented.len() && bool::from(t.as_bytes().ct_eq(presented.as_bytes()))
        })
    }
}

/// `<token>` of `name: Bearer <token>`.
pub(crate) fn bearer<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            let (scheme, rest) = v.split_once(' ')?;
            scheme.eq_ignore_ascii_case("bearer").then_some(rest.trim())
        })
        .filter(|t| !t.is_empty())
}

/// The account token a client presented, if any.
pub(crate) fn account_token(headers: &HeaderMap) -> Option<&str> {
    bearer(headers, RELAY_AUTHORIZATION_HEADER)
        .or_else(|| bearer(headers, header::AUTHORIZATION.as_str()))
}

fn text(status: StatusCode, message: impl Into<String>) -> Response {
    (status, message.into()).into_response()
}

/// gRPC status codes the relay produces itself.
#[derive(Clone, Copy)]
enum GrpcCode {
    Unauthenticated = 16,
    PermissionDenied = 7,
    Unavailable = 14,
    ResourceExhausted = 8,
    NotFound = 5,
    InvalidArgument = 3,
}

/// Refuses a client request. gRPC and gRPC-Web callers get a trailers-only
/// gRPC status (HTTP 200 + `grpc-status`), so SDKs see the right code
/// instead of an opaque HTTP error; everyone else gets `status`.
fn refuse(
    headers: &HeaderMap,
    status: StatusCode,
    code: GrpcCode,
    message: impl Into<String>,
) -> Response {
    let message = message.into();
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .filter(|ct| ct.starts_with("application/grpc"));
    let Some(content_type) = content_type else {
        return text(status, message);
    };
    let encoded = percent_encode_grpc(&message);
    let details = {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD_NO_PAD.encode(status_details(code, &message))
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header("grpc-status", (code as i32).to_string())
        .header("grpc-message", encoded)
        .header("grpc-status-details-bin", details)
        .body(Body::empty())
        .unwrap_or_else(|_| text(status, message))
}

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn put_bytes(out: &mut Vec<u8>, field: u64, bytes: &[u8]) {
    put_varint(out, (field << 3) | 2);
    put_varint(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

/// `google.rpc.Status{code, message, details: [Any(cua.env.v1.ErrorInfo)]}`,
/// encoded by hand so the relay needs no protobuf toolchain. `ErrorInfo`
/// reasons follow `cua.env.v1.ErrorReason`.
fn status_details(code: GrpcCode, message: &str) -> Vec<u8> {
    let reason: u64 = match code {
        GrpcCode::Unauthenticated => 1,  // ERROR_REASON_UNAUTHENTICATED
        GrpcCode::PermissionDenied => 3, // ERROR_REASON_PERMISSION_DENIED
        GrpcCode::Unavailable | GrpcCode::NotFound => 8, // ERROR_REASON_TARGET_UNAVAILABLE
        GrpcCode::ResourceExhausted => 20, // ERROR_REASON_RATE_LIMITED
        GrpcCode::InvalidArgument => 0,
    };
    let mut info = Vec::new();
    if reason != 0 {
        put_varint(&mut info, 1 << 3);
        put_varint(&mut info, reason);
    }
    put_bytes(&mut info, 2, message.as_bytes());
    let mut any = Vec::new();
    put_bytes(&mut any, 1, b"type.googleapis.com/cua.env.v1.ErrorInfo");
    put_bytes(&mut any, 2, &info);
    let mut status = Vec::new();
    put_varint(&mut status, 1 << 3);
    put_varint(&mut status, code as u64);
    put_bytes(&mut status, 2, message.as_bytes());
    put_bytes(&mut status, 3, &any);
    status
}

/// Percent-encodes a `grpc-message` value (gRPC over HTTP/2 spec).
fn percent_encode_grpc(message: &str) -> String {
    message
        .bytes()
        .map(|b| {
            if (0x20..=0x7e).contains(&b) && b != b'%' {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

async fn machine_connect(
    State(relay): State<Relay>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    let Some(id) = headers
        .get(MACHINE_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .filter(|id| valid_machine_id(id))
    else {
        return text(StatusCode::BAD_REQUEST, "missing or invalid machine id");
    };
    let registrant = if let Some(index) = relay.token_index(&headers) {
        if relay.directory.get(&id).is_some() {
            return text(
                StatusCode::CONFLICT,
                "machine id is registered to an account",
            );
        }
        Registrant::Static(index)
    } else {
        let record = bearer(&headers, header::AUTHORIZATION.as_str())
            .filter(|t| t.starts_with(MACHINE_TOKEN_PREFIX))
            .and_then(|t| relay.directory.machine_for_token(t));
        match record {
            Some(record) if record.id == id => Registrant::Account(record.owner.id),
            Some(_) => {
                return text(
                    StatusCode::FORBIDDEN,
                    "machine token belongs to another machine id",
                )
            }
            None => return text(StatusCode::UNAUTHORIZED, "invalid relay token"),
        }
    };
    let version = headers
        .get(VERSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .chars()
        .take(64)
        .collect::<String>();
    {
        let machines = relay.machines.lock().expect("machines");
        if let Registrant::Static(_) = registrant {
            let same_token = machines
                .values()
                .filter(|m| m.registrant == registrant && m.id != id)
                .count();
            if same_token >= relay.config.max_machines_per_token {
                return text(
                    StatusCode::TOO_MANY_REQUESTS,
                    "machine limit for this token reached",
                );
            }
        }
        if let Some(existing) = machines.get(&id) {
            if existing.registrant != registrant {
                return text(
                    StatusCode::CONFLICT,
                    "machine id registered with another token",
                );
            }
        }
    }
    let owner = match &registrant {
        Registrant::Account(owner) => Some(owner.clone()),
        Registrant::Static(_) => None,
    };
    let jwks = owner.as_ref().map(|_| {
        use base64::Engine as _;
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(relay.key.jwks().to_string())
    });
    let mut response = upgrade
        .max_message_size(64 * 1024 * 1024)
        .on_upgrade(move |socket| async move {
            let (inbound_tx, mut inbound_rx) = mpsc::channel(64);
            let (handle, driver) = mux::spawn(
                WsIo::new(socket),
                yamux::Mode::Client,
                inbound_tx,
                relay.config.machine_max_streams,
                relay.config.machine_window_bytes,
            );
            let generation = relay.generation.fetch_add(1, Ordering::SeqCst);
            let machine = Arc::new(Machine {
                id: id.clone(),
                registrant,
                version,
                connected_at: SystemTime::now(),
                generation,
                handle,
                counters: Arc::default(),
                active: AtomicU32::new(0),
                last_heartbeat: Mutex::new(Instant::now()),
                abort: driver.abort_handle(),
                clients_cut: Mutex::default(),
                clients: Mutex::default(),
            });
            if let Some(old) = relay
                .machines
                .lock()
                .expect("machines")
                .insert(id.clone(), machine.clone())
            {
                tracing::info!(machine = %id, "replacing previous session");
                old.cut_clients();
                old.abort.abort();
            }
            tracing::info!(machine = %id, "machine connected");
            // Heartbeat streams opened by the machine: echo one byte.
            let echo_machine = machine.clone();
            let echo = tokio::spawn(async move {
                while let Some(mut stream) = inbound_rx.recv().await {
                    let machine = echo_machine.clone();
                    tokio::spawn(async move {
                        let mut buf = [0u8; 1];
                        if stream.read_exact(&mut buf).await.is_ok() && buf[0] == HEARTBEAT_BYTE {
                            *machine.last_heartbeat.lock().expect("heartbeat") = Instant::now();
                            let _ = stream.write_all(&buf).await;
                            let _ = stream.flush().await;
                        }
                        let _ = stream.close().await;
                    });
                }
            });
            let idle = relay.config.machine_idle_timeout;
            let watchdog_machine = machine.clone();
            let watchdog = tokio::spawn(async move {
                loop {
                    tokio::time::sleep(idle / 4).await;
                    if watchdog_machine.last_heartbeat.lock().expect("heartbeat").elapsed() > idle {
                        tracing::info!(machine = %watchdog_machine.id, "no heartbeat; dropping machine");
                        watchdog_machine.abort.abort();
                        return;
                    }
                }
            });
            let _ = driver.await;
            echo.abort();
            watchdog.abort();
            machine.cut_clients();
            let mut machines = relay.machines.lock().expect("machines");
            if machines.get(&id).is_some_and(|m| m.generation == generation) {
                machines.remove(&id);
            }
            tracing::info!(machine = %id, "machine disconnected");
        });
    if let (Some(owner), Some(jwks)) = (owner, jwks) {
        if let (Ok(owner), Ok(jwks)) = (HeaderValue::from_str(&owner), HeaderValue::from_str(&jwks))
        {
            response
                .headers_mut()
                .insert(assertion::OWNER_HEADER, owner);
            response.headers_mut().insert(assertion::JWKS_HEADER, jwks);
        }
    }
    response
}

async fn machine_list(State(relay): State<Relay>, headers: HeaderMap) -> Response {
    let Some(admin) = relay.config.admin_token.as_deref() else {
        return text(StatusCode::NOT_FOUND, "admin endpoint disabled");
    };
    let ok = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|t| {
            use subtle::ConstantTimeEq as _;
            t.len() == admin.len() && bool::from(t.as_bytes().ct_eq(admin.as_bytes()))
        });
    if !ok {
        return text(StatusCode::UNAUTHORIZED, "invalid admin token");
    }
    axum::Json(relay.status()).into_response()
}

/// Resolves the machine and the path to forward for a client request. When
/// a host-mode domain is configured, path mode (`/m/<id>/…`) is refused:
/// every machine gets its own subdomain instead of sharing the relay's own
/// origin, so one machine's response can no longer be used against another
/// machine's viewer or against the relay's own pages (S6). Self-host a
/// relay without `--host-domain` to keep path mode (single registrable
/// origin, no wildcard certificate needed).
fn route(relay: &Relay, uri: &Uri, headers: &HeaderMap) -> Option<(String, String, String)> {
    let path = uri.path();
    let query = uri.query().map(|q| format!("?{q}")).unwrap_or_default();
    let domain = relay.config.host_domain.as_deref();
    if let Some(rest) = path.strip_prefix("/m/") {
        if domain.is_some() {
            return None;
        }
        let (id, tail) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        return Some((id.to_owned(), format!("{tail}{query}"), format!("/m/{id}")));
    }
    let domain = domain?;
    let host = headers.get(header::HOST)?.to_str().ok()?;
    let host = host.split(':').next()?;
    let id = host.strip_suffix(domain)?.strip_suffix('.')?;
    Some((id.to_owned(), format!("{path}{query}"), String::new()))
}

fn has_client_credential(uri: &Uri, headers: &HeaderMap, forwarded_path: &str) -> bool {
    if headers.contains_key(header::AUTHORIZATION)
        || headers.contains_key("x-cua-env-authorization")
    {
        return true;
    }
    has_forwardable_credential(uri, headers, forwarded_path)
}

/// A credential that survives the relay stripping client credential
/// headers from account-machine requests: a machine-minted ticket or signed
/// URL (or the unauthenticated health probe). The machine validates it.
fn has_forwardable_credential(uri: &Uri, headers: &HeaderMap, forwarded_path: &str) -> bool {
    if forwarded_path == "/health" || forwarded_path.starts_with("/health?") {
        return true;
    }
    let query = uri.query().unwrap_or("");
    if query
        .split('&')
        .any(|p| p.starts_with("ticket=") || p.starts_with("sig="))
    {
        return true;
    }
    headers
        .get_all(header::SEC_WEBSOCKET_PROTOCOL)
        .iter()
        .filter_map(|v| v.to_str().ok())
        // `rcdp.v2.ticket.` is the legacy media ticket form.
        .any(|v| v.contains("cua.ticket.") || v.contains("rcdp.v2.ticket."))
}

const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-connection",
    "proxy-authenticate",
    "proxy-authorization",
    "transfer-encoding",
    "upgrade",
];

struct ActiveGuard(Arc<Machine>, Option<String>, Arc<AtomicU64>);

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
        self.2.fetch_sub(1, Ordering::SeqCst);
        if let Some(user) = &self.1 {
            self.0.client_leave(user);
        }
    }
}

/// Headers carrying client credentials the relay removes before forwarding
/// to an account machine (the assertion replaces them).
const CLIENT_CREDENTIAL_HEADERS: &[&str] = &[
    "authorization",
    RELAY_AUTHORIZATION_HEADER,
    "x-cua-env-authorization",
    DEVICE_SESSION_HEADER,
];

/// An account client let through: who, and how its device stands.
pub(crate) struct AccountClient {
    pub(crate) who: Identity,
    pub(crate) gate: crate::device_api::Gate,
}

#[allow(clippy::result_large_err)]
/// Authorizes a client request for an account machine and rewrites its
/// credentials. Returns the account client to record in presence (none for
/// machine-minted tickets and signed URLs).
async fn authorize_account_client(
    relay: &Relay,
    id: &str,
    request: &mut Request,
    forwarded: &str,
    device_session: Option<&str>,
) -> Result<Option<AccountClient>, Response> {
    let headers = request.headers();
    let Some(record) = relay.directory.get(id) else {
        return Err(refuse(
            headers,
            StatusCode::BAD_GATEWAY,
            GrpcCode::Unavailable,
            format!("machine {id:?} is not registered"),
        ));
    };
    let is_health = forwarded == "/health" || forwarded.starts_with("/health?");
    let identity = match (account_token(headers), &relay.config.oidc) {
        (Some(token), Some(oidc)) => match oidc.validate(token).await {
            Ok(identity) => Some(identity),
            Err(e) if !has_forwardable_credential(request.uri(), headers, forwarded) => {
                return Err(refuse(
                    headers,
                    StatusCode::UNAUTHORIZED,
                    GrpcCode::Unauthenticated,
                    format!("invalid account token: {e}"),
                ))
            }
            Err(_) => None,
        },
        _ => None,
    };
    if !record.sharing && !is_health {
        return Err(refuse(
            headers,
            StatusCode::FORBIDDEN,
            GrpcCode::PermissionDenied,
            "the host stopped sharing this machine",
        ));
    }
    let mut gate = None;
    let assertion = match &identity {
        Some(who) => {
            let Some(role) = record.role_of(who) else {
                return Err(refuse(
                    headers,
                    StatusCode::FORBIDDEN,
                    GrpcCode::PermissionDenied,
                    "this machine is not shared with your account",
                ));
            };
            // The account token alone is not enough: the client device must
            // be enrolled (after the migration grace period).
            match crate::device_api::gate(relay, device_session, who) {
                Ok(g) => {
                    crate::device_api::record_access(relay, &g, who, &record, role);
                    gate = Some(g);
                }
                Err(message) => {
                    return Err(refuse(
                        headers,
                        StatusCode::FORBIDDEN,
                        GrpcCode::PermissionDenied,
                        message,
                    ))
                }
            }
            let now = assertion::now_secs();
            let claims = AssertionClaims {
                iss: relay.public_url(headers),
                aud: id.to_owned(),
                sub: who.user.clone(),
                acct: who.account.clone(),
                email: who.email.clone(),
                name: who.name.clone(),
                mid: id.to_owned(),
                role: role.as_str().to_owned(),
                scope: "env".into(),
                iat: now,
                exp: now + assertion::MAX_ASSERTION_TTL_SECS,
                jti: uuid::Uuid::new_v4().simple().to_string(),
            };
            Some(relay.key.sign(&claims))
        }
        // Tickets and signed URLs were minted by the machine for an
        // authorized caller; the machine validates them itself.
        // (Credential headers are stripped below, so they do not count.)
        None if has_forwardable_credential(request.uri(), headers, forwarded) => None,
        None => {
            return Err(refuse(
                headers,
                StatusCode::UNAUTHORIZED,
                GrpcCode::Unauthenticated,
                "sign in: this machine accepts cua.ai account tokens",
            ))
        }
    };
    let headers = request.headers_mut();
    for name in CLIENT_CREDENTIAL_HEADERS {
        headers.remove(*name);
    }
    if let Some(assertion) = assertion {
        if let Ok(value) = HeaderValue::from_str(&assertion) {
            headers.insert(ASSERTION_HEADER, value);
        }
    }
    Ok(identity
        .zip(gate)
        .map(|(who, gate)| AccountClient { who, gate }))
}

async fn proxy(State(relay): State<Relay>, mut request: Request) -> Response {
    let Some((id, forwarded, prefix)) = route(&relay, request.uri(), request.headers()) else {
        let message = match &relay.config.host_domain {
            Some(domain) => format!("use <machine-id>.{domain}/…"),
            None => "use /m/<machine-id>/…".to_owned(),
        };
        return refuse(
            request.headers(),
            StatusCode::NOT_FOUND,
            GrpcCode::NotFound,
            message,
        );
    };
    // Only the relay mints assertions; device sessions are for the relay
    // alone and never reach a machine.
    request.headers_mut().remove(ASSERTION_HEADER);
    let device_session = request.headers_mut().remove(DEVICE_SESSION_HEADER);
    let machine = relay.machines.lock().expect("machines").get(&id).cloned();
    let account_machine = matches!(
        machine.as_ref().map(|m| &m.registrant),
        Some(Registrant::Account(_))
    ) || (machine.is_none() && relay.directory.get(&id).is_some());
    let mut identity = None;
    let mut grace = false;
    if account_machine {
        let session = device_session.as_ref().and_then(|v| v.to_str().ok());
        match authorize_account_client(&relay, &id, &mut request, &forwarded, session).await {
            Ok(client) => {
                if let Some(client) = client {
                    grace = matches!(client.gate, crate::device_api::Gate::Grace);
                    identity = Some(client.who);
                }
            }
            Err(response) => return response,
        }
    } else if relay.config.require_client_credentials
        && !has_client_credential(request.uri(), request.headers(), &forwarded)
    {
        return refuse(
            request.headers(),
            StatusCode::UNAUTHORIZED,
            GrpcCode::Unauthenticated,
            "the relay forwards only requests carrying a spacesd credential",
        );
    }
    let Some(machine) = machine else {
        return refuse(
            request.headers(),
            StatusCode::BAD_GATEWAY,
            GrpcCode::Unavailable,
            format!("machine {id:?} is not connected"),
        );
    };
    if machine.active.fetch_add(1, Ordering::SeqCst) >= relay.config.max_streams_per_machine {
        machine.active.fetch_sub(1, Ordering::SeqCst);
        return refuse(
            request.headers(),
            StatusCode::SERVICE_UNAVAILABLE,
            GrpcCode::ResourceExhausted,
            "too many concurrent streams for this machine",
        );
    }
    // The global memory budget (S7): checked in addition to the per-machine
    // cap above, so one account's worth of machines (or one machine with
    // many slow-reading clients) cannot by itself buffer enough to bring
    // down every other machine's connection. Backpressure, not buffering:
    // refused outright, never queued.
    if relay.global_streams.fetch_add(1, Ordering::SeqCst) >= relay.config.max_global_streams {
        relay.global_streams.fetch_sub(1, Ordering::SeqCst);
        machine.active.fetch_sub(1, Ordering::SeqCst);
        return refuse(
            request.headers(),
            StatusCode::SERVICE_UNAVAILABLE,
            GrpcCode::ResourceExhausted,
            "the relay is at its concurrent-stream budget; retry shortly",
        );
    }
    if let Some(who) = &identity {
        machine.client_enter(who);
    }
    let guard = ActiveGuard(
        machine.clone(),
        identity.map(|w| w.user),
        relay.global_streams.clone(),
    );
    let cut = machine.cut_token();
    let stream = match machine.handle.open().await {
        Ok(s) => s,
        Err(e) => {
            return refuse(
                request.headers(),
                StatusCode::BAD_GATEWAY,
                GrpcCode::Unavailable,
                format!("machine tunnel: {e}"),
            )
        }
    };
    let io = TokioIo::new(Counted::new(stream, machine.counters.clone()).compat());

    let is_upgrade =
        request.version() <= Version::HTTP_11 && request.headers().contains_key(header::UPGRADE);
    let client_upgrade = is_upgrade.then(|| hyper::upgrade::on(&mut request));
    if !prefix.is_empty() {
        if let Ok(value) = HeaderValue::from_str(&prefix) {
            request.headers_mut().insert("x-forwarded-prefix", value);
        }
    }
    if request.version() == Version::HTTP_2 {
        let (mut sender, connection) =
            match hyper::client::conn::http2::handshake(TokioExecutor::new(), io).await {
                Ok(pair) => pair,
                Err(e) => return text(StatusCode::BAD_GATEWAY, format!("h2 handshake: {e}")),
            };
        // hyper drives the h2 connection on its own task, so the response
        // body (not the connection future) carries the guard and the cut.
        let guard = Arc::new(guard);
        let body_guard = guard.clone();
        let body_cut = cut.clone();
        tokio::spawn(async move {
            let _guard = guard;
            tokio::select! {
                _ = connection => {}
                _ = cut.cancelled() => {}
            }
        });
        let authority = request
            .headers()
            .get(header::HOST)
            .and_then(|h| h.to_str().ok())
            .map(str::to_owned)
            .or_else(|| request.uri().authority().map(|a| a.to_string()))
            .unwrap_or_else(|| "spacesd".into());
        match format!("http://{authority}{forwarded}").parse() {
            Ok(uri) => *request.uri_mut() = uri,
            Err(_) => {
                return refuse(
                    request.headers(),
                    StatusCode::BAD_REQUEST,
                    GrpcCode::InvalidArgument,
                    "invalid path",
                )
            }
        }
        for name in HOP_BY_HOP {
            request.headers_mut().remove(*name);
        }
        request.headers_mut().remove(header::HOST);
        return match sender.send_request(request).await {
            Ok(mut response) => {
                harden_machine_response(response.headers_mut(), &forwarded);
                if grace {
                    crate::device_api::flag_grace(&relay, response.headers_mut());
                }
                response.map(|b| Body::new(GuardedBody::new(b, body_guard, body_cut)))
            }
            Err(e) => text(StatusCode::BAD_GATEWAY, format!("forward: {e}")),
        };
    }

    let (mut sender, connection) = match hyper::client::conn::http1::handshake(io).await {
        Ok(pair) => pair,
        Err(e) => return text(StatusCode::BAD_GATEWAY, format!("h1 handshake: {e}")),
    };
    let upgrade_cut = cut.clone();
    // The upgraded socket outlives the HTTP connection task; both hold the
    // guard so presence and stream limits cover WebSockets.
    let guard = Arc::new(guard);
    let upgrade_guard = guard.clone();
    let body_guard = guard.clone();
    let body_cut = cut.clone();
    tokio::spawn(async move {
        let _guard = guard;
        tokio::select! {
            _ = connection.with_upgrades() => {}
            _ = cut.cancelled() => {}
        }
    });
    match forwarded.parse() {
        Ok(uri) => *request.uri_mut() = uri,
        Err(_) => return text(StatusCode::BAD_REQUEST, "invalid path"),
    }
    if !is_upgrade {
        for name in HOP_BY_HOP {
            request.headers_mut().remove(*name);
        }
    }
    let mut response = match sender.send_request(request).await {
        Ok(r) => r,
        Err(e) => return text(StatusCode::BAD_GATEWAY, format!("forward: {e}")),
    };
    harden_machine_response(response.headers_mut(), &forwarded);
    if grace {
        crate::device_api::flag_grace(&relay, response.headers_mut());
    }
    if response.status() == StatusCode::SWITCHING_PROTOCOLS {
        if let Some(client_upgrade) = client_upgrade {
            let machine_upgrade = hyper::upgrade::on(&mut response);
            tokio::spawn(async move {
                let _guard = upgrade_guard;
                match tokio::try_join!(client_upgrade, machine_upgrade) {
                    Ok((client, machine)) => {
                        let mut client = TokioIo::new(client);
                        let mut machine = TokioIo::new(machine);
                        tokio::select! {
                            _ = tokio::io::copy_bidirectional(&mut client, &mut machine) => {}
                            _ = upgrade_cut.cancelled() => {}
                        }
                    }
                    Err(e) => tracing::debug!(%e, "upgrade failed"),
                }
            });
        }
    }
    response.map(|b| Body::new(GuardedBody::new(b, body_guard, body_cut)))
}

/// Path prefix of the HTML5 viewer spacesd serves at a fixed location
/// (`cua_spacesd_html5::VIEWER_PATH`; not depended on directly, since
/// `cua-spacesd` is server-only and the viewer is client code). It is the
/// only machine-served page allowed to render as HTML on the relay's own
/// origin (S6): everything else a machine sends back is data a client
/// library parses (gRPC, gRPC-Web, `/files`), never a page a browser should
/// navigate to.
const KNOWN_VIEWER_PATH: &str = "/viewer";

/// Hardens a machine's response before it reaches the client (S6). Every
/// account machine shares the relay's own origin (path mode) and, in host
/// mode, its parent registrable domain (until machines move to a separate
/// origin entirely; see the crate README). A machine an attacker registers
/// (S5 lowers the odds, but does not make it impossible) must not be able
/// to use that shared origin against other users: to set a cookie there, to
/// claim a broader Service Worker scope than its own page and intercept
/// later requests (including another machine's viewer page), or to get a
/// browser to render its response as a page at all outside the one known
/// viewer path.
fn harden_machine_response(headers: &mut HeaderMap, forwarded_path: &str) {
    // spacesd sets no cookies (bearer tokens and tickets only); refuse one
    // that would ride the relay's own domain.
    headers.remove(header::SET_COOKIE);
    // `Service-Worker-Allowed` widens a worker's scope past its own script's
    // directory; without it a worker can only control paths at or below
    // where it was fetched from, but every machine's own script still sits
    // under the *same* origin as every other machine (path mode) or the
    // same parent domain (host mode), so strip it outright rather than try
    // to validate a scope.
    headers.remove("service-worker-allowed");
    headers.remove("service-worker");
    let path = forwarded_path.split('?').next().unwrap_or(forwarded_path);
    let is_known_viewer = path == KNOWN_VIEWER_PATH || path.starts_with("/viewer/");
    if !is_known_viewer {
        let is_html = headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.trim_start().to_ascii_lowercase().starts_with("text/html"));
        if is_html {
            // Never let a browser render an arbitrary machine response as a
            // page outside the one path meant to serve one.
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/octet-stream"),
            );
            headers.insert(
                header::CONTENT_DISPOSITION,
                HeaderValue::from_static("attachment"),
            );
        }
        if !headers.contains_key(header::CONTENT_SECURITY_POLICY) {
            headers.insert(
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_static("default-src 'none'; frame-ancestors 'none'"),
            );
        }
    }
    // Belt and suspenders against MIME-sniffing a crafted response into
    // something a browser executes or renders. The known viewer already
    // sets this itself (cua_spacesd_html5::serve); inserting again is a
    // harmless no-op for it.
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
}

/// A forwarded response body that keeps the stream's [`ActiveGuard`] alive
/// until it ends and fails as soon as the machine's clients are cut.
struct GuardedBody {
    inner: hyper::body::Incoming,
    _guard: Arc<ActiveGuard>,
    cut: std::pin::Pin<Box<tokio_util::sync::WaitForCancellationFutureOwned>>,
}

impl GuardedBody {
    fn new(inner: hyper::body::Incoming, guard: Arc<ActiveGuard>, cut: CancellationToken) -> Self {
        Self {
            inner,
            _guard: guard,
            cut: Box::pin(cut.cancelled_owned()),
        }
    }
}

impl http_body::Body for GuardedBody {
    type Data = bytes::Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        if std::future::Future::poll(self.cut.as_mut(), cx).is_ready() {
            return std::task::Poll::Ready(Some(Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionAborted,
                "the host stopped sharing",
            ))));
        }
        std::pin::Pin::new(&mut self.inner)
            .poll_frame(cx)
            .map_err(std::io::Error::other)
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> http_body::SizeHint {
        self.inner.size_hint()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn relay(domain: Option<&str>) -> Relay {
        Relay::new(RelayConfig {
            host_domain: domain.map(str::to_owned),
            ..RelayConfig::default()
        })
    }

    #[test]
    fn path_routing_without_a_host_domain() {
        let r = relay(None);
        let h = HeaderMap::new();
        assert_eq!(
            route(
                &r,
                &"/m/abc123/cua.env.v1.SystemService/Health?x=1"
                    .parse()
                    .unwrap(),
                &h
            ),
            Some((
                "abc123".into(),
                "/cua.env.v1.SystemService/Health?x=1".into(),
                "/m/abc123".into()
            ))
        );
        assert_eq!(route(&r, &"/m/abc123".parse().unwrap(), &h).unwrap().1, "/");
    }

    #[test]
    fn a_host_domain_routes_by_subdomain_and_refuses_path_mode() {
        // S6: once a machine has its own origin, the shared `/m/<id>` path
        // on the main origin no longer forwards anything, so a machine
        // response can no longer be used against that shared origin.
        let r = relay(Some("relay.example"));
        let h = HeaderMap::new();
        assert_eq!(
            route(
                &r,
                &"/m/abc123/cua.env.v1.SystemService/Health?x=1"
                    .parse()
                    .unwrap(),
                &h
            ),
            None
        );
        let mut host = HeaderMap::new();
        host.insert(header::HOST, "abc123.relay.example:443".parse().unwrap());
        assert_eq!(
            route(&r, &"/files?sig=x".parse().unwrap(), &host),
            Some(("abc123".into(), "/files?sig=x".into(), String::new()))
        );
        assert_eq!(route(&relay(None), &"/files".parse().unwrap(), &host), None);
    }

    #[test]
    fn machine_url_prefers_the_host_domain() {
        let mut config = RelayConfig {
            public_url: Some("https://relay.example".into()),
            ..RelayConfig::default()
        };
        let no_domain = Relay::new(config.clone());
        let h = HeaderMap::new();
        assert_eq!(
            no_domain.machine_url(&h, "abc123"),
            "https://relay.example/m/abc123"
        );
        config.host_domain = Some("m.relay.example".into());
        let with_domain = Relay::new(config);
        assert_eq!(
            with_domain.machine_url(&h, "abc123"),
            "https://abc123.m.relay.example"
        );
    }

    #[test]
    fn anonymous_requests_are_refused_but_credentials_pass() {
        let h = HeaderMap::new();
        assert!(!has_client_credential(&"/m/a/x".parse().unwrap(), &h, "/x"));
        assert!(has_client_credential(
            &"/m/a/health".parse().unwrap(),
            &h,
            "/health"
        ));
        assert!(has_client_credential(
            &"/m/a/tunnel?ticket=v1.x".parse().unwrap(),
            &h,
            "/tunnel"
        ));
        assert!(has_client_credential(
            &"/m/a/files?path=a&sig=b".parse().unwrap(),
            &h,
            "/files"
        ));
        let mut auth = HeaderMap::new();
        auth.insert("x-cua-env-authorization", "Bearer t".parse().unwrap());
        assert!(has_client_credential(
            &"/m/a/x".parse().unwrap(),
            &auth,
            "/x"
        ));
        // Headers are stripped for account machines, so they are not a
        // credential that lets an anonymous request through there.
        assert!(!has_forwardable_credential(
            &"/m/a/x".parse().unwrap(),
            &auth,
            "/x"
        ));
        assert!(has_forwardable_credential(
            &"/m/a/tunnel?ticket=v1.x".parse().unwrap(),
            &auth,
            "/tunnel"
        ));
    }

    #[test]
    fn machine_cookies_are_stripped() {
        let mut h = HeaderMap::new();
        h.append(
            header::SET_COOKIE,
            "a=1; Domain=example.com".parse().unwrap(),
        );
        h.append(header::SET_COOKIE, "b=2".parse().unwrap());
        h.insert(header::CONTENT_TYPE, "application/grpc".parse().unwrap());
        harden_machine_response(&mut h, "/tunnel");
        assert!(h.get(header::SET_COOKIE).is_none());
        assert_eq!(h.get(header::CONTENT_TYPE).unwrap(), "application/grpc");
    }

    #[test]
    fn a_machine_cannot_widen_a_service_worker_scope() {
        let mut h = HeaderMap::new();
        h.insert("service-worker-allowed", "/".parse().unwrap());
        h.insert("service-worker", "active".parse().unwrap());
        harden_machine_response(&mut h, "/anything");
        assert!(h.get("service-worker-allowed").is_none());
        assert!(h.get("service-worker").is_none());
    }

    #[test]
    fn only_the_known_viewer_may_answer_with_html() {
        let mut h = HeaderMap::new();
        h.insert(
            header::CONTENT_TYPE,
            "text/html; charset=utf-8".parse().unwrap(),
        );
        harden_machine_response(&mut h, "/phish?x=1");
        assert_eq!(
            h.get(header::CONTENT_TYPE).unwrap(),
            "application/octet-stream"
        );
        assert_eq!(h.get(header::CONTENT_DISPOSITION).unwrap(), "attachment");
        assert!(h.get(header::CONTENT_SECURITY_POLICY).is_some());
        assert_eq!(h.get(header::X_CONTENT_TYPE_OPTIONS).unwrap(), "nosniff");

        // The known viewer keeps its HTML and its own CSP (not overwritten).
        let mut viewer = HeaderMap::new();
        viewer.insert(
            header::CONTENT_TYPE,
            "text/html; charset=utf-8".parse().unwrap(),
        );
        viewer.insert(
            header::CONTENT_SECURITY_POLICY,
            "default-src 'self'".parse().unwrap(),
        );
        harden_machine_response(&mut viewer, "/viewer/index.html");
        assert_eq!(
            viewer.get(header::CONTENT_TYPE).unwrap(),
            "text/html; charset=utf-8"
        );
        assert_eq!(
            viewer.get(header::CONTENT_SECURITY_POLICY).unwrap(),
            "default-src 'self'"
        );
        // Bare /viewer (no trailing slash) counts too.
        let mut bare = HeaderMap::new();
        bare.insert(header::CONTENT_TYPE, "text/html".parse().unwrap());
        harden_machine_response(&mut bare, "/viewer");
        assert_eq!(bare.get(header::CONTENT_TYPE).unwrap(), "text/html");
        // A path that merely starts with "/viewer" as a prefix of another
        // word does not count.
        let mut lookalike = HeaderMap::new();
        lookalike.insert(header::CONTENT_TYPE, "text/html".parse().unwrap());
        harden_machine_response(&mut lookalike, "/viewership");
        assert_eq!(
            lookalike.get(header::CONTENT_TYPE).unwrap(),
            "application/octet-stream"
        );
    }
}
