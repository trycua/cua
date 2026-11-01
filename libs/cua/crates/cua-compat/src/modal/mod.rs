//! The Modal provider: the gRPC API the `modal` clients speak (Python, JS,
//! Go), served from Cua Spaces.
//!
//! - **Control plane** (`modal.client.ModalClient`): apps, secrets, volumes,
//!   mounts, images (built in Spaces from the client's Dockerfile layers and
//!   cached by spec hash), sandboxes, snapshots, dicts, queues.
//! - **Command router** (`modal.task_command_router.TaskCommandRouter`): exec,
//!   stdio, stdin, snapshots, on the same listener, with the JWT flow the
//!   client expects (`TaskGetCommandRouterAccess`, refresh on
//!   `UNAUTHENTICATED`).
//! - `/__modal/.bin/modal-sandbox-fs-tools` execs are answered by the
//!   gateway (see [`fstools`]).
//!
//! The wire shapes are generated from the descriptors of the Apache-2.0
//! `modal` client (`modal-proto/`); the implementation is ours. See
//! `maps/modal-coverage.md` for every call and its status.

pub mod bridge;
pub mod containers;
pub mod dockerfile;
pub mod exec;
pub mod fstools;
pub mod functions;
pub mod ids;
pub mod image;
pub mod jwt;
pub mod prewarm;
pub mod sandbox;
pub mod volsync;

mod api;
mod api_functions;
mod api_misc;
mod api_objects;
mod blobs;
mod db;
mod errors;
#[cfg(test)]
mod props;
mod router;
mod snapshots;
mod volumes;

/// Generated messages and server stubs, from the pinned client's
/// descriptors (`modal-proto/`, see `build.rs`).
#[allow(clippy::all, dead_code, unused_variables, missing_docs)]
pub mod pb {
    pub mod modal {
        pub mod client {
            include!(concat!(env!("OUT_DIR"), "/modal.client.rs"));
        }
        pub mod task_command_router {
            include!(concat!(env!("OUT_DIR"), "/modal.task_command_router.rs"));
        }
    }
    pub use modal::client as api;
    pub use modal::task_command_router as tcr;
}

use crate::auth::{KeyEntry, Keys};
use crate::backend::{Backend, Identity};
use axum::Router;
use axum::extract::State;
use axum::http::{Request, Response};
use axum::middleware::Next;
use cua_spacesd_client::SpacesdClient;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

pub use db::Db;

/// The Modal client version the gateway is tested with (the descriptors
/// come from it).
pub const CLIENT_VERSION: &str = "1.6.2";

/// Modal provider settings.
#[derive(Debug, Clone)]
pub struct ModalConfig {
    /// Where the API (and the command router) listens, as clients reach it
    /// (`http://127.0.0.1:<port>`).
    pub api_url: String,
    /// The domain tunnels are served under (`modal.localhost:<tls port>`).
    pub sandbox_domain: String,
    /// The gateway's directory (state, blobs, volumes).
    pub dir: PathBuf,
    /// The user processes run as unless the image says otherwise. Modal
    /// containers run as root.
    pub default_user: String,
    /// Lifetime of a command-router token (seconds).
    pub jwt_ttl_s: u64,
    /// `FROM` references the image builder maps to Cua images
    /// (`images.json`).
    pub image_map: HashMap<String, String>,
    /// The image a sandbox starts from when a layer's base is a stock
    /// distribution image (empty: Cua's canonical Linux image).
    pub base_image: String,
    /// Test only (`CUA_MODAL_BUILD_DRY_RUN=1`): image build commands are
    /// logged, not run, so hermetic tests can build over mock Spaces.
    pub dry_run_builds: bool,
    /// Test only: Function containers use the image as built, without the
    /// layer that installs the `modal` client.
    pub skip_runtime_layer: bool,
    /// Seconds without a heartbeat after which an ephemeral App (and what it
    /// made) is over.
    pub heartbeat_grace_s: f64,
}

impl ModalConfig {
    /// The gateway's address as a Function container dials it: its URL
    /// and port.
    pub fn container_server(&self) -> (String, u16) {
        let port = self
            .api_url
            .rsplit(':')
            .next()
            .and_then(|p| p.trim_end_matches('/').parse().ok())
            .unwrap_or(0);
        (format!("http://127.0.0.1:{port}"), port)
    }
}

impl Default for ModalConfig {
    fn default() -> Self {
        Self {
            api_url: "http://127.0.0.1:0".into(),
            sandbox_domain: "modal.localhost".into(),
            dir: PathBuf::new(),
            default_user: "root".into(),
            jwt_ttl_s: 3600,
            image_map: HashMap::new(),
            base_image: String::new(),
            dry_run_builds: false,
            skip_runtime_layer: false,
            heartbeat_grace_s: HEARTBEAT_GRACE_S,
        }
    }
}

