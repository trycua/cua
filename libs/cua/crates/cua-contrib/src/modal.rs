//! Modal Sandboxes (<https://modal.com/docs/guide/sandboxes>).
//!
//! Modal documents no HTTP API: its clients speak an internal gRPC API whose
//! proto warns that direct use is discouraged and not compatibility-
//! guaranteed. So this provider goes through Modal's **official Go SDK**
//! (`github.com/modal-labs/modal-client/go`, Apache-2.0) in a small helper
//! binary, `cua-modal-helper` (source: `crates/cua-contrib/modal-helper`,
//! `go build`), speaking one JSON request and response per call over
//! stdin/stdout. That is the least-maintenance route: Modal's own client
//! keeps up with its wire protocol; this module only maps the core's
//! requests.
//!
//! - **Image**: `Image.FromRegistry(<pinned ref>)` plus an `ENTRYPOINT []`
//!   layer (Modal keeps an image's ENTRYPOINT and passes the sandbox command
//!   as its arguments), then the image's `ENTRYPOINT` + `CMD` (from its
//!   registry config), or the create's command, as the sandbox command.
//!   Modal builds and caches both layers itself.
//! - **Entrypoint and environment**: the command runs with the sandbox
//!   environment, so the spacesd token is delivered at create.
//! - **Ports**: `EncryptedPorts` (TLS tunnels, HTTP/1.1; cua-spacesd answers
//!   gRPC-Web there); tunnels are fixed at create.
//! - **Auth**: `MODAL_TOKEN_ID` + `MODAL_TOKEN_SECRET` (or `cua auth provider
//!   set modal`), passed to the helper's environment only.
//! - **Helper**: `CUA_MODAL_HELPER`, else `cua-modal-helper` next to the
//!   running executable, else on `PATH`.

use crate::{
    MODAL_ENV,
    common::{self, CredentialStore, Secret},
    image_config::{self, ImageConfigSource},
};
use cua_sandbox_core::{
    Error, ImageMode, PortExposure, Provider, ProviderCapabilities, ProviderCreate,
    ProviderInstance, Result, RunKind, ServiceEndpoint, Status,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The helper's file name.
pub const HELPER: &str = "cua-modal-helper";
/// Default sandbox lifetime when the create sets none.
pub const DEFAULT_TTL: Duration = Duration::from_secs(3600);
/// Modal's longest sandbox lifetime.
const MAX_TTL: Duration = Duration::from_secs(24 * 3600);
/// The most the helper may print.
const MAX_REPLY: u64 = 4 << 20;

/// Modal configuration.
#[derive(Clone)]
pub struct ModalConfig {
    /// `MODAL_TOKEN_ID`.
    pub token_id: Option<Secret>,
    /// `MODAL_TOKEN_SECRET`.
    pub token_secret: Option<Secret>,
    /// The helper binary (`None`: look it up).
    pub helper: Option<PathBuf>,
    /// The Modal App cua's sandboxes belong to.
    pub app: String,
}

impl std::fmt::Debug for ModalConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModalConfig")
            .field("token_id", &self.token_id.as_ref().map(|_| "<set>"))
            .field("token_secret", &self.token_secret.as_ref().map(|_| "<set>"))
            .field("helper", &self.helper)
            .field("app", &self.app)
            .finish()
    }
}

impl ModalConfig {
    /// From `MODAL_TOKEN_ID` / `MODAL_TOKEN_SECRET` (or `cua auth provider
    /// set modal`), `CUA_MODAL_HELPER` and `CUA_MODAL_APP` (default
    /// `cua-sandboxes`).
    pub fn from_env() -> Self {
        let store = CredentialStore::default();
        let one = |v: &str| common::credential(&store, "modal", &[v]).map(|(s, _, _)| s);
        Self {
            token_id: one("MODAL_TOKEN_ID"),
            token_secret: one("MODAL_TOKEN_SECRET"),
            helper: std::env::var_os("CUA_MODAL_HELPER")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
            app: std::env::var("CUA_MODAL_APP")
                .ok()
                .filter(|a| !a.trim().is_empty())
                .unwrap_or_else(|| "cua-sandboxes".into()),
        }
    }
}

