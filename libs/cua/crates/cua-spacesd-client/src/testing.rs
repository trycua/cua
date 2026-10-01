//! In-process mock cua-spacesd for tests (feature `testing`).
//!
//! Implements the generated `System`, `Process`, `Filesystem`, `Computer`,
//! `Windows`, `Accessibility`, `Stream`, `Teleport` (session import only) and
//! `Tunnel` (`Forward` plus the ticketed `/tunnel` WebSocket, which relays to
//! a real loopback port; advertise `tunnel.forward` to expose it) server traits with in-memory state, served over native gRPC
//! and gRPC-Web on one loopback port (like the real driver). Processes are
//! *simulated*; nothing is executed on the host. Supported fake programs
//! (the first word of the command line, or of `sh -c "<line>"`):
//!
//! - `echo <words…>`: prints the words to stdout.
//! - `fail <code>`: prints to stderr and exits with `code`.
//! - `ticker <n> <interval_ms>`: prints `tick <i>` n times.
//! - `cat`: echoes stdin until it is closed.
//! - `sleep <ms>`: exits 0 after `ms`.
//! - `cp <src> <dst>`, `rm [-f] <path...>`, `test -f|-e <path>`: act on the
//!   mock's in-memory files (the ones `Filesystem` reads and writes).
//!
//! Anything else exits 127 with an error. [`CutProxy`] is a TCP proxy that
//! severs every connection after a byte budget, for resume tests.

use crate::error::status_with_reason;
use cua_proto::env::v1::{
    self as pb, accessibility_service_server::*, computer_service_server::*,
    filesystem_service_server::*, presence_service_server::*, process_service_server::*,
    stream_service_server::*, system_service_server::*, teleport_service_server::*,
    tunnel_service_server::*, windows_service_server::*,
};
use futures_util::StreamExt;
use http::{Request, Response};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    future::Future,
    net::SocketAddr,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{Notify, mpsc, oneshot},
};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Status, body::Body};

type Stream<T> = Pin<Box<dyn futures_util::Stream<Item = Result<T, Status>> + Send>>;

/// How the mock authenticates callers.
#[derive(Clone, Debug, Default)]
pub struct MockAuth {
    /// Required env token (direct mode: `authorization`).
    pub token: Option<String>,
    /// Fleet gateway emulation: required path prefix, gateway bearer and
    /// claim header. In this mode the env token is read from
    /// `x-cua-env-authorization`.
    pub gateway: Option<MockGateway>,
    /// Path prefix to strip without gateway checks (relay emulation, e.g.
    /// `/m/<machine-id>`).
    pub prefix: Option<String>,
}

/// Fleet gateway emulation settings.
#[derive(Clone, Debug)]
pub struct MockGateway {
    /// Path prefix, e.g. `/api/svc/ns/sbx-env`.
    pub prefix: String,
    /// Expected gateway bearer.
    pub bearer: String,
    /// Expected `X-Cua-Fleet-Claim`.
    pub claim: String,
}

/// Observations the tests assert on.
#[derive(Debug, Default)]
pub struct Observed {
    /// Last decoded principal.
    pub principal: Mutex<Option<pb::Principal>>,
    /// Last pointer action (debug string).
    pub pointer: Mutex<Vec<String>>,
    /// Keyboard actions (debug string).
    pub keyboard: Mutex<Vec<String>>,
    /// Requests seen by content-type family.
    pub grpc_web_requests: AtomicU64,
    /// Native gRPC requests.
    pub grpc_requests: AtomicU64,
    /// UploadChunk calls.
    pub upload_chunks: AtomicU64,
    /// WriteFile streams.
    pub write_file_streams: AtomicU64,
    /// Window and app actions (`activate w1`, `bounds w1 …`, `open …`).
    pub windows: Mutex<Vec<String>>,
    /// Accessibility actions (`act <element> <action> <value>`).
    pub accessibility: Mutex<Vec<String>>,
    /// Screenshot requests.
    pub screenshots: Mutex<Vec<pb::ScreenshotRequest>>,
}

#[derive(Default)]
struct Upload {
    path: String,
    header: pb::WriteFileHeader,
    data: Vec<u8>,
}

struct Proc {
    pid: u32,
    tag: String,
    /// (offset, kind 0=stdout 1=stderr 2=pty, bytes)
    output: Vec<(u64, u8, Vec<u8>)>,
    end_offset: u64,
    scrollback: u64,
    end: Option<pb::ProcessEnd>,
    stdin: Option<mpsc::UnboundedSender<Option<Vec<u8>>>>,
    applied_seq: HashMap<String, u64>,
    changed: Arc<Notify>,
}

impl Proc {
    fn scrollback_start(&self) -> u64 {
        self.output.first().map(|o| o.0).unwrap_or(self.end_offset)
    }
}

/// Shared mock state.
#[derive(Default)]
pub struct MockState {
    files: Mutex<HashMap<String, Vec<u8>>>,
    uploads: Mutex<HashMap<String, Upload>>,
    procs: Mutex<HashMap<u32, Proc>>,
    next_pid: AtomicU32,
    clipboard: Mutex<(Option<String>, u64)>,
    cursor: Mutex<(f64, f64)>,
    windows: Mutex<Vec<pb::WindowInfo>>,
    /// Observations.
    pub observed: Observed,
    auth: MockAuth,
    /// Answer `GetCapabilities` with UNIMPLEMENTED (not a spacesd).
    pub refuse_capabilities: AtomicBool,
    /// Report pointer input as delivered in the foreground without moving
    /// the pointer (input that reached nothing).
    pub pointer_not_moved: AtomicBool,
    /// The `desktop` component `Health` reports (none: an older spacesd).
    pub desktop_health: Mutex<Option<pb::ComponentHealth>>,
    /// Extra supported features reported by `GetCapabilities` (for clients
    /// that gate on stream / window features).
    extra_features: Mutex<Vec<String>>,
    /// `GetCapabilitiesResponse::machine_seal_public_key` (S1); empty (an
    /// older guest) unless a test sets one with
    /// [`MockState::set_machine_seal_public_key`].
    machine_seal_public_key: Mutex<Vec<u8>>,
    /// In-progress teleport imports by id.
    teleport_uploads: Mutex<HashMap<String, TeleportUpload>>,
    teleport_imports: Mutex<Vec<TeleportImport>>,
    /// `TeleportService.WipeImport` requests, in order.
    teleport_wipes: Mutex<Vec<pb::WipeImportRequest>>,
    /// The relay this mock was attached to (`SystemService.AttachRelay`),
    /// until `DetachRelay`.
    pub relay_attached: Mutex<Option<pb::AttachRelayRequest>>,
    /// Teleport apps `GetManifest` reports as unsupported.
    pub teleport_unsupported: Mutex<Vec<String>>,
    /// Presence: joined participants and their event streams.
    presence: Mutex<Vec<(String, PresenceSender)>>,
    next_participant: AtomicU32,
    /// Tunnel forwards by ticket.
    forwards: Mutex<HashMap<String, MockForward>>,
    next_forward: AtomicU32,
    /// `/tunnel` WebSocket attaches (accepted and refused).
    tunnel_attaches: Mutex<Vec<TunnelAttach>>,
    /// Report `Diagnose` / `DiagnoseOnce` answer with (FAILED_PRECONDITION
    /// until set).
    diagnose_report: Mutex<Option<pb::DiagnoseReport>>,
}

#[derive(Clone)]
struct MockForward {
    id: String,
    port: u16,
    closed: Arc<tokio::sync::watch::Sender<bool>>,
}

/// One `/tunnel` WebSocket attach the mock saw.
#[derive(Clone, Debug, Default)]
pub struct TunnelAttach {
    /// Request path and query, as received (before prefix stripping).
    pub path: String,
    /// `authorization` header.
    pub authorization: Option<String>,
    /// `x-cua-fleet-claim` header.
    pub claim: Option<String>,
    /// `x-cua-env-authorization` header.
    pub env_authorization: Option<String>,
    /// Whether the attach was accepted.
    pub accepted: bool,
}

/// A joined participant's event stream.
type PresenceSender = mpsc::Sender<Result<pb::JoinResponse, Status>>;

/// An in-progress teleport import: (app, bytes, chunks).
type TeleportUpload = (String, Vec<u8>, usize);

/// A session bundle the mock's `TeleportService.ImportSession` accepted
/// (verified against its SHA-256; nothing is imported anywhere).
#[derive(Clone, Debug)]
pub struct TeleportImport {
    /// Client-chosen import id.
    pub import_id: String,
    /// Provider / app id.
    pub app: String,
    /// The whole bundle.
    pub bundle: Vec<u8>,
    /// Chunks it arrived in.
    pub chunks: usize,
    /// Import options on the final chunk.
    pub options: pb::ImportOptions,
}

impl MockState {
    /// Session bundles committed through `TeleportService.ImportSession`
    /// and not wiped since.
    pub fn teleport_imports(&self) -> Vec<TeleportImport> {
        self.teleport_imports.lock().unwrap().clone()
    }

    /// `TeleportService.WipeImport` requests received, in order.
    pub fn teleport_wipes(&self) -> Vec<pb::WipeImportRequest> {
        self.teleport_wipes.lock().unwrap().clone()
    }

    /// Stored file content.
    pub fn file(&self, path: &str) -> Option<Vec<u8>> {
        self.files.lock().unwrap().get(path).cloned()
    }

    /// Puts a file.
    pub fn put_file(&self, path: &str, data: Vec<u8>) {
        self.files.lock().unwrap().insert(path.into(), data);
    }

    /// Sets the report `Diagnose` streams (one `check` event per check, then
    /// the report) and `DiagnoseOnce` returns.
    pub fn set_diagnose_report(&self, report: pb::DiagnoseReport) {
        *self.diagnose_report.lock().unwrap() = Some(report);
    }

    /// Replaces the window list (the default is [`default_windows`]).
    pub fn set_windows(&self, windows: Vec<pb::WindowInfo>) {
        *self.windows.lock().unwrap() = windows;
    }

    /// Current windows.
    pub fn windows(&self) -> Vec<pb::WindowInfo> {
        self.windows.lock().unwrap().clone()
    }

    /// `/tunnel` WebSocket attaches so far.
    pub fn tunnel_attaches(&self) -> Vec<TunnelAttach> {
        self.tunnel_attaches.lock().unwrap().clone()
    }

    /// Ids of the tunnel forwards still open.
    pub fn open_forwards(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .forwards
            .lock()
            .unwrap()
            .values()
            .map(|f| f.id.clone())
            .collect();
        ids.sort();
        ids
    }

    /// Also report `names` as supported features in `GetCapabilities`.
    pub fn advertise(&self, names: &[&str]) {
        self.extra_features
            .lock()
            .unwrap()
            .extend(names.iter().map(|n| n.to_string()));
    }

    /// Report `key` as this mock guest's sealed-delivery public key (S1).
    pub fn set_machine_seal_public_key(&self, key: [u8; 32]) {
        *self.machine_seal_public_key.lock().unwrap() = key.to_vec();
    }
}

/// A mock window.
pub fn mock_window(
    id: &str,
    title: &str,
    app: &str,
    bounds: (f64, f64, f64, f64),
) -> pb::WindowInfo {
    pb::WindowInfo {
        r#ref: Some(pb::WindowRef {
            id: id.into(),
            epoch: 1,
        }),
        title: title.into(),
        app: Some(pb::AppInfo {
            name: app.into(),
            app_id: format!("mock.{}", app.to_ascii_lowercase()),
            pid: 4000,
        }),
        bounds: Some(pb::Rect {
            x: bounds.0,
            y: bounds.1,
            width: bounds.2,
            height: bounds.3,
        }),
        display_id: "primary".into(),
        state: pb::WindowState::Normal as i32,
        kind: pb::WindowKind::Standard as i32,
        focused: false,
        on_screen: true,
        z_order: 0,
    }
}

