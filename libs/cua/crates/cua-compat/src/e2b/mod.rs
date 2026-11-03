//! The E2B provider: E2B's REST control plane and envd (the in-sandbox
//! agent's Connect-RPC process and filesystem services, `/files`,
//! `/health`), served from Cua Spaces. Unmodified `e2b` SDK code runs
//! against it with `E2B_API_URL`, `E2B_SANDBOX_URL` and `E2B_API_KEY` (see
//! `cua compat env`).
//!
//! The wire shapes follow E2B's public API spec and envd protos (Apache-2.0,
//! e2b-dev/infra) and the MIT `e2b` SDK; the implementation is ours.

mod api;
mod code_interpreter;
mod egress;
mod envd;
mod files;
mod fsops;
mod golden;
mod pb;
pub mod prewarm;
mod secrets;
pub(crate) mod templates;
mod watch;

pub use pb::{permissions_string, timestamp as pb_timestamp};

use crate::auth::{KeyEntry, Keys};
use crate::backend::{Backend, BackendError, Liveness};
use crate::state::{Record, Store};
use axum::Router;
use axum::body::Body;
use http::{HeaderMap, Response, StatusCode, header};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

/// The envd version the gateway reports. It gates SDK features, all served:
/// default users, stdin, octet-stream uploads, recursive watch, file
/// metadata (0.6.2, as `user.e2b.*` extended attributes), entries on watch
/// events (0.6.3) and the network-mount watch flag (0.6.4).
pub const ENVD_VERSION: &str = "0.6.4";
/// envd's port inside an E2B sandbox (host routing: `49983-<id>.<domain>`).
pub const ENVD_PORT: u16 = 49983;
/// E2B's default sandbox lifetime (seconds).
pub const DEFAULT_TIMEOUT_S: u64 = 300;

/// What `create(template=...)` names.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemplateTarget {
    /// A Cua image (a fresh Space).
    Image(String),
    /// A Cua checkpoint (a fork).
    Checkpoint(String),
}

/// E2B provider settings.
#[derive(Debug, Clone)]
pub struct E2bConfig {
    /// The sandbox domain returned to clients (host routing for
    /// `get_host`), e.g. `e2b.localhost:49443`.
    pub sandbox_domain: String,
    /// The guest user for E2B's default user (`user`).
    pub default_user: String,
    /// Create `default_user` (with passwordless sudo, like E2B's base
    /// template) in new image-based sandboxes that lack it.
    pub create_default_user: bool,
    /// Template aliases: E2B template or snapshot id -> Cua image or
    /// checkpoint (`templates.json`).
    pub templates: HashMap<String, TemplateTarget>,
}

impl Default for E2bConfig {
    fn default() -> Self {
        Self {
            sandbox_domain: "e2b.localhost".into(),
            default_user: "user".into(),
            create_default_user: true,
            templates: HashMap::new(),
        }
    }
}

/// The E2B provider's shared state.
pub struct E2b {
    pub cfg: E2bConfig,
    pub backend: Arc<dyn Backend>,
    pub store: Arc<Store>,
    pub keys: Arc<RwLock<Keys>>,
    pub secrets: Arc<secrets::Secrets>,
    pub templates: Arc<templates::Templates>,
    clients: Mutex<HashMap<String, cua_spacesd_client::SpacesdClient>>,
    homes: Mutex<HashMap<(String, String), String>>,
    /// Users known to exist in a sandbox (`getent passwd`).
    users: Mutex<std::collections::HashSet<(String, String)>>,
    /// Polling watchers: id -> watched directory (for relative names).
    pub watchers: Mutex<HashMap<String, watch::Watcher>>,
    /// The user each sandbox's cua-spacesd runs as.
    pub drivers: crate::guest::Drivers,
    /// Booted base sandboxes forked for each create ([`golden`]).
    pub golden: golden::Golden,
}