/// A raw tunnel: the loopback address and the task accepting on it.
pub type RawTunnel = (String, tokio::task::JoinHandle<()>);

/// A volume file kept for block downloads: key, bytes, when it was read.
pub type VolCache = (String, Arc<Vec<u8>>, tokio::time::Instant);

/// Runtime state that is not persisted.
#[derive(Default)]
pub struct Live {
    /// Exec sessions by `(task id, exec id)`.
    pub execs: Mutex<HashMap<(String, String), Arc<exec::Session>>>,
    /// Entrypoint sessions by sandbox id.
    pub entrypoints: Mutex<HashMap<String, Arc<exec::Session>>>,
    /// cua-spacesd clients by sandbox id.
    pub clients: Mutex<HashMap<String, SpacesdClient>>,
    pub drivers: crate::guest::Drivers,
    /// What each sandbox's cua-spacesd runs as and on (user, linux).
    pub guest: Mutex<HashMap<String, (String, bool)>>,
    /// Image builds in flight, by spec hash.
    pub builds: Mutex<HashMap<String, image::BuildFuture>>,
    /// Image build logs, by image id.
    pub build_logs: Mutex<HashMap<String, Arc<image::BuildLog>>>,
    /// Open raw tunnels by `(sandbox id, port)`: the loopback address.
    pub raw_tunnels: Mutex<HashMap<(String, u16), RawTunnel>>,
    /// Volume mounts being synced with the host, by sandbox id or task id.
    pub volsyncs: Mutex<HashMap<String, Arc<volsync::VolSync>>>,
    /// The last volume file read for block downloads: key, bytes, when.
    pub vol_cache: Mutex<Option<VolCache>>,
    /// A change counter per sandbox, for waiters.
    pub events: Mutex<HashMap<String, tokio::sync::watch::Sender<u64>>>,
    /// Last activity per sandbox (for `idle_timeout`).
    pub activity: Mutex<HashMap<String, tokio::time::Instant>>,
    /// Function container images (the function's image plus the runtime
    /// layer), by cache key.
    pub fn_images: Mutex<HashMap<String, Arc<image::Built>>>,
    pub fn_image_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

/// The Modal provider's shared state.
pub struct Modal {
    pub cfg: ModalConfig,
    pub backend: Arc<dyn Backend>,
    pub keys: Arc<RwLock<Keys>>,
    pub db: Db,
    pub signer: jwt::Signer,
    pub blobs: blobs::Blobs,
    pub live: Live,
    /// Function calls, inputs and the containers serving them.
    pub fns: std::sync::Mutex<functions::Fns>,
    /// Bumped whenever a call, input or container changes.
    pub fn_events: tokio::sync::watch::Sender<u64>,
    /// How Function containers run.
    pub containers: Arc<dyn containers::ContainerRuntime>,
    /// When each layer's build started and ended (ns), for `ImageBuildChainGet`.
    pub build_spans: std::sync::Mutex<HashMap<String, (u64, u64)>>,
    /// Container output by app id.
    pub app_logs: std::sync::Mutex<HashMap<String, Vec<containers::AppLogLine>>>,
}

/// The Function container runtime: Spaces (a test can pick the local one).
fn default_runtime() -> Arc<dyn containers::ContainerRuntime> {
    #[cfg(any(test, feature = "testing"))]
    if let Ok(python) = std::env::var("CUA_MODAL_LOCAL_RUNTIME") {
        return Arc::new(containers::LocalRuntime {
            python,
            pythonpath: std::env::var("CUA_MODAL_LOCAL_PYTHONPATH").unwrap_or_default(),
        });
    }
    Arc::new(containers::SpaceRuntime)
}

/// Who is calling: the API key entry, attached by the auth layer, and
/// whether the client is one of the JS/Go libraries (`x-modal-libmodal-version`).
#[derive(Clone, Debug)]
pub struct Caller(pub KeyEntry, pub bool);

impl Caller {
    pub fn key_id(&self) -> String {
        self.0.id()
    }

