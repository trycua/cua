//! The Daytona provider: Daytona's REST API (`/api/...`), its in-sandbox
//! toolbox (`/api/toolbox/<id>/...`), preview URLs and the Socket.IO event
//! stream, served from Cua Spaces. Unmodified `daytona` SDK code runs with
//! `DAYTONA_API_URL` and `DAYTONA_API_KEY` (see `cua compat env`).
//!
//! The wire shapes follow the Apache-2.0 Daytona SDK and its generated
//! clients; the implementation is ours (no server code was read).

#![allow(clippy::result_large_err)] // handlers return a ready `Response` as their error

mod api;
mod build;
mod charts;
mod computer;
mod create;
mod dockerfile;
mod git;
mod guestctx;
mod interpreter;
mod lsp;
mod objectstore;
pub mod prewarm;
mod secrets;
mod snapshots;
mod socketio;
mod ssh;
mod stdio;
mod stores;
mod toolbox;
mod volumes;
mod warm;

use crate::auth::{KeyEntry, Keys};
use crate::backend::{Backend, BackendError, Identity, Liveness};
use crate::e2b::TemplateTarget;
use crate::state::{Record, Store};
use axum::Router;
use axum::body::Body;
use http::{HeaderMap, Response, StatusCode, header};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock, broadcast};

/// Daytona provider settings.
#[derive(Debug, Clone)]
pub struct DaytonaConfig {
    /// `toolboxProxyUrl` returned in every sandbox (`http://<api>/api/toolbox`).
    pub toolbox_url: String,
    /// Preview host domain (`daytona.localhost:<host port>`).
    pub preview_domain: String,
    /// The guest user for Daytona's default user.
    pub default_user: String,
    pub create_default_user: bool,
    /// Snapshot names registered with `POST /snapshots` or `templates.json`.
    pub snapshots_file: Option<PathBuf>,
    /// Where volumes and secrets are kept (next to `snapshots_file`).
    pub dir: Option<PathBuf>,
}

impl DaytonaConfig {
    /// The gateway's base URL (`http://<api>`), from the toolbox URL.
    pub fn api_base(&self) -> String {
        self.toolbox_url
            .strip_suffix("/api/toolbox")
            .unwrap_or(&self.toolbox_url)
            .to_string()
    }

    /// The object storage endpoint clients upload build contexts to: HTTPS
    /// on the host router (`s3.<domain>`), because the Python SDK's S3
    /// client refuses plain HTTP.
    pub fn object_url(&self) -> String {
        format!("https://s3.{}", self.preview_domain)
    }
}

/// A snapshot name Daytona clients use, and what it is in Cua.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SnapshotEntry {
    pub id: String,
    pub name: String,
    pub target: TemplateTarget,
    #[serde(default = "active")]
    pub state: String,
    pub created_at_ms: u64,
    #[serde(default)]
    pub source_sandbox_id: Option<String>,
    /// A snapshot made from a Dockerfile: built into image layers each time
    /// a sandbox starts from it (the core caches the build by content).
    #[serde(default)]
    pub build: Option<SnapshotBuild>,
    /// The key that made it (`None`: registered by the operator, every key
    /// sees it).
    #[serde(default)]
    pub key_id: Option<String>,
    /// Resources sandboxes started from it get (cores, GiB; 0: default).
    #[serde(default)]
    pub cpu: u32,
    #[serde(default)]
    pub memory: u32,
    #[serde(default)]
    pub disk: u32,
    #[serde(default)]
    pub gpu: u32,
    #[serde(default)]
    pub entrypoint: Option<Vec<String>>,
    /// The image shown as `imageName` (the Dockerfile's base for a build).
    #[serde(default)]
    pub image_name: Option<String>,
    #[serde(default)]
    pub error_reason: Option<String>,
    #[serde(default)]
    pub updated_ms: Option<u64>,
    #[serde(default)]
    pub last_used_ms: Option<u64>,
}

impl SnapshotEntry {
    pub fn visible_to(&self, key_id: &str) -> bool {
        self.key_id.as_deref().is_none_or(|k| k == key_id)
    }
}

/// What a Dockerfile snapshot is built from.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SnapshotBuild {
    pub dockerfile: String,
    #[serde(default)]
    pub context_hashes: Vec<String>,
}

fn active() -> String {
    "active".into()
}