impl E2b {
    pub fn new(
        cfg: E2bConfig,
        backend: Arc<dyn Backend>,
        store: Arc<Store>,
        keys: Arc<RwLock<Keys>>,
        dir: Option<std::path::PathBuf>,
    ) -> Arc<Self> {
        Arc::new(Self {
            cfg,
            backend,
            store,
            keys,
            secrets: secrets::Secrets::open(dir.as_ref().map(|d| d.join("secrets.json")))
                .expect("secrets file"),
            templates: templates::Templates::open(
                dir.as_ref().map(|d| d.join("built-templates.json")),
                dir.as_ref()
                    .map(|d| d.join("template-uploads"))
                    .unwrap_or_else(|| std::env::temp_dir().join("cua-compat-template-uploads")),
            )
            .expect("templates file"),
            clients: Mutex::default(),
            homes: Mutex::default(),
            users: Mutex::default(),
            watchers: Mutex::default(),
            drivers: Default::default(),
            golden: Default::default(),
        })
    }

    /// The API key entry for a request (`X-API-KEY`), if valid.
    pub async fn caller(&self, headers: &HeaderMap) -> Option<KeyEntry> {
        let presented = headers.get("x-api-key").and_then(|v| v.to_str().ok())?;
        self.keys.read().await.find(presented).cloned()
    }

    /// The sandbox's spacesd client (cached).
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

    /// Forgets cached connections to a sandbox (after kill, pause).
    pub async fn forget(&self, id: &str) {
        self.clients.lock().await.remove(id);
        self.homes.lock().await.retain(|(sid, _), _| sid != id);
        self.users.lock().await.retain(|(sid, _)| sid != id);
        self.drivers.forget(id).await;
    }

    /// Deletes a sandbox the gateway made.
    pub async fn kill(&self, id: &str) -> bool {
        self.forget(id).await;
        crate::state::kill(&self.store, self.backend.as_ref(), id).await
    }

    /// Resumes a paused sandbox (a request to it with auto-resume, or
    /// `connect`). A sandbox paused without its memory (`pause(mode=
    /// "filesystem")`, `connect(on_resume="reboot")`, auto-pause without
    /// memory) comes back as a reboot from its disk: processes and tmpfs
    /// contents are gone.
    pub async fn resume(&self, r: &Record) -> Result<(), BackendError> {
        self.backend.resume(&r.identity(), &r.cua_name).await?;
        self.forget(&r.id).await;
        self.store.update(&r.id, |r| r.paused = false).await;
        let reboot = self.store.get(&r.id).await.is_some_and(|r| {
            r.extra
                .get("reboot_on_resume")
                .and_then(serde_json::Value::as_bool)
                == Some(true)
        });
        if reboot {
            self.reboot_guest(r).await?;
            self.store
                .update(&r.id, |r| {
                    r.extra.remove("reboot_on_resume");
                })
                .await;
        }
        Ok(())
    }

    /// Reboots the guest operating system (disk kept, processes gone) and
    /// waits until cua-spacesd answers from the new boot. QEMU keeps the VM
    /// across a guest reboot; a kind whose VM ends with it fails the wait.
    pub async fn reboot_guest(&self, r: &Record) -> Result<(), BackendError> {
        use cua_spacesd_client::{Command, pb};
        let boot_id = |c: cua_spacesd_client::SpacesdClient| async move {
            tokio::time::timeout(
                std::time::Duration::from_secs(5),
                c.run(Command::new("cat").arg("/proc/sys/kernel/random/boot_id")),
            )
            .await
            .ok()
            .and_then(Result::ok)
            .filter(|o| o.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        };
        let c = self.spacesd(r).await?;
        let before = boot_id(c.clone()).await;
        let driver = self.drivers.of(&r.id, &c).await;
        let (command, args, user, cwd) = crate::guest::as_user_in(
            &driver,
            "root",
            "/bin/sh".into(),
            vec![
                "-c".into(),
                "sleep 1; exec systemctl reboot || exec reboot".into(),
            ],
            String::new(),
        );
        let started = c
            .process()
            .start_process(pb::StartProcessRequest {
                config: Some(pb::ProcessConfig {
                    command,
                    args,
                    user,
                    cwd,
                    ..Default::default()
                }),
                kill_on_disconnect: false,
                ..Default::default()
            })
            .await
            .map_err(|e| BackendError::Other(format!("rebooting the guest: {e}")))?;
        drop(started);
        self.forget(&r.id).await;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(240);
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            if let Ok(c) = self.backend.spacesd(&r.identity(), &r.cua_name).await
                && let Some(now) = boot_id(c).await
                && Some(&now) != before.as_ref()
            {
                self.forget(&r.id).await;
                if code_interpreter::wants(&r.template_id)
                    && let Ok(c) = self.spacesd(r).await
                {
                    let driver = self.drivers.of(&r.id, &c).await;
                    code_interpreter::provision(self, &c, &driver).await;
                }
                return Ok(());
            }
            if std::time::Instant::now() > deadline {
                return Err(BackendError::Timeout(
                    "the sandbox did not come back from its reboot".into(),
                ));
            }
        }
    }