    pub fn identity(&self) -> Identity {
        Identity {
            daemon: self.0.daemon.clone(),
            token: self.0.token.clone(),
        }
    }
}

/// The token id a Modal client sends for a gateway key: `ak-` and 20 hex
/// characters of the key's SHA-256. The token secret is the key.
pub fn token_id(key: &str) -> String {
    use sha2::Digest;
    format!(
        "ak-{}",
        &hex::encode(sha2::Sha256::digest(key.as_bytes()))[..20]
    )
}

impl Modal {
    /// The command router's URL for a client. The Python client takes
    /// `http` for a loopback server; the JS and Go libraries insist on an
    /// `https` URL and then connect without TLS when the server is on
    /// loopback.
    pub fn router_url(&self, caller: &Caller) -> String {
        if caller.1 {
            self.cfg.api_url.replacen("http://", "https://", 1)
        } else {
            self.cfg.api_url.clone()
        }
    }

    /// The Cua image Spaces start from when an Image names no Cua image of its
    /// own (`python:*`, `ubuntu:*`, no base): the gateway's `--image`, else the
    /// Cua Linux image by name (so the choice does not depend on a runtime's
    /// default, which may be a minimal image with no working package sources).
    /// The names of the Cua Spaces this gateway's records still claim: live
    /// Sandboxes (also after a restart), the Spaces of Function containers,
    /// and the Spaces of image builds in flight (`modal-build-*` names in
    /// `owned`, the journal's, while a build runs). The journal's sweeper
    /// ends every owned Space not in this set.
    pub async fn claimed_spaces(&self, owned: &[String]) -> std::collections::HashSet<String> {
        let mut set: std::collections::HashSet<String> = self.db.read(|d| {
            d.sandboxes
                .values()
                .filter(|s| s.state != sandbox::SbState::Finished)
                .filter_map(|s| s.cua_name.clone())
                .collect()
        });
        if let Ok(f) = self.fns.lock() {
            for q in f.queues.values() {
                for c in q.containers.values() {
                    if let Some((_, name)) = &c.space {
                        set.insert(name.clone());
                    }
                }
            }
        }
        if !self.live.builds.lock().await.is_empty() {
            set.extend(
                owned
                    .iter()
                    .filter(|n| n.starts_with("modal-build-"))
                    .cloned(),
            );
        }
        set
    }

    pub fn base_image(&self) -> String {
        if self.cfg.base_image.is_empty() {
            "ghcr.io/trycua/linux:24.04".to_string()
        } else {
            self.cfg.base_image.clone()
        }
    }

    pub fn new(
        cfg: ModalConfig,
        backend: Arc<dyn Backend>,
        keys: Arc<RwLock<Keys>>,
        signer_secret: &str,
    ) -> std::io::Result<Arc<Modal>> {
        std::fs::create_dir_all(&cfg.dir)?;
        let db = Db::open(Some(cfg.dir.join("objects.json")))?;
        let blobs = blobs::Blobs::open(cfg.dir.join("blobs"))?;
        Ok(Arc::new(Modal {
            cfg,
            backend,
            keys,
            db,
            signer: jwt::Signer::new(signer_secret),
            blobs,
            live: Live::default(),
            fns: Default::default(),
            fn_events: tokio::sync::watch::channel(0).0,
            containers: default_runtime(),
            app_logs: Default::default(),
            build_spans: Default::default(),
        }))
    }

    /// An in-memory instance for unit tests.
    #[cfg(test)]
    pub fn for_tests(
        backend: Arc<dyn Backend>,
    ) -> std::io::Result<(Arc<Modal>, tempfile::TempDir)> {
        let dir = tempfile::tempdir()?;
        let keys = Keys {
            keys: vec![KeyEntry {
                key: "modal_test".into(),
                label: "t".into(),
                daemon: None,
                token: None,
            }],
        };
        let cfg = ModalConfig {
            dir: dir.path().to_path_buf(),
            ..Default::default()
        };
        let m = Modal::new(cfg, backend, Arc::new(RwLock::new(keys)), "modal_test")?;
        Ok((m, dir))
    }

    /// The key entry for a token id and secret.
    pub async fn authenticate(&self, id: &str, secret: &str) -> Option<KeyEntry> {
        let keys = self.keys.read().await;
        let k = keys.find(secret)?;
        crate::auth::same(&token_id(&k.key), id).then(|| k.clone())
    }

    /// The sandbox's cua-spacesd client (cached).
    pub async fn spacesd(
        &self,
        rec: &sandbox::SandboxRec,
    ) -> Result<SpacesdClient, crate::backend::BackendError> {
        if let Some(c) = self.live.clients.lock().await.get(&rec.id) {
            return Ok(c.clone());
        }
        let name = rec.cua_name.as_deref().ok_or_else(|| {
            crate::backend::BackendError::NotFound("sandbox is not scheduled".into())
        })?;
        let c = self.backend.spacesd(&rec.identity(), name).await?;
        self.live
            .clients
            .lock()
            .await
            .insert(rec.id.clone(), c.clone());
        Ok(c)
    }

