//! The daemon: `cua.daemon.v1` + env passthrough + media bridge, served on a
//! Unix socket (no token; the socket is mode 0600) and/or loopback TCP
//! (bearer token). Both listeners serve native gRPC (h2c) and gRPC-Web.

use crate::{
    CreateRequest, Discovery, Error, Result, Runtime, VERSION,
    convert::{self, from_dur, info_to_pb, probe_from_pb, provider_from_pb},
    passthrough,
};
use axum::body::Body as AxumBody;
use cua_proto::daemon::v1::{
    self as pb, daemon_service_server::*, runtime_service_server::*, sandbox_service_server::*,
    space_service_server::*,
};
use cua_proto::env::v1 as envpb;
use http::{Request, Response, StatusCode};
use std::{
    collections::HashMap,
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};
use tokio::sync::watch;
use tonic::Status;
use tower::{Service, ServiceExt};

/// Where and how to listen.
#[derive(Clone, Debug)]
pub struct ServerConfig {
    /// Unix socket (unix only). Created with mode 0600; a stale socket file
    /// is replaced.
    pub socket_path: Option<PathBuf>,
    /// Loopback TCP address (for example `127.0.0.1:0`). Needs `token`.
    pub loopback: Option<SocketAddr>,
    /// Bearer token of the loopback listener.
    pub token: String,
    /// Discovery file to write (and remove on shutdown).
    pub discovery_path: Option<PathBuf>,
    /// Lifetime of media bridge tickets.
    pub bridge_ticket_ttl: Duration,
}

impl ServerConfig {
    /// Defaults: `~/.cua/cua.sock` (unix), loopback `127.0.0.1:0`, a random
    /// token, `~/.cua/daemon.json`. The Spaces registry lives where the
    /// runtime's `spaces_home` says (default `~/.cua`).
    pub fn default_paths() -> Self {
        Self {
            socket_path: cfg!(unix).then(crate::default_socket_path),
            loopback: Some(SocketAddr::from(([127, 0, 0, 1], 0))),
            token: crate::random_token(),
            discovery_path: Some(crate::default_discovery_path()),
            bridge_ticket_ttl: Duration::from_secs(60),
        }
    }
}

/// A media bridge registered by `OpenMediaBridge`.
#[derive(Clone, Debug)]
pub(crate) struct Bridge {
    pub upstream_url: String,
    pub headers: Vec<(String, String)>,
    pub expires: SystemTime,
}

pub(crate) struct Shared {
    pub runtime: Runtime,
    pub token: String,
    pub info: Mutex<pb::GetInfoResponse>,
    pub bridges: Mutex<HashMap<String, Bridge>>,
    pub bridge_ttl: Duration,
    pub shutdown: watch::Sender<bool>,
    /// Requests being served, and the last time one started (unix secs):
    /// maintenance runs only when the daemon is idle.
    pub inflight: std::sync::atomic::AtomicUsize,
    pub last_activity: std::sync::atomic::AtomicU64,
    /// Spaces this machine provides to its owner's devices
    /// (`cua.env.v1.HostSpacesService`, reached through the host's
    /// cua-spacesd).
    #[cfg(feature = "spaces")]
    pub host_spaces: Arc<cua_spaces::host_spaces::HostSpacesServer>,
}

/// Counts a request as in flight until dropped.
struct Busy(Arc<Shared>);