/// The mock's initial windows: a focused terminal and an editor.
pub fn default_windows() -> Vec<pb::WindowInfo> {
    let mut term = mock_window("w1", "Terminal", "Terminal", (100.0, 50.0, 800.0, 600.0));
    term.focused = true;
    vec![
        term,
        mock_window(
            "w2",
            "notes.txt - Editor",
            "Editor",
            (300.0, 200.0, 640.0, 480.0),
        ),
    ]
}

/// A running mock server.
pub struct MockServer {
    /// Bound address.
    pub addr: SocketAddr,
    /// State.
    pub state: Arc<MockState>,
    shutdown: Option<oneshot::Sender<()>>,
}

impl Drop for MockServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

impl MockServer {
    /// Starts a mock on 127.0.0.1:0.
    pub async fn start(auth: MockAuth) -> Self {
        let state = Arc::new(MockState {
            auth,
            next_pid: AtomicU32::new(1000),
            windows: Mutex::new(default_windows()),
            ..Default::default()
        });
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let svc = Svc(state.clone());
        let (tx, rx) = oneshot::channel();
        let layer_state = state.clone();
        // A front on the same port: `/tunnel` WebSocket upgrades are served
        // here, everything else goes to tonic.
        let (conn_tx, conn_rx) = mpsc::channel::<std::io::Result<TcpStream>>(64);
        let front_state = state.clone();
        tokio::spawn(async move {
            loop {
                let stream = tokio::select! {
                    r = listener.accept() => match r {
                        Ok((s, _)) => s,
                        Err(_) => return,
                    },
                    _ = conn_tx.closed() => return,
                };
                let conn_tx = conn_tx.clone();
                let state = front_state.clone();
                tokio::spawn(async move {
                    if is_tunnel_upgrade(&stream).await {
                        serve_tunnel(stream, state).await;
                    } else {
                        let _ = conn_tx.send(Ok(stream)).await;
                    }
                });
            }
        });
        tokio::spawn(async move {
            tonic::transport::Server::builder()
                .accept_http1(true)
                .layer(tonic_web::GrpcWebLayer::new())
                .layer(tower::layer::layer_fn(move |inner| AuthLayer {
                    inner,
                    state: layer_state.clone(),
                }))
                .add_service(SystemServiceServer::new(svc.clone()))
                .add_service(
                    ProcessServiceServer::new(svc.clone())
                        .max_decoding_message_size(64 << 20)
                        .max_encoding_message_size(64 << 20),
                )
                .add_service(
                    FilesystemServiceServer::new(svc.clone())
                        .max_decoding_message_size(64 << 20)
                        .max_encoding_message_size(64 << 20),
                )
                .add_service(ComputerServiceServer::new(svc.clone()))
                .add_service(WindowsServiceServer::new(svc.clone()))
                .add_service(AccessibilityServiceServer::new(svc.clone()))
                .add_service(StreamServiceServer::new(svc.clone()))
                .add_service(PresenceServiceServer::new(svc.clone()))
                .add_service(TunnelServiceServer::new(svc.clone()))
                .add_service(
                    TeleportServiceServer::new(svc)
                        .max_decoding_message_size(64 << 20)
                        .max_encoding_message_size(64 << 20),
                )
                .serve_with_incoming_shutdown(ReceiverStream::new(conn_rx), async {
                    let _ = rx.await;
                })
                .await
                .unwrap();
        });
        Self {
            addr,
            state,
            shutdown: Some(tx),
        }
    }

    /// `http://127.0.0.1:<port>`.
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }
}

#[derive(Clone)]
struct AuthLayer<S> {
    inner: S,
    state: Arc<MockState>,
}

impl<S> tower::Service<Request<Body>> for AuthLayer<S>
where
    S: tower::Service<Request<Body>, Response = Response<Body>> + Clone + Send + 'static,
    S::Future: Send,
{
    type Response = Response<Body>;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Response<Body>, S::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<Body>) -> Self::Future {
        let state = self.state.clone();
        let is_web =
            req.headers().get("x-grpc-web").is_some() || req.version() == http::Version::HTTP_11;
        if is_web {
            state
                .observed
                .grpc_web_requests
                .fetch_add(1, Ordering::Relaxed);
        } else {
            state.observed.grpc_requests.fetch_add(1, Ordering::Relaxed);
        }
        if let Some(p) = req.headers().get(cua_proto::metadata::PRINCIPAL_BIN) {
            use base64::Engine;
            use prost::Message;
            let raw = p.to_str().unwrap_or_default().trim_end_matches('=');
            if let Ok(bytes) = base64::engine::general_purpose::STANDARD_NO_PAD.decode(raw) {
                *state.observed.principal.lock().unwrap() = pb::Principal::decode(&bytes[..]).ok();
            }
        }
        let headers = req.headers().clone();
        let header = |name: &str| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        let mut denied: Option<Status> = None;
        let mut env_auth = header("authorization");
        let prefix = state
            .auth
            .gateway
            .as_ref()
            .map(|g| g.prefix.clone())
            .or_else(|| state.auth.prefix.clone());
        if let Some(prefix) = prefix {
            let path = req.uri().path().to_string();
            match path.strip_prefix(&prefix) {
                Some(rest) if rest.starts_with('/') => {
                    let mut parts = req.uri().clone().into_parts();
                    parts.path_and_query = Some(rest.parse().unwrap());
                    *req.uri_mut() = http::Uri::from_parts(parts).unwrap();
                }
                _ => denied = Some(Status::not_found(format!("no route {path}"))),
            }
        }
        if let Some(gw) = &state.auth.gateway {
            if header("authorization").as_deref() != Some(&format!("Bearer {}", gw.bearer)) {
                denied = Some(Status::unauthenticated("gateway bearer"));
            }
            if header("x-cua-fleet-claim").as_deref() != Some(gw.claim.as_str()) {
                denied = Some(Status::permission_denied("claim header"));
            }
            env_auth = header(crate::transport::ENV_TOKEN_HEADER);
        }
        if let Some(t) = &state.auth.token
            && env_auth.as_deref() != Some(&format!("Bearer {t}"))
        {
            denied = Some(Status::unauthenticated("bad token"));
        }
        if let Some(status) = denied {
            return Box::pin(async move { Ok(status.into_http()) });
        }
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);
        Box::pin(async move { inner.call(req).await })
    }
}

#[derive(Clone)]
struct Svc(Arc<MockState>);

fn fs_err(
    reason: pb::ErrorReason,
    code: tonic::Code,
    msg: &str,
    meta: &[(&str, String)],
) -> Status {
    status_with_reason(
        code,
        reason,
        msg,
        meta.iter().map(|(k, v)| (k.to_string(), v.clone())),
    )
}

fn entry(path: &str, size: u64) -> pb::EntryInfo {
    pb::EntryInfo {
        name: path.rsplit('/').next().unwrap_or_default().into(),
        path: path.into(),
        r#type: pb::FileType::File as i32,
        size,
        mode: 0o644,
        ..Default::default()
    }
}

fn dur(d: &Option<pbjson_types::Duration>, default: Duration) -> Duration {
    d.as_ref()
        .map(|d| Duration::new(d.seconds.max(0) as u64, d.nanos.max(0) as u32))
        .filter(|d| !d.is_zero())
        .unwrap_or(default)
}

#[tonic::async_trait]
impl SystemService for Svc {
    async fn get_capabilities(
        &self,
        _: tonic::Request<pb::GetCapabilitiesRequest>,
    ) -> Result<tonic::Response<pb::GetCapabilitiesResponse>, Status> {
        if self.0.refuse_capabilities.load(Ordering::Relaxed) {
            return Err(Status::unimplemented("not a spacesd"));
        }
        Ok(tonic::Response::new(pb::GetCapabilitiesResponse {
            version: "0.0.0-mock".into(),
            protocol_version: cua_proto::ENV_PROTOCOL_VERSION,
            protocol_revision: cua_proto::ENV_PROTOCOL_REVISION,
            os: Some(pb::OperatingSystem {
                family: pb::OsFamily::Linux as i32,
                name: "MockOS".into(),
                version: "1".into(),
                kernel: "mock".into(),
                pretty_name: "MockOS 1.0.3 LTS".into(),
            }),
            runtime: pb::Runtime::Container as i32,
            features: vec![
                pb::Feature {
                    name: "pty".into(),
                    supported: true,
                    ..Default::default()
                },
                pb::Feature {
                    name: "a11y".into(),
                    supported: false,
                    limitation: "mock has no accessibility".into(),
                    ..Default::default()
                },
            ]
            .into_iter()
            .chain(
                self.0
                    .extra_features
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|n| pb::Feature {
                        name: n.clone(),
                        supported: true,
                        ..Default::default()
                    }),
            )
            .collect(),
            hostname: "mock".into(),
            limits: Some(pb::Limits {
                max_chunk_bytes: 4 << 20,
                max_message_bytes: 8 << 20,
                preferred_chunk_bytes: 1 << 20,
                default_scrollback_bytes: 1 << 20,
            }),
            machine_seal_public_key: self.0.machine_seal_public_key.lock().unwrap().clone(),
            ..Default::default()
        }))
    }

    async fn init(
        &self,
        _: tonic::Request<pb::InitRequest>,
    ) -> Result<tonic::Response<pb::InitResponse>, Status> {
        Ok(tonic::Response::new(pb::InitResponse::default()))
    }

    async fn health(
        &self,
        _: tonic::Request<pb::HealthRequest>,
    ) -> Result<tonic::Response<pb::HealthResponse>, Status> {
        let components: Vec<pb::ComponentHealth> = self
            .0
            .desktop_health
            .lock()
            .unwrap()
            .iter()
            .cloned()
            .collect();
        let status = components
            .iter()
            .map(|c| c.status)
            .max()
            .unwrap_or(pb::HealthStatus::Serving as i32);
        Ok(tonic::Response::new(pb::HealthResponse {
            status,
            components,
            ..Default::default()
        }))
    }

    async fn metrics(
        &self,
        _: tonic::Request<pb::MetricsRequest>,
    ) -> Result<tonic::Response<pb::MetricsResponse>, Status> {
        // A 4 GiB guest with a 32 GiB disk, both its own.
        Ok(tonic::Response::new(pb::MetricsResponse {
            memory_total_bytes: 4 << 30,
            memory_used_bytes: 1_610_612_736,
            memory_limited: true,
            disk_total_bytes: 32 << 30,
            disk_used_bytes: 9_663_676_416,
            disk_limited: true,
            ..Default::default()
        }))
    }

    async fn shutdown(
        &self,
        _: tonic::Request<pb::ShutdownRequest>,
    ) -> Result<tonic::Response<pb::ShutdownResponse>, Status> {
        Err(Status::unimplemented("mock"))
    }

    type DiagnoseStream = Stream<pb::DiagnoseResponse>;

    async fn diagnose(
        &self,
        _: tonic::Request<pb::DiagnoseRequest>,
    ) -> Result<tonic::Response<Self::DiagnoseStream>, Status> {
        let report = self.mock_report()?;
        let mut events: Vec<Result<pb::DiagnoseResponse, Status>> = Vec::new();
        for check in &report.checks {
            events.push(Ok(pb::DiagnoseResponse {
                event: Some(pb::diagnose_response::Event::Started(
                    pb::DiagnoseCheckStarted {
                        id: check.id.clone(),
                        group: check.group.clone(),
                    },
                )),
            }));
            events.push(Ok(pb::DiagnoseResponse {
                event: Some(pb::diagnose_response::Event::Check(check.clone())),
            }));
        }
        events.push(Ok(pb::DiagnoseResponse {
            event: Some(pb::diagnose_response::Event::Report(report)),
        }));
        Ok(tonic::Response::new(Box::pin(futures_util::stream::iter(
            events,
        ))))
    }

    async fn diagnose_once(
        &self,
        _: tonic::Request<pb::DiagnoseOnceRequest>,
    ) -> Result<tonic::Response<pb::DiagnoseOnceResponse>, Status> {
        Ok(tonic::Response::new(pb::DiagnoseOnceResponse {
            report: Some(self.mock_report()?),
        }))
    }

    async fn create_viewer_ticket(
        &self,
        req: tonic::Request<pb::CreateViewerTicketRequest>,
    ) -> Result<tonic::Response<pb::CreateViewerTicketResponse>, Status> {
        let req = req.into_inner();
        let ttl = dur(&req.ttl, Duration::from_secs(3600));
        let ticket = format!("mock-viewer.{}", ttl.as_secs());
        let expires = std::time::SystemTime::now() + ttl;
        let since = expires
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        Ok(tonic::Response::new(pb::CreateViewerTicketResponse {
            viewer_path: format!("{}/#ticket={ticket}", cua_proto::metadata::VIEWER_PATH),
            ticket,
            expires_at: Some(pbjson_types::Timestamp {
                seconds: since.as_secs() as i64,
                nanos: since.subsec_nanos() as i32,
            }),
            files_root: match req.files_root.as_str() {
                "" => String::new(),
                "~" => "/home/mock".into(),
                other => other.into(),
            },
        }))
    }

    async fn attach_relay(
        &self,
        req: tonic::Request<pb::AttachRelayRequest>,
    ) -> Result<tonic::Response<pb::AttachRelayResponse>, Status> {
        let req = req.into_inner();
        let machine_id = req.machine_id.clone();
        *self.0.relay_attached.lock().unwrap() = Some(req);
        Ok(tonic::Response::new(pb::AttachRelayResponse { machine_id }))
    }

    async fn detach_relay(
        &self,
        _: tonic::Request<pb::DetachRelayRequest>,
    ) -> Result<tonic::Response<pb::DetachRelayResponse>, Status> {
        let detached = self.0.relay_attached.lock().unwrap().take().is_some();
        Ok(tonic::Response::new(pb::DetachRelayResponse { detached }))
    }
}