/// The Modal provider.
pub struct Modal {
    config: ModalConfig,
    images: Arc<dyn ImageConfigSource>,
}

#[derive(Serialize, Default)]
struct Request<'a> {
    op: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    app: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    image: &'a str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    argv: Vec<String>,
    #[serde(skip_serializing_if = "is_zero_f")]
    cpus: f64,
    #[serde(skip_serializing_if = "is_zero_u")]
    memory_mib: u64,
    #[serde(skip_serializing_if = "is_zero_u")]
    timeout_secs: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    ports: Vec<u16>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    env: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    tags: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "str::is_empty")]
    id: &'a str,
    /// A Modal GPU type (`T4`, `A100`); empty: none.
    #[serde(skip_serializing_if = "str::is_empty")]
    gpu: &'a str,
    deadline_secs: u64,
}

/// The GPU types a Modal sandbox can reserve (Modal's own names), and how
/// they read.
const MODAL_GPUS: &[(&str, &str)] = &[
    ("T4", "NVIDIA T4"),
    ("L4", "NVIDIA L4"),
    ("A10G", "NVIDIA A10G"),
    ("A100", "NVIDIA A100"),
    ("H100", "NVIDIA H100"),
];

fn is_zero_f(v: &f64) -> bool {
    *v == 0.0
}
fn is_zero_u(v: &u64) -> bool {
    *v == 0
}

#[derive(Deserialize, Default)]
struct Reply {
    #[serde(default)]
    sandbox: Option<SandboxDoc>,
    #[serde(default)]
    sandboxes: Vec<SandboxDoc>,
    #[serde(default)]
    error: Option<HelperError>,
}

#[derive(Deserialize)]
struct SandboxDoc {
    id: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    tags: BTreeMap<String, String>,
    #[serde(default)]
    tunnels: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct HelperError {
    kind: String,
    message: String,
}

impl Modal {
    /// From the environment.
    pub fn from_env() -> Self {
        Self::new(ModalConfig::from_env())
    }

    /// With `config`, reading image configs from the registry.
    pub fn new(config: ModalConfig) -> Self {
        Self {
            config,
            images: image_config::registry(),
        }
    }

    /// Replaces where image configs come from (tests).
    pub fn with_image_configs(mut self, images: Arc<dyn ImageConfigSource>) -> Self {
        self.images = images;
        self
    }

    fn tokens(&self) -> Result<(&Secret, &Secret)> {
        match (&self.config.token_id, &self.config.token_secret) {
            (Some(i), Some(s)) => Ok((i, s)),
            _ => Err(common::not_configured("modal", MODAL_ENV)),
        }
    }

    /// The helper binary.
    pub fn helper(&self) -> Result<PathBuf> {
        if let Some(h) = &self.config.helper {
            return if h.is_file() {
                Ok(h.clone())
            } else {
                Err(helper_missing(Some(h)))
            };
        }
        let exe = if cfg!(windows) {
            format!("{HELPER}.exe")
        } else {
            HELPER.to_string()
        };
        if let Some(dir) = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(Path::to_path_buf))
            && dir.join(&exe).is_file()
        {
            return Ok(dir.join(&exe));
        }
        std::env::var_os("PATH")
            .into_iter()
            .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
            .map(|d| d.join(&exe))
            .find(|p| p.is_file())
            .ok_or_else(|| helper_missing(None))
    }