    /// The axum router: both gRPC services and the blob endpoints, behind
    /// their auth.
    pub fn router(self: &Arc<Self>) -> Router {
        use axum::routing::any;
        let api = pb::api::modal_client_server::ModalClientServer::new(api::Api(self.clone()))
            .max_decoding_message_size(256 << 20)
            .max_encoding_message_size(256 << 20);
        let tcr = pb::tcr::task_command_router_server::TaskCommandRouterServer::new(
            router::Router_(self.clone()),
        )
        .max_decoding_message_size(256 << 20)
        .max_encoding_message_size(256 << 20);
        let api_routes = Router::new()
            .route_service("/modal.client.ModalClient/{method}", api)
            .layer(axum::middleware::from_fn(explain_unimplemented))
            .layer(axum::middleware::from_fn_with_state(self.clone(), api_auth));
        let tcr_routes = Router::new()
            .route_service("/modal.task_command_router.TaskCommandRouter/{method}", tcr)
            .layer(axum::middleware::from_fn(explain_unimplemented))
            .layer(axum::middleware::from_fn_with_state(self.clone(), tcr_auth));
        Router::new()
            .merge(api_routes)
            .merge(tcr_routes)
            .route(
                "/_cua/blob/{id}",
                any(blobs::serve).with_state(self.clone()),
            )
            .route(
                "/_cua/vol/{token}",
                any(blobs::serve_volume).with_state(self.clone()),
            )
    }
}

/// Replaces the generated "Not yet implemented" of an RPC the gateway does not
/// serve with the reason (see [`errors::unserved_reason`]).
async fn explain_unimplemented(
    req: Request<axum::body::Body>,
    next: Next,
) -> Response<axum::body::Body> {
    let rpc = req
        .uri()
        .path()
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string();
    let mut resp = next.run(req).await;
    let stub = resp.headers().get("grpc-status").is_some_and(|s| s == "12")
        && resp.headers().get("grpc-message").is_some_and(|m| {
            m.to_str()
                .is_ok_and(|m| m.replace("%20", " ") == "Not yet implemented")
        });
    if stub {
        let text = errors::unserved_reason(&rpc);
        let enc = tonic::Status::unimplemented(text).into_http::<axum::body::Body>();
        if let Some(m) = enc.headers().get("grpc-message") {
            resp.headers_mut().insert("grpc-message", m.clone());
        }
    }
    resp
}

fn grpc_error(status: tonic::Status) -> Response<axum::body::Body> {
    status.into_http::<axum::body::Body>()
}

async fn api_auth(
    State(m): State<Arc<Modal>>,
    mut req: Request<axum::body::Body>,
    next: Next,
) -> Response<axum::body::Body> {
    let h = req.headers();
    let get = |k: &str| h.get(k).and_then(|v| v.to_str().ok()).unwrap_or_default();
    let (id, secret) = (get("x-modal-token-id"), get("x-modal-token-secret"));
    // A Function container presents the task token the gateway gave it.
    let task_token = get("x-cua-task").to_string();
    if !task_token.is_empty() {
        let claims = match m.signer.verify(&task_token, "task") {
            Ok(c) => c,
            Err(why) => {
                return grpc_error(tonic::Status::unauthenticated(format!(
                    "Invalid task token: {why}"
                )));
            }
        };
        let key_id = claims["key"].as_str().unwrap_or_default().to_string();
        let entry = m
            .keys
            .read()
            .await
            .keys
            .iter()
            .find(|k| k.id() == key_id)
            .cloned();
        let Some(k) = entry else {
            return grpc_error(tonic::Status::unauthenticated("The task's key is gone."));
        };
        req.extensions_mut().insert(Caller(k, false));
        req.extensions_mut().insert(router::TaskClaim(
            claims["sub"].as_str().unwrap_or_default().to_string(),
        ));
        return next.run(req).await;
    }
    if id.is_empty() && secret.is_empty() {
        return grpc_error(tonic::Status::unauthenticated(
            "Token missing. Could not authenticate client. Set MODAL_TOKEN_ID and MODAL_TOKEN_SECRET (`cua compat env --provider modal`).",
        ));
    }
    match m.authenticate(id, secret).await {
        Some(k) => {
            let lib = req.headers().contains_key("x-modal-libmodal-version");
            req.extensions_mut().insert(Caller(k, lib));
            next.run(req).await
        }
        None => grpc_error(tonic::Status::unauthenticated(
            "Could not authenticate client: the token id and secret were not recognised by this gateway.",
        )),
    }
}

async fn tcr_auth(
    State(m): State<Arc<Modal>>,
    mut req: Request<axum::body::Body>,
    next: Next,
) -> Response<axum::body::Body> {
    let token = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default()
        .to_string();
    match m.signer.verify(&token, "tcr") {
        Ok(claims) => {
            let task = claims["sub"].as_str().unwrap_or_default().to_string();
            req.extensions_mut().insert(router::TaskClaim(task));
            next.run(req).await
        }
        // The client refreshes the token and retries on UNAUTHENTICATED.
        Err(why) => grpc_error(tonic::Status::unauthenticated(format!(
            "invalid command router token: {why}"
        ))),
    }
}

/// Starts the Modal provider for a gateway: its state, a reconcile of the
/// sandboxes recorded before this process started, and the image map.
pub async fn start(
    opts: &crate::GatewayOptions,
    dir: &std::path::Path,
    api_url: &str,
    sandbox_domain: &str,
    api_key: &str,
    backend: Arc<dyn Backend>,
    keys: Arc<RwLock<Keys>>,
) -> Result<Arc<Modal>, crate::GatewayError> {
    let image_map: HashMap<String, String> = match std::fs::read(dir.join("images.json")) {
        Ok(b) => serde_json::from_slice(&b)
            .map_err(|e| crate::GatewayError::Config(format!("images.json: {e}")))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
        Err(e) => return Err(e.into()),
    };
    let jwt_ttl_s = std::env::var("CUA_MODAL_JWT_TTL_S")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3600);
    let cfg = ModalConfig {
        api_url: api_url.to_string(),
        sandbox_domain: sandbox_domain.to_string(),
        dir: dir.join("modal"),
        default_user: opts.default_user.clone(),
        jwt_ttl_s,
        image_map,
        base_image: opts.defaults.image.clone(),
        dry_run_builds: std::env::var("CUA_MODAL_BUILD_DRY_RUN").as_deref() == Ok("1"),
        skip_runtime_layer: std::env::var("CUA_MODAL_SKIP_RUNTIME_LAYER").as_deref() == Ok("1"),
        heartbeat_grace_s: std::env::var("CUA_MODAL_HEARTBEAT_GRACE_S")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(HEARTBEAT_GRACE_S),
    };
    let reap_every = std::env::var("CUA_MODAL_REAP_INTERVAL_S")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(REAP_INTERVAL_S)
        .max(1);
    let m = Modal::new(cfg, backend, keys, api_key)?;
    m.reconcile().await;
    tokio::spawn(containers::autoscaler(Arc::downgrade(&m)));
    // Ephemeral apps whose client stopped heartbeating end with their
    // sandboxes; snapshot images and ephemeral objects expire.
    let weak = Arc::downgrade(&m);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(reap_every)).await;
            let Some(m) = weak.upgrade() else { return };
            m.reap().await;
        }
    });
    Ok(m)
}