impl Svc {
    fn mock_report(&self) -> Result<pb::DiagnoseReport, Status> {
        self.0
            .diagnose_report
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| Status::failed_precondition("mock: no diagnose report set"))
    }
}

// ---------------------------------------------------------------- processes

enum Script {
    Echo(String),
    Fail(i32),
    Ticker(u32, u64),
    Cat,
    Sleep(u64),
    Cp(String, String),
    Rm(Vec<String>, bool),
    Test(String),
    Unknown(String),
}

fn parse_script(cfg: &pb::ProcessConfig) -> Script {
    let mut words: Vec<String> = if (cfg.command.ends_with("sh") || cfg.command.ends_with("bash"))
        && cfg
            .args
            .first()
            .map(|a| a.starts_with('-'))
            .unwrap_or(false)
    {
        cfg.args
            .get(1)
            .map(|l| l.split_whitespace().map(str::to_string).collect())
            .unwrap_or_default()
    } else {
        std::iter::once(cfg.command.clone())
            .chain(cfg.args.iter().cloned())
            .collect()
    };
    if words.is_empty() {
        return Script::Unknown(String::new());
    }
    let prog = words.remove(0);
    let num = |i: usize, d: u64| words.get(i).and_then(|w| w.parse().ok()).unwrap_or(d);
    match prog.rsplit('/').next().unwrap_or_default() {
        "echo" => Script::Echo(words.join(" ")),
        "fail" => Script::Fail(num(0, 1) as i32),
        "ticker" => Script::Ticker(num(0, 3) as u32, num(1, 10)),
        "cat" => Script::Cat,
        "sleep" => Script::Sleep(num(0, 10)),
        "cp" if words.len() == 2 => Script::Cp(words[0].clone(), words[1].clone()),
        "rm" => Script::Rm(
            words
                .iter()
                .filter(|w| !w.starts_with('-'))
                .cloned()
                .collect(),
            words.iter().any(|w| w.starts_with('-') && w.contains('f')),
        ),
        "test" if words.len() == 2 && (words[0] == "-f" || words[0] == "-e") => {
            Script::Test(words[1].clone())
        }
        other => Script::Unknown(other.into()),
    }
}

impl Svc {
    fn emit(&self, pid: u32, kind: u8, data: Vec<u8>) {
        let mut procs = self.0.procs.lock().unwrap();
        if let Some(p) = procs.get_mut(&pid) {
            let off = p.end_offset;
            p.end_offset += data.len() as u64;
            p.output.push((off, kind, data));
            // Enforce the scrollback ring.
            while p.output.len() > 1 && p.end_offset - p.output[0].0 > p.scrollback {
                p.output.remove(0);
            }
            p.changed.notify_waiters();
        }
    }

    fn finish(&self, pid: u32, code: Option<i32>, timed_out: bool, error: String) {
        let mut procs = self.0.procs.lock().unwrap();
        if let Some(p) = procs.get_mut(&pid)
            && p.end.is_none()
        {
            p.end = Some(pb::ProcessEnd {
                exit_code: code,
                signal: if code.is_none() && error.is_empty() {
                    pb::Signal::Kill as i32
                } else {
                    0
                },
                timed_out,
                error,
                ended_at: None,
            });
            p.stdin = None;
            p.changed.notify_waiters();
        }
    }

    fn find(&self, sel: &Option<pb::ProcessSelector>) -> Result<u32, Status> {
        let procs = self.0.procs.lock().unwrap();
        let found = match sel.as_ref().and_then(|s| s.selector.clone()) {
            Some(pb::process_selector::Selector::Pid(pid)) => {
                procs.contains_key(&pid).then_some(pid)
            }
            Some(pb::process_selector::Selector::Tag(tag)) => {
                procs.values().filter(|p| p.tag == tag).map(|p| p.pid).max()
            }
            None => None,
        };
        found.ok_or_else(|| {
            fs_err(
                pb::ErrorReason::ProcessNotFound,
                tonic::Code::NotFound,
                "no such process",
                &[],
            )
        })
    }

    /// Streams Start, replay from `from`, live output, End, with keepalives.
    fn attach_stream<T: Send + 'static>(
        &self,
        pid: u32,
        from: u64,
        keepalive: Duration,
        wrap: fn(pb::ProcessEvent) -> T,
    ) -> Stream<T> {
        let (tx, rx) = mpsc::channel::<Result<T, Status>>(64);
        let svc = self.clone();
        tokio::spawn(async move {
            let (start, changed) = {
                let procs = svc.0.procs.lock().unwrap();
                let p = &procs[&pid];
                (
                    pb::ProcessStart {
                        pid,
                        tag: p.tag.clone(),
                        started_at: None,
                        scrollback_start_offset: p.scrollback_start(),
                        output_end_offset: p.end_offset,
                    },
                    p.changed.clone(),
                )
            };
            let send = |ev| wrap(pb::ProcessEvent { event: Some(ev) });
            if tx
                .send(Ok(send(pb::process_event::Event::Start(start))))
                .await
                .is_err()
            {
                return;
            }
            let mut next = from;
            loop {
                let notified = changed.notified();
                let (chunks, end) = {
                    let procs = svc.0.procs.lock().unwrap();
                    let p = &procs[&pid];
                    let chunks: Vec<_> = p
                        .output
                        .iter()
                        .filter(|(off, _, d)| off + d.len() as u64 > next)
                        .cloned()
                        .collect();
                    (chunks, p.end.clone())
                };
                for (off, kind, mut data) in chunks {
                    let mut off = off;
                    if off < next {
                        data.drain(..(next - off) as usize);
                        off = next;
                    }
                    next = off + data.len() as u64;
                    let output = match kind {
                        0 => pb::process_data::Output::Stdout(data),
                        1 => pb::process_data::Output::Stderr(data),
                        _ => pb::process_data::Output::Pty(data),
                    };
                    let ev = pb::process_event::Event::Data(pb::ProcessData {
                        offset: off,
                        output: Some(output),
                    });
                    if tx.send(Ok(send(ev))).await.is_err() {
                        return;
                    }
                }
                if let Some(end) = end {
                    let _ = tx.send(Ok(send(pb::process_event::Event::End(end)))).await;
                    return;
                }
                tokio::select! {
                    _ = notified => {}
                    _ = tokio::time::sleep(keepalive) => {
                        let ev = pb::process_event::Event::Keepalive(pb::KeepAlive {});
                        if tx.send(Ok(send(ev))).await.is_err() {
                            return;
                        }
                    }
                }
            }
        });
        Box::pin(ReceiverStream::new(rx))
    }
}

#[tonic::async_trait]
impl ProcessService for Svc {
    type StartProcessStream = Stream<pb::StartProcessResponse>;
    type ConnectProcessStream = Stream<pb::ConnectProcessResponse>;

    async fn start_process(
        &self,
        req: tonic::Request<pb::StartProcessRequest>,
    ) -> Result<tonic::Response<Self::StartProcessStream>, Status> {
        let req = req.into_inner();
        let cfg = req.config.clone().unwrap_or_default();
        let script = parse_script(&cfg);
        let pid = self.0.next_pid.fetch_add(1, Ordering::Relaxed);
        let (stdin_tx, mut stdin_rx) = mpsc::unbounded_channel::<Option<Vec<u8>>>();
        {
            let mut procs = self.0.procs.lock().unwrap();
            if !req.tag.is_empty() && procs.values().any(|p| p.tag == req.tag && p.end.is_none()) {
                return Err(Status::already_exists("tag in use"));
            }
            procs.retain(|_, p| !(p.tag == req.tag && !req.tag.is_empty()));
            procs.insert(
                pid,
                Proc {
                    pid,
                    tag: req.tag.clone(),
                    output: vec![],
                    end_offset: 0,
                    scrollback: if req.scrollback_bytes == 0 {
                        1 << 20
                    } else {
                        req.scrollback_bytes
                    },
                    end: None,
                    stdin: req.stdin.then_some(stdin_tx),
                    applied_seq: HashMap::new(),
                    changed: Arc::new(Notify::new()),
                },
            );
        }
        let pty = req.pty.is_some();
        let out_kind = if pty { 2 } else { 0 };
        let timeout = cfg
            .timeout
            .as_ref()
            .map(|_| dur(&cfg.timeout, Duration::MAX));
        let svc = self.clone();
        tokio::spawn(async move {
            let run = async {
                match script {
                    Script::Echo(s) => {
                        svc.emit(pid, out_kind, format!("{s}\n").into_bytes());
                        (Some(0), String::new())
                    }
                    Script::Fail(code) => {
                        svc.emit(pid, if pty { 2 } else { 1 }, b"failed\n".to_vec());
                        (Some(code), String::new())
                    }
                    Script::Ticker(n, ms) => {
                        for i in 0..n {
                            svc.emit(pid, out_kind, format!("tick {i}\n").into_bytes());
                            tokio::time::sleep(Duration::from_millis(ms)).await;
                        }
                        (Some(0), String::new())
                    }
                    Script::Cat => {
                        while let Some(Some(chunk)) = stdin_rx.recv().await {
                            svc.emit(pid, out_kind, chunk);
                        }
                        (Some(0), String::new())
                    }
                    Script::Sleep(ms) => {
                        tokio::time::sleep(Duration::from_millis(ms)).await;
                        (Some(0), String::new())
                    }
                    Script::Cp(src, dst) => {
                        let data = svc.0.files.lock().unwrap().get(&src).cloned();
                        match data {
                            Some(d) => {
                                svc.0.files.lock().unwrap().insert(dst, d);
                                (Some(0), String::new())
                            }
                            None => {
                                let msg =
                                    format!("cp: cannot stat '{src}': No such file or directory\n");
                                svc.emit(pid, if pty { 2 } else { 1 }, msg.into_bytes());
                                (Some(1), String::new())
                            }
                        }
                    }
                    Script::Rm(paths, force) => {
                        let mut missing = false;
                        for p in paths {
                            missing |= svc.0.files.lock().unwrap().remove(&p).is_none();
                        }
                        (Some(i32::from(missing && !force)), String::new())
                    }
                    Script::Test(path) => {
                        let exists = svc.0.files.lock().unwrap().contains_key(&path);
                        (Some(i32::from(!exists)), String::new())
                    }
                    Script::Unknown(p) => (None, format!("executable not found: {p}")),
                }
            };
            match timeout {
                Some(t) => match tokio::time::timeout(t, run).await {
                    Ok((code, err)) => svc.finish(pid, code, false, err),
                    Err(_) => svc.finish(pid, None, true, String::new()),
                },
                None => {
                    let (code, err) = run.await;
                    svc.finish(pid, code, false, err);
                }
            }
        });
        let keepalive = dur(&req.keepalive_interval, Duration::from_secs(30));
        Ok(tonic::Response::new(self.attach_stream(
            pid,
            0,
            keepalive,
            |e| pb::StartProcessResponse { event: Some(e) },
        )))
    }