    async fn call(&self, req: &Request<'_>) -> Result<Reply> {
        let (id, secret) = self.tokens()?;
        let helper = self.helper()?;
        let body = serde_json::to_vec(req)?;
        let mut child = tokio::process::Command::new(&helper)
            .env("MODAL_TOKEN_ID", id.expose())
            .env("MODAL_TOKEN_SECRET", secret.expose())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| Error::Http(format!("modal: cannot run {}: {e}", helper.display())))?;
        let mut stdin = child.stdin.take().expect("piped");
        stdin.write_all(&body).await?;
        drop(stdin);
        let mut stdout = child.stdout.take().expect("piped").take(MAX_REPLY);
        let mut stderr = child.stderr.take().expect("piped").take(64 << 10);
        let budget = Duration::from_secs(req.deadline_secs + 30);
        let run = async {
            let mut out = Vec::new();
            let mut err = Vec::new();
            let (a, b) = tokio::join!(stdout.read_to_end(&mut out), stderr.read_to_end(&mut err));
            a?;
            b?;
            let status = child.wait().await?;
            Ok::<_, std::io::Error>((status, out, err))
        };
        let (status, out, err) = tokio::time::timeout(budget, run)
            .await
            .map_err(|_| Error::Timeout(format!("modal {} within {budget:?}", req.op)))??;
        let reply: Reply = serde_json::from_slice(&out).map_err(|e| {
            Error::Http(format!(
                "modal {}: the helper exited with {status} and no answer ({e}): {}",
                req.op,
                String::from_utf8_lossy(&err)
                    .chars()
                    .take(300)
                    .collect::<String>()
            ))
        })?;
        if let Some(e) = reply.error {
            return Err(match e.kind.as_str() {
                "auth" => Error::ContribNotConfigured(format!(
                    "modal: Modal refused MODAL_TOKEN_ID / MODAL_TOKEN_SECRET: {}",
                    e.message
                )),
                "not_found" => Error::NotFound(format!("modal: {}", e.message)),
                "invalid" => Error::InvalidArgument(format!("modal {}: {}", req.op, e.message)),
                "timeout" => Error::Timeout(format!("modal {}: {}", req.op, e.message)),
                _ => Error::Http(format!("modal {}: {}", req.op, e.message)),
            });
        }
        Ok(reply)
    }

    fn instance(s: &SandboxDoc, name: Option<&str>) -> ProviderInstance {
        let status = match s.status.as_str() {
            "running" => Status::Running,
            "stopped" => Status::Stopped,
            other => Status::Unknown(other.into()),
        };
        let name = name
            .map(str::to_string)
            .or_else(|| s.tags.get("cua.name").cloned())
            .unwrap_or_else(|| s.id.clone());
        let mut i = ProviderInstance::new(&s.id, name, status);
        for (port, url) in &s.tunnels {
            if let Ok(p) = port.parse::<u16>() {
                i.endpoints.insert(
                    p,
                    ServiceEndpoint {
                        url: url.trim_end_matches('/').to_string(),
                        headers: vec![],
                    },
                );
            }
        }
        i
    }
}

fn helper_missing(at: Option<&Path>) -> Error {
    Error::Unsupported {
        provider: cua_sandbox_core::ProviderKind::Contrib,
        op: format!(
            "modal: the Modal provider runs Modal's official Go SDK through {HELPER}{}; build it \
             with `go build` in libs/cua/crates/cua-contrib/modal-helper and put it next to cua, \
             on PATH, or in CUA_MODAL_HELPER",
            at.map(|p| format!(" (not found at {})", p.display()))
                .unwrap_or_default()
        ),
    }
}