impl Busy {
    fn new(s: Arc<Shared>) -> Self {
        use std::sync::atomic::Ordering::SeqCst;
        s.inflight.fetch_add(1, SeqCst);
        s.last_activity.store(unix_now(), SeqCst);
        Self(s)
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering::SeqCst;
        self.0.inflight.fetch_sub(1, SeqCst);
        self.0.last_activity.store(unix_now(), SeqCst);
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// First maintenance pass after start.
const MAINTENANCE_FIRST: Duration = Duration::from_secs(60);
/// Idle time before a maintenance pass may run.
const MAINTENANCE_IDLE: Duration = Duration::from_secs(300);
/// Spacing of maintenance passes.
const MAINTENANCE_EVERY: Duration = Duration::from_secs(3600);

/// Reaps orphaned ephemeral sandboxes, collects the cache and caps logs:
/// once a minute after start, then hourly while no request is in flight
/// and none arrived for five minutes. `CUA_DAEMON_MAINTENANCE=0` turns it
/// off.
async fn maintenance_loop(shared: Arc<Shared>, mut shutdown: watch::Receiver<bool>) {
    use std::sync::atomic::Ordering::SeqCst;
    let Ok(vmm) = shared.runtime.vmm().cloned() else {
        return;
    };
    let state = shared.runtime.sandboxes().state().clone();
    let mut next = tokio::time::Instant::now() + MAINTENANCE_FIRST;
    let mut ran = false;
    loop {
        tokio::select! {
            _ = shutdown.changed() => return,
            _ = tokio::time::sleep_until(next) => {}
        }
        let idle_for = unix_now().saturating_sub(shared.last_activity.load(SeqCst));
        // The first pass (orphans of a crash before this daemon started)
        // only waits for in-flight requests; later ones for five idle
        // minutes.
        if shared.inflight.load(SeqCst) > 0 || (ran && idle_for < MAINTENANCE_IDLE.as_secs()) {
            next = tokio::time::Instant::now() + Duration::from_secs(60);
            continue;
        }
        let config = cua_disk::CacheConfig::load();
        let gc = config.auto_gc.then(|| cua_disk::GcOptions {
            grace: cua_disk::gc::AUTO_GRACE,
            budget: (config.budget == cua_disk::Budget::Off).then_some(cua_disk::Budget::Off),
            ..Default::default()
        });
        let r =
            crate::maintenance::run(vmm.as_ref(), &state, cua_disk::Layout::default(), gc, false)
                .await;
        if !r.reaped.is_empty() || r.gc.as_ref().is_some_and(|g| g.freed > 0) {
            tracing::info!(
                reaped = r.reaped.len(),
                freed = r.gc.as_ref().map(|g| g.freed).unwrap_or(0),
                "daemon maintenance"
            );
        }
        ran = true;
        next = tokio::time::Instant::now() + MAINTENANCE_EVERY;
    }
}

/// Set to `0` to keep this daemon from supervising persistent agents.
pub const ENV_DAEMON_PERSISTENT: &str = "CUA_DAEMON_PERSISTENT";

/// How often the persistent-agent supervisor runs (routine wake latency is
/// at most this).
#[cfg(feature = "spaces")]
pub const PERSISTENT_TICK: Duration = Duration::from_secs(1);

/// A running daemon.
pub struct DaemonHandle {
    /// Loopback URL, when listening on TCP.
    pub loopback_url: Option<String>,
    /// Unix socket, when listening on one.
    pub socket_path: Option<PathBuf>,
    /// Loopback token.
    pub token: String,
    shared: Arc<Shared>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    discovery_path: Option<PathBuf>,
}

impl DaemonHandle {
    /// A handle that requests shutdown when sent `true` (signal handlers).
    pub fn shutdown_trigger(&self) -> watch::Sender<bool> {
        self.shared.shutdown.clone()
    }

    /// Requests shutdown.
    pub fn shutdown(&self) {
        let _ = self.shared.shutdown.send(true);
    }

    /// Waits until shutdown was requested and the listeners stopped.
    pub async fn wait(mut self) {
        let mut rx = self.shared.shutdown.subscribe();
        while !*rx.borrow() {
            if rx.changed().await.is_err() {
                break;
            }
        }
        // What an extension serves must not outlive the daemon (a mount,
        // writes not yet stored): each ends first, bounded.
        for a in self.shared.runtime.attached_extensions() {
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(60),
                a.shutdown(&self.shared.runtime),
            )
            .await;
        }
        for t in self.tasks.drain(..) {
            t.abort();
            let _ = t.await;
        }
        self.cleanup();
    }

    fn cleanup(&self) {
        if let Some(p) = &self.socket_path {
            let _ = std::fs::remove_file(p);
        }
        if let Some(p) = &self.discovery_path
            && Discovery::read(p).is_some_and(|d| d.pid == std::process::id())
        {
            let _ = std::fs::remove_file(p);
        }
    }
}

impl Drop for DaemonHandle {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

/// Starts the daemon listeners and returns once they accept connections.
pub async fn start(runtime: Runtime, config: ServerConfig) -> Result<DaemonHandle> {
    cua_spacesd_client::transport::ensure_crypto_provider();
    if config.socket_path.is_none() && config.loopback.is_none() {
        return Err(Error::InvalidArgument(
            "the daemon needs a socket path or a loopback address".into(),
        ));
    }
    // A discovery file of another daemon that still accepts connections
    // wins; one left by a daemon that exited is replaced below (like a
    // stale socket).
    if let Some(p) = &config.discovery_path
        && let Some(d) = Discovery::read(p)
        && d.pid != std::process::id()
        && crate::pid_alive(d.pid)
        && d.listening()
    {
        return Err(Error::InvalidArgument(format!(
            "a daemon is already running (pid {}, {})",
            d.pid,
            p.display()
        )));
    }
    let (tx, _) = watch::channel(false);
    // The daemon hosts local public URLs itself.
    runtime.mark_share_host();
    let features = {
        let mut f = vec!["env".into(), "fleet".into(), "sandboxes".into()];
        if cfg!(feature = "spaces") {
            f.extend(["spaces".into(), "mcp".into()]);
        }
        // Its extensions, as `extension:<name>` (the Cua Spaces build
        // reports `extension:cua-spaces`).
        #[cfg(feature = "spaces")]
        f.extend(
            runtime
                .extension_names()
                .iter()
                .map(|n| format!("{}{n}", crate::extension::FEATURE_PREFIX)),
        );
        f
    };
    #[cfg(feature = "spaces")]
    let host_spaces = cua_spaces::host_spaces::HostSpacesServer::new(runtime.spaces().clone());
    // Running Spaces' thumbnails stay fresh while a client asks for them
    // (the Spaces apps' notch and previews, SDK scripts); idle otherwise.
    #[cfg(feature = "spaces")]
    let _thumbnails = runtime.spaces().spawn_thumbnail_refresh();
    let own = std::env::current_exe()
        .ok()
        .and_then(|e| crate::identity::of(&e));
    let shared = Arc::new(Shared {
        #[cfg(feature = "spaces")]
        host_spaces,
        runtime,
        token: config.token.clone(),
        info: Mutex::new(pb::GetInfoResponse {
            version: VERSION.into(),
            pid: std::process::id(),
            socket_path: String::new(),
            loopback_url: String::new(),
            loopback_token: config.token.clone(),
            features,
            started_at: Some(convert::ts(SystemTime::now())),
            // Taken now, at start: a later rebuild of the file must read
            // as a different build.
            executable: own
                .as_ref()
                .map(|i| i.executable.clone())
                .unwrap_or_default(),
            build_id: own.map(|i| i.build_id).unwrap_or_default(),
        }),
        bridges: Mutex::new(HashMap::new()),
        bridge_ttl: config.bridge_ticket_ttl,
        shutdown: tx,
        inflight: Default::default(),
        last_activity: std::sync::atomic::AtomicU64::new(unix_now()),
    });

    let mut tasks = Vec::new();
    let mut loopback_url = None;
    if let Some(addr) = config.loopback {
        if !addr.ip().is_loopback() {
            return Err(Error::InvalidArgument(format!(
                "refusing to listen on non-loopback address {addr}"
            )));
        }
        let listener = tokio::net::TcpListener::bind(addr).await?;
        let local = listener.local_addr()?;
        let url = format!("http://{local}");
        shared.info.lock().unwrap().loopback_url = url.clone();
        loopback_url = Some(url);
        let app = app(shared.clone(), true);
        let rx = shared.shutdown.subscribe();
        tasks.push(tokio::spawn(accept_loop(listener, app, rx)));
    }
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut socket_path: Option<PathBuf> = None;
    #[cfg(unix)]
    if let Some(path) = &config.socket_path {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        if path.exists() {
            if tokio::net::UnixStream::connect(path).await.is_ok() {
                return Err(Error::InvalidArgument(format!(
                    "a daemon is already listening on {}",
                    path.display()
                )));
            }
            std::fs::remove_file(path)?;
        }
        let listener = tokio::net::UnixListener::bind(path)?;
        crate::restrict_permissions(path)?;
        shared.info.lock().unwrap().socket_path = path.display().to_string();
        socket_path = Some(path.clone());
        let app = app(shared.clone(), false);
        let rx = shared.shutdown.subscribe();
        tasks.push(tokio::spawn(accept_loop(listener, app, rx)));
    }
    #[cfg(not(unix))]
    if config.socket_path.is_some() && config.loopback.is_none() {
        return Err(Error::Unsupported(
            "Unix sockets are not available on this platform; use loopback".into(),
        ));
    }
    if std::env::var(crate::maintenance::ENV_DAEMON_MAINTENANCE).as_deref() != Ok("0") {
        tasks.push(tokio::spawn(maintenance_loop(
            shared.clone(),
            shared.shutdown.subscribe(),
        )));
    }
    // A host in direct mode forwards a port to each Space it provides:
    // open them again (nothing for any other machine).
    #[cfg(feature = "spaces")]
    {
        let host_spaces = shared.host_spaces.clone();
        tasks.push(tokio::spawn(async move {
            host_spaces.restore_direct_forwards().await;
        }));
    }
    // Extension tasks (the Cua Spaces build: the persistent-agent
    // supervisor and the Keyvault socket beside `cua.sock`).
    #[cfg(feature = "spaces")]
    for a in shared.runtime.attached_extensions() {
        tasks.extend(a.daemon_tasks(
            &shared.runtime,
            socket_path.as_ref().and_then(|p| p.parent()),
        ));
    }
    if let Some(p) = &config.discovery_path {
        Discovery {
            pid: std::process::id(),
            socket_path: socket_path.as_ref().map(|p| p.display().to_string()),
            loopback_url: loopback_url.clone(),
            token: Some(config.token.clone()),
            version: VERSION.into(),
        }
        .write(p)?;
    }
    Ok(DaemonHandle {
        loopback_url,
        socket_path,
        token: config.token,
        shared,
        tasks,
        discovery_path: config.discovery_path,
    })
}

type BoxedApp = tower::util::BoxCloneSyncService<
    Request<hyper::body::Incoming>,
    Response<AxumBody>,
    std::convert::Infallible,
>;

trait Accept: Send + 'static {
    type Io: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static;
    fn accept_io(&self) -> impl std::future::Future<Output = std::io::Result<Self::Io>> + Send;
}

impl Accept for tokio::net::TcpListener {
    type Io = tokio::net::TcpStream;
    async fn accept_io(&self) -> std::io::Result<Self::Io> {
        let (s, _) = self.accept().await?;
        let _ = s.set_nodelay(true);
        Ok(s)
    }
}

#[cfg(unix)]
impl Accept for tokio::net::UnixListener {
    type Io = tokio::net::UnixStream;
    async fn accept_io(&self) -> std::io::Result<Self::Io> {
        Ok(self.accept().await?.0)
    }
}

async fn accept_loop<L: Accept>(listener: L, app: BoxedApp, mut shutdown: watch::Receiver<bool>) {
    use hyper_util::{
        rt::{TokioExecutor, TokioIo},
        server::conn::auto::Builder,
        service::TowerToHyperService,
    };
    let mut conns = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            accepted = listener.accept_io() => {
                let Ok(stream) = accepted else { continue };
                let svc = TowerToHyperService::new(app.clone());
                conns.spawn(async move {
                    let _ = Builder::new(TokioExecutor::new())
                        .serve_connection_with_upgrades(TokioIo::new(stream), svc)
                        .await;
                });
            }
            Some(_) = conns.join_next(), if !conns.is_empty() => {}
        }
    }
    conns.abort_all();
}