/// Tunnel traffic: `<port>-<sandbox id>.<domain>` resolves to a forward of
/// the sandbox's port.
pub struct Ports(pub Arc<Modal>);

impl Ports {
    /// Is `port` an encrypted port the sandbox declared (a public tunnel)?
    fn public_tunnel(&self, id: &str, port: u16) -> bool {
        self.0.db.read(|d| {
            d.sandboxes
                .values()
                .find(|s| s.id.eq_ignore_ascii_case(id) && s.running())
                .is_some_and(|rec| {
                    matches!(
                        rec.def().open_ports_oneof,
                        Some(pb::api::sandbox::OpenPortsOneof::OpenPorts(p))
                            if p.ports.iter().any(|x| x.port == u32::from(port) && !x.unencrypted)
                    )
                })
        })
    }

    /// The claims of a valid connect token in the request head for this
    /// sandbox and port.
    fn connect_claims(&self, id: &str, port: u16, head: &[u8]) -> Option<serde_json::Value> {
        let auth = crate::host::header_value(head, "authorization")?;
        let token = auth.strip_prefix("Bearer ")?;
        let claims = self.0.signer.verify(token, "conn").ok()?;
        (claims["sub"].as_str()?.eq_ignore_ascii_case(id)
            && claims["port"].as_u64()? == u64::from(port))
        .then_some(claims)
    }
}

#[async_trait::async_trait]
impl crate::host::PortResolver for Ports {
    async fn traffic_token(&self, _id: &str) -> Option<(String, String)> {
        None
    }