#[async_trait::async_trait]
impl Provider for Modal {
    fn name(&self) -> &'static str {
        "modal"
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            kinds: vec![RunKind::Container],
            runtime: "gvisor",
            arches: vec!["amd64"],
            image_mode: ImageMode::Direct,
            ports: PortExposure::Https,
            command: true,
            env_to_entrypoint: true,
            // Modal takes registry credentials as its own Secret objects.
            private_registry: false,
            suspend: false,
            max_cpus: None,
            max_memory_mb: None,
            credential_env: MODAL_ENV,
            gpus: MODAL_GPUS
                .iter()
                .map(|(id, label)| cua_sandbox_core::gpu::GpuOption {
                    id: (*id).into(),
                    label: (*label).into(),
                    experimental: false,
                    supported: true,
                    reason: String::new(),
                    learn_more: Some("https://modal.com/docs/guide/gpu".into()),
                    usd_per_hour: None,
                })
                .collect(),
        }
    }

    fn check_configured(&self) -> Result<()> {
        self.tokens()?;
        self.helper().map(|_| ())
    }

    async fn create(&self, spec: &ProviderCreate) -> Result<ProviderInstance> {
        let cfg = self
            .images
            .config(&spec.image, spec.registry_credentials.as_ref())
            .await?;
        let argv = cfg.argv(spec.command.as_deref());
        if argv.is_empty() {
            return Err(Error::UnsupportedImage(format!(
                "{} has no ENTRYPOINT or CMD: pass a command for Modal",
                spec.image.reference
            )));
        }
        let ttl = spec.ttl.unwrap_or(DEFAULT_TTL).min(MAX_TTL);
        let req = Request {
            op: "create",
            app: &self.config.app,
            image: &spec.image.pinned_ref,
            argv,
            cpus: f64::from(spec.cpus),
            memory_mib: spec.memory_mb,
            timeout_secs: ttl.as_secs(),
            ports: spec.ports.clone(),
            env: spec.env.clone(),
            tags: spec.labels.clone(),
            gpu: spec.gpu.as_deref().unwrap_or_default(),
            deadline_secs: spec.timeout.as_secs().max(60),
            ..Default::default()
        };
        let reply = self.call(&req).await?;
        let doc = reply
            .sandbox
            .ok_or_else(|| Error::Http("modal create: no sandbox in the answer".into()))?;
        let instance = Self::instance(&doc, Some(&spec.name));
        let missing: Vec<u16> = spec
            .ports
            .iter()
            .copied()
            .filter(|p| !instance.endpoints.contains_key(p))
            .collect();
        if !missing.is_empty() {
            let _ = self.delete(&instance.id).await;
            return Err(Error::Http(format!(
                "modal create: no tunnel for guest ports {missing:?}"
            )));
        }
        Ok(instance)
    }

    async fn get(&self, id: &str) -> Result<ProviderInstance> {
        let reply = self
            .call(&Request {
                op: "get",
                id,
                // Any port asks for the tunnels; Modal returns every one.
                ports: vec![cua_sandbox_core::ENV_PORT],
                deadline_secs: 60,
                ..Default::default()
            })
            .await?;
        let doc = reply
            .sandbox
            .ok_or_else(|| Error::NotFound(format!("modal sandbox {id}")))?;
        Ok(Self::instance(&doc, None))
    }

    async fn list(&self) -> Result<Vec<ProviderInstance>> {
        let reply = self
            .call(&Request {
                op: "list",
                tags: [("cua.managed".to_string(), "true".to_string())].into(),
                deadline_secs: 60,
                ..Default::default()
            })
            .await?;
        Ok(reply
            .sandboxes
            .iter()
            .map(|s| Self::instance(s, None))
            .collect())
    }

    async fn delete(&self, id: &str) -> Result<()> {
        match self
            .call(&Request {
                op: "delete",
                id,
                deadline_secs: 60,
                ..Default::default()
            })
            .await
        {
            Ok(_) | Err(Error::NotFound(_)) => Ok(()),
            Err(e) => Err(e),
        }
    }

    fn endpoint(&self, instance: &ProviderInstance, port: u16) -> Result<ServiceEndpoint> {
        instance.endpoints.get(&port).cloned().ok_or_else(|| {
            Error::InvalidArgument(format!(
                "Modal port {port} of {} has no tunnel (Modal tunnels are fixed at create: \
                 declare it with --port)",
                instance.id
            ))
        })
    }
}