    /// Whether `user` exists in the sandbox: envd answers 401 `invalid
    /// username` when it does not. (A guest without `getent` is trusted.)
    pub async fn check_user(
        &self,
        r: &Record,
        c: &cua_spacesd_client::SpacesdClient,
        user: &str,
    ) -> Result<(), crate::connect::RpcError> {
        let key = (r.id.clone(), user.to_string());
        if user.is_empty() || self.users.lock().await.contains(&key) {
            return Ok(());
        }
        let cmd = cua_spacesd_client::Command::new("getent").args(["passwd", user]);
        match tokio::time::timeout(std::time::Duration::from_secs(10), c.run(cmd)).await {
            Ok(Ok(out)) if out.status.code == Some(2) => Err(crate::connect::RpcError::new(
                crate::connect::Code::Unauthenticated,
                format!("invalid username: '{user}'"),
            )),
            _ => {
                self.users.lock().await.insert(key);
                Ok(())
            }
        }
    }

    /// The home directory of `user` in a sandbox without a record (a
    /// template build).
    pub async fn home_of(
        &self,
        c: &cua_spacesd_client::SpacesdClient,
        _driver: &str,
        user: &str,
    ) -> String {
        let cmd = cua_spacesd_client::Command::new("getent").args(["passwd", user]);
        if let Ok(Ok(out)) =
            tokio::time::timeout(std::time::Duration::from_secs(10), c.run(cmd)).await
            && out.success()
            && let Some(h) = String::from_utf8_lossy(&out.stdout)
                .lines()
                .next()
                .and_then(|l| l.split(':').nth(5))
                .filter(|h| h.starts_with('/'))
        {
            return h.to_string();
        }
        if user == "root" {
            "/root".into()
        } else {
            format!("/home/{user}")
        }
    }

    /// The home directory of `user` in a sandbox (`getent passwd`, cached;
    /// `/root` or `/home/<user>` when it cannot be read).
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

    /// The live record for an envd request, or the error response E2B's
    /// edge gives (502 "sandbox not found", 401 bad token). A paused
    /// sandbox with auto-resume is resumed here.
    pub async fn envd_record(
        &self,
        headers: &HeaderMap,
        host_id: Option<&str>,
    ) -> Result<Record, Response<Body>> {
        self.envd_record_with(headers, host_id, true).await
    }

    /// [`Self::envd_record`], optionally without the access-token check
    /// (envd's `/health` answers without one, like E2B's).
    pub async fn envd_record_with(
        &self,
        headers: &HeaderMap,
        host_id: Option<&str>,
        require_token: bool,
    ) -> Result<Record, Response<Body>> {
        let id = headers
            .get("e2b-sandbox-id")
            .and_then(|v| v.to_str().ok())
            .or(host_id)
            .unwrap_or_default()
            .to_string();
        let Some(r) = self.store.get(&id).await else {
            return Err(not_found_edge(&id));
        };
        let token = headers
            .get("x-access-token")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        if require_token && !crate::auth::same(token, &r.access_token) {
            return Err(edge_error(
                StatusCode::UNAUTHORIZED,
                "unauthorized: invalid or missing X-Access-Token",
                &id,
            ));
        }
        if r.paused {
            if !r.auto_resume {
                // E2B's edge does not know a paused sandbox that cannot
                // auto-resume: it answers like a missing one.
                return Err(not_found_edge(&id));
            }
            if let Err(e) = self.resume(&r).await {
                return Err(edge_error(
                    StatusCode::BAD_GATEWAY,
                    &format!("resuming the sandbox failed: {e}"),
                    &id,
                ));
            }
        }
        Ok(r)
    }

    /// Whether a record's sandbox is gone (deleted outside the gateway);
    /// drops the record when it is.
    pub async fn check_gone(&self, r: &Record) -> bool {
        if let Ok(Liveness::Gone) = self.backend.liveness(&r.identity(), &r.cua_name).await {
            self.kill(&r.id).await;
            return true;
        }
        false
    }
}