pub struct Daytona {
    pub cfg: DaytonaConfig,
    pub backend: Arc<dyn Backend>,
    pub store: Arc<Store>,
    pub keys: Arc<RwLock<Keys>>,
    clients: Mutex<HashMap<String, cua_spacesd_client::SpacesdClient>>,
    homes: Mutex<HashMap<(String, String), String>>,
    /// `(key id, event name, payload)` for the Socket.IO stream.
    pub events: broadcast::Sender<(String, String, Value)>,
    pub sessions: toolbox::Sessions,
    pub ptys: toolbox::Ptys,
    /// Signed preview tokens: token -> (sandbox id, port, expiry unix ms).
    pub signed: Mutex<HashMap<String, (String, u16, u64)>>,
    pub snapshots: Mutex<Vec<SnapshotEntry>>,
    pub volumes: stores::Persist<stores::VolumeEntry>,
    pub secrets: stores::Persist<stores::SecretEntry>,
    pub warm_pools: stores::Persist<stores::WarmPoolEntry>,
    pub objects: objectstore::ObjectStore,
    pub build_logs: Mutex<HashMap<String, Arc<build::LogBuf>>>,
    pub interp: interpreter::Registry,
    pub lsp: lsp::Registry,
    pub computer: computer::Registry,
    pub ssh: ssh::Registry,
    /// The guest shell per sandbox (zsh, bash or sh).
    pub shells: Mutex<HashMap<String, String>>,
    /// The user each sandbox's cua-spacesd runs as.
    pub drivers: crate::guest::Drivers,
}

impl Daytona {
    pub fn new(
        cfg: DaytonaConfig,
        backend: Arc<dyn Backend>,
        store: Arc<Store>,
        keys: Arc<RwLock<Keys>>,
        seed: HashMap<String, TemplateTarget>,
    ) -> Arc<Self> {
        let mut snapshots: Vec<SnapshotEntry> = cfg
            .snapshots_file
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        for (name, target) in seed {
            if !snapshots.iter().any(|s| s.name == name) {
                snapshots.push(SnapshotEntry {
                    id: name.clone(),
                    name,
                    target,
                    state: active(),
                    created_at_ms: crate::state::now_ms(),
                    source_sandbox_id: None,
                    build: None,
                    key_id: None,
                    cpu: 0,
                    memory: 0,
                    disk: 0,
                    gpu: 0,
                    entrypoint: None,
                    image_name: None,
                    error_reason: None,
                    updated_ms: None,
                    last_used_ms: None,
                });
            }
        }
        let cfg_dir = cfg.dir.clone();
        Arc::new(Self {
            cfg,
            backend,
            store,
            keys,
            clients: Mutex::default(),
            homes: Mutex::default(),
            events: broadcast::channel(256).0,
            sessions: toolbox::Sessions::default(),
            ptys: toolbox::Ptys::default(),
            signed: Mutex::default(),
            snapshots: Mutex::new(snapshots),
            volumes: stores::Persist::open(cfg_dir.as_ref().map(|d| d.join("volumes.json"))),
            secrets: stores::Persist::open(cfg_dir.as_ref().map(|d| d.join("secrets.json"))),
            warm_pools: stores::Persist::open(cfg_dir.as_ref().map(|d| d.join("warm-pools.json"))),
            objects: objectstore::ObjectStore::new(
                cfg_dir
                    .clone()
                    .unwrap_or_else(|| {
                        std::env::temp_dir().join(format!("cua-daytona-{}", std::process::id()))
                    })
                    .join("objects-root"),
            ),
            build_logs: Mutex::default(),
            interp: Default::default(),
            lsp: Default::default(),
            computer: Default::default(),
            ssh: Default::default(),
            shells: Mutex::default(),
            drivers: Default::default(),
        })
    }

    pub async fn save_snapshots(&self) {
        let Some(p) = &self.cfg.snapshots_file else {
            return;
        };
        let list = self.snapshots.lock().await.clone();
        if let Err(e) = crate::tls::write_file(
            p,
            &serde_json::to_vec_pretty(&list).expect("snapshots"),
            0o600,
        ) {
            tracing::warn!("compat: saving {}: {e}", p.display());
        }
    }