fn app(shared: Arc<Shared>, require_token: bool) -> BoxedApp {
    let svc = Svc(shared.clone());
    let grpc = tonic::service::Routes::new(SandboxServiceServer::new(svc.clone()))
        .add_service(SpaceServiceServer::new(svc.clone()))
        .add_service(RuntimeServiceServer::new(svc.clone()))
        .add_service(DaemonServiceServer::new(svc.clone()));
    #[cfg(feature = "spaces")]
    let grpc = grpc.add_service(
        cua_proto::env::v1::host_spaces_service_server::HostSpacesServiceServer::new(
            crate::host_svc::HostSvc(svc.0.clone()),
        ),
    );
    let grpc = grpc.into_axum_router();
    let router = axum::Router::new()
        .route(
            "/v1/sandboxes/{name}/env/{*rest}",
            axum::routing::any(passthrough::env_passthrough),
        )
        .route(
            "/v1/sandboxes/{name}/svc/{service}/{*rest}",
            axum::routing::any(passthrough::service_passthrough),
        )
        .route(
            "/v1/spaces/{key}/env/{*rest}",
            axum::routing::any(passthrough::space_env_passthrough),
        )
        .route(
            "/v1/spaces/{key}/svc/{service}/{*rest}",
            axum::routing::any(passthrough::space_service_passthrough),
        )
        .route(
            "/v1/bridge/media",
            axum::routing::get(passthrough::bridge_media),
        )
        .with_state(shared.clone())
        .merge(grpc);
    // The Spaces MCP server over streamable HTTP. The listener's own auth
    // (loopback bearer, or the 0600 socket) already guards it.
    #[cfg(feature = "spaces")]
    let router = {
        let mut mcp = cua_spaces::mcp::McpServer::new(shared.runtime.spaces().clone());
        if let Some(broker) = shared.runtime.session_broker() {
            mcp = mcp.with_session_broker(broker);
        }
        router.merge(cua_spaces::mcp::http::router(mcp, None))
    };
    // tonic-web answers every non-gRPC-Web HTTP/1.1 request with 400, so
    // only gRPC-Web goes through it; native gRPC (h2c), WebSocket upgrades
    // and plain HTTP reach the router directly.
    let web = tower::ServiceBuilder::new()
        .layer(tonic_web::GrpcWebLayer::new())
        .service(router.clone());
    let token = shared.token.clone();
    let authed = tower::service_fn(move |req: Request<hyper::body::Incoming>| {
        let mut web = web.clone();
        let mut router = router.clone();
        let token = token.clone();
        let busy = Busy::new(shared.clone());
        async move {
            let _busy = busy;
            let req = req.map(AxumBody::new);
            if require_token && !authorized(&req, &token) {
                return Ok::<_, std::convert::Infallible>(unauthorized(&req));
            }
            let grpc_web = req
                .headers()
                .get(http::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|c| c.starts_with("application/grpc-web"));
            if grpc_web {
                let resp = match ServiceExt::<Request<AxumBody>>::ready(&mut web).await {
                    Ok(s) => s.call(req).await,
                    Err(e) => match e {},
                };
                return Ok(match resp {
                    Ok(r) => r.map(AxumBody::new),
                    Err(e) => match e {},
                });
            }
            let resp = match ServiceExt::<Request<AxumBody>>::ready(&mut router).await {
                Ok(s) => s.call(req).await,
                Err(e) => match e {},
            };
            Ok(match resp {
                Ok(r) => r,
                Err(e) => match e {},
            })
        }
    });
    let cors = tower_http::cors::CorsLayer::very_permissive().expose_headers([
        http::HeaderName::from_static("grpc-status"),
        http::HeaderName::from_static("grpc-message"),
        http::HeaderName::from_static("grpc-status-details-bin"),
    ]);
    let svc = tower::ServiceBuilder::new()
        .layer(cors)
        .service(authed)
        .map_response(|r: Response<_>| r.map(AxumBody::new));
    tower::util::BoxCloneSyncService::new(svc)
}