/// The user an envd request runs as: `Authorization: Basic base64(user:)`,
/// else the default user.
pub fn request_user(headers: &HeaderMap, default_user: &str) -> String {
    use base64::Engine;
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "))
        .and_then(|b| {
            base64::engine::general_purpose::STANDARD
                .decode(b.trim())
                .ok()
        })
        .and_then(|b| String::from_utf8(b).ok())
        .map(|s| s.split(':').next().unwrap_or_default().to_string())
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| default_user.to_string())
}

/// The default user of a sandbox: the built template's `USER`, else E2B's
/// default (`user`).
pub fn default_user_of<'a>(e: &'a E2b, r: &'a Record) -> &'a str {
    r.extra
        .get("user")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(&e.cfg.default_user)
}

/// Resolves a path the way envd does: `~` and relative paths are under
/// the user's home.
pub fn resolve_path(path: &str, home: &str) -> String {
    let joined = if path == "~" {
        home.to_string()
    } else if let Some(rest) = path.strip_prefix("~/") {
        format!("{home}/{rest}")
    } else if path.starts_with('/') {
        path.to_string()
    } else {
        format!("{home}/{path}")
    };
    normalize(&joined)
}

fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for p in path.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    format!("/{}", parts.join("/"))
}

/// E2B's edge error for an unknown sandbox (the SDK maps it to its
/// "sandbox not found / timeout" exception).
pub fn not_found_edge(id: &str) -> Response<Body> {
    edge_error(StatusCode::BAD_GATEWAY, "The sandbox was not found", id)
}

pub fn edge_error(status: StatusCode, message: &str, id: &str) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({"sandboxId": id, "message": message, "code": status.as_u16()}).to_string(),
        ))
        .expect("valid response")
}

/// The E2B routes (REST control plane and envd) on one router.
pub fn router(e2b: Arc<E2b>) -> Router {
    api::routes(e2b.clone()).merge(envd::routes(e2b))
}

/// `get_host` traffic: resolves `<port>-<id>` to a forward of the
/// sandbox's port (resuming it first when it auto-resumes).
pub struct Ports(pub Arc<E2b>);

#[async_trait::async_trait]
impl crate::host::PortResolver for Ports {
    async fn traffic_token(&self, id: &str) -> Option<(String, String)> {
        let r = self.0.store.get(id).await?;
        r.traffic_token
            .map(|t| ("e2b-traffic-access-token".to_string(), t))
    }

    async fn resolve(&self, id: &str, port: u16) -> Result<String, (StatusCode, String)> {
        let e = &self.0;
        let Some(r) = e.store.get(id).await else {
            return Err((StatusCode::BAD_GATEWAY, "The sandbox was not found".into()));
        };
        if r.paused {
            if !r.auto_resume {
                return Err((StatusCode::BAD_GATEWAY, "The sandbox was not found".into()));
            }
            e.resume(&r).await.map_err(|err| {
                (
                    StatusCode::BAD_GATEWAY,
                    format!("resuming the sandbox: {err}"),
                )
            })?;
        }
        e.backend
            .forward(&r.identity(), &r.cua_name, port)
            .await
            .map_err(|err| match err {
                BackendError::NotFound(_) => {
                    (StatusCode::BAD_GATEWAY, "The sandbox was not found".into())
                }
                other => (
                    StatusCode::BAD_GATEWAY,
                    format!("The sandbox is running but port is not open ({other})"),
                ),
            })
    }
}

/// The Host header's `49983-<id>.<domain>` sandbox id, when the request
/// came through the TLS host router.
#[derive(Clone, Debug)]
pub struct HostSandbox(pub String);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_paths_like_envd() {
        assert_eq!(resolve_path("a.txt", "/home/user"), "/home/user/a.txt");
        assert_eq!(resolve_path("~/x/../y", "/home/user"), "/home/user/y");
        assert_eq!(resolve_path("/tmp//a/./b", "/root"), "/tmp/a/b");
        assert_eq!(resolve_path("~", "/root"), "/root");
    }

    #[test]
    fn reads_the_basic_auth_user() {
        let mut h = HeaderMap::new();
        assert_eq!(request_user(&h, "user"), "user");
        h.insert(header::AUTHORIZATION, "Basic cm9vdDo=".parse().unwrap()); // "root:"
        assert_eq!(request_user(&h, "user"), "root");
    }
}