    /// The caller's key (`Authorization: Bearer <key>`).
    pub async fn caller(&self, headers: &HeaderMap) -> Option<KeyEntry> {
        let presented = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))?
            .trim();
        self.keys.read().await.find(presented).cloned()
    }

    pub fn identity(key: &KeyEntry) -> Identity {
        Identity {
            daemon: key.daemon.clone(),
            token: key.token.clone(),
        }
    }

    /// A record of the caller by id or name.
    pub async fn find(&self, key: &KeyEntry, id_or_name: &str) -> Option<Record> {
        let kid = key.id();
        self.store.all().await.into_iter().find(|r| {
            r.key_id == kid
                && (r.id == id_or_name
                    || r.extra.get("name").and_then(Value::as_str) == Some(id_or_name))
        })
    }

    pub async fn spacesd(
        &self,
        r: &Record,
    ) -> Result<cua_spacesd_client::SpacesdClient, BackendError> {
        if let Some(c) = self.clients.lock().await.get(&r.id) {
            return Ok(c.clone());
        }
        let c = self.backend.spacesd(&r.identity(), &r.cua_name).await?;
        self.clients.lock().await.insert(r.id.clone(), c.clone());
        Ok(c)
    }

    pub async fn forget(&self, id: &str) {
        self.clients.lock().await.remove(id);
        self.homes.lock().await.retain(|(sid, _), _| sid != id);
        self.shells.lock().await.remove(id);
        self.drivers.forget(id).await;
    }

    pub async fn home(&self, r: &Record, user: &str) -> String {
        let key = (r.id.clone(), user.to_string());
        if let Some(h) = self.homes.lock().await.get(&key) {
            return h.clone();
        }
        let fallback = if user == "root" {
            "/root".to_string()
        } else {
            format!("/home/{user}")
        };
        let home = match self.spacesd(r).await {
            Ok(c) => {
                let cmd = cua_spacesd_client::Command::new("getent").args(["passwd", user]);
                match tokio::time::timeout(std::time::Duration::from_secs(10), c.run(cmd)).await {
                    Ok(Ok(out)) if out.success() => String::from_utf8_lossy(&out.stdout)
                        .lines()
                        .next()
                        .and_then(|l| l.split(':').nth(5))
                        .filter(|h| h.starts_with('/'))
                        .map(str::to_string)
                        .unwrap_or(fallback),
                    _ => fallback,
                }
            }
            Err(_) => fallback,
        };
        self.homes.lock().await.insert(key, home.clone());
        home
    }

    /// The sandbox's user (`user` at create, default the configured one).
    pub fn user_of(&self, r: &Record) -> String {
        r.extra
            .get("user")
            .and_then(Value::as_str)
            .filter(|u| !u.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| self.cfg.default_user.clone())
    }

    pub async fn kill(&self, id: &str) -> bool {
        self.forget(id).await;
        self.sessions.drop_sandbox(id).await;
        self.interp.drop_sandbox(id).await;
        self.lsp.drop_sandbox(id).await;
        self.computer.drop_sandbox(id).await;
        self.ssh.drop_sandbox(id).await;
        crate::state::kill(&self.store, self.backend.as_ref(), id).await
    }

    /// Publishes a state change on the event stream.
    pub async fn announce(&self, r: &Record, old: &str) {
        let dto = api::sandbox_dto(self, r);
        let new = dto["state"].clone();
        let _ = self.events.send((
            r.key_id.clone(),
            "sandbox.state.updated".into(),
            json!({"sandbox": dto, "oldState": old, "newState": new}),
        ));
    }

    pub async fn check_gone(&self, r: &Record) -> bool {
        // A sandbox whose image is still building has no Cua sandbox yet.
        if matches!(
            r.extra.get("dtState").and_then(Value::as_str),
            Some("pending_build" | "building_snapshot" | "resizing")
        ) {
            return false;
        }
        if let Ok(Liveness::Gone) = self.backend.liveness(&r.identity(), &r.cua_name).await {
            self.kill(&r.id).await;
            return true;
        }
        false
    }
}

/// A new Daytona-style id (a v4 UUID).
pub fn api_uuid() -> String {
    api::uuid()
}

/// The `source` of an error from Daytona's API (the control plane).
pub const SOURCE_API: &str = "DAYTONA_API";
/// The `source` of an error from the sandbox daemon (the toolbox).
pub const SOURCE_DAEMON: &str = "DAYTONA_DAEMON";

/// Daytona's error body: `{statusCode, message, error}` from the API
/// (no machine code: the SDK maps the status).
pub fn error(status: StatusCode, message: impl Into<String>) -> Response<Body> {
    coded(status, SOURCE_API, None, message)
}