    /// A declared encrypted port is a public tunnel; any other port is
    /// reachable only with a connect token (`Sandbox.create_connect_token`).
    async fn traffic_token_for(&self, id: &str, port: u16) -> Option<(String, String)> {
        if self.public_tunnel(id, port) {
            return None;
        }
        // Never matches: the token is checked by `alternative_credential_for`.
        Some(("authorization".into(), ids::new_id("deny")))
    }

    async fn alternative_credential_for(&self, id: &str, port: u16, head: &[u8]) -> bool {
        self.connect_claims(id, port, head).is_some()
    }

    fn unauthorized_status(&self) -> http::StatusCode {
        http::StatusCode::UNAUTHORIZED
    }

    /// Ports declared with `h2_ports` negotiate HTTP/2.
    async fn wants_h2(&self, id: &str, port: u16) -> bool {
        self.0.db.read(|d| {
            d.sandboxes
                .values()
                .find(|s| s.id.eq_ignore_ascii_case(id) && s.running())
                .is_some_and(|rec| {
                    matches!(
                        rec.def().open_ports_oneof,
                        Some(pb::api::sandbox::OpenPortsOneof::OpenPorts(p))
                            if p.ports.iter().any(|x| x.port == u32::from(port)
                                && !x.unencrypted
                                && x.tunnel_type == Some(pb::api::TunnelType::H2 as i32))
                    )
                })
        })
    }

    /// Adds Modal's verified-metadata header for a connect token, takes the
    /// token out of the request, and ends the connection after the
    /// response so the next request is authorized again.
    async fn rewrite_head(&self, id: &str, port: u16, head: bytes::Bytes) -> bytes::Bytes {
        let Some(claims) = self.connect_claims(id, port, &head) else {
            return head;
        };
        let text = String::from_utf8_lossy(&head).into_owned();
        let upgrade = crate::host::header_value(&head, "upgrade").is_some();
        let mut lines: Vec<&str> = text.split("\r\n").collect();
        // The blank line ends the head.
        let end = lines
            .iter()
            .position(|l| l.is_empty())
            .unwrap_or(lines.len());
        let mut out: Vec<String> = Vec::new();
        for (i, l) in lines.drain(..end).enumerate() {
            let name = l
                .split_once(':')
                .map(|(k, _)| k.trim().to_ascii_lowercase())
                .unwrap_or_default();
            let drop = i > 0
                && (name == "authorization"
                    || name == "x-verified-user-data"
                    || (!upgrade && name == "connection"));
            if !drop {
                out.push(l.to_string());
            }
        }
        if let Some(meta) = claims["meta"].as_str().filter(|m| !m.is_empty()) {
            out.push(format!("X-Verified-User-Data: {meta}"));
        }
        if !upgrade {
            out.push("Connection: close".into());
        }
        let mut s = out.join("\r\n");
        s.push_str("\r\n\r\n");
        // Anything after the head (the start of a body) is kept.
        let body_at = text.find("\r\n\r\n").map(|i| i + 4).unwrap_or(text.len());
        let mut v = s.into_bytes();
        v.extend_from_slice(&head[body_at.min(head.len())..]);
        bytes::Bytes::from(v)
    }

    async fn resolve(&self, id: &str, port: u16) -> Result<String, (http::StatusCode, String)> {
        let rec = self
            .0
            .db
            .read(|d| {
                d.sandboxes
                    .values()
                    .find(|s| s.id.eq_ignore_ascii_case(id) && s.running())
                    .cloned()
            })
            .ok_or_else(|| {
                (
                    http::StatusCode::NOT_FOUND,
                    "The sandbox was not found".into(),
                )
            })?;
        // Reached only for a declared encrypted port or after the router
        // accepted a connect token for this port.
        let name = rec.cua_name.clone().ok_or_else(|| {
            (
                http::StatusCode::BAD_GATEWAY,
                "The sandbox is starting".to_string(),
            )
        })?;
        self.0
            .backend
            .forward(&rec.identity(), &name, port)
            .await
            .map_err(|e| {
                (
                    http::StatusCode::BAD_GATEWAY,
                    format!("port {port} is not reachable: {e}"),
                )
            })
    }
}

impl Modal {
    /// After a restart: sandboxes whose Space is gone end as terminated;
    /// those still running are supervised again.
    pub async fn reconcile(self: &Arc<Self>) {
        let live: Vec<sandbox::SandboxRec> = self.db.read(|d| {
            d.sandboxes
                .values()
                .filter(|s| s.running())
                .cloned()
                .collect()
        });
        for rec in live {
            let gone = match &rec.cua_name {
                None => true,
                Some(n) => matches!(
                    self.backend.liveness(&rec.identity(), n).await,
                    Ok(crate::backend::Liveness::Gone)
                ),
            };
            if gone {
                self.finish(
                    &rec.id,
                    sandbox::ResultRec {
                        status: sandbox::status::TERMINATED,
                        exitcode: 0,
                        exception: "the gateway restarted and the sandbox's Space was gone".into(),
                    },
                )
                .await;
                continue;
            }
            self.resume_supervision(&rec).await;
        }
    }
}