    async fn connect_process(
        &self,
        req: tonic::Request<pb::ConnectProcessRequest>,
    ) -> Result<tonic::Response<Self::ConnectProcessStream>, Status> {
        let req = req.into_inner();
        let pid = self.find(&req.process)?;
        let from = {
            let procs = self.0.procs.lock().unwrap();
            let p = &procs[&pid];
            match req.replay_from_offset {
                Some(o) => o.max(p.scrollback_start()),
                None => p
                    .end_offset
                    .saturating_sub(req.replay_bytes)
                    .max(p.scrollback_start()),
            }
        };
        let keepalive = dur(&req.keepalive_interval, Duration::from_secs(30));
        Ok(tonic::Response::new(self.attach_stream(
            pid,
            from,
            keepalive,
            |e| pb::ConnectProcessResponse { event: Some(e) },
        )))
    }

    async fn list_processes(
        &self,
        req: tonic::Request<pb::ListProcessesRequest>,
    ) -> Result<tonic::Response<pb::ListProcessesResponse>, Status> {
        let req = req.into_inner();
        let procs = self.0.procs.lock().unwrap();
        let mut list: Vec<_> = procs
            .values()
            .filter(|p| req.include_exited || p.end.is_none())
            .filter(|p| p.tag.starts_with(&req.tag_prefix))
            .map(|p| pb::ProcessInfo {
                pid: p.pid,
                tag: p.tag.clone(),
                state: if p.end.is_some() {
                    pb::ProcessState::Exited
                } else {
                    pb::ProcessState::Running
                } as i32,
                end: p.end.clone(),
                scrollback_start_offset: p.scrollback_start(),
                output_end_offset: p.end_offset,
                ..Default::default()
            })
            .collect();
        list.sort_by_key(|p| std::cmp::Reverse(p.pid));
        Ok(tonic::Response::new(pb::ListProcessesResponse {
            processes: list,
        }))
    }

    async fn send_input(
        &self,
        req: tonic::Request<pb::SendInputRequest>,
    ) -> Result<tonic::Response<pb::SendInputResponse>, Status> {
        let req = req.into_inner();
        let pid = self.find(&req.process)?;
        let mut procs = self.0.procs.lock().unwrap();
        let p = procs.get_mut(&pid).unwrap();
        let mut duplicate = false;
        if req.sequence > 0 {
            let last = p.applied_seq.get(&req.writer_id).copied();
            match last {
                Some(l) if req.sequence <= l => duplicate = true,
                Some(l) if req.sequence != l + 1 => {
                    return Err(fs_err(
                        pb::ErrorReason::SequenceGap,
                        tonic::Code::FailedPrecondition,
                        "gap",
                        &[("expected_sequence", (l + 1).to_string())],
                    ));
                }
                _ => {}
            }
            if !duplicate {
                p.applied_seq.insert(req.writer_id.clone(), req.sequence);
            }
        }
        if !duplicate {
            let data = match req.input.and_then(|i| i.input) {
                Some(pb::process_input::Input::Stdin(d))
                | Some(pb::process_input::Input::Pty(d)) => d,
                None => vec![],
            };
            match &p.stdin {
                Some(tx) => {
                    let _ = tx.send(Some(data));
                }
                None => return Err(Status::failed_precondition("stdin closed")),
            }
        }
        Ok(tonic::Response::new(pb::SendInputResponse {
            applied_sequence: p.applied_seq.get(&req.writer_id).copied().unwrap_or(0),
            duplicate,
        }))
    }