/// An error from the sandbox daemon, with its machine-readable `code`
/// (`FILE_NOT_FOUND`, `GIT_AUTH_FAILED`, ...). The SDKs map
/// `(source, code)` to the precise exception class before the status
/// (`daytona/common/errors.py` `CODE_TO_ERROR`, SDK 0.225.0).
pub fn daemon_error(status: StatusCode, code: &str, message: impl Into<String>) -> Response<Body> {
    coded(status, SOURCE_DAEMON, Some(code), message)
}

/// A daemon error with no machine code (the SDK maps its status).
pub fn daemon_plain(status: StatusCode, message: impl Into<String>) -> Response<Body> {
    coded(status, SOURCE_DAEMON, None, message)
}

fn coded(
    status: StatusCode,
    source: &str,
    code: Option<&str>,
    message: impl Into<String>,
) -> Response<Body> {
    let mut body = json!({
        "statusCode": status.as_u16(),
        "message": message.into(),
        "error": status.canonical_reason().unwrap_or(""),
        // The toolbox's ErrorResponse also requires these.
        "path": "",
        "timestamp": time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
        "source": source,
    });
    if let Some(c) = code {
        body["code"] = json!(c);
    }
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("response")
}

pub fn ok_json(v: Value) -> Response<Body> {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(v.to_string()))
        .expect("response")
}

pub fn backend_error(e: BackendError) -> Response<Body> {
    let status = match e {
        BackendError::NotFound(_) => StatusCode::NOT_FOUND,
        BackendError::Invalid(_) | BackendError::Unsupported(_) => StatusCode::BAD_REQUEST,
        BackendError::Timeout(_) => StatusCode::GATEWAY_TIMEOUT,
        BackendError::RateLimited(_) => StatusCode::TOO_MANY_REQUESTS,
        BackendError::Other(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    error(status, e.to_string())
}

/// The Daytona routes: `/api/...`, `/api/toolbox/<id>/...` and
/// `/api/socket.io/`.
pub fn router(d: Arc<Daytona>) -> Router {
    Router::new()
        .nest("/api/toolbox", toolbox::routes(d.clone()))
        .merge(socketio::routes(d.clone()))
        .merge(snapshots::routes(d.clone()))
        .merge(ssh::routes(d.clone()))
        .merge(volumes::routes(d.clone()))
        .merge(warm::routes(d.clone()))
        .merge(secrets::routes(d.clone()))
        .merge(objectstore::routes(d.clone()))
        .merge(api::routes(d))
}

/// Preview traffic: `<port>-<id>.<domain>` or `<port>-<signed token>.<domain>`.
pub struct Ports(pub Arc<Daytona>);

#[async_trait::async_trait]
impl crate::host::PortResolver for Ports {
    async fn traffic_token(&self, id: &str) -> Option<(String, String)> {
        if self.0.signed.lock().await.contains_key(id) {
            return None;
        }
        let r = self.0.store.get(id).await?;
        if r.extra.get("public").and_then(Value::as_bool) == Some(true) {
            return None;
        }
        r.traffic_token
            .map(|t| ("x-daytona-preview-token".to_string(), t))
    }

    fn unauthorized_status(&self) -> StatusCode {
        StatusCode::UNAUTHORIZED
    }

    async fn alternative_credential(&self, id: &str, head: &[u8]) -> bool {
        let Some(key) = crate::host::header_value(head, "authorization")
            .and_then(|v| v.strip_prefix("Bearer ").map(|k| k.trim().to_string()))
        else {
            return false;
        };
        let Some(entry) = self.0.keys.read().await.find(&key).cloned() else {
            return false;
        };
        self.0
            .store
            .get(id)
            .await
            .is_some_and(|r| r.key_id == entry.id())
    }

    async fn resolve(&self, id: &str, port: u16) -> Result<String, (StatusCode, String)> {
        let d = &self.0;
        let id = match d.signed.lock().await.get(id) {
            Some((sid, p, exp)) => {
                if *p != port || *exp < crate::state::now_ms() {
                    return Err((StatusCode::FORBIDDEN, "signed preview URL expired".into()));
                }
                sid.clone()
            }
            None => id.to_string(),
        };
        let Some(r) = d.store.get(&id).await else {
            return Err((StatusCode::NOT_FOUND, "sandbox not found".into()));
        };
        if r.paused {
            return Err((StatusCode::BAD_GATEWAY, "sandbox is not started".into()));
        }
        api::touch(d, &r).await;
        d.backend
            .forward(&r.identity(), &r.cua_name, port)
            .await
            .map_err(|e| {
                (
                    StatusCode::BAD_GATEWAY,
                    format!("port {port} is not reachable: {e}"),
                )
            })
    }
}