/// An ephemeral app's client heartbeats every 15 seconds; this many seconds
/// without one and the app is over.
pub const HEARTBEAT_GRACE_S: f64 = 90.0;

/// Seconds between housekeeping passes (`CUA_MODAL_REAP_INTERVAL_S` for tests).
const REAP_INTERVAL_S: u64 = 15;

impl Modal {
    /// Periodic housekeeping.
    pub async fn reap(self: &Arc<Self>) {
        let now = db::now_s();
        let lapsed: Vec<String> = self.db.read(|d| {
            d.apps
                .values()
                .filter(|a| {
                    a.ephemeral && !a.stopped && now - a.heartbeat_at > self.cfg.heartbeat_grace_s
                })
                .map(|a| a.id.clone())
                .collect()
        });
        for id in lapsed {
            self.db.write(|d| {
                if let Some(a) = d.apps.get_mut(&id) {
                    a.stopped = true;
                }
            });
            let sbs: Vec<String> = self.db.read(|d| {
                d.sandboxes
                    .values()
                    .filter(|s| s.app_id == id && s.running())
                    .map(|s| s.id.clone())
                    .collect()
            });
            for sb in sbs {
                self.terminate(&sb).await;
            }
        }
        // Ephemeral volumes, dicts and queues whose client is gone.
        let gone_vols: Vec<db::VolumeRec> = self.db.read(|d| {
            d.volumes
                .values()
                .filter(|v| v.ephemeral && now - v.heartbeat_at > self.cfg.heartbeat_grace_s)
                .cloned()
                .collect()
        });
        for v in gone_vols {
            let who = self.identity_of_key(&v.key_id).await;
            let caller = Caller(
                KeyEntry {
                    key: String::new(),
                    label: String::new(),
                    daemon: who.daemon,
                    token: who.token,
                },
                false,
            );
            self.vol_purge(&caller, &v).await;
            self.db.write(|d| d.volumes.remove(&v.id));
        }
        self.db.write(|d| {
            d.dicts
                .retain(|_, x| !x.ephemeral || now - x.heartbeat_at <= self.cfg.heartbeat_grace_s);
            d.queues
                .retain(|_, x| !x.ephemeral || now - x.heartbeat_at <= self.cfg.heartbeat_grace_s);
        });
        self.expire_images().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::mock::MockBackend;
    use axum::body::Body;
    use http_body_util::BodyExt;
    use prost::Message;
    use tower::ServiceExt;

    /// Every RPC of the pinned client's descriptors, as `Service/Method`.
    fn rpcs() -> Vec<(String, String)> {
        let fds = prost_types::FileDescriptorSet::decode(
            &include_bytes!("../../modal-proto/modal-1.6.2.fds")[..],
        )
        .expect("descriptor set");
        let mut out = Vec::new();
        for f in &fds.file {
            let pkg = f.package.clone().unwrap_or_default();
            for s in &f.service {
                for m in &s.method {
                    out.push((format!("{pkg}.{}", s.name()), m.name().to_string()));
                }
            }
        }
        out
    }

    fn rec(id: &str, name: Option<&str>, state: sandbox::SbState) -> sandbox::SandboxRec {
        sandbox::SandboxRec {
            id: id.into(),
            key_id: "k".into(),
            identity_daemon: None,
            identity_token: None,
            app_id: "ap-x".into(),
            name: None,
            tags: Default::default(),
            created_at: 0.0,
            image_id: "im-x".into(),
            task_id: format!("ta-{id}"),
            cua_name: name.map(str::to_string),
            state,
            started_at: 0.0,
            finished_at: 0.0,
            result: None,
            definition: String::new(),
            env: Default::default(),
            error: None,
            tunnels: Vec::new(),
            restored: false,
            v2: true,
            exit_snapshot: None,
            exit_snapshot_error: None,
            bucket_credentials: Default::default(),
        }
    }

    /// What the journal's sweeper must not end: live sandboxes' Spaces (also
    /// after a restart, from the database), Function containers' Spaces, and
    /// image builds in flight; a finished sandbox's Space is unclaimed.
    #[tokio::test]
    async fn claimed_spaces_are_what_the_records_still_own() {
        let (m, _dir) = Modal::for_tests(Arc::new(MockBackend::new())).unwrap();
        m.db.write(|d| {
            d.sandboxes.insert(
                "sb-1".into(),
                rec("sb-1", Some("modal-sb-1"), sandbox::SbState::Running),
            );
            d.sandboxes.insert(
                "sb-2".into(),
                rec("sb-2", Some("modal-sb-2"), sandbox::SbState::Finished),
            );
            d.sandboxes.insert(
                "sb-3".into(),
                rec("sb-3", None, sandbox::SbState::Scheduling),
            );
        });
        {
            let mut f = m.fns.lock().unwrap();
            let q = f.queues.entry("fu-1".into()).or_default();
            let now = tokio::time::Instant::now();
            q.containers.insert(
                "ta-1".into(),
                functions::ContainerInfo {
                    task_id: "ta-1".into(),
                    started_at: now,
                    ready: true,
                    running_inputs: 0,
                    last_busy: now,
                    stop: false,
                    cancel: Vec::new(),
                    terminate_on_cancel: false,
                    space: Some((crate::backend::Identity::default(), "modal-fn-1".into())),
                },
            );
        }
        let owned = vec!["modal-build-im-1".to_string(), "modal-sb-2".to_string()];
        let set = m.claimed_spaces(&owned).await;
        assert!(set.contains("modal-sb-1"));
        assert!(set.contains("modal-fn-1"));
        assert!(
            !set.contains("modal-sb-2"),
            "a finished sandbox's Space is not claimed"
        );
        assert!(!set.contains("modal-build-im-1"), "no build is in flight");
        m.live.builds.lock().await.insert(
            "h".into(),
            futures_util::FutureExt::shared(futures_util::FutureExt::boxed(
                futures_util::future::ready(Ok(Arc::new(image::Built::default()))),
            )),
        );
        let set = m.claimed_spaces(&owned).await;
        assert!(
            set.contains("modal-build-im-1"),
            "a build in flight claims its Space"
        );
    }

    /// Calls every RPC with an empty request; the ones the gateway serves
    /// answer something other than the generated "Not yet implemented".
    #[tokio::test]
    async fn the_served_rpcs_are_the_committed_list() {
        let (m, _dir) = Modal::for_tests(Arc::new(MockBackend::new())).unwrap();
        let app = m.router();
        let key = "modal_test";
        let tcr = m.signer.mint("tcr", "ta-nothing", 600);
        let mut served = Vec::new();
        let mut all = 0;
        for (svc, method) in rpcs() {
            all += 1;
            let path = format!("/{svc}/{method}");
            let mut req = http::Request::post(&path)
                .header("content-type", "application/grpc")
                .header("te", "trailers");
            if svc.ends_with("TaskCommandRouter") {
                req = req.header("authorization", format!("Bearer {tcr}"));
            } else {
                req = req
                    .header("x-modal-token-id", token_id(key))
                    .header("x-modal-token-secret", key);
            }
            let body = Body::from(vec![0u8; 5]);
            let resp = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                app.clone().oneshot(req.body(body).unwrap()),
            )
            .await;
            let Ok(Ok(resp)) = resp else {
                // It blocks on an empty request: it is served.
                served.push(format!("{svc}/{method}"));
                continue;
            };
            let mut status = resp
                .headers()
                .get("grpc-status")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            let mut message = resp
                .headers()
                .get("grpc-message")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            if status.is_none()
                && let Ok(Ok(c)) = tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    resp.into_body().collect(),
                )
                .await
                && let Some(t) = c.trailers()
            {
                status = t
                    .get("grpc-status")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
                message = t
                    .get("grpc-message")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
            }
            // `grpc-message` is percent-encoded.
            let default_stub = status.as_deref() == Some("12")
                && message.as_deref().is_some_and(|m| {
                    m.replace("%20", " ")
                        .contains(" is not available on Cua Spaces (")
                });
            if !default_stub {
                served.push(format!("{svc}/{method}"));
            }
        }
        served.sort();
        let listing = format!("{}\n", served.join("\n"));
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/modal-proto/served-rpcs.txt");
        if std::env::var("UPDATE_SERVED").as_deref() == Ok("1") {
            std::fs::write(path, &listing).unwrap();
        }
        let committed = std::fs::read_to_string(path).unwrap_or_default();
        assert_eq!(
            committed,
            listing,
            "the served RPCs changed ({} of {all}); run with UPDATE_SERVED=1 and update maps/modal-coverage.md",
            served.len()
        );
    }
}