    async fn stream_input(
        &self,
        req: tonic::Request<tonic::Streaming<pb::StreamInputRequest>>,
    ) -> Result<tonic::Response<pb::StreamInputResponse>, Status> {
        let mut s = req.into_inner();
        let mut pid = None;
        let mut total = 0u64;
        while let Some(msg) = s.message().await? {
            match msg.message {
                Some(pb::stream_input_request::Message::Process(sel)) => {
                    pid = Some(self.find(&Some(sel))?)
                }
                Some(pb::stream_input_request::Message::Input(i)) => {
                    let pid = pid.ok_or_else(|| Status::invalid_argument("process first"))?;
                    if let Some(
                        pb::process_input::Input::Stdin(d) | pb::process_input::Input::Pty(d),
                    ) = i.input
                    {
                        total += d.len() as u64;
                        if let Some(tx) = &self.0.procs.lock().unwrap()[&pid].stdin {
                            let _ = tx.send(Some(d));
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(tonic::Response::new(pb::StreamInputResponse {
            bytes_written: total,
        }))
    }

    async fn signal_process(
        &self,
        req: tonic::Request<pb::SignalProcessRequest>,
    ) -> Result<tonic::Response<pb::SignalProcessResponse>, Status> {
        let pid = self.find(&req.into_inner().process)?;
        self.finish(pid, None, false, String::new());
        Ok(tonic::Response::new(pb::SignalProcessResponse {}))
    }

    async fn close_stdin(
        &self,
        req: tonic::Request<pb::CloseStdinRequest>,
    ) -> Result<tonic::Response<pb::CloseStdinResponse>, Status> {
        let pid = self.find(&req.into_inner().process)?;
        if let Some(tx) = self
            .0
            .procs
            .lock()
            .unwrap()
            .get_mut(&pid)
            .unwrap()
            .stdin
            .take()
        {
            let _ = tx.send(None);
        }
        Ok(tonic::Response::new(pb::CloseStdinResponse {}))
    }

    async fn resize_pty(
        &self,
        _: tonic::Request<pb::ResizePtyRequest>,
    ) -> Result<tonic::Response<pb::ResizePtyResponse>, Status> {
        Ok(tonic::Response::new(pb::ResizePtyResponse {}))
    }
}

// --------------------------------------------------------------- filesystem

#[tonic::async_trait]
impl FilesystemService for Svc {
    type WatchDirStream = Stream<pb::WatchDirResponse>;
    type ReadFileStream = Stream<pb::ReadFileResponse>;

    async fn stat(
        &self,
        req: tonic::Request<pb::StatRequest>,
    ) -> Result<tonic::Response<pb::StatResponse>, Status> {
        let path = req.into_inner().path;
        let files = self.0.files.lock().unwrap();
        let data = files.get(&path).ok_or_else(|| {
            fs_err(
                pb::ErrorReason::PathNotFound,
                tonic::Code::NotFound,
                "missing",
                &[("path", path.clone())],
            )
        })?;
        Ok(tonic::Response::new(pb::StatResponse {
            entry: Some(entry(&path, data.len() as u64)),
        }))
    }

    async fn list_dir(
        &self,
        req: tonic::Request<pb::ListDirRequest>,
    ) -> Result<tonic::Response<pb::ListDirResponse>, Status> {
        let dir = req.into_inner().path.trim_end_matches('/').to_string() + "/";
        let files = self.0.files.lock().unwrap();
        let mut entries: Vec<_> = files
            .iter()
            .filter(|(p, _)| p.starts_with(&dir))
            .map(|(p, d)| entry(p, d.len() as u64))
            .collect();
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(tonic::Response::new(pb::ListDirResponse {
            entries,
            next_page_token: String::new(),
        }))
    }

    async fn make_dir(
        &self,
        req: tonic::Request<pb::MakeDirRequest>,
    ) -> Result<tonic::Response<pb::MakeDirResponse>, Status> {
        let path = req.into_inner().path;
        Ok(tonic::Response::new(pb::MakeDirResponse {
            entry: Some(pb::EntryInfo {
                r#type: pb::FileType::Directory as i32,
                ..entry(&path, 0)
            }),
            created: true,
        }))
    }

    async fn r#move(
        &self,
        req: tonic::Request<pb::MoveRequest>,
    ) -> Result<tonic::Response<pb::MoveResponse>, Status> {
        let req = req.into_inner();
        let mut files = self.0.files.lock().unwrap();
        let data = files.remove(&req.source).ok_or_else(|| {
            fs_err(
                pb::ErrorReason::PathNotFound,
                tonic::Code::NotFound,
                "missing",
                &[],
            )
        })?;
        let size = data.len() as u64;
        files.insert(req.destination.clone(), data);
        Ok(tonic::Response::new(pb::MoveResponse {
            entry: Some(entry(&req.destination, size)),
        }))
    }

    async fn remove(
        &self,
        req: tonic::Request<pb::RemoveRequest>,
    ) -> Result<tonic::Response<pb::RemoveResponse>, Status> {
        let req = req.into_inner();
        let removed = self.0.files.lock().unwrap().remove(&req.path);
        if removed.is_none() && !req.missing_ok {
            return Err(fs_err(
                pb::ErrorReason::PathNotFound,
                tonic::Code::NotFound,
                "missing",
                &[],
            ));
        }
        Ok(tonic::Response::new(pb::RemoveResponse {}))
    }

    async fn watch_dir(
        &self,
        _: tonic::Request<pb::WatchDirRequest>,
    ) -> Result<tonic::Response<Self::WatchDirStream>, Status> {
        Err(Status::unimplemented("mock"))
    }

    async fn create_watcher(
        &self,
        _: tonic::Request<pb::CreateWatcherRequest>,
    ) -> Result<tonic::Response<pb::CreateWatcherResponse>, Status> {
        Err(Status::unimplemented("mock"))
    }

    async fn get_watcher_events(
        &self,
        _: tonic::Request<pb::GetWatcherEventsRequest>,
    ) -> Result<tonic::Response<pb::GetWatcherEventsResponse>, Status> {
        Err(Status::unimplemented("mock"))
    }

    async fn remove_watcher(
        &self,
        _: tonic::Request<pb::RemoveWatcherRequest>,
    ) -> Result<tonic::Response<pb::RemoveWatcherResponse>, Status> {
        Err(Status::unimplemented("mock"))
    }

    async fn read_file(
        &self,
        req: tonic::Request<pb::ReadFileRequest>,
    ) -> Result<tonic::Response<Self::ReadFileStream>, Status> {
        let req = req.into_inner();
        let data = self.0.file(&req.path).ok_or_else(|| {
            fs_err(
                pb::ErrorReason::PathNotFound,
                tonic::Code::NotFound,
                "missing",
                &[("path", req.path.clone())],
            )
        })?;
        let chunk = if req.chunk_size == 0 {
            1 << 20
        } else {
            req.chunk_size as usize
        };
        let start = (req.offset as usize).min(data.len());
        let end = if req.length == 0 {
            data.len()
        } else {
            (start + req.length as usize).min(data.len())
        };
        let path = req.path.clone();
        let (tx, rx) = mpsc::channel(4);
        tokio::spawn(async move {
            let mut hasher = Sha256::new();
            let _ = tx
                .send(Ok(pb::ReadFileResponse {
                    message: Some(pb::read_file_response::Message::Entry(entry(
                        &path,
                        data.len() as u64,
                    ))),
                }))
                .await;
            let mut off = start;
            while off < end {
                let e = (off + chunk).min(end);
                hasher.update(&data[off..e]);
                let msg = pb::ReadFileResponse {
                    message: Some(pb::read_file_response::Message::Chunk(pb::FileChunk {
                        offset: off as u64,
                        data: data[off..e].to_vec(),
                    })),
                };
                if tx.send(Ok(msg)).await.is_err() {
                    return;
                }
                off = e;
            }
            let _ = tx
                .send(Ok(pb::ReadFileResponse {
                    message: Some(pb::read_file_response::Message::End(pb::ReadFileEnd {
                        bytes_read: (end - start) as u64,
                        sha256: if req.compute_sha256 {
                            hex::encode(hasher.finalize())
                        } else {
                            String::new()
                        },
                    })),
                }))
                .await;
        });
        Ok(tonic::Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    async fn write_file(
        &self,
        req: tonic::Request<tonic::Streaming<pb::WriteFileRequest>>,
    ) -> Result<tonic::Response<pb::WriteFileResponse>, Status> {
        self.0
            .observed
            .write_file_streams
            .fetch_add(1, Ordering::Relaxed);
        let mut s = req.into_inner();
        let header = match s.message().await?.and_then(|m| m.message) {
            Some(pb::write_file_request::Message::Header(h)) => h,
            _ => return Err(Status::invalid_argument("header first")),
        };
        let mut data = Vec::with_capacity(header.expected_size as usize);
        while let Some(m) = s.next().await {
            if let Some(pb::write_file_request::Message::Data(d)) = m?.message {
                data.extend_from_slice(&d);
            }
        }
        let sha = hex::encode(Sha256::digest(&data));
        if !header.expected_sha256.is_empty() && header.expected_sha256 != sha {
            return Err(fs_err(
                pb::ErrorReason::ChecksumMismatch,
                tonic::Code::DataLoss,
                "sha",
                &[],
            ));
        }
        let size = data.len() as u64;
        // Like the real server: append adds to the file (the digest covers
        // the bytes of this write), create-new refuses an existing file.
        let existing = self.0.files.lock().unwrap().get(&header.path).cloned();
        let data = match (pb::WriteMode::try_from(header.mode), existing) {
            (Ok(pb::WriteMode::Append), Some(mut before)) => {
                before.extend_from_slice(&data);
                before
            }
            (Ok(pb::WriteMode::CreateNew), Some(_)) => {
                return Err(fs_err(
                    pb::ErrorReason::PathExists,
                    tonic::Code::AlreadyExists,
                    "exists",
                    &[],
                ));
            }
            _ => data,
        };
        self.0.put_file(&header.path, data);
        Ok(tonic::Response::new(pb::WriteFileResponse {
            entry: Some(entry(&header.path, size)),
            sha256: sha,
        }))
    }

    async fn begin_upload(
        &self,
        req: tonic::Request<pb::BeginUploadRequest>,
    ) -> Result<tonic::Response<pb::BeginUploadResponse>, Status> {
        let req = req.into_inner();
        let header = req.header.unwrap_or_default();
        let id = if req.upload_id.is_empty() {
            format!("srv-{}", rand::random::<u64>())
        } else {
            req.upload_id
        };
        let mut uploads = self.0.uploads.lock().unwrap();
        let up = uploads.entry(id.clone()).or_insert_with(|| Upload {
            path: header.path.clone(),
            header: header.clone(),
            data: vec![],
        });
        if up.path != header.path {
            *up = Upload {
                path: header.path.clone(),
                header,
                data: vec![],
            };
        }
        Ok(tonic::Response::new(pb::BeginUploadResponse {
            upload_id: id,
            received_bytes: up.data.len() as u64,
            max_chunk_bytes: 4 << 20,
            expires_at: None,
        }))
    }

    async fn upload_chunk(
        &self,
        req: tonic::Request<pb::UploadChunkRequest>,
    ) -> Result<tonic::Response<pb::UploadChunkResponse>, Status> {
        self.0
            .observed
            .upload_chunks
            .fetch_add(1, Ordering::Relaxed);
        let req = req.into_inner();
        let mut uploads = self.0.uploads.lock().unwrap();
        let up = uploads.get_mut(&req.upload_id).ok_or_else(|| {
            fs_err(
                pb::ErrorReason::SessionNotFound,
                tonic::Code::NotFound,
                "no upload",
                &[],
            )
        })?;
        let received = up.data.len() as u64;
        let end = req.offset + req.data.len() as u64;
        let mut duplicate = false;
        if end <= received {
            duplicate = true;
        } else if req.offset != received {
            return Err(fs_err(
                pb::ErrorReason::OffsetMismatch,
                tonic::Code::FailedPrecondition,
                "offset",
                &[("expected_offset", received.to_string())],
            ));
        } else {
            up.data.extend_from_slice(&req.data);
        }
        Ok(tonic::Response::new(pb::UploadChunkResponse {
            received_bytes: up.data.len() as u64,
            duplicate,
            expires_at: None,
        }))
    }

    async fn commit_upload(
        &self,
        req: tonic::Request<pb::CommitUploadRequest>,
    ) -> Result<tonic::Response<pb::CommitUploadResponse>, Status> {
        let req = req.into_inner();
        let up = self
            .0
            .uploads
            .lock()
            .unwrap()
            .remove(&req.upload_id)
            .ok_or_else(|| {
                fs_err(
                    pb::ErrorReason::SessionNotFound,
                    tonic::Code::NotFound,
                    "no upload",
                    &[],
                )
            })?;
        let sha = hex::encode(Sha256::digest(&up.data));
        let expected = if req.sha256.is_empty() {
            up.header.expected_sha256.clone()
        } else {
            req.sha256
        };
        if !expected.is_empty() && expected != sha {
            return Err(fs_err(
                pb::ErrorReason::ChecksumMismatch,
                tonic::Code::DataLoss,
                "sha",
                &[],
            ));
        }
        let size = up.data.len() as u64;
        self.0.put_file(&up.path, up.data);
        Ok(tonic::Response::new(pb::CommitUploadResponse {
            entry: Some(entry(&up.path, size)),
            sha256: sha,
        }))
    }

    async fn abort_upload(
        &self,
        req: tonic::Request<pb::AbortUploadRequest>,
    ) -> Result<tonic::Response<pb::AbortUploadResponse>, Status> {
        self.0
            .uploads
            .lock()
            .unwrap()
            .remove(&req.into_inner().upload_id);
        Ok(tonic::Response::new(pb::AbortUploadResponse {}))
    }

    async fn create_signed_url(
        &self,
        _: tonic::Request<pb::CreateSignedUrlRequest>,
    ) -> Result<tonic::Response<pb::CreateSignedUrlResponse>, Status> {
        Err(Status::unimplemented("mock"))
    }
}

// ----------------------------------------------------------------- computer

/// The 1x1 PNG the mock returns as a screenshot.
pub const MOCK_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xf0,
    0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x89, 0x99, 0x3d, 0x1d, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

#[tonic::async_trait]
impl ComputerService for Svc {
    async fn screenshot(
        &self,
        req: tonic::Request<pb::ScreenshotRequest>,
    ) -> Result<tonic::Response<pb::ScreenshotResponse>, Status> {
        let req = req.into_inner();
        self.0
            .observed
            .screenshots
            .lock()
            .unwrap()
            .push(req.clone());
        // The mock "screen" is 1280x800 logical points at scale 1; the PNG is
        // a 1x1 placeholder, but geometry is reported as a real driver would.
        let mut bounds = match &req.source {
            Some(pb::screenshot_request::Source::Window(w)) => self
                .0
                .windows
                .lock()
                .unwrap()
                .iter()
                .find(|i| i.r#ref.as_ref().map(|r| &r.id) == Some(&w.id))
                .and_then(|i| i.bounds)
                .ok_or_else(|| Status::not_found(format!("no window {}", w.id)))?,
            _ => pb::Rect {
                x: 0.0,
                y: 0.0,
                width: 1280.0,
                height: 800.0,
            },
        };
        if let Some(r) = req.region {
            bounds = pb::Rect {
                x: bounds.x + r.x,
                y: bounds.y + r.y,
                width: r.width,
                height: r.height,
            };
        }
        let long = bounds.width.max(bounds.height);
        let scale = if req.max_dimension > 0 && long > req.max_dimension as f64 {
            req.max_dimension as f64 / long
        } else {
            1.0
        };
        Ok(tonic::Response::new(pb::ScreenshotResponse {
            logical_bounds: Some(bounds),
            image: MOCK_PNG.to_vec(),
            format: pb::ImageFormat::Png as i32,
            image_size: Some(pb::PixelSize {
                width: (bounds.width * scale).round() as u32,
                height: (bounds.height * scale).round() as u32,
            }),
            native_size: Some(pb::PixelSize {
                width: bounds.width as u32,
                height: bounds.height as u32,
            }),
            scale,
            screenshot_id: "shot-1".into(),
            display_id: "primary".into(),
            ..Default::default()
        }))
    }

    async fn pointer(
        &self,
        req: tonic::Request<pb::PointerRequest>,
    ) -> Result<tonic::Response<pb::PointerResponse>, Status> {
        let req = req.into_inner();
        let pos = match &req.action {
            Some(pb::pointer_request::Action::Click(c)) => c.position,
            Some(pb::pointer_request::Action::Move(m)) => m.position,
            Some(pb::pointer_request::Action::Drag(d)) => d.to,
            _ => None,
        };
        if let Some(p) = pos {
            *self.0.cursor.lock().unwrap() = (p.x, p.y);
        }
        self.0
            .observed
            .pointer
            .lock()
            .unwrap()
            .push(format!("{:?}", req.action));
        let (x, y) = *self.0.cursor.lock().unwrap();
        Ok(tonic::Response::new(pb::PointerResponse {
            report: Some(pb::DeliveryReport {
                delivery: pb::Delivery::Foreground as i32,
                pointer_moved: !self.0.pointer_not_moved.load(Ordering::Relaxed),
                ..Default::default()
            }),
            cursor_position: Some(pb::Point { x, y }),
        }))
    }

    async fn keyboard(
        &self,
        req: tonic::Request<pb::KeyboardRequest>,
    ) -> Result<tonic::Response<pb::KeyboardResponse>, Status> {
        self.0
            .observed
            .keyboard
            .lock()
            .unwrap()
            .push(format!("{:?}", req.into_inner().action));
        Ok(tonic::Response::new(pb::KeyboardResponse { report: None }))
    }

    async fn get_clipboard(
        &self,
        _: tonic::Request<pb::GetClipboardRequest>,
    ) -> Result<tonic::Response<pb::GetClipboardResponse>, Status> {
        let (text, generation) = self.0.clipboard.lock().unwrap().clone();
        Ok(tonic::Response::new(pb::GetClipboardResponse {
            content: Some(pb::ClipboardContent {
                text,
                file_paths: vec![],
                image_png: None,
            }),
            generation,
        }))
    }

    async fn set_clipboard(
        &self,
        req: tonic::Request<pb::SetClipboardRequest>,
    ) -> Result<tonic::Response<pb::SetClipboardResponse>, Status> {
        let mut cb = self.0.clipboard.lock().unwrap();
        cb.0 = req.into_inner().content.and_then(|c| c.text);
        cb.1 += 1;
        Ok(tonic::Response::new(pb::SetClipboardResponse {
            generation: cb.1,
        }))
    }

    async fn get_cursor_position(
        &self,
        _: tonic::Request<pb::GetCursorPositionRequest>,
    ) -> Result<tonic::Response<pb::GetCursorPositionResponse>, Status> {
        let (x, y) = *self.0.cursor.lock().unwrap();
        Ok(tonic::Response::new(pb::GetCursorPositionResponse {
            position: Some(pb::Point { x, y }),
            display_id: "primary".into(),
        }))
    }

    async fn list_displays(
        &self,
        _: tonic::Request<pb::ListDisplaysRequest>,
    ) -> Result<tonic::Response<pb::ListDisplaysResponse>, Status> {
        Ok(tonic::Response::new(pb::ListDisplaysResponse {
            displays: vec![pb::Display {
                id: "primary".into(),
                name: "MOCK-0".into(),
                primary: true,
                scale_factor: 1.0,
                native_size: Some(pb::PixelSize {
                    width: 1280,
                    height: 800,
                }),
                ..Default::default()
            }],
        }))
    }
}

impl Svc {
    fn window_mut<T>(
        &self,
        r: &Option<pb::WindowRef>,
        f: impl FnOnce(&mut pb::WindowInfo) -> T,
    ) -> Result<T, Status> {
        let id = r.as_ref().map(|r| r.id.clone()).unwrap_or_default();
        let mut ws = self.0.windows.lock().unwrap();
        let w = ws
            .iter_mut()
            .find(|w| w.r#ref.as_ref().map(|r| r.id.as_str()) == Some(id.as_str()))
            .ok_or_else(|| {
                status_with_reason(
                    tonic::Code::NotFound,
                    pb::ErrorReason::TargetUnavailable,
                    format!("no window {id}"),
                    std::iter::empty(),
                )
            })?;
        Ok(f(w))
    }

    fn log_window(&self, line: String) {
        self.0.observed.windows.lock().unwrap().push(line);
    }
}

#[tonic::async_trait]
impl WindowsService for Svc {
    async fn list_windows(
        &self,
        req: tonic::Request<pb::ListWindowsRequest>,
    ) -> Result<tonic::Response<pb::ListWindowsResponse>, Status> {
        let f = req.into_inner().filter.unwrap_or_default();
        let lower = |s: &str| s.to_lowercase();
        let windows = self
            .0
            .windows
            .lock()
            .unwrap()
            .iter()
            .filter(|w| {
                let app = w.app.as_ref().map(|a| a.name.as_str()).unwrap_or_default();
                (f.title_contains.is_empty() || lower(&w.title).contains(&lower(&f.title_contains)))
                    && (f.app_name_contains.is_empty()
                        || lower(app).contains(&lower(&f.app_name_contains)))
                    && (f.pid == 0 || w.app.as_ref().map(|a| a.pid) == Some(f.pid))
            })
            .cloned()
            .collect();
        Ok(tonic::Response::new(pb::ListWindowsResponse { windows }))
    }

    type WatchWindowsStream = Stream<pb::WatchWindowsResponse>;

    async fn watch_windows(
        &self,
        _: tonic::Request<pb::WatchWindowsRequest>,
    ) -> Result<tonic::Response<Self::WatchWindowsStream>, Status> {
        Err(Status::unimplemented("mock: WatchWindows"))
    }

    async fn get_window(
        &self,
        req: tonic::Request<pb::GetWindowRequest>,
    ) -> Result<tonic::Response<pb::GetWindowResponse>, Status> {
        let w = self.window_mut(&req.into_inner().window, |w| w.clone())?;
        Ok(tonic::Response::new(pb::GetWindowResponse {
            window: Some(w),
        }))
    }

    async fn activate_window(
        &self,
        req: tonic::Request<pb::ActivateWindowRequest>,
    ) -> Result<tonic::Response<pb::ActivateWindowResponse>, Status> {
        let r = req.into_inner().window;
        let id = r.as_ref().map(|r| r.id.clone()).unwrap_or_default();
        self.window_mut(&r, |_| ())?;
        for w in self.0.windows.lock().unwrap().iter_mut() {
            w.focused = w.r#ref.as_ref().map(|r| &r.id) == Some(&id);
        }
        self.log_window(format!("activate {id}"));
        let w = self.window_mut(&r, |w| w.clone())?;
        Ok(tonic::Response::new(pb::ActivateWindowResponse {
            window: Some(w),
        }))
    }

    async fn set_window_bounds(
        &self,
        req: tonic::Request<pb::SetWindowBoundsRequest>,
    ) -> Result<tonic::Response<pb::SetWindowBoundsResponse>, Status> {
        let req = req.into_inner();
        let w = self.window_mut(&req.window, |w| {
            let b = w.bounds.get_or_insert_with(Default::default);
            if let Some(p) = req.position {
                b.x = p.x;
                b.y = p.y;
            }
            if let Some(v) = req.width {
                b.width = v;
            }
            if let Some(v) = req.height {
                b.height = v;
            }
            w.clone()
        })?;
        let b = w.bounds.unwrap_or_default();
        self.log_window(format!(
            "bounds {} {},{},{},{}",
            w.r#ref.as_ref().map(|r| r.id.as_str()).unwrap_or_default(),
            b.x,
            b.y,
            b.width,
            b.height
        ));
        Ok(tonic::Response::new(pb::SetWindowBoundsResponse {
            window: Some(w),
        }))
    }

    async fn minimize_window(
        &self,
        req: tonic::Request<pb::MinimizeWindowRequest>,
    ) -> Result<tonic::Response<pb::MinimizeWindowResponse>, Status> {
        let w = self.window_mut(&req.into_inner().window, |w| {
            w.state = pb::WindowState::Minimized as i32;
            w.clone()
        })?;
        self.log_window(format!(
            "minimize {}",
            w.r#ref.as_ref().map(|r| r.id.as_str()).unwrap_or_default()
        ));
        Ok(tonic::Response::new(pb::MinimizeWindowResponse {
            window: Some(w),
        }))
    }

    async fn maximize_window(
        &self,
        req: tonic::Request<pb::MaximizeWindowRequest>,
    ) -> Result<tonic::Response<pb::MaximizeWindowResponse>, Status> {
        let w = self.window_mut(&req.into_inner().window, |w| {
            w.state = pb::WindowState::Maximized as i32;
            w.clone()
        })?;
        self.log_window(format!(
            "maximize {}",
            w.r#ref.as_ref().map(|r| r.id.as_str()).unwrap_or_default()
        ));
        Ok(tonic::Response::new(pb::MaximizeWindowResponse {
            window: Some(w),
        }))
    }

    async fn restore_window(
        &self,
        req: tonic::Request<pb::RestoreWindowRequest>,
    ) -> Result<tonic::Response<pb::RestoreWindowResponse>, Status> {
        let w = self.window_mut(&req.into_inner().window, |w| {
            w.state = pb::WindowState::Normal as i32;
            w.clone()
        })?;
        Ok(tonic::Response::new(pb::RestoreWindowResponse {
            window: Some(w),
        }))
    }

    async fn close_window(
        &self,
        req: tonic::Request<pb::CloseWindowRequest>,
    ) -> Result<tonic::Response<pb::CloseWindowResponse>, Status> {
        let r = req.into_inner().window;
        self.window_mut(&r, |_| ())?;
        let id = r.map(|r| r.id).unwrap_or_default();
        self.0
            .windows
            .lock()
            .unwrap()
            .retain(|w| w.r#ref.as_ref().map(|r| &r.id) != Some(&id));
        self.log_window(format!("close {id}"));
        Ok(tonic::Response::new(pb::CloseWindowResponse {
            closed: true,
        }))
    }

    async fn launch_app(
        &self,
        req: tonic::Request<pb::LaunchAppRequest>,
    ) -> Result<tonic::Response<pb::LaunchAppResponse>, Status> {
        let req = req.into_inner();
        let app = match req.app.and_then(|a| a.app) {
            Some(pb::app_spec::App::AppId(s))
            | Some(pb::app_spec::App::Executable(s))
            | Some(pb::app_spec::App::Name(s)) => s,
            None => return Err(Status::invalid_argument("app is required")),
        };
        self.log_window(format!("launch {app} {}", req.args.join(" ")));
        let pid = self.0.next_pid.fetch_add(1, Ordering::Relaxed);
        Ok(tonic::Response::new(pb::LaunchAppResponse {
            pid,
            windows: vec![],
        }))
    }

    async fn open(
        &self,
        req: tonic::Request<pb::OpenRequest>,
    ) -> Result<tonic::Response<pb::OpenResponse>, Status> {
        let target = match req.into_inner().target {
            Some(pb::open_request::Target::Url(u)) => u,
            Some(pb::open_request::Target::Path(p)) => p,
            None => return Err(Status::invalid_argument("target is required")),
        };
        self.log_window(format!("open {target}"));
        Ok(tonic::Response::new(pb::OpenResponse { pid: 0 }))
    }
}

fn mock_nodes() -> Vec<pb::AccessibilityNode> {
    let node = |id: &str, parent: &str, depth: u32, role: &str, name: &str, x: f64, y: f64| {
        pb::AccessibilityNode {
            element_id: id.into(),
            parent_id: parent.into(),
            depth,
            role: role.into(),
            name: name.into(),
            bounds: Some(pb::Rect {
                x,
                y,
                width: 80.0,
                height: 24.0,
            }),
            actions: vec![pb::AccessibilityAction::Press as i32],
            ..Default::default()
        }
    };
    vec![
        node("e0", "", 0, "window", "Terminal", 100.0, 50.0),
        node("e1", "e0", 1, "button", "OK", 140.0, 90.0),
        node("e2", "e0", 1, "text_field", "Search", 240.0, 90.0),
    ]
}

#[tonic::async_trait]
impl AccessibilityService for Svc {
    async fn get_tree(
        &self,
        req: tonic::Request<pb::GetTreeRequest>,
    ) -> Result<tonic::Response<pb::GetTreeResponse>, Status> {
        let req = req.into_inner();
        let mut nodes = mock_nodes();
        if req.max_depth > 0 {
            nodes.retain(|n| n.depth <= req.max_depth);
        }
        Ok(tonic::Response::new(pb::GetTreeResponse {
            snapshot_id: "ax-1".into(),
            window: req.window,
            nodes,
            truncated: false,
        }))
    }

    async fn find(
        &self,
        req: tonic::Request<pb::FindRequest>,
    ) -> Result<tonic::Response<pb::FindResponse>, Status> {
        let q = req.into_inner().query.unwrap_or_default();
        let nodes = mock_nodes()
            .into_iter()
            .filter(|n| {
                (q.role.is_empty() || n.role == q.role)
                    && (q.name.is_empty() || n.name == q.name)
                    && (q.name_contains.is_empty()
                        || n.name
                            .to_lowercase()
                            .contains(&q.name_contains.to_lowercase()))
            })
            .collect();
        Ok(tonic::Response::new(pb::FindResponse {
            snapshot_id: "ax-1".into(),
            nodes,
        }))
    }

    async fn act(
        &self,
        req: tonic::Request<pb::ActRequest>,
    ) -> Result<tonic::Response<pb::ActResponse>, Status> {
        let req = req.into_inner();
        let el = req.element.unwrap_or_default();
        if !mock_nodes().iter().any(|n| n.element_id == el.element_id) {
            return Err(Status::not_found(format!("no element {}", el.element_id)));
        }
        let action = pb::AccessibilityAction::try_from(req.action)
            .map(|a| a.as_str_name().to_string())
            .unwrap_or_default();
        self.0
            .observed
            .accessibility
            .lock()
            .unwrap()
            .push(format!("act {} {action} {}", el.element_id, req.value));
        Ok(tonic::Response::new(pb::ActResponse { report: None }))
    }
}

#[tonic::async_trait]
impl StreamService for Svc {
    async fn list_targets(
        &self,
        req: tonic::Request<pb::ListTargetsRequest>,
    ) -> Result<tonic::Response<pb::ListTargetsResponse>, Status> {
        // The mock windows as window targets (when asked for), all streamable.
        let targets = if req.into_inner().include_windows {
            self.0
                .windows()
                .into_iter()
                .map(|w| pb::StreamTarget {
                    target: Some(pb::stream_target::Target::Window(w)),
                    available: true,
                    ..Default::default()
                })
                .collect()
        } else {
            vec![]
        };
        Ok(tonic::Response::new(pb::ListTargetsResponse {
            targets,
            ..Default::default()
        }))
    }

    async fn open_media(
        &self,
        req: tonic::Request<pb::OpenMediaRequest>,
    ) -> Result<tonic::Response<pb::OpenMediaResponse>, Status> {
        let req = req.into_inner();
        Ok(tonic::Response::new(pb::OpenMediaResponse {
            media_session_id: "media-1".into(),
            ticket: "ticket-abc".into(),
            ws_path: "/media?ticket=ticket-abc".into(),
            codec: pb::MediaCodec::H264 as i32,
            max_fps: if req.max_fps == 0 { 30 } else { req.max_fps },
            wire_version: cua_proto::MEDIA_WIRE_VERSION,
            ..Default::default()
        }))
    }

    async fn set_preferences(
        &self,
        _: tonic::Request<pb::SetPreferencesRequest>,
    ) -> Result<tonic::Response<pb::SetPreferencesResponse>, Status> {
        Ok(tonic::Response::new(pb::SetPreferencesResponse::default()))
    }

    async fn request_keyframe(
        &self,
        _: tonic::Request<pb::RequestKeyframeRequest>,
    ) -> Result<tonic::Response<pb::RequestKeyframeResponse>, Status> {
        Ok(tonic::Response::new(pb::RequestKeyframeResponse {}))
    }

    async fn close_media(
        &self,
        _: tonic::Request<pb::CloseMediaRequest>,
    ) -> Result<tonic::Response<pb::CloseMediaResponse>, Status> {
        Ok(tonic::Response::new(pb::CloseMediaResponse {}))
    }
}

impl MockState {
    /// Sends a presence event (for example `CursorShapeChanged`,
    /// `RosterHeartbeat` or a `ParticipantLeft` with a reason) to every
    /// joined participant, as the server would.
    pub fn presence_inject(&self, event: pb::join_response::Event) {
        self.presence_broadcast(event);
    }

    /// Sends `event` to every joined participant (bounded channels; a full or
    /// closed one is dropped).
    fn presence_broadcast(&self, event: pb::join_response::Event) {
        self.presence.lock().unwrap().retain(|(_, tx)| {
            tx.try_send(Ok(pb::JoinResponse {
                event: Some(event.clone()),
            }))
            .is_ok()
        });
    }
}

/// A minimal `PresenceService`: `Join` answers `joined` with a fresh
/// participant id, `UpdateCursor` broadcasts `cursor_moved`, `Leave`
/// broadcasts `participant_left`. The feature is only advertised after
/// `MockState::advertise(&["presence"])`.
#[tonic::async_trait]
impl PresenceService for Svc {
    type JoinStream = Stream<pb::JoinResponse>;

    async fn join(
        &self,
        req: tonic::Request<pb::JoinRequest>,
    ) -> Result<tonic::Response<Self::JoinStream>, Status> {
        let principal = req.into_inner().principal;
        let id = format!(
            "p-{}",
            self.0.next_participant.fetch_add(1, Ordering::SeqCst) + 1
        );
        let participant = pb::Participant {
            participant_id: id.clone(),
            principal,
            ..Default::default()
        };
        self.0
            .presence_broadcast(pb::join_response::Event::ParticipantJoined(
                participant.clone(),
            ));
        let (tx, rx) = mpsc::channel(64);
        let _ = tx.try_send(Ok(pb::JoinResponse {
            event: Some(pb::join_response::Event::Joined(pb::PresenceJoined {
                participant: Some(participant),
                roster: vec![],
                datagrams: None,
            })),
        }));
        self.0.presence.lock().unwrap().push((id, tx));
        Ok(tonic::Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    async fn update_cursor(
        &self,
        req: tonic::Request<pb::UpdateCursorRequest>,
    ) -> Result<tonic::Response<pb::UpdateCursorResponse>, Status> {
        let r = req.into_inner();
        self.0
            .presence_broadcast(pb::join_response::Event::CursorMoved(pb::CursorMoved {
                participant_id: r.participant_id,
                cursor: r.cursor,
                at: Some({
                    let d = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default();
                    pbjson_types::Timestamp {
                        seconds: d.as_secs() as i64,
                        nanos: d.subsec_nanos() as i32,
                    }
                }),
            }));
        Ok(tonic::Response::new(pb::UpdateCursorResponse {}))
    }

    async fn leave(
        &self,
        req: tonic::Request<pb::LeaveRequest>,
    ) -> Result<tonic::Response<pb::LeaveResponse>, Status> {
        let id = req.into_inner().participant_id;
        self.0.presence.lock().unwrap().retain(|(p, _)| *p != id);
        self.0
            .presence_broadcast(pb::join_response::Event::ParticipantLeft(
                pb::ParticipantLeft {
                    participant_id: id,
                    reason: pb::LeaveReason::Left as i32,
                },
            ));
        Ok(tonic::Response::new(pb::LeaveResponse {}))
    }
}

#[tonic::async_trait]
impl TeleportService for Svc {
    async fn get_manifest(
        &self,
        req: tonic::Request<pb::GetManifestRequest>,
    ) -> Result<tonic::Response<pb::GetManifestResponse>, Status> {
        let app = req.into_inner().app;
        let unsupported = self.0.teleport_unsupported.lock().unwrap().contains(&app);
        Ok(tonic::Response::new(pb::GetManifestResponse {
            supported: !unsupported && !app.is_empty(),
            limitation: if unsupported {
                format!("mock: no provider for {app:?}")
            } else {
                String::new()
            },
            bundle_version: 1,
            ..Default::default()
        }))
    }

    async fn import_session(
        &self,
        req: tonic::Request<pb::ImportSessionRequest>,
    ) -> Result<tonic::Response<pb::ImportSessionResponse>, Status> {
        let req = req.into_inner();
        let mut uploads = self.0.teleport_uploads.lock().unwrap();
        if req.offset == 0 {
            uploads
                .entry(req.import_id.clone())
                .or_insert_with(|| (req.app.clone(), Vec::new(), 0));
        }
        let Some((_, data, chunks)) = uploads.get_mut(&req.import_id) else {
            return Err(status_with_reason(
                tonic::Code::NotFound,
                pb::ErrorReason::SessionNotFound,
                "no import",
                std::iter::empty(),
            ));
        };
        let received = data.len() as u64;
        let end = req.offset + req.data.len() as u64;
        let duplicate = if req.offset == received {
            data.extend_from_slice(&req.data);
            *chunks += 1;
            false
        } else if end <= received {
            true
        } else {
            return Err(status_with_reason(
                tonic::Code::FailedPrecondition,
                pb::ErrorReason::OffsetMismatch,
                "offset",
                [("expected_offset".to_string(), received.to_string())],
            ));
        };
        let received = data.len() as u64;
        if !req.commit {
            return Ok(tonic::Response::new(pb::ImportSessionResponse {
                received_bytes: received,
                duplicate,
                result: None,
            }));
        }
        let digest = hex::encode(Sha256::digest(&data[..]));
        if !digest.eq_ignore_ascii_case(&req.sha256) {
            uploads.remove(&req.import_id);
            return Err(status_with_reason(
                tonic::Code::FailedPrecondition,
                pb::ErrorReason::ChecksumMismatch,
                format!("bundle sha256 {digest} does not match {}", req.sha256),
                std::iter::empty(),
            ));
        }
        let (app, bundle, chunks) = uploads.remove(&req.import_id).unwrap_or_default();
        let options = req.options.unwrap_or_default();
        self.0
            .teleport_imports
            .lock()
            .unwrap()
            .push(TeleportImport {
                import_id: req.import_id.clone(),
                app: app.clone(),
                bundle,
                chunks,
                options: options.clone(),
            });
        Ok(tonic::Response::new(pb::ImportSessionResponse {
            received_bytes: received,
            duplicate,
            result: Some(pb::ImportResult {
                imported: vec![app],
                skipped: vec![],
                launched: options.launch_after,
            }),
        }))
    }

    async fn wipe_import(
        &self,
        req: tonic::Request<pb::WipeImportRequest>,
    ) -> Result<tonic::Response<pb::WipeImportResponse>, Status> {
        let req = req.into_inner();
        self.0.teleport_wipes.lock().unwrap().push(req.clone());
        let mut imports = self.0.teleport_imports.lock().unwrap();
        let wiped: Vec<String> = if req.all {
            imports.drain(..).map(|i| i.import_id).collect()
        } else if imports.iter().any(|i| i.import_id == req.import_id) {
            imports.retain(|i| i.import_id != req.import_id);
            vec![req.import_id]
        } else {
            return Err(status_with_reason(
                tonic::Code::NotFound,
                pb::ErrorReason::SessionNotFound,
                format!("unknown or expired import {:?}", req.import_id),
                std::iter::empty(),
            ));
        };
        let mut wiped = wiped;
        wiped.dedup();
        Ok(tonic::Response::new(pb::WipeImportResponse {
            wiped_import_ids: wiped,
            removed_paths: vec![],
            keychain_items_removed: 0,
            cookie_rows_removed: 0,
        }))
    }

    async fn begin_receive_files(
        &self,
        _: tonic::Request<pb::BeginReceiveFilesRequest>,
    ) -> Result<tonic::Response<pb::BeginReceiveFilesResponse>, Status> {
        Err(Status::unimplemented("mock: file transfers"))
    }

    async fn receive_files_chunk(
        &self,
        _: tonic::Request<pb::ReceiveFilesChunkRequest>,
    ) -> Result<tonic::Response<pb::ReceiveFilesChunkResponse>, Status> {
        Err(Status::unimplemented("mock: file transfers"))
    }

    async fn commit_receive_files(
        &self,
        _: tonic::Request<pb::CommitReceiveFilesRequest>,
    ) -> Result<tonic::Response<pb::CommitReceiveFilesResponse>, Status> {
        Err(Status::unimplemented("mock: file transfers"))
    }

    async fn abort_receive_files(
        &self,
        _: tonic::Request<pb::AbortReceiveFilesRequest>,
    ) -> Result<tonic::Response<pb::AbortReceiveFilesResponse>, Status> {
        Err(Status::unimplemented("mock: file transfers"))
    }
}

// ------------------------------------------------------------------ Tunnel

#[tonic::async_trait]
impl TunnelService for Svc {
    async fn forward(
        &self,
        request: tonic::Request<pb::ForwardRequest>,
    ) -> Result<tonic::Response<pb::ForwardResponse>, Status> {
        let body = request.into_inner();
        let port = u16::try_from(body.port)
            .ok()
            .filter(|p| *p != 0)
            .ok_or_else(|| Status::invalid_argument("port must be 1-65535"))?;
        // The mock only ever relays to this host's loopback (a test server).
        if !(body.host.is_empty() || body.host == "127.0.0.1" || body.host == "localhost") {
            return Err(Status::permission_denied("mock forwards to loopback only"));
        }
        let n = self.0.next_forward.fetch_add(1, Ordering::SeqCst) + 1;
        let id = format!("fwd-{n}");
        let ticket = format!("tkt-{n}-{:08x}", rand::random::<u32>());
        let ttl = dur(&body.ttl, Duration::from_secs(600));
        let expires = std::time::SystemTime::now() + ttl;
        let since = expires
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        self.0.forwards.lock().unwrap().insert(
            ticket.clone(),
            MockForward {
                id: id.clone(),
                port,
                closed: Arc::new(tokio::sync::watch::channel(false).0),
            },
        );
        Ok(tonic::Response::new(pb::ForwardResponse {
            forward_id: id,
            ws_path: format!("{}?ticket={ticket}", cua_proto::metadata::TUNNEL_WS_PATH),
            ticket,
            expires_at: Some(pbjson_types::Timestamp {
                seconds: since.as_secs() as i64,
                nanos: since.subsec_nanos() as i32,
            }),
        }))
    }

    async fn list_forwards(
        &self,
        _: tonic::Request<pb::ListForwardsRequest>,
    ) -> Result<tonic::Response<pb::ListForwardsResponse>, Status> {
        let forwards = self
            .0
            .forwards
            .lock()
            .unwrap()
            .values()
            .map(|f| pb::ForwardInfo {
                forward_id: f.id.clone(),
                host: "127.0.0.1".into(),
                port: f.port as u32,
                ..Default::default()
            })
            .collect();
        Ok(tonic::Response::new(pb::ListForwardsResponse { forwards }))
    }

    async fn close_forward(
        &self,
        request: tonic::Request<pb::CloseForwardRequest>,
    ) -> Result<tonic::Response<pb::CloseForwardResponse>, Status> {
        let id = request.into_inner().forward_id;
        let mut forwards = self.0.forwards.lock().unwrap();
        let ticket = forwards
            .iter()
            .find(|(_, f)| f.id == id)
            .map(|(t, _)| t.clone())
            .ok_or_else(|| Status::not_found(format!("forward {id}")))?;
        if let Some(f) = forwards.remove(&ticket) {
            let _ = f.closed.send(true);
        }
        Ok(tonic::Response::new(pb::CloseForwardResponse {}))
    }

    async fn start_hotspot(
        &self,
        _: tonic::Request<pb::StartHotspotRequest>,
    ) -> Result<tonic::Response<pb::StartHotspotResponse>, Status> {
        Err(Status::unimplemented("mock: hotspot"))
    }

    async fn stop_hotspot(
        &self,
        _: tonic::Request<pb::StopHotspotRequest>,
    ) -> Result<tonic::Response<pb::StopHotspotResponse>, Status> {
        Err(Status::unimplemented("mock: hotspot"))
    }

    async fn get_hotspot_status(
        &self,
        _: tonic::Request<pb::GetHotspotStatusRequest>,
    ) -> Result<tonic::Response<pb::GetHotspotStatusResponse>, Status> {
        Err(Status::unimplemented("mock: hotspot"))
    }
}

/// Whether a fresh connection is a `GET …/tunnel` request (peeked, bounded).
async fn is_tunnel_upgrade(stream: &TcpStream) -> bool {
    let mut buf = [0u8; 1024];
    for _ in 0..50 {
        match stream.peek(&mut buf).await {
            Ok(n) if n >= 4 => {
                let head = &buf[..n];
                if !head.starts_with(b"GET ") {
                    return false;
                }
                if let Some(end) = head.windows(2).position(|w| w == b"\r\n") {
                    let line = String::from_utf8_lossy(&head[..end]);
                    let path = line.split(' ').nth(1).unwrap_or_default();
                    let path = path.split('?').next().unwrap_or_default();
                    return path.ends_with(cua_proto::metadata::TUNNEL_WS_PATH);
                }
                if n == buf.len() {
                    return false;
                }
            }
            Ok(0) | Err(_) => return false,
            Ok(_) => {}
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

/// Serves one `/tunnel` WebSocket like the driver: gateway checks, ticket,
/// then a byte splice to the forward's loopback port.
async fn serve_tunnel(stream: TcpStream, state: Arc<MockState>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_tungstenite::tungstenite::{
        Message,
        handshake::server::{ErrorResponse, Request as WsRequest, Response as WsResponse},
    };
    let found: Arc<Mutex<Option<MockForward>>> = Arc::default();
    let slot = found.clone();
    let cb_state = state.clone();
    #[allow(clippy::result_large_err)] // tungstenite's callback signature
    let callback = move |req: &WsRequest, resp: WsResponse| -> Result<WsResponse, ErrorResponse> {
        let header = |k: &str| {
            req.headers()
                .get(k)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        let full = req
            .uri()
            .path_and_query()
            .map(|p| p.as_str().to_string())
            .unwrap_or_default();
        let mut attach = TunnelAttach {
            path: full.clone(),
            authorization: header("authorization"),
            claim: header(crate::transport::FLEET_CLAIM_HEADER),
            env_authorization: header(crate::transport::ENV_TOKEN_HEADER),
            accepted: false,
        };
        let refuse = |status: u16, msg: &str| {
            let mut r = ErrorResponse::new(Some(msg.to_string()));
            *r.status_mut() = http::StatusCode::from_u16(status).unwrap();
            r
        };
        let verdict = (|| {
            let prefix = cb_state
                .auth
                .gateway
                .as_ref()
                .map(|g| g.prefix.clone())
                .or_else(|| cb_state.auth.prefix.clone())
                .unwrap_or_default();
            let path = req.uri().path();
            if path != format!("{prefix}{}", cua_proto::metadata::TUNNEL_WS_PATH) {
                return Err(refuse(404, "no route"));
            }
            if let Some(gw) = &cb_state.auth.gateway {
                if attach.authorization.as_deref() != Some(&format!("Bearer {}", gw.bearer)) {
                    return Err(refuse(401, "gateway bearer"));
                }
                if attach.claim.as_deref() != Some(gw.claim.as_str()) {
                    return Err(refuse(403, "claim header"));
                }
            }
            let ticket = req
                .uri()
                .query()
                .unwrap_or_default()
                .split('&')
                .find_map(|kv| kv.strip_prefix("ticket="))
                .unwrap_or_default();
            let forward = cb_state.forwards.lock().unwrap().get(ticket).cloned();
            let Some(forward) = forward else {
                return Err(refuse(401, "bad ticket"));
            };
            *slot.lock().unwrap() = Some(forward);
            Ok(())
        })();
        attach.accepted = verdict.is_ok();
        cb_state.tunnel_attaches.lock().unwrap().push(attach);
        verdict.map(|()| resp)
    };
    let Ok(ws) = tokio_tungstenite::accept_hdr_async(stream, callback).await else {
        return;
    };
    let Some(forward) = found.lock().unwrap().take() else {
        return;
    };
    let Ok(target) = TcpStream::connect(("127.0.0.1", forward.port)).await else {
        return;
    };
    let mut closed = forward.closed.subscribe();
    let (mut tcp_rx, mut tcp_tx) = target.into_split();
    let (mut ws_tx, mut ws_rx) = ws.split();
    let up = async {
        while let Some(Ok(message)) = ws_rx.next().await {
            match message {
                Message::Binary(data) => {
                    if tcp_tx.write_all(&data).await.is_err() {
                        break;
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
        let _ = tcp_tx.shutdown().await;
    };
    let down = async {
        use futures_util::SinkExt;
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match tcp_rx.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let data = bytes::Bytes::copy_from_slice(&buf[..n]);
                    if ws_tx.send(Message::Binary(data)).await.is_err() {
                        break;
                    }
                }
            }
        }
        let _ = ws_tx.send(Message::Close(None)).await;
    };
    tokio::select! {
        _ = async { tokio::join!(up, down) } => {}
        _ = closed.wait_for(|c| *c) => {}
    }
}

// ----------------------------------------------------------------- CutProxy

/// A loopback TCP proxy that can sever all live connections once a byte
/// budget (client→server) is spent, to simulate a network drop.
pub struct CutProxy {
    /// Proxy address.
    pub addr: SocketAddr,
    budget: Arc<AtomicU64>,
    armed: Arc<AtomicBool>,
    /// Number of cuts performed.
    pub cuts: Arc<AtomicU64>,
    kill: Arc<Notify>,
}

impl CutProxy {
    /// Proxies to `upstream`.
    pub async fn start(upstream: SocketAddr) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let budget = Arc::new(AtomicU64::new(0));
        let armed = Arc::new(AtomicBool::new(false));
        let cuts = Arc::new(AtomicU64::new(0));
        let kill = Arc::new(Notify::new());
        let (b, a, c, k) = (budget.clone(), armed.clone(), cuts.clone(), kill.clone());
        tokio::spawn(async move {
            loop {
                let Ok((client, _)) = listener.accept().await else {
                    return;
                };
                let (b, a, c, k) = (b.clone(), a.clone(), c.clone(), k.clone());
                tokio::spawn(async move {
                    let Ok(server) = TcpStream::connect(upstream).await else {
                        return;
                    };
                    let _ = client.set_nodelay(true);
                    let _ = server.set_nodelay(true);
                    let (mut cr, mut cw) = client.into_split();
                    let (mut sr, mut sw) = server.into_split();
                    let up = async {
                        use tokio::io::{AsyncReadExt, AsyncWriteExt};
                        let mut buf = vec![0u8; 64 * 1024];
                        loop {
                            let n = cr.read(&mut buf).await?;
                            if n == 0 {
                                return Ok::<_, std::io::Error>(());
                            }
                            if a.load(Ordering::SeqCst) {
                                let left = b.fetch_sub(n as u64, Ordering::SeqCst);
                                if left <= n as u64 && a.swap(false, Ordering::SeqCst) {
                                    c.fetch_add(1, Ordering::SeqCst);
                                    k.notify_waiters();
                                    return Ok(());
                                }
                            }
                            sw.write_all(&buf[..n]).await?;
                        }
                    };
                    let down = tokio::io::copy(&mut sr, &mut cw);
                    tokio::select! {
                        _ = up => {}
                        _ = down => {}
                        _ = k.notified() => {}
                    }
                });
            }
        });
        Self {
            addr,
            budget,
            armed,
            cuts,
            kill,
        }
    }

    /// Severs every connection once `bytes` more client→server bytes pass.
    pub fn cut_after(&self, bytes: u64) {
        self.budget.store(bytes, Ordering::SeqCst);
        self.armed.store(true, Ordering::SeqCst);
    }

    /// Severs every live connection now.
    pub fn cut_now(&self) {
        self.cuts.fetch_add(1, Ordering::SeqCst);
        self.kill.notify_waiters();
    }

    /// `http://127.0.0.1:<port>`.
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }
}