fn authorized(req: &Request<AxumBody>, token: &str) -> bool {
    let path = req.uri().path();
    // Ticket-authenticated route (browsers cannot set headers).
    if path == "/v1/bridge/media" {
        return true;
    }
    let expected = format!("Bearer {token}");
    let presented = req
        .headers()
        .get(http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    presented.is_some_and(|p| constant_time_eq(p.as_bytes(), expected.as_bytes()))
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn unauthorized(req: &Request<AxumBody>) -> Response<AxumBody> {
    let grpc = req
        .headers()
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|c| c.starts_with("application/grpc"));
    if grpc {
        let status = Error::Unauthenticated("missing or wrong daemon token".into()).to_status();
        let mut resp: Response<AxumBody> = status.into_http();
        if let Some(ct) = req.headers().get(http::header::CONTENT_TYPE) {
            resp.headers_mut()
                .insert(http::header::CONTENT_TYPE, ct.clone());
        }
        resp
    } else {
        Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .body(AxumBody::from("missing or wrong daemon token"))
            .unwrap()
    }
}

// ------------------------------------------------------------------ services

#[derive(Clone)]
pub(crate) struct Svc(pub(crate) Arc<Shared>);

impl Svc {
    pub(crate) fn rt(&self) -> &Runtime {
        &self.0.runtime
    }

    /// The env connection and WebSocket headers a media bridge for `name`
    /// relays to: a registered Space (a `relay:` or legacy `space://…` id, or
    /// a name the Spaces
    /// registry resolves when no sandbox has it), with its gateway headers
    /// (Fleet bearer, claim, env token), else a sandbox.
    async fn media_upstream(
        &self,
        name: &str,
    ) -> Result<(cua_spacesd_client::SpacesdClient, Vec<(String, String)>)> {
        #[cfg(feature = "spaces")]
        {
            let spaces = self.rt().spaces();
            let is_space = name.starts_with("space://")
                || name.starts_with("relay:")
                || (self.rt().record(name).await.is_err() && spaces.resolve(name).is_ok());
            if is_space {
                let space = spaces.space(name).await?;
                let headers = space.websocket_headers().await?;
                return Ok((space.spacesd()?.clone(), headers));
            }
        }
        let a = self.rt().env(name, None).await?;
        Ok((a.client, a.ws_headers))
    }
}

pub(crate) fn st(e: Error) -> Status {
    e.to_status()
}

pub(crate) type R<T> = std::result::Result<tonic::Response<T>, Status>;

pub(crate) fn ok<T>(t: T) -> R<T> {
    Ok(tonic::Response::new(t))
}

#[tonic::async_trait]
impl SandboxService for Svc {
    async fn list_sandboxes(
        &self,
        req: tonic::Request<pb::ListSandboxesRequest>,
    ) -> R<pb::ListSandboxesResponse> {
        let req = req.into_inner();
        #[allow(deprecated)]
        let provider = match req.location.trim() {
            "" => provider_from_pb(req.provider),
            l => match cua_sandbox_core::Location::parse(l) {
                Some(cua_sandbox_core::Location::Local) => {
                    Some(cua_sandbox_core::ProviderKind::Local)
                }
                Some(cua_sandbox_core::Location::Cloud) => {
                    Some(cua_sandbox_core::ProviderKind::Fleet)
                }
                Some(cua_sandbox_core::Location::Direct) => {
                    Some(cua_sandbox_core::ProviderKind::Direct)
                }
                _ => {
                    return Err(st(Error::InvalidArgument(format!(
                        "location {l:?}: expected local, cloud or direct"
                    ))));
                }
            },
        };
        let list = self
            .rt()
            .list_filtered(provider, req.include_cloud.unwrap_or(true))
            .await
            .map_err(st)?;
        ok(pb::ListSandboxesResponse {
            sandboxes: list.sandboxes.iter().map(info_to_pb).collect(),
            warnings: list.warnings,
        })
    }

    // The deprecated request fields still count when `location` / `runtime`
    // are empty (older clients).
    #[allow(deprecated)]
    async fn create_sandbox(
        &self,
        req: tonic::Request<pb::CreateSandboxRequest>,
    ) -> R<pb::CreateSandboxResponse> {
        let r = req.into_inner();
        let non_empty = |s: String| (!s.is_empty()).then_some(s);
        let wait_for = r
            .wait_for
            .iter()
            .map(probe_from_pb)
            .collect::<Result<Vec<_>>>()
            .map_err(st)?;
        let ports = r
            .ports
            .iter()
            .map(|p| u16::try_from(*p).map_err(|_| st(Error::InvalidArgument("port".into()))))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let create = CreateRequest {
            location: non_empty(r.location),
            kind: non_empty(r.kind),
            runtime: non_empty(r.runtime),
            provider: provider_from_pb(r.provider),
            name: non_empty(r.name),
            image: r.image,
            url: non_empty(r.url),
            token: non_empty(r.token),
            pool: non_empty(r.pool),
            os: non_empty(r.os),
            cpus: Some(r.cpus),
            memory_mb: Some(r.memory_mb),
            ports,
            services: r.services.into_iter().map(|(k, v)| (k, v as u16)).collect(),
            wait_for,
            ready_timeout: r.ready_timeout.as_ref().map(from_dur),
            env: r.env.into_iter().collect(),
            command: (!r.command.is_empty()).then_some(r.command),
            fleet_runtime: non_empty(r.fleet_runtime),
            fleet_replicas: Some(r.fleet_replicas),
            fleet_ttl_seconds: Some(r.fleet_ttl_seconds),
            fleet_warm: r.fleet_warm,
            fleet_max_pool_size: Some(r.fleet_max_pool_size).filter(|m| *m > 0),
            fleet_apply: r.fleet_apply,
            labels: r.labels.into_iter().collect(),
            sidecars: r
                .sidecars
                .into_iter()
                .map(crate::convert::sidecar_from_pb)
                .collect::<Result<Vec<_>>>()
                .map_err(st)?,
            registry_credentials: r
                .registry_secret
                .map(crate::convert::registry_secret_from_pb)
                .transpose()
                .map_err(st)?,
            build: r
                .build
                .map(crate::convert::build_from_pb)
                .transpose()
                .map_err(st)?,
            container_runtime: non_empty(r.container_runtime),
            network: non_empty(r.network),
            owner_pid: (r.owner_pid > 0).then_some(r.owner_pid),
            keep_on_failure: r.keep_on_failure,
            gpu: non_empty(r.gpu),
        };
        let rec = self.rt().create(create).await.map_err(st)?;
        ok(pb::CreateSandboxResponse {
            sandbox: Some(info_to_pb(&rec)),
        })
    }

    async fn get_sandbox(
        &self,
        req: tonic::Request<pb::GetSandboxRequest>,
    ) -> R<pb::GetSandboxResponse> {
        let rec = self.rt().record(&req.into_inner().name).await.map_err(st)?;
        ok(pb::GetSandboxResponse {
            sandbox: Some(info_to_pb(&rec)),
        })
    }

    async fn cancel_create_sandbox(
        &self,
        req: tonic::Request<pb::CancelCreateSandboxRequest>,
    ) -> R<pb::CancelCreateSandboxResponse> {
        let what = self
            .rt()
            .cancel_create(&req.into_inner().name)
            .await
            .map_err(st)?;
        ok(pb::CancelCreateSandboxResponse {
            cancelled: what.is_some(),
            message: what.unwrap_or_else(|| "no create of that name is running".into()),
        })
    }

    async fn delete_sandbox(
        &self,
        req: tonic::Request<pb::DeleteSandboxRequest>,
    ) -> R<pb::DeleteSandboxResponse> {
        self.rt().delete(&req.into_inner().name).await.map_err(st)?;
        ok(pb::DeleteSandboxResponse {})
    }

    async fn connect_sandbox(
        &self,
        req: tonic::Request<pb::ConnectSandboxRequest>,
    ) -> R<pb::ConnectSandboxResponse> {
        let rec = self
            .rt()
            .connect(&req.into_inner().name)
            .await
            .map_err(st)?;
        ok(pb::ConnectSandboxResponse {
            sandbox: Some(info_to_pb(&rec)),
        })
    }

    async fn suspend_sandbox(
        &self,
        req: tonic::Request<pb::SuspendSandboxRequest>,
    ) -> R<pb::SuspendSandboxResponse> {
        self.rt()
            .suspend(&req.into_inner().name)
            .await
            .map_err(st)?;
        ok(pb::SuspendSandboxResponse {})
    }

    async fn resume_sandbox(
        &self,
        req: tonic::Request<pb::ResumeSandboxRequest>,
    ) -> R<pb::ResumeSandboxResponse> {
        self.rt().resume(&req.into_inner().name).await.map_err(st)?;
        ok(pb::ResumeSandboxResponse {})
    }

    async fn restart_sandbox(
        &self,
        req: tonic::Request<pb::RestartSandboxRequest>,
    ) -> R<pb::RestartSandboxResponse> {
        self.rt()
            .restart(&req.into_inner().name)
            .await
            .map_err(st)?;
        ok(pb::RestartSandboxResponse {})
    }

    async fn keep_alive(
        &self,
        req: tonic::Request<pb::KeepAliveRequest>,
    ) -> R<pb::KeepAliveResponse> {
        let r = req.into_inner();
        let d = r
            .duration
            .as_ref()
            .map(from_dur)
            .unwrap_or(Duration::from_secs(600));
        self.rt().keep_alive(&r.name, d).await.map_err(st)?;
        ok(pb::KeepAliveResponse {})
    }

    async fn wait_ready(
        &self,
        req: tonic::Request<pb::WaitReadyRequest>,
    ) -> R<pb::WaitReadyResponse> {
        let r = req.into_inner();
        let probes = r
            .probes
            .iter()
            .map(probe_from_pb)
            .collect::<Result<Vec<_>>>()
            .map_err(st)?;
        let t = r
            .timeout
            .as_ref()
            .map(from_dur)
            .unwrap_or(Duration::from_secs(300));
        self.rt()
            .wait_ready(&r.name, &probes, t)
            .await
            .map_err(st)?;
        ok(pb::WaitReadyResponse {})
    }

    async fn service_request(
        &self,
        req: tonic::Request<pb::ServiceRequestRequest>,
    ) -> R<pb::ServiceRequestResponse> {
        let r = req.into_inner();
        let t = r
            .timeout
            .as_ref()
            .map(from_dur)
            .unwrap_or(Duration::from_secs(30));
        let body = (!r.body.is_empty()).then(|| r.body.to_vec());
        let headers: Vec<(String, String)> =
            r.headers.into_iter().map(|h| (h.name, h.value)).collect();
        let resp = self
            .rt()
            .service_request(&r.name, &r.service, &r.method, &r.path, &headers, body, t)
            .await
            .map_err(st)?;
        ok(pb::ServiceRequestResponse {
            status: resp.status as u32,
            headers: resp
                .headers
                .into_iter()
                .map(|(name, value)| pb::HttpHeader { name, value })
                .collect(),
            body: resp.body,
        })
    }

    async fn forward_port(
        &self,
        req: tonic::Request<pb::ForwardPortRequest>,
    ) -> R<pb::ForwardPortResponse> {
        let r = req.into_inner();
        let port = u16::try_from(r.port)
            .map_err(|_| st(Error::InvalidArgument(format!("port {}", r.port))))?;
        let f = self.rt().forward(&r.name, port).await.map_err(st)?;
        ok(pb::ForwardPortResponse {
            forward_id: f.id,
            guest_port: f.guest_port as u32,
            local_addr: f.local_addr.unwrap_or_default(),
            url: f.url.unwrap_or_default(),
        })
    }

    async fn get_service_url(
        &self,
        req: tonic::Request<pb::GetServiceUrlRequest>,
    ) -> R<pb::GetServiceUrlResponse> {
        let r = req.into_inner();
        let url = self
            .rt()
            .service_url(&r.name, &r.service)
            .await
            .map_err(st)?;
        ok(pb::GetServiceUrlResponse { url })
    }

    async fn create_public_url(
        &self,
        req: tonic::Request<pb::CreatePublicUrlRequest>,
    ) -> R<pb::CreatePublicUrlResponse> {
        let r = req.into_inner();
        let ttl = r.ttl.as_ref().map(from_dur);
        let u = if r.upstream_url.is_empty() {
            let label = (!r.label.is_empty()).then_some(r.label);
            self.rt()
                .public_url(&r.name, &r.service, ttl, label)
                .await
                .map_err(st)?
        } else {
            self.rt()
                .host_share(
                    &r.upstream_url,
                    ttl.unwrap_or(crate::shares::DEFAULT_TTL),
                    &r.name,
                    &r.service,
                )
                .await
                .map_err(st)?
        };
        ok(pb::CreatePublicUrlResponse {
            public_url: Some(convert::public_url_to_pb(&u)),
        })
    }

    async fn revoke_public_url(
        &self,
        req: tonic::Request<pb::RevokePublicUrlRequest>,
    ) -> R<pb::RevokePublicUrlResponse> {
        let r = req.into_inner();
        self.rt()
            .revoke_public_url(&r.name, &r.id)
            .await
            .map_err(st)?;
        ok(pb::RevokePublicUrlResponse {})
    }

    async fn close_forward(
        &self,
        req: tonic::Request<pb::CloseForwardRequest>,
    ) -> R<pb::CloseForwardResponse> {
        self.rt()
            .close_forward(&req.into_inner().forward_id)
            .map_err(st)?;
        ok(pb::CloseForwardResponse {})
    }

    async fn get_service_endpoint(
        &self,
        req: tonic::Request<pb::GetServiceEndpointRequest>,
    ) -> R<pb::GetServiceEndpointResponse> {
        let r = req.into_inner();
        // Fails with NotFound for an unknown sandbox or service.
        self.rt()
            .service_endpoint(&r.name, &r.service)
            .await
            .map_err(st)?;
        let (base, token) = {
            let info = self.0.info.lock().unwrap();
            (info.loopback_url.clone(), info.loopback_token.clone())
        };
        if base.is_empty() {
            return Err(st(Error::Unsupported(
                "the service passthrough needs the daemon's loopback listener".into(),
            )));
        }
        ok(pb::GetServiceEndpointResponse {
            url: format!(
                "{base}/v1/sandboxes/{}/svc/{}",
                passthrough::encode_segment(&r.name),
                passthrough::encode_segment(&r.service)
            ),
            headers: vec![pb::HttpHeader {
                name: "authorization".into(),
                value: format!("Bearer {token}"),
            }],
        })
    }

    async fn get_env_endpoint(
        &self,
        req: tonic::Request<pb::GetEnvEndpointRequest>,
    ) -> R<pb::GetEnvEndpointResponse> {
        let r = req.into_inner();
        let a = self
            .rt()
            .env(&r.name, r.probe_timeout.as_ref().map(from_dur))
            .await
            .map_err(st)?;
        let caps = a.client.capabilities().await.map_err(|e| st(e.into()))?;
        let (base, token) = {
            let info = self.0.info.lock().unwrap();
            (info.loopback_url.clone(), info.loopback_token.clone())
        };
        if base.is_empty() {
            return Err(st(Error::Unsupported(
                "the env passthrough needs the daemon's loopback listener".into(),
            )));
        }
        ok(pb::GetEnvEndpointResponse {
            url: format!(
                "{base}/v1/sandboxes/{}/env",
                passthrough::encode_segment(&r.name)
            ),
            token,
            spacesd_version: caps.version,
        })
    }
}

#[tonic::async_trait]
impl RuntimeService for Svc {
    async fn doctor(&self, _: tonic::Request<pb::DoctorRequest>) -> R<pb::DoctorResponse> {
        let report = crate::local::doctor_report().await;
        ok(pb::DoctorResponse {
            checks: crate::local::doctor_checks(&report),
            report_json: serde_json::to_string(&report).unwrap_or_default(),
        })
    }

    async fn setup(&self, req: tonic::Request<pb::SetupRequest>) -> R<pb::SetupResponse> {
        let r = req.into_inner();
        ok(pb::SetupResponse {
            steps: crate::local::setup(&r.components, r.dry_run)
                .await
                .map_err(st)?,
        })
    }

    async fn pull_image(
        &self,
        req: tonic::Request<pb::PullImageRequest>,
    ) -> R<pb::PullImageResponse> {
        let vmm = self.rt().vmm().map_err(st)?.clone();
        let i = crate::local::pull_image(&vmm, &req.into_inner().reference)
            .await
            .map_err(st)?;
        ok(pb::PullImageResponse {
            image: Some(pb::LocalImage {
                reference: i.reference,
                kind: i.kind,
                location: i.location,
                size_bytes: i.size_bytes,
            }),
        })
    }

    async fn build_image(
        &self,
        req: tonic::Request<pb::BuildImageRequest>,
    ) -> R<pb::BuildImageResponse> {
        let r = req.into_inner();
        let vmm = self.rt().vmm().map_err(st)?.clone();
        let out_dir = crate::cua_home()
            .join("build")
            .join(format!("out-{:x}", rand::random::<u32>()));
        let push = (!r.push.is_empty()).then_some(r.push);
        let result = crate::local::build_image(&vmm, &r.spec_json, &r.base, &out_dir, push)
            .await
            .map_err(st)?;
        ok(pb::BuildImageResponse { result })
    }

    async fn push_image(
        &self,
        req: tonic::Request<pb::PushImageRequest>,
    ) -> R<pb::PushImageResponse> {
        let r = req.into_inner();
        ok(pb::PushImageResponse {
            digest: crate::local::push_image(&r.reference, &r.destination)
                .await
                .map_err(st)?,
        })
    }
}

#[tonic::async_trait]
impl DaemonService for Svc {
    async fn get_info(&self, _: tonic::Request<pb::GetInfoRequest>) -> R<pb::GetInfoResponse> {
        ok(self.0.info.lock().unwrap().clone())
    }

    async fn open_media_bridge(
        &self,
        req: tonic::Request<pb::OpenMediaBridgeRequest>,
    ) -> R<pb::OpenMediaBridgeResponse> {
        let r = req.into_inner();
        let open: envpb::OpenMediaRequest = if r.open_media_json.trim().is_empty() {
            envpb::OpenMediaRequest {
                target: Some(envpb::MediaTarget {
                    target: Some(envpb::media_target::Target::DisplayId("primary".into())),
                }),
                ..Default::default()
            }
        } else {
            serde_json::from_str(&r.open_media_json)
                .map_err(|e| st(Error::InvalidArgument(format!("open_media_json: {e}"))))?
        };
        let (client, ws_headers) = self.media_upstream(&r.name).await.map_err(st)?;
        let mut resp = client
            .stream()
            .open_media(open)
            .await
            .map_err(|s| st(cua_spacesd_client::Error::from(s).into()))?
            .into_inner();
        let ws_path = if resp.ws_path.is_empty() {
            format!(
                "{}?ticket={}",
                cua_proto::metadata::MEDIA_WS_PATH,
                resp.ticket
            )
        } else {
            resp.ws_path.clone()
        };
        let upstream_url = client.endpoint().ws_url(&ws_path);
        let ticket = crate::random_token();
        let expires = SystemTime::now() + self.0.bridge_ttl;
        {
            let mut bridges = self.0.bridges.lock().unwrap();
            let now = SystemTime::now();
            bridges.retain(|_, b| b.expires > now);
            bridges.insert(
                ticket.clone(),
                Bridge {
                    upstream_url,
                    headers: ws_headers,
                    expires,
                },
            );
        }
        resp.ticket = String::new();
        resp.ws_path = String::new();
        let base = self.0.info.lock().unwrap().loopback_url.clone();
        if base.is_empty() {
            return Err(st(Error::Unsupported(
                "the media bridge needs the daemon's loopback listener".into(),
            )));
        }
        let ws_base = base.replacen("http://", "ws://", 1);
        ok(pb::OpenMediaBridgeResponse {
            ws_url: format!("{ws_base}/v1/bridge/media?ticket={ticket}"),
            ticket,
            expires_at: Some(convert::ts(expires)),
            open_media_response_json: serde_json::to_string(&resp)
                .map_err(|e| st(Error::Internal(e.to_string())))?,
        })
    }

    async fn shutdown(&self, _: tonic::Request<pb::ShutdownRequest>) -> R<pb::ShutdownResponse> {
        let tx = self.0.shutdown.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let _ = tx.send(true);
        });
        ok(pb::ShutdownResponse {})
    }
}

impl Shared {
    /// Registers a bridge directly (tests and in-process hosts).
    pub(crate) fn take_bridge(&self, ticket: &str) -> Option<Bridge> {
        let bridges = self.bridges.lock().unwrap();
        bridges
            .get(ticket)
            .filter(|b| b.expires > SystemTime::now())
            .cloned()
    }
}
