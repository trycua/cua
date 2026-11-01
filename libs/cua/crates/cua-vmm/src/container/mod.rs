//! OCI container backend over the Docker Engine API (`bollard`).
//!
//! Runs rootfs images such as the gVisor `docker-*` tags. Isolation is
//! chosen per engine:
//!
//! 1. `runsc` (gVisor) when the engine has it registered;
//! 2. otherwise, when [`ContainerConfig::allow_install_runsc`] is set and the
//!    engine is provisionable (Colima VM, native Linux), install it and retry;
//! 3. otherwise `runc`, reported as [`Isolation::Runc`] so callers can surface
//!    the weaker sandbox (or fail when [`ContainerConfig::require_gvisor`]).
//!
//! [`DockerExec`] is the agentless guest-exec path for containers.

pub mod egress;
pub mod engine;
pub mod gvisor;
pub mod snapshot;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bollard::Docker;
use bollard::exec::{CreateExecOptions, StartExecOptions, StartExecResults};
use bollard::models::{ContainerCreateBody, HostConfig, PortBinding};
use bollard::query_parameters::{
    CommitContainerOptions, CreateContainerOptions, CreateImageOptions, ListContainersOptions,
    RemoveContainerOptions, StopContainerOptions, TagImageOptions,
};
use futures::StreamExt;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use crate::error::{Result, VmmError};
use crate::exec::{ExecOutput, ExecRequest, GuestExec};
use crate::host;
use crate::runtime::Runtime;
use crate::types::*;

pub use engine::{EngineEndpoint, EngineKind};

const LABEL_SANDBOX: &str = "cua.sandbox";
const LABEL_NAME: &str = "cua.sandbox.name";
const LABEL_BACKEND: &str = "cua.vmm.backend";
/// On sidecar containers: the sandbox whose network namespace they share.
/// Sidecars carry no [`LABEL_BACKEND`], so they are not listed as sandboxes.
const LABEL_SIDECAR_OF: &str = "cua.sandbox.sidecar-of";
/// Sidecar defaults (500m / 512Mi).
const SIDECAR_NANO_CPUS: i64 = 500_000_000;
pub(crate) const SIDECAR_MEMORY_BYTES: i64 = 512 * 1024 * 1024;
/// Engine repository of `ensure_base` images (pulled images re-tagged).
pub const BASE_REPO: &str = "cua-vmm/base";
/// Engine repository of checkpoints and of the intermediate images this
/// backend commits for port rebinds and forks.
pub const CKPT_REPO: &str = "cua-vmm/checkpoint";

/// On every container, image and volume the SDK creates: `true`. Garbage
/// collection only ever touches objects that carry it (plus the images the
/// SDK itself pulled, see [`ledger`]).
pub const LABEL_MANAGED: &str = "ai.cua.managed";
/// What the object is: `sandbox`, `sidecar`, `build` (build containers),
/// `checkpoint`, `intermediate` (port rebinds and fork sources),
/// `build-image` (local image builds) or `base`.
pub const LABEL_KIND: &str = "ai.cua.kind";
/// Unix seconds the container was created.
pub const LABEL_CREATED: &str = "ai.cua.created";
/// `true` on the container of an ephemeral sandbox.
pub const LABEL_EPHEMERAL: &str = "ai.cua.ephemeral";
/// On a warm-pool member's container ([`StartSpec::pool_member`]): `true`,
/// or [`POOL_KEEP`] for a member of a shape the host keeps warm on purpose,
/// which holds the built-in runtime's VM up. Labels are fixed at create, so
/// a claimed Space keeps it; only the [`crate::POOL_MEMBER_PREFIX`] name
/// tells an unclaimed member.
pub const LABEL_POOL: &str = "ai.cua.pool";
/// [`LABEL_POOL`] of a member the host keeps warm on purpose.
pub const POOL_KEEP: &str = "keep";

/// Environment variables a Spaces token may arrive in.
const TOKEN_ENV: [&str; 2] = ["CUA_ENV_TOKEN", "CUA_SPACESD_TOKEN"];
/// Which cua home created the object ([`home_tag`]): several homes
/// (`CUA_HOME`) can share one engine, and each one's garbage collection
/// only touches its own objects.
pub const LABEL_HOME: &str = "ai.cua.home";

/// A short stable tag of a cua home directory (12 hex of its SHA-256).
pub fn home_tag_for(home: &std::path::Path) -> String {
    use sha2::{Digest, Sha256};
    let d = Sha256::digest(home.to_string_lossy().as_bytes());
    d.iter().take(6).map(|b| format!("{b:02x}")).collect()
}

/// [`home_tag_for`] of this process's cua home.
pub fn home_tag() -> String {
    home_tag_for(&host::cua_home())
}

/// The `ai.cua.kind` of an image this backend writes into `repo:tag`.
pub fn image_kind(repo: &str, tag: &str) -> &'static str {
    match repo {
        CKPT_REPO if tag.ends_with("-rebind") || tag.starts_with("fork-") => "intermediate",
        CKPT_REPO => "checkpoint",
        BASE_REPO => "base",
        _ => "build-image",
    }
}

/// The Dockerfile `LABEL` instruction that marks an image the SDK wrote.
fn managed_label_change(repo: &str, tag: &str) -> String {
    format!(
        "LABEL {LABEL_MANAGED}=true {LABEL_KIND}={} {LABEL_HOME}={}",
        image_kind(repo, tag),
        home_tag()
    )
}

pub mod ledger;

/// Configuration for [`ContainerRuntime`].
#[derive(Clone, Debug)]
pub struct ContainerConfig {
    /// Engine URI override; discovered like the docker CLI when `None`.
    pub endpoint: Option<String>,
    /// Use gVisor when possible.
    pub prefer_gvisor: bool,
    /// Install `runsc` into the engine when it is missing and provisionable.
    pub allow_install_runsc: bool,
    /// Fail instead of falling back to `runc`.
    pub require_gvisor: bool,
    /// Seconds `stop` waits before SIGKILL.
    pub stop_timeout: i32,
    /// Warm starts of gVisor Spaces from golden memory checkpoints, where
    /// the engine supports them ([`snapshot`]). On unless
    /// `CUA_GVISOR_WARM=0`.
    pub warm: bool,
}

/// Set to `1` (shared pool hosts do): a Space never runs on `runc` when
/// gVisor is missing; creating it fails with the reason instead.
pub const REQUIRE_GVISOR_ENV: &str = "CUA_REQUIRE_GVISOR";

/// Whether `value` of [`REQUIRE_GVISOR_ENV`] turns the requirement on.
pub fn require_gvisor_from(value: Option<&str>) -> bool {
    matches!(value.map(str::trim), Some("1" | "true" | "yes" | "on"))
}

impl Default for ContainerConfig {
    fn default() -> Self {
        Self {
            endpoint: None,
            prefer_gvisor: true,
            allow_install_runsc: false,
            require_gvisor: require_gvisor_from(std::env::var(REQUIRE_GVISOR_ENV).ok().as_deref()),
            stop_timeout: 10,
            warm: std::env::var(snapshot::WARM_ENV).map_or(true, |v| v != "0"),
        }
    }
}

/// The container backend.
pub struct ContainerRuntime {
    docker: Docker,
    endpoint: EngineEndpoint,
    cfg: ContainerConfig,
    /// Resolved OCI runtime name (`runsc`/`runc`), decided once per process.
    oci_runtime: Mutex<Option<String>>,
    /// CPUs the engine reports (`docker info` NCPU), read once per process.
    engine_cpus: Mutex<Option<i64>>,
    /// Memory checkpoints on this engine ([`snapshot`]), probed once.
    snap: tokio::sync::OnceCell<snapshot::Support>,
}

/// The OOM score adjustment of a Space's container (higher dies first).
pub const SPACE_OOM_SCORE_ADJ: i64 = 500;

/// The lock that keeps one pull of `reference` at a time in this process.
fn pull_gate(reference: &str) -> std::sync::Arc<tokio::sync::Mutex<()>> {
    static GATES: std::sync::LazyLock<
        std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>>,
    > = std::sync::LazyLock::new(Default::default);
    GATES
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .entry(reference.to_string())
        .or_default()
        .clone()
}

/// Env var that overrides the checkpoint/import request budget (seconds).
pub const BULK_TIMEOUT_ENV: &str = "CUA_CHECKPOINT_TIMEOUT_SECS";
const BULK_TIMEOUT_DEFAULT_SECS: u64 = 1800;

/// Request budget for checkpoint commit/import: `CUA_CHECKPOINT_TIMEOUT_SECS`
/// when set to a positive integer, else 30 minutes (a 4.5 GB rootfs takes
/// about a minute, so this leaves a wide margin on slow disks).
fn bulk_timeout() -> Duration {
    bulk_timeout_from(std::env::var(BULK_TIMEOUT_ENV).ok().as_deref())
}

fn bulk_timeout_from(v: Option<&str>) -> Duration {
    let secs = v
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|s| *s > 0)
        .unwrap_or(BULK_TIMEOUT_DEFAULT_SECS);
    Duration::from_secs(secs)
}

/// An engine error from a bulk request; a timeout names the knob.
fn bulk_err(what: &str, e: bollard::errors::Error) -> VmmError {
    let msg = e.to_string();
    if msg.to_ascii_lowercase().contains("timeout") {
        VmmError::Engine(format!(
            "{what}: the engine did not finish within {} s ({msg}); raise {BULK_TIMEOUT_ENV} for a larger rootfs",
            bulk_timeout().as_secs()
        ))
    } else {
        VmmError::Engine(format!("{what}: {msg}"))
    }
}

fn engine_err(e: bollard::errors::Error) -> VmmError {
    VmmError::Engine(e.to_string())
}

fn is_not_found(e: &bollard::errors::Error) -> bool {
    matches!(
        e,
        bollard::errors::Error::DockerResponseServerError {
            status_code: 404,
            ..
        }
    )
}

impl ContainerRuntime {
    /// Connect to the engine (discovering it when no endpoint is configured).
    pub async fn connect(cfg: ContainerConfig) -> Result<Self> {
        let endpoint = match &cfg.endpoint {
            Some(uri) => EngineEndpoint {
                kind: engine::classify(uri),
                uri: uri.clone(),
                source: "config".into(),
            },
            None => match engine::discover() {
                Some(ep) => ep,
                // Docker Desktop quit (its context's socket is gone): start
                // it, rather than install one.
                None => match engine::stopped_context() {
                    Some(ep) => {
                        return Err(VmmError::NotRunning {
                            what: format!("container engine at {}", ep.uri),
                            hint: engine::start_hint(&ep.kind).into(),
                        });
                    }
                    None => {
                        return Err(VmmError::missing(
                            "a container engine",
                            format!(
                                "no Docker-API socket found (DOCKER_HOST, docker context, \
                                 ~/.colima, ~/.orbstack, ~/.docker/run, /var/run/docker.sock); {}",
                                engine::install_hint()
                            ),
                        ));
                    }
                },
            },
        };
        let at = format!("container engine at {}", endpoint.uri);
        let check = engine::check_socket(&endpoint.uri).await;
        match &check {
            Some(engine::SocketCheck::PermissionDenied) => {
                return Err(VmmError::PermissionDenied(engine::permission_hint(
                    &endpoint.uri,
                )));
            }
            Some(engine::SocketCheck::Missing | engine::SocketCheck::NotListening) => {
                return Err(VmmError::NotRunning {
                    what: at,
                    hint: engine::start_hint(&endpoint.kind).into(),
                });
            }
            _ => {}
        }
        let docker = connect_uri(&endpoint.uri).await?;
        docker.ping().await.map_err(|e| match check {
            // Something accepts on the socket but does not speak the
            // Docker API as expected: an engine error, not a stopped one.
            Some(engine::SocketCheck::Accepting) => {
                VmmError::Engine(format!("{at} did not answer ping: {e}"))
            }
            _ => VmmError::NotRunning {
                what: at,
                hint: format!(
                    "it did not answer ({e}); {}",
                    engine::start_hint(&endpoint.kind)
                ),
            },
        })?;
        Ok(Self {
            docker,
            endpoint,
            cfg,
            oci_runtime: Mutex::new(None),
            engine_cpus: Mutex::new(None),
            snap: tokio::sync::OnceCell::new(),
        })
    }

    pub fn endpoint(&self) -> &EngineEndpoint {
        &self.endpoint
    }

    pub fn docker(&self) -> &Docker {
        &self.docker
    }

    /// Every container on this engine (sandboxes and their sidecars), its
    /// status and its memory limit in MiB (a container with no limit counts
    /// as the default 2 GiB). For sizing the built-in runtime's VM, which
    /// runs nothing else.
    pub async fn memory_reservations(&self) -> Result<Vec<(String, Status, u64)>> {
        let rows = self
            .docker
            .list_containers(Some(ListContainersOptions {
                all: true,
                ..Default::default()
            }))
            .await
            .map_err(engine_err)?;
        let mut out = Vec::new();
        for c in rows {
            let Some(name) = c
                .names
                .as_ref()
                .and_then(|n| n.first())
                .map(|n| n.trim_start_matches('/').to_string())
            else {
                continue;
            };
            let Some(i) = self.inspect(&name).await? else {
                continue;
            };
            let bytes = i
                .host_config
                .as_ref()
                .and_then(|h| h.memory)
                .filter(|b| *b > 0)
                .unwrap_or(2048 << 20);
            out.push((name, Self::status_from(&i), (bytes as u64) >> 20));
        }
        Ok(out)
    }

    /// NVIDIA GPUs for containers under `runtime` (`container` for runc,
    /// `gvisor`) on this host ([`crate::gpu::container_support`]).
    pub async fn gpu_support_for(&self, runtime: &str) -> crate::gpu::GpuSupport {
        let nvidia = std::env::consts::OS == "linux"
            && self
                .runtimes()
                .await
                .is_ok_and(|r| r.iter().any(|r| r == "nvidia"));
        crate::gpu::container_support(std::env::consts::OS, runtime, nvidia)
    }

    /// OCI runtimes the engine has registered.
    pub async fn runtimes(&self) -> Result<Vec<String>> {
        let info = self.docker.info().await.map_err(engine_err)?;
        let mut v: Vec<String> = info.runtimes.unwrap_or_default().into_keys().collect();
        v.sort();
        Ok(v)
    }

    /// The extra arguments the engine runs runtime `name` with (`None` when
    /// it is not registered).
    pub async fn runtime_args(&self, name: &str) -> Result<Option<Vec<String>>> {
        let info = self.docker.info().await.map_err(engine_err)?;
        Ok(info
            .runtimes
            .unwrap_or_default()
            .remove(name)
            .map(|r| r.runtime_args.unwrap_or_default()))
    }

    /// Decide `runsc` vs `runc`, provisioning gVisor when permitted.
    pub async fn resolve_oci_runtime(&self) -> Result<String> {
        let mut cached = self.oci_runtime.lock().await;
        if let Some(r) = cached.as_ref() {
            return Ok(r.clone());
        }
        let chosen = self.decide_runtime().await?;
        *cached = Some(chosen.clone());
        Ok(chosen)
    }

    async fn decide_runtime(&self) -> Result<String> {
        if !self.cfg.prefer_gvisor && !self.cfg.require_gvisor {
            return Ok("runc".into());
        }
        if self.runtimes().await?.iter().any(|r| r == "runsc") {
            return Ok("runsc".into());
        }
        let plan = gvisor::install_plan(&self.endpoint.kind);
        if self.cfg.allow_install_runsc {
            match &plan {
                Ok(plan) => {
                    tracing::info!("{}", plan.description);
                    self.install_runsc(plan).await?;
                    return Ok("runsc".into());
                }
                Err(hint) if self.cfg.require_gvisor => {
                    return Err(VmmError::missing("gVisor (runsc)", hint.clone()));
                }
                Err(hint) => tracing::warn!("cannot install runsc: {hint}; falling back to runc"),
            }
        } else if self.cfg.require_gvisor {
            let hint = match plan {
                Ok(p) => format!("set allow_install_runsc to let cua {}", p.description),
                Err(h) => h,
            };
            return Err(VmmError::missing("gVisor (runsc)", hint));
        }
        tracing::warn!(
            "gVisor (runsc) not available on {}; using runc",
            self.endpoint.uri
        );
        Ok("runc".into())
    }

    async fn install_runsc(&self, plan: &gvisor::InstallPlan) -> Result<()> {
        for step in &plan.steps {
            match step {
                gvisor::Step::Run { argv } => {
                    let prog = host::which(&argv[0]).ok_or_else(|| {
                        VmmError::missing(argv[0].clone(), "needed to install runsc")
                    })?;
                    let args: Vec<&str> = argv[1..].iter().map(String::as_str).collect();
                    host::run(prog, &args).await?;
                }
                gvisor::Step::ColimaRuntime { config } => {
                    let changed = gvisor::register_in_colima_config(config).map_err(|e| {
                        VmmError::other(format!("cannot update {}: {e}", config.display()))
                    })?;
                    tracing::info!(config = %config.display(), changed, "registered runsc in colima config");
                }
            }
        }
        // dockerd restarts; wait for it to come back with runsc registered.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
        while tokio::time::Instant::now() < deadline {
            if let Ok(rts) = connect_uri(&self.endpoint.uri)
                .await?
                .info()
                .await
                .map(|i| {
                    i.runtimes
                        .unwrap_or_default()
                        .into_keys()
                        .collect::<Vec<_>>()
                })
                && rts.iter().any(|r| r == "runsc")
            {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        Err(VmmError::other(
            "runsc installed but the engine did not register it within 300s",
        ))
    }

    /// Pull `reference` for `platform` unless already present.
    pub async fn ensure_image(&self, reference: &str, platform: Option<&str>) -> Result<()> {
        self.ensure_image_with(reference, platform, None).await
    }

    /// [`Self::ensure_image`] with explicit registry credentials (used when
    /// they apply to the reference's registry; otherwise the
    /// `CUA_REGISTRY_*` environment, then anonymous).
    pub async fn ensure_image_with(
        &self,
        reference: &str,
        platform: Option<&str>,
        creds: Option<&RegistryCredentials>,
    ) -> Result<()> {
        self.ensure_image_sized(reference, platform, creds, &[])
            .await
    }

    /// [`Self::ensure_image_with`], knowing the image's layers `(digest,
    /// size)` from its manifest, so the pull's progress has its total, and
    /// counts the layers the engine already has, from the start.
    pub async fn ensure_image_sized(
        &self,
        reference: &str,
        platform: Option<&str>,
        creds: Option<&RegistryCredentials>,
        layers: &[(String, u64)],
    ) -> Result<()> {
        if self.docker.inspect_image(reference).await.is_ok() {
            ledger::PullLedger::default().touch(reference);
            return Ok(());
        }
        // One pull per image at a time in this process: the engine does not
        // share concurrent pulls of one image, so 200 creates on a fresh
        // host each downloaded and unpacked it (64 GB written for a 1.2 GB
        // image, the disk saturated for minutes). The rest wait and find it.
        let gate = pull_gate(reference);
        let _one = gate.lock().await;
        if self.docker.inspect_image(reference).await.is_ok() {
            ledger::PullLedger::default().touch(reference);
            return Ok(());
        }
        // The engine stores images in its own VM or data root; on this host
        // that grows the engine's disk image, so the host volume is checked.
        crate::disk::ensure_space(
            &crate::host::cua_home(),
            crate::disk::CONTAINER_PULL_ESTIMATE,
            &format!("pull {reference}"),
        )?;
        let creds = creds
            .filter(|c| c.applies_to(registry_of(reference)))
            .map(|c| bollard::auth::DockerCredentials {
                username: Some(c.username.clone()),
                password: Some(c.password.clone()),
                serveraddress: Some(registry_of(reference).to_string()),
                ..Default::default()
            })
            .or_else(|| registry_credentials(reference));
        let (from_image, tag) = split_tag(reference);
        let opts = CreateImageOptions {
            from_image: Some(from_image),
            tag: (!tag.is_empty()).then_some(tag),
            platform: platform.unwrap_or_default().to_string(),
            ..Default::default()
        };
        tracing::info!(image = reference, "pulling image");
        use crate::progress::{Phase, Progress};
        crate::progress::report(Progress::phase(Phase::Pulling).detail(reference));
        // One fraction over downloading and extracting the layers, each by
        // bytes (the engine reports both).
        let mut bytes = crate::progress::Bytes::default();
        let mut extracted = crate::progress::Bytes::default();
        let mut last = -1.0f64;
        let mut meter = crate::progress::Meter::new();
        // The engine names a layer by the first 12 hex digits of its digest.
        let seeded: Vec<String> = layers.iter().map(|(d, _)| layer_id(d)).collect();
        for ((_, size), id) in layers.iter().zip(&seeded) {
            bytes.update(id, 0, *size);
            extracted.update(id, 0, *size);
        }
        // Layers the engine already has get no event at all: once it says
        // what it pulls ("Pulling fs layer" for each, up front), the seeded
        // layers it never named are there already.
        let mut named = std::collections::HashSet::new();
        let mut existing_settled = seeded.is_empty();
        let mut attempt = 1u32;
        'attempts: loop {
            let mut stream = self
                .docker
                .create_image(Some(opts.clone()), None, creds.clone());
            while let Some(item) = stream.next().await {
                // The engine's message never carries the credentials. A network
                // drop mid-pull is tried again (the engine keeps the layers it
                // finished); anything else fails at once.
                let info = match item {
                    Ok(info) => info,
                    Err(e) => {
                        let message = e.to_string();
                        if attempt < PULL_ATTEMPTS && pull_error_is_transient(&message) {
                            tracing::warn!(image = reference, attempt, error = %message, "image pull broke; trying again");
                            tokio::time::sleep(pull_retry_delay(attempt)).await;
                            attempt += 1;
                            continue 'attempts;
                        }
                        return Err(VmmError::Engine(format!("pull {reference}: {message}")));
                    }
                };
                if let Some(id) = info.id.as_deref() {
                    named.insert(id.to_string());
                }
                let announcing = matches!(
                    info.status.as_deref(),
                    Some("Pulling fs layer" | "Waiting") | None
                ) || info
                    .status
                    .as_deref()
                    .is_some_and(|s| s.starts_with("Pulling from"));
                if !existing_settled && !announcing {
                    existing_settled = true;
                    for id in seeded.iter().filter(|id| !named.contains(*id)) {
                        bytes.complete(id);
                        extracted.complete(id);
                    }
                }
                // Layer byte counts: "Downloading" carries (current, total),
                // and so does "Extracting" on the classic image store (the
                // containerd store reports seconds instead, so a layer counts as
                // extracted at "Pull complete"); "Download complete" finishes a
                // download, "Pull complete" / "Already exists" a layer.
                let layer = info.id.as_deref().unwrap_or_default();
                let size = |d: &bollard::models::ProgressDetail| {
                    (
                        d.current.unwrap_or(0).max(0) as u64,
                        d.total.unwrap_or(0).max(0) as u64,
                    )
                };
                let changed = match (info.status.as_deref(), info.progress_detail.as_ref()) {
                    (Some("Downloading"), Some(d)) => {
                        let (c, t) = size(d);
                        bytes.update(layer, c, t);
                        // The layer's extraction, still to come, weighs its size.
                        if extracted.fraction_of(layer).is_none() {
                            extracted.update(layer, 0, t);
                        }
                        true
                    }
                    (Some("Extracting"), Some(d)) if d.total.unwrap_or(0) > 0 => {
                        let (c, t) = size(d);
                        bytes.complete(layer);
                        extracted.update(layer, c, t);
                        true
                    }
                    (Some("Download complete"), _) => {
                        bytes.complete(layer);
                        true
                    }
                    (Some("Pull complete" | "Already exists"), _) => {
                        bytes.complete(layer);
                        extracted.complete(layer);
                        true
                    }
                    _ => false,
                };
                if !changed {
                    continue;
                }
                let (done, total) = bytes.totals();
                let fraction = pull_fraction(bytes.fraction(), extracted.fraction());
                // The download's bytes, a few reports a second (the meter's
                // throttle); a percent of the fraction always reports too.
                let transfer = (total > 0).then(|| meter.sample(done, total)).flatten();
                let moved = fraction.is_some_and(|f| (f - last).abs() >= 0.01);
                if transfer.is_some() || moved {
                    let mut p = Progress::phase(Phase::Pulling).detail(reference);
                    if let Some(f) = fraction {
                        last = f;
                        p = p.fraction(f);
                    }
                    if total > 0 {
                        p = p.bytes(transfer.unwrap_or_else(|| meter.peek(done, total)));
                    }
                    crate::progress::report(p);
                }
            }
            break;
        }
        // Remember that the SDK pulled it (it was not here before), so the
        // cache GC may remove it again once nothing uses it.
        if let Ok(i) = self.docker.inspect_image(reference).await
            && let Some(id) = i.id
        {
            ledger::PullLedger::default().record(reference, &id);
        }
        Ok(())
    }

    async fn inspect(
        &self,
        name: &str,
    ) -> Result<Option<bollard::models::ContainerInspectResponse>> {
        match self.docker.inspect_container(name, None).await {
            Ok(i) => Ok(Some(i)),
            Err(e) if is_not_found(&e) => Ok(None),
            Err(e) => Err(engine_err(e)),
        }
    }

    fn status_from(i: &bollard::models::ContainerInspectResponse) -> Status {
        use bollard::models::ContainerStateStatusEnum as S;
        match i.state.as_ref().and_then(|s| s.status) {
            Some(S::RUNNING) | Some(S::RESTARTING) => Status::Running,
            Some(S::PAUSED) => Status::Paused,
            Some(S::CREATED) | Some(S::EXITED) | Some(S::DEAD) => Status::Stopped,
            Some(S::REMOVING) | Some(S::STOPPING) => Status::Unknown("stopping".into()),
            _ => Status::Unknown("unknown".into()),
        }
    }

    /// How the container `name` ended, when it has stopped: the engine's
    /// exit code and whether the kernel killed it for memory
    /// (`State.OOMKilled`). `None` while it runs, when it never ran, or
    /// when there is no such container.
    pub async fn exit_of(&self, name: &str) -> Result<Option<ExitRecord>> {
        Ok(self.inspect(name).await?.as_ref().and_then(Self::exit_from))
    }

    fn exit_from(i: &bollard::models::ContainerInspectResponse) -> Option<ExitRecord> {
        use bollard::models::ContainerStateStatusEnum as S;
        let s = i.state.as_ref()?;
        if !matches!(s.status, Some(S::EXITED) | Some(S::DEAD)) {
            return None;
        }
        // Docker's zero time: created, never started.
        if s.finished_at
            .as_deref()
            .is_none_or(|t| t.starts_with("0001-"))
        {
            return None;
        }
        Some(ExitRecord {
            exit_code: s.exit_code.unwrap_or(0),
            oom_killed: s.oom_killed.unwrap_or(false),
        })
    }

    fn endpoints_from(i: &bollard::models::ContainerInspectResponse) -> Endpoints {
        let mut ports = BTreeMap::new();
        if let Some(map) = i.network_settings.as_ref().and_then(|n| n.ports.as_ref()) {
            for (k, v) in map {
                let Some((port, proto)) = k.split_once('/') else {
                    continue;
                };
                if proto != "tcp" {
                    continue;
                }
                let (Ok(guest), Some(bindings)) = (port.parse::<u16>(), v) else {
                    continue;
                };
                if let Some(hp) = bindings
                    .iter()
                    .filter(|b| b.host_ip.as_deref().is_none_or(|ip| !ip.contains(':')))
                    .find_map(|b| b.host_port.as_deref()?.parse::<u16>().ok())
                {
                    ports.insert(guest, hp);
                }
            }
        }
        Endpoints {
            host: "127.0.0.1".into(),
            ports,
            container_id: i.id.clone(),
            ..Default::default()
        }
    }

    fn isolation_from(i: &bollard::models::ContainerInspectResponse) -> Isolation {
        match i.host_config.as_ref().and_then(|h| h.runtime.as_deref()) {
            Some("runsc") | Some(snapshot::WRAPPER_RUNTIME) => Isolation::Gvisor,
            _ => Isolation::Runc,
        }
    }

    fn instance_from(&self, name: &str, i: &bollard::models::ContainerInspectResponse) -> Instance {
        Instance {
            name: name.to_string(),
            backend: BackendKind::Container,
            status: Self::status_from(i),
            endpoints: Self::endpoints_from(i),
            isolation: Self::isolation_from(i),
            arch: None,
        }
    }

    fn published(i: &bollard::models::ContainerInspectResponse) -> Vec<u16> {
        i.host_config
            .as_ref()
            .and_then(|h| h.port_bindings.as_ref())
            .map(|m| {
                m.keys()
                    .filter_map(|k| k.split('/').next()?.parse().ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The engine's CPU count, or 0 when it does not say. The engine may be a
    /// VM (Docker Desktop, Colima) with fewer CPUs than the host.
    async fn engine_cpus(&self) -> i64 {
        let mut cached = self.engine_cpus.lock().await;
        if let Some(n) = *cached {
            return n;
        }
        let n = match self.docker.info().await {
            Ok(info) => info.ncpu.unwrap_or(0),
            Err(_) => return 0,
        };
        *cached = Some(n);
        n
    }

    /// Create (not start) a container. `restore`: a memory checkpoint
    /// directory it restores on start instead of booting.
    async fn create(
        &self,
        name: &str,
        image: &str,
        spec: &StartSpec,
        base_labels: HashMap<String, String>,
        platform: Option<String>,
        restore: Option<&str>,
    ) -> Result<()> {
        let runtime = self.group_runtime(spec).await?;
        let engine_cpus = self.engine_cpus().await;
        if let Some(arch) = spec.arch
            && arch != crate::Arch::host()
            && runtime == "runsc"
        {
            return Err(VmmError::invalid(format!(
                "{image} has no {} build and gVisor (runsc) cannot run a {} image under \
                 emulation; start it with the runc runtime (Python: runtime=\"runc\")",
                crate::Arch::host().oci(),
                arch.oci()
            )));
        }
        // gVisor Spaces run under the cua-runsc wrapper where the engine
        // has it, so they can be checkpointed with memory and restored.
        let engine_runtime = if runtime == "runsc" && self.support().await.ok() {
            snapshot::WRAPPER_RUNTIME.to_string()
        } else {
            runtime
        };
        if restore.is_some() && engine_runtime != snapshot::WRAPPER_RUNTIME {
            return Err(VmmError::other(
                "this engine cannot restore memory checkpoints",
            ));
        }
        // An image without cua-spacesd gets one: its command runs under the
        // injected supervisor (as root; the supervisor drops to the image's
        // user for the command).
        let wrapped;
        let spec = match &spec.inject_spacesd {
            Some(_) => {
                let info = self.docker.inspect_image(image).await.map_err(engine_err)?;
                let run = run_config_of(info.config.as_ref());
                let mut s = spec.clone();
                s.command = Some(crate::inject::wrapper_command(
                    &run,
                    spec.command.as_deref(),
                ));
                wrapped = s;
                &wrapped
            }
            None => spec,
        };
        let mut body = create_body(name, image, spec, base_labels, engine_runtime);
        // Its own MAC, so egress rules can be keyed by it before it starts
        // ([`egress`]); no raw sockets under a policy that blocks anything.
        if let Some((net, mac)) = self.mac_config(name).await {
            body.networking_config = Some(net);
            body.labels
                .get_or_insert_with(Default::default)
                .insert(egress::LABEL_MAC.into(), mac);
        }
        if (egress::policy_blocks(&spec.egress) || egress::blocks(name))
            && let Some(hc) = body.host_config.as_mut()
        {
            hc.cap_drop = Some(vec!["NET_RAW".into()]);
        }
        if self.forwards_ports_by_number()
            && let Some(hc) = body.host_config.as_mut()
            && let Some(bindings) = hc.port_bindings.as_mut()
        {
            pick_host_ports(bindings)?;
        }
        if let Some(hc) = body.host_config.as_mut() {
            if let Some(dir) = restore {
                hc.annotations = Some(HashMap::from([(
                    snapshot::ANNOTATION_RESTORE.to_string(),
                    dir.to_string(),
                )]));
            }
            hc.nano_cpus = hc.nano_cpus.map(|n| {
                let capped = cap_nano_cpus(n, engine_cpus);
                if capped != n {
                    tracing::info!(
                        container = name,
                        requested_cpus = spec.cpus,
                        engine_cpus,
                        "the engine has fewer CPUs than requested; the container may use all of them"
                    );
                }
                capped
            });
        }
        let opts = CreateContainerOptions {
            name: Some(name.to_string()),
            platform: platform.unwrap_or_default(),
        };
        // Detached: the engine finishes a create whose request the caller
        // dropped (a cancelled create), so a container made after the
        // cancel looked would be left stopped. A dropped create is awaited
        // in the background and its container removed.
        let docker = self.docker.clone();
        let undo_docker = self.docker.clone();
        let created = name.to_string();
        crate::cleanup::complete_or_undo(
            async move { docker.create_container(Some(opts), body).await },
            move |_| async move {
                let _ = undo_docker
                    .remove_container(
                        &created,
                        Some(RemoveContainerOptions {
                            force: true,
                            v: true,
                            ..Default::default()
                        }),
                    )
                    .await;
            },
        )
        .await
        .map_err(engine_err)?;
        if let Some(binary) = &spec.inject_spacesd
            && let Err(e) = self.upload_spacesd(name, binary).await
        {
            self.remove_force(name).await;
            return Err(e);
        }
        Ok(())
    }

    /// Puts the static cua-spacesd into container `name` (created, not yet
    /// started) at [`crate::inject::GUEST_PATH`].
    async fn upload_spacesd(&self, name: &str, binary: &std::path::Path) -> Result<()> {
        let binary = binary.to_path_buf();
        let tar = tokio::task::spawn_blocking(move || crate::inject::layer_tar(&binary))
            .await
            .map_err(|e| VmmError::other(e.to_string()))??;
        let opts = bollard::query_parameters::UploadToContainerOptions {
            path: "/".into(),
            ..Default::default()
        };
        self.docker
            .upload_to_container(
                name,
                Some(opts),
                bollard::body_full(bytes::Bytes::from(tar)),
            )
            .await
            .map_err(engine_err)
    }

    /// Removes the image container `image_id` ran on when it is an untagged
    /// checkpoint or fork-source image this backend wrote (a deleted
    /// checkpoint a fork kept alive). Best effort: an image another
    /// container still uses is refused by the engine and stays.
    async fn drop_orphaned_image(&self, image_id: &str) {
        let Ok(img) = self.docker.inspect_image(image_id).await else {
            return;
        };
        let tagless = img.repo_tags.as_ref().is_none_or(|t| t.is_empty());
        let labels = img.config.and_then(|c| c.labels).unwrap_or_default();
        let ours = labels.get(LABEL_MANAGED).is_some_and(|v| v == "true")
            && matches!(
                labels.get(LABEL_KIND).map(String::as_str),
                Some("checkpoint" | "intermediate")
            )
            && labels.get(LABEL_HOME).is_none_or(|h| *h == home_tag());
        if tagless && ours {
            let _ = self
                .docker
                .remove_image(
                    image_id,
                    None::<bollard::query_parameters::RemoveImageOptions>,
                    None,
                )
                .await;
        }
    }

    /// Removes container `name` (and its anonymous volumes), ignoring errors.
    async fn remove_force(&self, name: &str) {
        let _ = self
            .docker
            .remove_container(
                name,
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await;
    }

    /// Commit a container to `repo:tag`.
    async fn commit(&self, container: &str, repo: &str, tag: &str, pause: bool) -> Result<String> {
        let opts = CommitContainerOptions {
            container: Some(container.to_string()),
            repo: Some(repo.to_string()),
            tag: Some(tag.to_string()),
            pause,
            changes: Some(managed_label_change(repo, tag)),
            ..Default::default()
        };
        let id = self
            .bulk_docker()
            .commit_container(opts, bollard::models::ContainerConfig::default())
            .await
            .map_err(|e| bulk_err("commit checkpoint", e))?;
        Ok(id.id)
    }

    fn platform_for(spec: &StartSpec) -> Option<String> {
        spec.arch.map(|a| format!("linux/{}", a.oci()))
    }

    /// Stream the running container's rootfs out via exec `tar` and import it
    /// as `cua-vmm/checkpoint:<tag>`, re-applying the container config.
    async fn export_import(
        &self,
        name: &str,
        i: &bollard::models::ContainerInspectResponse,
        tag: &str,
    ) -> Result<()> {
        self.export_import_as(name, CKPT_REPO, tag, config_changes(i.config.as_ref()))
            .await
    }

    /// Imports the running container `name`'s rootfs as `repo:tag`, with
    /// `changes` (Dockerfile instructions: `ENV k="v"`, `ENTRYPOINT [..]`,
    /// `CMD [..]`, `WORKDIR`, `USER`, `EXPOSE`) as its image config. The
    /// rootfs is streamed out through exec (`tar`), so it includes writes
    /// gVisor keeps in its own overlay. Image builds use this with the base
    /// image's config plus the build's environment and ports.
    pub async fn import_rootfs(
        &self,
        name: &str,
        repo: &str,
        tag: &str,
        changes: Vec<String>,
    ) -> Result<()> {
        self.export_import_as(name, repo, tag, changes).await
    }

    async fn export_import_as(
        &self,
        name: &str,
        repo: &str,
        tag: &str,
        changes: Vec<String>,
    ) -> Result<()> {
        let mut changes = changes;
        changes.push(managed_label_change(repo, tag));
        let exec = self.exec_handle(name);
        let script = "cd / && exec tar -cf - $(ls -A / | grep -vxE 'proc|sys|dev')";
        let (stream, exec_id) = exec.stream_stdout(script).await?;
        let opts = CreateImageOptions {
            from_src: Some("-".into()),
            repo: Some(repo.into()),
            tag: Some(tag.into()),
            changes,
            ..Default::default()
        };
        let body = bollard::body_try_stream(stream);
        let bulk = self.bulk_docker();
        let mut progress = bulk.create_image(Some(opts), Some(body), None);
        while let Some(item) = progress.next().await {
            item.map_err(|e| bulk_err(&format!("import checkpoint {tag}"), e))?;
        }
        let code = self
            .docker
            .inspect_exec(&exec_id)
            .await
            .map_err(engine_err)?
            .exit_code
            .unwrap_or(-1);
        // GNU tar exits 1 for "file changed as we read it"; the archive is still valid.
        if code != 0 && code != 1 {
            return Err(VmmError::other(format!(
                "rootfs export of '{name}' failed: tar exited {code}"
            )));
        }
        Ok(())
    }

    /// The engine client for requests that move a whole rootfs (checkpoint
    /// commit/import). bollard's 120 s request timeout covers the entire
    /// import, which the engine answers only once it has read the archive,
    /// so a multi-GB rootfs hit it. Budget: [`bulk_timeout`].
    fn bulk_docker(&self) -> Docker {
        self.docker.clone().with_timeout(bulk_timeout())
    }

    /// An exec handle for a running container.
    pub fn exec_handle(&self, name: &str) -> DockerExec {
        DockerExec {
            docker: self.docker.clone(),
            container: name.to_string(),
        }
    }
}

/// Whether a container has never been started (a fork waiting for its
/// first start).
fn never_started(i: &bollard::models::ContainerInspectResponse) -> bool {
    i.state
        .as_ref()
        .and_then(|s| s.started_at.as_deref())
        .is_none_or(|t| t.starts_with("0001-"))
}

/// The memory checkpoint a container restores when it starts.
fn restore_dir(i: &bollard::models::ContainerInspectResponse) -> Option<String> {
    i.host_config
        .as_ref()?
        .annotations
        .as_ref()?
        .get(snapshot::ANNOTATION_RESTORE)
        .cloned()
}

/// Split `repo[:tag]` (digest refs keep the digest in `from_image`).
fn split_tag(reference: &str) -> (String, String) {
    if reference.contains('@') {
        return (reference.to_string(), String::new());
    }
    match reference.rsplit_once(':') {
        Some((repo, tag)) if !tag.contains('/') => (repo.to_string(), tag.to_string()),
        _ => (reference.to_string(), "latest".to_string()),
    }
}

/// Credentials for private registries from `CUA_REGISTRY_USERNAME`/`PASSWORD`
/// (the engine falls back to anonymous; docker-config credentials are applied
/// by the engine's own credential helpers only for the CLI, not the API).
fn registry_credentials(_reference: &str) -> Option<bollard::auth::DockerCredentials> {
    let user = std::env::var("CUA_REGISTRY_USERNAME").ok()?;
    let pass = std::env::var("CUA_REGISTRY_PASSWORD").ok()?;
    Some(bollard::auth::DockerCredentials {
        username: Some(user),
        password: Some(pass),
        ..Default::default()
    })
}

fn connect_uri_once(uri: &str) -> std::result::Result<Docker, bollard::errors::Error> {
    let v = bollard::API_DEFAULT_VERSION;
    if uri.starts_with("unix://") || uri.starts_with("npipe://") || uri.starts_with('/') {
        Docker::connect_with_socket(uri, 120, v)
    } else {
        Docker::connect_with_http(uri, 120, v)
    }
}

async fn connect_uri(uri: &str) -> Result<Docker> {
    if !(uri.starts_with("unix://")
        || uri.starts_with("npipe://")
        || uri.starts_with('/')
        || uri.starts_with("tcp://")
        || uri.starts_with("http://"))
    {
        return Err(VmmError::invalid(format!(
            "unsupported engine endpoint '{uri}'"
        )));
    }
    // Docker Desktop's pipe has one listening instance at a time: a client
    // that opens it in the same instant as another gets ERROR_PIPE_BUSY, which
    // bollard reports as "Socket not found" (CUA-1523). The pipe is there:
    // ask again a moment later.
    let mut tries = 0;
    loop {
        match connect_uri_once(uri) {
            Err(bollard::errors::Error::SocketNotFoundError(_))
                if cfg!(windows) && uri.starts_with("npipe://") && tries < 40 =>
            {
                tries += 1;
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            r => return r.map_err(engine_err),
        }
    }
}

#[async_trait]
impl Runtime for ContainerRuntime {
    fn kind(&self) -> BackendKind {
        BackendKind::Container
    }

    /// Pull the image and tag it `cua-vmm/base:<base_name>`.
    async fn ensure_base(&self, image: &ImageSource, base_name: &str) -> Result<CheckpointInfo> {
        validate_name(base_name)?;
        let ImageSource::Oci { reference } = image else {
            return Err(VmmError::invalid("container bases must be OCI references"));
        };
        let tagged = format!("{BASE_REPO}:{base_name}");
        if self.docker.inspect_image(&tagged).await.is_err() {
            self.ensure_image(reference, None).await?;
            let opts = TagImageOptions {
                repo: Some(BASE_REPO.into()),
                tag: Some(base_name.into()),
            };
            self.docker
                .tag_image(reference, Some(opts))
                .await
                .map_err(engine_err)?;
        }
        Ok(CheckpointInfo::now(
            base_name,
            BackendKind::Container,
            Some(reference.clone()),
        ))
    }

    async fn start(&self, spec: &StartSpec) -> Result<Instance> {
        validate_name(&spec.name)?;
        crate::types::reject_restricted_network(BackendKind::Container, spec)?;
        crate::egress::check(BackendKind::Container, &spec.egress)?;
        crate::types::check_mounts(BackendKind::Container, &spec.mounts)?;
        crate::filetable::admit("a container")?;
        crate::types::reject_gpu(BackendKind::Container, spec, &[crate::gpu::NVIDIA])?;
        if spec.gpu.is_some() {
            let runtime = self.group_runtime(spec).await?;
            let label = if runtime == "runsc" {
                "gvisor"
            } else {
                "container"
            };
            self.gpu_support_for(label)
                .await
                .pick(crate::gpu::NVIDIA)
                .map_err(VmmError::invalid)?;
        }
        // Refuse a sidecar group gVisor cannot run before anything is
        // pulled or created.
        self.group_runtime(spec).await?;
        let existing = self.inspect(&spec.name).await?;
        // The policy: the start spec's for a new container (or one that has
        // none kept); a kept one wins (`set_egress` changed it since).
        match (&existing, egress::kept(&spec.name)) {
            (Some(_), Some(_)) => {}
            (Some(_), None) if spec.egress.is_empty() => {}
            _ if spec.egress.is_empty() => egress::forget(&spec.name),
            _ => egress::keep(
                &spec.name,
                &egress::Kept {
                    policy: spec.egress.clone(),
                    table: None,
                },
            )?,
        }
        // A memory restore about to happen: (the token its cua-spacesd
        // accepts, the Space's own token, the checkpoint directory).
        let mut restored: Option<(Option<String>, Option<String>, String)> = None;
        // A golden checkpoint to record once this cold start is ready.
        let mut record_golden: Option<(snapshot::GoldenPlan, String)> = None;
        let info = match existing {
            Some(mut i) => {
                // A fork that never ran takes this start's environment (its
                // own token) and ports; a memory fork restores now.
                let forked = i
                    .config
                    .as_ref()
                    .and_then(|c| c.labels.as_ref())
                    .is_some_and(|l| l.contains_key("cua.vmm.source"));
                if forked && never_started(&i) {
                    let env = i
                        .config
                        .as_ref()
                        .and_then(|c| c.env.clone())
                        .unwrap_or_default();
                    let merged = snapshot::merge_env(&env, &spec.env);
                    if let Some(dir) = restore_dir(&i) {
                        restored =
                            Some((snapshot::token_of(&env), snapshot::token_of(&merged), dir));
                    }
                    let have = Self::published(&i);
                    if merged != env || spec.ports.iter().any(|p| !have.contains(p)) {
                        self.recreate(&spec.name, &i, merged, &spec.ports).await?;
                        i = self
                            .inspect(&spec.name)
                            .await?
                            .ok_or_else(|| VmmError::NotFound(spec.name.clone()))?;
                    }
                }
                let st = Self::status_from(&i);
                if st == Status::Paused {
                    self.docker
                        .unpause_container(&spec.name)
                        .await
                        .map_err(engine_err)?;
                } else if st != Status::Running {
                    let have = Self::published(&i);
                    if spec.ports.iter().any(|p| !have.contains(p))
                        || self.needs_recreate(&spec.name, &i)
                    {
                        // Port bindings (and the MAC and capabilities an
                        // egress policy needs) are fixed at create time:
                        // recreate from a commit so filesystem changes
                        // survive.
                        let img = format!("{CKPT_REPO}:{}-rebind", spec.name);
                        self.commit(
                            &spec.name,
                            CKPT_REPO,
                            &format!("{}-rebind", spec.name),
                            false,
                        )
                        .await?;
                        self.docker
                            .remove_container(
                                &spec.name,
                                Some(RemoveContainerOptions {
                                    force: true,
                                    ..Default::default()
                                }),
                            )
                            .await
                            .map_err(engine_err)?;
                        let labels = i
                            .config
                            .as_ref()
                            .and_then(|c| c.labels.clone())
                            .unwrap_or_default();
                        self.create(&spec.name, &img, spec, labels, None, None)
                            .await?;
                    }
                    self.start_policed(&spec.name).await?;
                }
                self.inspect(&spec.name)
                    .await?
                    .ok_or_else(|| VmmError::NotFound(spec.name.clone()))?
            }
            None => {
                let ImageSource::Oci { reference } = &spec.image else {
                    return match spec.image {
                        ImageSource::Existing => Err(VmmError::NotFound(spec.name.clone())),
                        _ => Err(VmmError::invalid(
                            "the container backend runs OCI images, not disk files",
                        )),
                    };
                };
                let platform = Self::platform_for(spec);
                self.ensure_image_sized(
                    reference,
                    platform.as_deref(),
                    spec.registry_auth.as_ref(),
                    &spec.pull_layers,
                )
                .await?;
                // Pull sidecar images before anything is created, so a bad
                // sidecar ref leaves nothing behind.
                for sc in &spec.sidecars {
                    self.ensure_image_with(
                        &sc.image,
                        platform.as_deref(),
                        spec.registry_auth.as_ref(),
                    )
                    .await?;
                }
                crate::progress::report(crate::progress::Progress::phase(
                    crate::progress::Phase::Creating,
                ));
                // A warm-pool member's token (its pool secret) goes into a
                // file instead of the environment: the environment is fixed
                // for the container's life, and the Space that claims it
                // must keep its own token across restarts.
                let original = spec;
                let (spec, secret) = pool_member_spec(spec)?;
                let spec = spec.as_ref();
                // Warm: restore this configuration's golden checkpoint.
                let plan = self.golden_plan(original, reference).await;
                if let Some(rec) = plan.as_ref().and_then(|p| p.record.as_ref()) {
                    match self
                        .start_warm(spec, reference, platform.clone(), rec, secret.as_deref())
                        .await
                    {
                        Ok(inst) => return Ok(inst),
                        Err(e) => {
                            // A checkpoint this engine cannot restore any more
                            // (a new runsc, a changed kernel): drop it, boot cold.
                            tracing::warn!(name = spec.name, error = %e, "golden checkpoint did not restore; booting cold");
                            let _ = self
                                .docker
                                .remove_container(
                                    &spec.name,
                                    Some(RemoveContainerOptions {
                                        force: true,
                                        ..Default::default()
                                    }),
                                )
                                .await;
                            self.drop_golden(rec).await;
                        }
                    }
                }
                record_golden = plan
                    .filter(|p| p.record.is_none())
                    .map(|p| (p, reference.clone()));
                // A host port picked for a published port can be taken
                // inside the engine's machine by the time the container
                // starts (the built-in runtime picks numbers on the Mac):
                // start again with new ones.
                let mut attempt = 0;
                loop {
                    attempt += 1;
                    self.create(
                        &spec.name,
                        reference,
                        spec,
                        HashMap::new(),
                        platform.clone(),
                        None,
                    )
                    .await?;
                    if let Some(secret) = &secret
                        && let Err(e) = self.write_token_file(&spec.name, secret).await
                    {
                        self.remove_force(&spec.name).await;
                        return Err(e);
                    }
                    crate::progress::report(crate::progress::Progress::phase(
                        crate::progress::Phase::Booting,
                    ));
                    match self.start_policed(&spec.name).await {
                        Ok(()) => break,
                        Err(e) => {
                            self.unpolice(&spec.name).await;
                            self.remove_force(&spec.name).await;
                            let msg = e.to_string();
                            if attempt < PORT_CONFLICT_ATTEMPTS && is_port_conflict(&msg) {
                                tracing::info!(name = spec.name, attempt, error = %msg, "host port taken; starting with new ones");
                                continue;
                            }
                            return Err(e);
                        }
                    }
                }
                self.inspect(&spec.name)
                    .await?
                    .ok_or_else(|| VmmError::NotFound(spec.name.clone()))?
            }
        };
        let inst = self.instance_from(&spec.name, &info);
        if inst.status != Status::Running {
            return Err(VmmError::other(format!(
                "container '{}' exited right after start (status {:?})",
                spec.name, inst.status
            )));
        }
        if let Some((auth, token, dir)) = restored {
            let spacesd = inst
                .endpoints
                .host_port(snapshot::SPACESD_PORT)
                .map(|p| format!("http://{}:{p}", inst.endpoints.host));
            if let Err(e) =
                snapshot::give_identity(&self.docker, &spec.name, spacesd, auth, token).await
            {
                // A clone must never run on with its source's identity.
                let _ = self
                    .docker
                    .kill_container(
                        &spec.name,
                        None::<bollard::query_parameters::KillContainerOptions>,
                    )
                    .await;
                return Err(e);
            }
            if dir.contains("/fork-") {
                // Its private checkpoint is loaded (open files stay readable).
                let helper = self.helper();
                tokio::spawn(async move {
                    let _ = helper.remove(&[&dir]).await;
                });
            }
        }
        // Sidecars join the sandbox's network namespace and start before
        // readiness is checked (a probe may target a sidecar's port).
        if let Err(e) = self.ensure_sidecars(spec).await {
            let _ = self.delete(&spec.name).await;
            return Err(e);
        }
        if !spec.probes.is_empty() {
            crate::progress::report(crate::progress::Progress::phase(
                crate::progress::Phase::WaitingForServices,
            ));
        }
        self.wait_probes(
            &spec.name,
            &inst.endpoints,
            &spec.probes,
            spec.ready_timeout,
        )
        .await?;
        if let Some((plan, reference)) = record_golden {
            self.spawn_golden(spec, &reference, plan).await;
        }
        Ok(inst)
    }

    async fn stop(&self, name: &str) -> Result<()> {
        for sc in self.sidecar_names(name).await? {
            let opts = StopContainerOptions {
                t: Some(self.cfg.stop_timeout),
                signal: None,
            };
            let _ = self.docker.stop_container(&sc, Some(opts)).await;
        }
        let opts = StopContainerOptions {
            t: Some(self.cfg.stop_timeout),
            signal: None,
        };
        match self.docker.stop_container(name, Some(opts)).await {
            Ok(()) => Ok(()),
            Err(e) if is_not_found(&e) => Err(VmmError::NotFound(name.into())),
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 304, ..
            }) => Ok(()),
            Err(e) => Err(engine_err(e)),
        }
    }

    async fn suspend(&self, name: &str) -> Result<()> {
        self.docker
            .pause_container(name)
            .await
            .map_err(engine_err)?;
        for sc in self.sidecar_names(name).await? {
            let _ = self.docker.pause_container(&sc).await;
        }
        Ok(())
    }

    /// Unpauses a paused container, or starts a stopped one again (a
    /// `docker stop`, an engine restart), then its sidecars, which join its
    /// network namespace and so start after it.
    async fn resume(&self, name: &str) -> Result<Instance> {
        let before = self
            .inspect(name)
            .await?
            .ok_or_else(|| VmmError::NotFound(name.into()))?;
        if Self::status_from(&before) == Status::Stopped {
            if self.needs_recreate(name, &before) {
                return Err(VmmError::invalid(format!(
                    "{name} must be recreated for its egress policy before it runs again: \
                     start it (not resume)"
                )));
            }
            self.start_policed(name).await?;
            for sc in self.sidecar_names(name).await? {
                let _ = self.docker.start_container(&sc, None).await;
            }
        } else {
            self.docker
                .unpause_container(name)
                .await
                .map_err(engine_err)?;
            for sc in self.sidecar_names(name).await? {
                let _ = self.docker.unpause_container(&sc).await;
            }
        }
        let i = self
            .inspect(name)
            .await?
            .ok_or_else(|| VmmError::NotFound(name.into()))?;
        Ok(self.instance_from(name, &i))
    }

    /// `source` is a container, a checkpoint or a base. The new container is
    /// created (stopped) with the source's config and runtime.
    async fn fork(&self, source: &str, new_name: &str) -> Result<()> {
        validate_name(new_name)?;
        if self.inspect(new_name).await?.is_some() {
            return Err(VmmError::AlreadyExists(new_name.into()));
        }
        let mut labels = HashMap::new();
        labels.insert("cua.vmm.source".to_string(), source.to_string());
        // A fork inherits its source's egress policy (applied at its start).
        egress::copy(source, new_name)?;
        // Memory: a running gVisor Space (checkpointed now) or a memory
        // checkpoint; the fork restores it when it starts.
        let src = self.inspect(source).await?;
        if let Some(i) = &src
            && Self::under_wrapper(i)
            && Self::status_from(i) == Status::Running
            && self.support().await.ok()
        {
            let rec = self
                .mem_checkpoint(source, i, &format!("fork-{new_name}"))
                .await?;
            return self.create_restoring(new_name, &rec, labels).await;
        }
        if src.is_none()
            && let Some(rec) = snapshot::read_record("checkpoints", source)
            && rec.engine == self.endpoint.uri
        {
            return self.create_restoring(new_name, &rec, labels).await;
        }
        let mut spec = StartSpec::new(new_name, ImageSource::Existing);
        let image = if let Some(i) = src {
            let tag = format!("fork-{new_name}");
            if Self::isolation_from(&i) == Isolation::Gvisor
                && Self::status_from(&i) == Status::Running
            {
                self.export_import(source, &i, &tag).await?;
            } else {
                self.commit(source, CKPT_REPO, &tag, true).await?;
            }
            if let Some(cfg) = &i.config {
                spec.env = cfg
                    .env
                    .iter()
                    .flatten()
                    .filter_map(|kv| {
                        kv.split_once('=')
                            .map(|(k, v)| (k.to_string(), v.to_string()))
                    })
                    .collect();
            }
            spec.ports = Self::published(&i);
            format!("{CKPT_REPO}:{tag}")
        } else if self
            .docker
            .inspect_image(&format!("{CKPT_REPO}:{source}"))
            .await
            .is_ok()
        {
            format!("{CKPT_REPO}:{source}")
        } else if self
            .docker
            .inspect_image(&format!("{BASE_REPO}:{source}"))
            .await
            .is_ok()
        {
            format!("{BASE_REPO}:{source}")
        } else {
            return Err(VmmError::NotFound(source.into()));
        };
        self.create(new_name, &image, &spec, labels, None, None)
            .await
    }

    /// Snapshot the container filesystem to `cua-vmm/checkpoint:<checkpoint>`.
    ///
    /// runc: `docker commit`. gVisor: runsc keeps rootfs writes in its own
    /// sandbox overlay (`--overlay2=root:self`), which `docker commit` cannot
    /// see, so the rootfs is streamed out through exec (`tar`) and imported as
    /// a single-layer image carrying the original config.
    async fn checkpoint(&self, name: &str, checkpoint: &str) -> Result<CheckpointInfo> {
        validate_name(checkpoint)?;
        let i = self
            .inspect(name)
            .await?
            .ok_or_else(|| VmmError::NotFound(name.into()))?;
        // Forks of the checkpoint inherit the policy.
        egress::copy(name, checkpoint)?;
        if Self::under_wrapper(&i)
            && Self::status_from(&i) == Status::Running
            && self.support().await.ok()
        {
            // Memory and the root overlay, in well under a second.
            let mut rec = self
                .mem_checkpoint(name, &i, &format!("ckpt-{checkpoint}"))
                .await?;
            rec.name = checkpoint.to_string();
            snapshot::write_record("checkpoints", &rec)?;
        } else if Self::isolation_from(&i) == Isolation::Gvisor
            && Self::status_from(&i) == Status::Running
        {
            self.export_import(name, &i, checkpoint).await?;
        } else {
            self.commit(name, CKPT_REPO, checkpoint, true).await?;
        }
        Ok(CheckpointInfo::now(
            checkpoint,
            BackendKind::Container,
            Some(name.into()),
        ))
    }

    async fn delete_checkpoint(&self, checkpoint: &str) -> Result<()> {
        egress::forget(checkpoint);
        if let Some(rec) = snapshot::read_record("checkpoints", checkpoint) {
            if rec.engine == self.endpoint.uri {
                self.helper().ensure().await?;
                self.helper().remove(&[&rec.dir]).await?;
            }
            snapshot::remove_record("checkpoints", checkpoint);
            return Ok(());
        }
        let image = format!("{CKPT_REPO}:{checkpoint}");
        match self
            .docker
            .remove_image(
                &image,
                None::<bollard::query_parameters::RemoveImageOptions>,
                None,
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(e) if is_not_found(&e) => Err(VmmError::NotFound(image)),
            // Forks made from the checkpoint still run on its image ("must
            // be forced"): forced removal only untags it. The layers stay
            // for those containers and go with the last of them
            // ([`Self::drop_orphaned_image`]).
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 409, ..
            }) => {
                self.docker
                    .remove_image(
                        &image,
                        Some(bollard::query_parameters::RemoveImageOptions {
                            force: true,
                            noprune: true,
                            ..Default::default()
                        }),
                        None,
                    )
                    .await
                    .map_err(engine_err)?;
                Ok(())
            }
            Err(e) => Err(engine_err(e)),
        }
    }

    async fn list(&self) -> Result<Vec<InstanceSummary>> {
        let mut filters = HashMap::new();
        filters.insert(
            "label".to_string(),
            vec![format!("{LABEL_BACKEND}=container")],
        );
        let opts = ListContainersOptions {
            all: true,
            filters: Some(filters),
            ..Default::default()
        };
        let rows = self
            .docker
            .list_containers(Some(opts))
            .await
            .map_err(engine_err)?;
        use bollard::models::ContainerSummaryStateEnum as S;
        Ok(rows
            .into_iter()
            .map(|c| InstanceSummary {
                // The engine's name first: a warm-pool claim renames the
                // container, and its labels are fixed at create.
                name: c
                    .names
                    .as_ref()
                    .and_then(|n| n.first().map(|s| s.trim_start_matches('/').to_string()))
                    .or_else(|| c.labels.as_ref().and_then(|l| l.get(LABEL_NAME).cloned()))
                    .unwrap_or_default(),
                backend: BackendKind::Container,
                status: match c.state {
                    Some(S::RUNNING) => Status::Running,
                    Some(S::PAUSED) => Status::Paused,
                    Some(S::EXITED) | Some(S::CREATED) | Some(S::DEAD) => Status::Stopped,
                    other => Status::Unknown(format!("{other:?}").to_lowercase()),
                },
            })
            .collect())
    }

    async fn status(&self, name: &str) -> Result<Status> {
        let i = self
            .inspect(name)
            .await?
            .ok_or_else(|| VmmError::NotFound(name.into()))?;
        Ok(Self::status_from(&i))
    }

    /// On the engine host at once (keyed by the container's MAC, bound to
    /// its bridge port while it runs); a stopped container that was not
    /// created for a policy gets it at its next start, recreated first.
    async fn set_egress(
        &self,
        name: &str,
        policy: &crate::egress::EgressPolicy,
    ) -> Result<crate::egress::Enforcement> {
        crate::egress::check(BackendKind::Container, policy)?;
        let i = self
            .inspect(name)
            .await?
            .ok_or_else(|| VmmError::NotFound(name.into()))?;
        let running = matches!(Self::status_from(&i), Status::Running | Status::Paused);
        let previous = egress::kept(name);
        egress::keep(
            name,
            &egress::Kept {
                policy: policy.clone(),
                table: previous.as_ref().and_then(|k| k.table.clone()),
            },
        )?;
        if !running && self.needs_recreate(name, &i) {
            // Applied, recreated, at its next start.
            return Ok(crate::egress::Enforcement::Host);
        }
        if let Err(e) = self.police(name, policy).await {
            // Keep what was there.
            match previous {
                Some(k) => egress::keep(name, &k)?,
                None => egress::forget(name),
            }
            return Err(e);
        }
        Ok(crate::egress::Enforcement::Host)
    }

    async fn egress(&self, name: &str) -> Result<crate::egress::EgressPolicy> {
        Ok(egress::kept(name).map(|k| k.policy).unwrap_or_default())
    }

    async fn delete(&self, name: &str) -> Result<()> {
        let opts = RemoveContainerOptions {
            force: true,
            v: true,
            ..Default::default()
        };
        // A restored container: the engine forgets it, and a fork that never
        // started still owns its private checkpoint.
        let (restored, image_id) = match self.inspect(name).await {
            Ok(Some(i)) => (
                restore_dir(&i).map(|d| (i.id.clone().unwrap_or_default(), d)),
                i.image.clone(),
            ),
            _ => (None, None),
        };
        // Sidecars first: they hold the sandbox's network namespace.
        for sc in self.sidecar_names(name).await.unwrap_or_default() {
            let _ = self.docker.remove_container(&sc, Some(opts.clone())).await;
        }
        match self.docker.remove_container(name, Some(opts)).await {
            Ok(()) => {}
            Err(e) if is_not_found(&e) => {
                egress::forget(name);
                return Err(VmmError::NotFound(name.into()));
            }
            Err(e) => return Err(engine_err(e)),
        }
        self.unpolice(name).await;
        if let Some((id, dir)) = restored {
            let helper = self.helper();
            let _ = helper.forget(&[&id]).await;
            if dir.contains("/fork-") {
                let _ = helper.remove(&[&dir]).await;
            }
        }
        if let Some(id) = image_id {
            self.drop_orphaned_image(&id).await;
        }
        // Intermediate images this backend created for the instance (port
        // rebinds, fork sources). Best effort: still-referenced images stay.
        for tag in [format!("{name}-rebind"), format!("fork-{name}")] {
            let _ = self
                .docker
                .remove_image(
                    &format!("{CKPT_REPO}:{tag}"),
                    None::<bollard::query_parameters::RemoveImageOptions>,
                    None,
                )
                .await;
        }
        Ok(())
    }

    async fn endpoints(&self, name: &str) -> Result<Endpoints> {
        let i = self
            .inspect(name)
            .await?
            .ok_or_else(|| VmmError::NotFound(name.into()))?;
        Ok(Self::endpoints_from(&i))
    }

    async fn guest_exec(&self, name: &str) -> Result<Option<Arc<dyn GuestExec>>> {
        Ok(Some(Arc::new(self.exec_handle(name))))
    }

    /// Reads the container's `/proc/net/tcp{,6}` through the engine (exec
    /// `cat`, no shell): the published port's userland proxy accepts before
    /// the guest listens, so only the container's own netns can tell.
    async fn guest_tcp_listening(&self, name: &str, guest_port: u16) -> Result<Option<bool>> {
        let text = match self
            .exec_capture(name, &["cat", "/proc/net/tcp", "/proc/net/tcp6"])
            .await
        {
            Ok(t) => t,
            Err(VmmError::NotFound(n)) => return Err(VmmError::NotFound(n)),
            // No `cat` in the image, or exec refused: the caller falls back.
            Err(e) => {
                tracing::debug!(container = name, error = %e, "procfs probe unavailable");
                return Ok(None);
            }
        };
        Ok(crate::probe::proc_net_listens(&text, guest_port))
    }

    /// Binds a running warm-pool member to a Space: writes the Space's
    /// token where the container reads it at every start (so a restart of
    /// cua-spacesd or of the container keeps it), then renames the
    /// container `new_name`. The caller rotates the running cua-spacesd's
    /// token with `Init` (authenticated with the member's pool secret).
    ///
    /// A container's hostname is fixed when it is created: every container
    /// gets the engine's neutral default (its short id), never a Space
    /// name, so a claimed Space looks like a cold one and `hostname` in
    /// `identity` is not applied.
    async fn claim(
        &self,
        member: &str,
        new_name: &str,
        identity: &ClaimIdentity,
    ) -> Result<Instance> {
        validate_name(new_name)?;
        let i = self
            .inspect(member)
            .await?
            .ok_or_else(|| VmmError::NotFound(member.into()))?;
        let pooled = i
            .config
            .as_ref()
            .and_then(|c| c.labels.as_ref())
            .is_some_and(|l| l.contains_key(LABEL_POOL));
        if !crate::is_pool_member(member) || !pooled {
            return Err(VmmError::invalid(format!(
                "{member} is not a warm-pool member"
            )));
        }
        if Self::status_from(&i) != Status::Running {
            return Err(VmmError::other(format!(
                "warm-pool member {member} is not running"
            )));
        }
        // By id: the rename runs at the same time.
        let id = i.id.clone().unwrap_or_else(|| member.to_string());
        // Two copies: inside the running guest (what a cua-spacesd restart
        // reads), and in the container's rootfs on the engine's side (what
        // the next container start reads: gVisor keeps the guest's own
        // writes in a sandbox overlay that does not outlive the sandbox).
        let persist = async {
            match identity.token.as_deref().filter(|t| !t.is_empty()) {
                Some(token) => {
                    let (inside, rootfs) = tokio::join!(
                        self.persist_token(&id, token),
                        self.write_token_file(&id, token)
                    );
                    inside.and(rootfs)
                }
                None => Ok(()),
            }
        };
        let rename = async {
            let opts = bollard::query_parameters::RenameContainerOptions {
                name: new_name.to_string(),
            };
            match self.docker.rename_container(&id, opts).await {
                Ok(()) => Ok(()),
                Err(e) if is_not_found(&e) => Err(VmmError::NotFound(member.into())),
                Err(bollard::errors::Error::DockerResponseServerError {
                    status_code: 409, ..
                }) => Err(VmmError::AlreadyExists(new_name.into())),
                Err(e) => Err(engine_err(e)),
            }
        };
        let (persisted, renamed) = tokio::join!(persist, rename);
        renamed?;
        if egress::kept(member).is_some() {
            egress::copy(member, new_name)?;
            egress::forget(member);
        }
        if let Err(e) = persisted {
            // Renamed to the Space but without its token on disk: it must
            // not become the Space. The caller starts it cold.
            let _ = self
                .docker
                .remove_container(
                    &id,
                    Some(RemoveContainerOptions {
                        force: true,
                        v: true,
                        ..Default::default()
                    }),
                )
                .await;
            return Err(e);
        }
        Ok(self.instance_from(new_name, &i))
    }
}

/// `spec` as a container create needs it: a warm-pool member's Spaces token
/// (its pool secret) moves out of the environment, returned to be written
/// to [`TOKEN_FILE`] before the container starts.
fn pool_member_spec(spec: &StartSpec) -> Result<(std::borrow::Cow<'_, StartSpec>, Option<String>)> {
    if !spec.pool_member {
        return Ok((std::borrow::Cow::Borrowed(spec), None));
    }
    let mut s = spec.clone();
    let mut secret = None;
    for k in TOKEN_ENV {
        if let Some(v) = s.env.remove(k).filter(|v| !v.is_empty()) {
            secret.get_or_insert(v);
        }
    }
    let Some(secret) = secret else {
        return Err(VmmError::invalid(
            "a warm-pool member needs its pool secret (CUA_ENV_TOKEN)",
        ));
    };
    s.labels
        .entry(LABEL_POOL.into())
        .or_insert_with(|| "true".into());
    Ok((std::borrow::Cow::Owned(s), Some(secret)))
}

/// Where a container's Spaces token lives when it is not in the
/// environment (relative to `/`). The Spaces images' `ensure-env-token.sh`
/// copies it to `/run/cua/env-token` at every container start, where
/// cua-spacesd reads it.
const TOKEN_FILE: &str = "etc/cua/env-token";

/// A tar holding `TOKEN_FILE` (root, 0600) with `token`.
fn token_tar(token: &str) -> Result<Vec<u8>> {
    let mut b = tar::Builder::new(Vec::new());
    let mut dir = tar::Header::new_gnu();
    dir.set_entry_type(tar::EntryType::Directory);
    dir.set_mode(0o755);
    dir.set_uid(0);
    dir.set_gid(0);
    dir.set_size(0);
    dir.set_mtime(host::now_secs());
    b.append_data(&mut dir, "etc/cua/", std::io::empty())?;
    let body = format!("{token}\n");
    let mut h = tar::Header::new_gnu();
    h.set_entry_type(tar::EntryType::Regular);
    h.set_mode(0o600);
    h.set_uid(0);
    h.set_gid(0);
    h.set_size(body.len() as u64);
    h.set_mtime(host::now_secs());
    b.append_data(&mut h, TOKEN_FILE, body.as_bytes())?;
    Ok(b.into_inner()?)
}

impl ContainerRuntime {
    /// Whether the engine runs in a VM whose published ports a forwarder
    /// opens on this machine under the same number (Colima and the built-in
    /// runtime, through Lima). The engine picks those numbers from its own
    /// view, so another engine on this machine (OrbStack, a second VM) may
    /// already hold one here, and a client would reach the wrong guest:
    /// such engines get ports this machine has free.
    fn forwards_ports_by_number(&self) -> bool {
        matches!(self.endpoint.kind, EngineKind::Colima { .. })
            || (cfg!(target_os = "macos") && crate::managed::is_endpoint(&self.endpoint.uri))
    }

    /// Writes a container's Spaces token to [`TOKEN_FILE`] in its rootfs on
    /// the engine's side: what every later start of the container reads.
    /// A running gVisor guest does not see it (it keeps its own writes in a
    /// sandbox overlay over the rootfs as it was at start).
    async fn write_token_file(&self, name: &str, token: &str) -> Result<()> {
        let opts = bollard::query_parameters::UploadToContainerOptions {
            path: "/".into(),
            ..Default::default()
        };
        self.docker
            .upload_to_container(
                name,
                Some(opts),
                bollard::body_full(bytes::Bytes::from(token_tar(token)?)),
            )
            .await
            .map_err(engine_err)
    }

    /// Replaces a running container's Spaces token in [`TOKEN_FILE`] and
    /// in `/run/cua/env-token` (what a cua-spacesd restart reads), keeping
    /// each file's owner and mode. The token travels on stdin, never argv.
    async fn persist_token(&self, name: &str, token: &str) -> Result<()> {
        const SCRIPT: &str = "set -e; t=$(cat); umask 077; mkdir -p /etc/cua; \
             printf '%s\\n' \"$t\" > /etc/cua/env-token; \
             if [ -f /run/cua/env-token ]; then printf '%s\\n' \"$t\" > /run/cua/env-token; fi";
        let req = ExecRequest {
            user: Some("0".into()),
            ..ExecRequest::sh(SCRIPT)
                .stdin(token.as_bytes().to_vec())
                .timeout(Duration::from_secs(20))
        };
        let out = self.exec_handle(name).exec(req).await?;
        if out.exit_code != 0 {
            return Err(VmmError::other(format!(
                "could not store the Space's token in {name} (exit {}): {}",
                out.exit_code,
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(())
    }

    /// This home's warm-pool members ([`LABEL_POOL`] containers still
    /// named [`crate::POOL_MEMBER_PREFIX`]...), each with whether it runs
    /// and whether the host keeps its shape warm on purpose
    /// ([`POOL_KEEP`]).
    pub async fn pool_members(&self) -> Result<Vec<PoolMemberInfo>> {
        let mut filters = HashMap::new();
        filters.insert(
            "label".to_string(),
            vec![
                LABEL_POOL.to_string(),
                format!("{LABEL_HOME}={}", home_tag()),
            ],
        );
        let opts = ListContainersOptions {
            all: true,
            filters: Some(filters),
            ..Default::default()
        };
        let rows = self
            .docker
            .list_containers(Some(opts))
            .await
            .map_err(engine_err)?;
        use bollard::models::ContainerSummaryStateEnum as S;
        Ok(rows
            .into_iter()
            .filter_map(|c| {
                let name = c
                    .names
                    .as_ref()?
                    .first()?
                    .trim_start_matches('/')
                    .to_string();
                crate::is_pool_member(&name).then(|| PoolMemberInfo {
                    keep: c
                        .labels
                        .as_ref()
                        .and_then(|l| l.get(LABEL_POOL))
                        .is_some_and(|v| v == POOL_KEEP),
                    running: matches!(c.state, Some(S::RUNNING) | Some(S::PAUSED)),
                    name,
                })
            })
            .collect())
    }

    /// The engine's memory in MiB (the VM's, for an engine in a VM).
    pub async fn memory_total_mb(&self) -> Option<u64> {
        let info = self.docker.info().await.ok()?;
        info.mem_total
            .and_then(|b| u64::try_from(b).ok())
            .map(|b| b / (1024 * 1024))
            .filter(|mb| *mb > 0)
    }

    /// What container `name` uses now in MiB (its memory less the page
    /// cache it could drop, as `docker stats` counts it).
    pub async fn memory_used_mb(&self, name: &str) -> Option<u64> {
        let opts = bollard::query_parameters::StatsOptions {
            stream: false,
            one_shot: true,
        };
        let s = self.docker.stats(name, Some(opts)).next().await?.ok()?;
        let m = s.memory_stats?;
        let usage = m.usage?;
        let cache = m
            .stats
            .as_ref()
            .and_then(|st| {
                st.get("inactive_file")
                    .or_else(|| st.get("total_inactive_file"))
            })
            .copied()
            .unwrap_or(0);
        Some(usage.saturating_sub(cache) / (1024 * 1024))
    }

    /// Every running container of the engine (this home's or not), for
    /// what the engine's memory holds.
    pub async fn running_containers(&self) -> Result<Vec<String>> {
        let rows = self
            .docker
            .list_containers(Some(ListContainersOptions {
                all: false,
                ..Default::default()
            }))
            .await
            .map_err(engine_err)?;
        Ok(rows
            .into_iter()
            .filter_map(|c| Some(c.names?.first()?.trim_start_matches('/').to_string()))
            .collect())
    }
    /// The OCI runtime for `spec`'s containers: the explicit
    /// [`StartSpec::container_runtime`], else the engine's choice (gVisor
    /// when available). Sidecars under gVisor are refused (see
    /// [`choose_group_runtime`]); nothing silently falls back to runc.
    async fn group_runtime(&self, spec: &StartSpec) -> Result<String> {
        let explicit = spec.container_runtime.as_deref();
        let resolved = if explicit == Some("runc") {
            "runc".to_string()
        } else {
            self.resolve_oci_runtime().await?
        };
        choose_group_runtime(explicit, &resolved, !spec.sidecars.is_empty())
    }

    /// Creates and starts `spec.sidecars` that are not running yet, each in
    /// the sandbox container's network namespace.
    async fn ensure_sidecars(&self, spec: &StartSpec) -> Result<()> {
        if spec.sidecars.is_empty() {
            return Ok(());
        }
        let runtime = self.group_runtime(spec).await?;
        let platform = Self::platform_for(spec);
        for sc in &spec.sidecars {
            let cname = sidecar_container_name(&spec.name, &sc.name);
            validate_name(&cname)?;
            match self.inspect(&cname).await? {
                Some(i) if Self::status_from(&i) == Status::Running => continue,
                Some(_) => {
                    // A stopped sidecar may point at a namespace that is
                    // gone: recreate it.
                    let _ = self
                        .docker
                        .remove_container(
                            &cname,
                            Some(RemoveContainerOptions {
                                force: true,
                                ..Default::default()
                            }),
                        )
                        .await;
                }
                None => {}
            }
            self.ensure_image_with(&sc.image, platform.as_deref(), spec.registry_auth.as_ref())
                .await?;
            let body = sidecar_body(&spec.name, sc, runtime.clone());
            self.docker
                .create_container(
                    Some(CreateContainerOptions {
                        name: Some(cname.clone()),
                        platform: platform.clone().unwrap_or_default(),
                    }),
                    body,
                )
                .await
                .map_err(|e| VmmError::Engine(format!("sidecar {}: {e}", sc.name)))?;
            self.docker
                .start_container(&cname, None)
                .await
                .map_err(|e| VmmError::Engine(format!("sidecar {}: {e}", sc.name)))?;
        }
        Ok(())
    }

    /// Containers created as sidecars of sandbox `name`.
    async fn sidecar_names(&self, name: &str) -> Result<Vec<String>> {
        let mut filters = HashMap::new();
        filters.insert(
            "label".to_string(),
            vec![format!("{LABEL_SIDECAR_OF}={name}")],
        );
        let rows = self
            .docker
            .list_containers(Some(ListContainersOptions {
                all: true,
                filters: Some(filters),
                ..Default::default()
            }))
            .await
            .map_err(engine_err)?;
        Ok(rows
            .into_iter()
            .filter_map(|c| {
                c.names?
                    .first()
                    .map(|n| n.trim_start_matches('/').to_string())
            })
            .collect())
    }

    /// Replaces never-started container `name` (`i`) by one with `env` and
    /// `ports` added to its published ports; everything else (image,
    /// command, labels, runtime, restore annotation) is kept.
    async fn recreate(
        &self,
        name: &str,
        i: &bollard::models::ContainerInspectResponse,
        env: Vec<String>,
        ports: &[u16],
    ) -> Result<()> {
        let cfg = i.config.clone().unwrap_or_default();
        let mut hc = i.host_config.clone().unwrap_or_default();
        let mut bindings = hc.port_bindings.clone().unwrap_or_default();
        let mut exposed = cfg.exposed_ports.clone().unwrap_or_default();
        for p in ports {
            let key = format!("{p}/tcp");
            if !exposed.contains(&key) {
                exposed.push(key.clone());
            }
            bindings.entry(key).or_insert_with(|| {
                Some(vec![PortBinding {
                    host_ip: Some("127.0.0.1".into()),
                    host_port: Some(String::new()),
                }])
            });
        }
        hc.port_bindings = Some(bindings);
        let mut labels = cfg.labels;
        let networking_config = match self.mac_config(name).await {
            Some((net, mac)) => {
                labels
                    .get_or_insert_with(Default::default)
                    .insert(egress::LABEL_MAC.into(), mac);
                Some(net)
            }
            None => None,
        };
        if egress::blocks(name) {
            hc.cap_drop = Some(vec!["NET_RAW".into()]);
        }
        let body = ContainerCreateBody {
            image: cfg.image,
            env: Some(env),
            entrypoint: cfg.entrypoint,
            cmd: cfg.cmd,
            labels,
            exposed_ports: (!exposed.is_empty()).then_some(exposed),
            working_dir: cfg.working_dir,
            user: cfg.user,
            stop_signal: cfg.stop_signal,
            // Kept as created (off, see `create_body`).
            healthcheck: cfg.healthcheck.or_else(|| Some(no_healthcheck())),
            host_config: Some(hc),
            networking_config,
            ..Default::default()
        };
        self.docker
            .remove_container(
                name,
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await
            .map_err(engine_err)?;
        self.docker
            .create_container(
                Some(CreateContainerOptions {
                    name: Some(name.to_string()),
                    platform: String::new(),
                }),
                body,
            )
            .await
            .map_err(engine_err)?;
        Ok(())
    }

    /// A fresh start served by golden checkpoint `rec`: the container is
    /// created with `spec` (its own token), restores the checkpoint,
    /// rotates the golden token to its own and is ready.
    async fn start_warm(
        &self,
        spec: &StartSpec,
        reference: &str,
        platform: Option<String>,
        rec: &snapshot::SnapRecord,
        secret: Option<&str>,
    ) -> Result<Instance> {
        let started = std::time::Instant::now();
        self.create(
            &spec.name,
            reference,
            spec,
            HashMap::new(),
            platform,
            Some(&rec.dir),
        )
        .await?;
        // A warm-pool member's pool secret, where a restart reads it.
        if let Some(secret) = secret {
            self.write_token_file(&spec.name, secret).await?;
        }
        crate::progress::report(crate::progress::Progress::phase(
            crate::progress::Phase::Booting,
        ));
        self.start_policed(&spec.name).await?;
        let info = self
            .inspect(&spec.name)
            .await?
            .ok_or_else(|| VmmError::NotFound(spec.name.clone()))?;
        let inst = self.instance_from(&spec.name, &info);
        if inst.status != Status::Running {
            return Err(VmmError::other(format!(
                "container '{}' exited right after its restore",
                spec.name
            )));
        }
        let restored_ms = started.elapsed().as_millis() as u64;
        let spacesd = inst
            .endpoints
            .host_port(snapshot::SPACESD_PORT)
            .map(|p| format!("http://{}:{p}", inst.endpoints.host));
        let token = secret.map(str::to_string).or_else(|| {
            snapshot::TOKEN_KEYS
                .iter()
                .find_map(|k| spec.env.get(*k).filter(|v| !v.is_empty()).cloned())
        });
        snapshot::give_identity(&self.docker, &spec.name, spacesd, rec.token(), token).await?;
        self.wait_probes(
            &spec.name,
            &inst.endpoints,
            &spec.probes,
            spec.ready_timeout,
        )
        .await?;
        tracing::info!(
            name = spec.name,
            restored_ms,
            ms = started.elapsed().as_millis() as u64,
            "gVisor Space started warm"
        );
        Ok(inst)
    }

    /// Readiness probes where a TCP probe asks the container's netns
    /// ([`Runtime::guest_tcp_listening`]) and only falls back to the
    /// host-side check when the image cannot answer.
    async fn wait_probes(
        &self,
        name: &str,
        ep: &Endpoints,
        probes: &[Probe],
        timeout: Duration,
    ) -> Result<()> {
        let deadline = tokio::time::Instant::now() + timeout;
        for probe in probes {
            let mut backoff = crate::probe::Backoff::default();
            loop {
                let ok = match probe {
                    Probe::Tcp { port } => match self.guest_tcp_listening(name, *port).await? {
                        Some(listening) => listening,
                        None => crate::probe::check(ep, probe).await?,
                    },
                    other => crate::probe::check(ep, other).await?,
                };
                if ok {
                    break;
                }
                // Fail fast when the container's command exited: its
                // probe can never pass.
                if let Some(i) = self.inspect(name).await?
                    && Self::status_from(&i) == Status::Stopped
                {
                    let code = i.state.as_ref().and_then(|s| s.exit_code);
                    return Err(VmmError::other(format!(
                        "container '{name}' exited (code {}) before {probe:?} passed; \
                         see `docker logs {name}` or `cua sb logs {name}`",
                        code.map_or_else(|| "unknown".into(), |c| c.to_string())
                    )));
                }
                if tokio::time::Instant::now() >= deadline {
                    return Err(VmmError::Timeout {
                        name: name.to_string(),
                        secs: timeout.as_secs(),
                        detail: format!("probe {probe:?} never passed"),
                    });
                }
                tokio::time::sleep(backoff.step()).await;
            }
        }
        Ok(())
    }

    /// Runs `argv` in the container (no shell) and returns its stdout, capped
    /// at 4 MiB and 5 s. The exit code is ignored: `cat` exits non-zero when
    /// one of several files is missing but still prints the rest.
    async fn exec_capture(&self, name: &str, argv: &[&str]) -> Result<String> {
        const CAP: usize = 4 << 20;
        let fut = async {
            let opts = CreateExecOptions::<String> {
                cmd: Some(argv.iter().map(|a| a.to_string()).collect()),
                attach_stdout: Some(true),
                attach_stderr: Some(false),
                ..Default::default()
            };
            let created = match self.docker.create_exec(name, opts).await {
                Ok(c) => c,
                Err(e) if is_not_found(&e) => return Err(VmmError::NotFound(name.into())),
                Err(e) => return Err(engine_err(e)),
            };
            let started = self
                .docker
                .start_exec(&created.id, None::<StartExecOptions>)
                .await
                .map_err(engine_err)?;
            let mut out = Vec::new();
            if let StartExecResults::Attached { mut output, .. } = started {
                while let Some(msg) = output.next().await {
                    if let bollard::container::LogOutput::StdOut { message } =
                        msg.map_err(engine_err)?
                    {
                        out.extend_from_slice(&message);
                        if out.len() >= CAP {
                            break;
                        }
                    }
                }
            }
            Ok(String::from_utf8_lossy(&out).into_owned())
        };
        tokio::time::timeout(Duration::from_secs(5), fut)
            .await
            .map_err(|_| VmmError::other(format!("exec {argv:?} in '{name}' timed out")))?
    }
}

/// Dockerfile-style `changes` that restore a container's config on import.
fn config_changes(cfg: Option<&bollard::models::ContainerConfig>) -> Vec<String> {
    let Some(c) = cfg else { return vec![] };
    let json = |v: &Vec<String>| serde_json::to_string(v).unwrap_or_else(|_| "[]".into());
    let mut out = Vec::new();
    for e in c.env.iter().flatten() {
        if let Some((k, v)) = e.split_once('=') {
            out.push(format!(
                "ENV {k}={}",
                serde_json::to_string(v).unwrap_or_default()
            ));
        }
    }
    if let Some(ep) = c.entrypoint.as_ref().filter(|v| !v.is_empty()) {
        out.push(format!("ENTRYPOINT {}", json(ep)));
    }
    if let Some(cmd) = c.cmd.as_ref().filter(|v| !v.is_empty()) {
        out.push(format!("CMD {}", json(cmd)));
    }
    if let Some(wd) = c.working_dir.as_ref().filter(|s| !s.is_empty()) {
        out.push(format!("WORKDIR {wd}"));
    }
    if let Some(u) = c.user.as_ref().filter(|s| !s.is_empty()) {
        out.push(format!("USER {u}"));
    }
    for p in c.exposed_ports.iter().flatten() {
        out.push(format!("EXPOSE {p}"));
    }
    if let Some(sig) = c.stop_signal.as_ref().filter(|s| !s.is_empty()) {
        out.push(format!("STOPSIGNAL {sig}"));
    }
    out
}

/// [`GuestExec`] over the Docker exec API — no agent or sshd in the image.
#[derive(Clone)]
pub struct DockerExec {
    docker: Docker,
    container: String,
}

impl DockerExec {
    /// Run `script` and stream its stdout (for large outputs such as a rootfs
    /// tar). Returns the stream and the exec id (inspect it for the exit code
    /// after the stream ends).
    pub async fn stream_stdout(
        &self,
        script: &str,
    ) -> Result<(
        std::pin::Pin<
            Box<
                dyn futures::Stream<Item = std::result::Result<bytes::Bytes, std::io::Error>>
                    + Send,
            >,
        >,
        String,
    )> {
        let opts = CreateExecOptions::<String> {
            cmd: Some(vec!["/bin/sh".into(), "-c".into(), script.to_string()]),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            ..Default::default()
        };
        let created = self
            .docker
            .create_exec(&self.container, opts)
            .await
            .map_err(engine_err)?;
        let started = self
            .docker
            .start_exec(&created.id, None)
            .await
            .map_err(engine_err)?;
        let StartExecResults::Attached { output, input } = started else {
            return Err(VmmError::other("exec unexpectedly detached"));
        };
        drop(input);
        let stream = output.filter_map(|msg| async move {
            match msg {
                Ok(bollard::container::LogOutput::StdOut { message }) => Some(Ok(message)),
                Ok(bollard::container::LogOutput::StdErr { message }) => {
                    tracing::debug!(stderr = %String::from_utf8_lossy(&message), "exec stderr");
                    None
                }
                Ok(_) => None,
                Err(e) => Some(Err(std::io::Error::other(e.to_string()))),
            }
        });
        Ok((Box::pin(stream), created.id))
    }
}

#[async_trait]
impl GuestExec for DockerExec {
    async fn exec(&self, req: ExecRequest) -> Result<ExecOutput> {
        let fut = async {
            let opts = CreateExecOptions::<String> {
                cmd: Some(vec!["/bin/sh".into(), "-c".into(), req.script.clone()]),
                env: (!req.env.is_empty())
                    .then(|| req.env.iter().map(|(k, v)| format!("{k}={v}")).collect()),
                user: req.user.clone(),
                attach_stdin: Some(req.stdin.is_some()),
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                ..Default::default()
            };
            let created = self
                .docker
                .create_exec(&self.container, opts)
                .await
                .map_err(engine_err)?;
            let started = self
                .docker
                .start_exec(
                    &created.id,
                    Some(StartExecOptions {
                        detach: false,
                        tty: false,
                        output_capacity: None,
                    }),
                )
                .await
                .map_err(engine_err)?;
            let mut out = ExecOutput::default();
            if let StartExecResults::Attached {
                mut output,
                mut input,
            } = started
            {
                if let Some(data) = &req.stdin {
                    input.write_all(data).await?;
                    input.shutdown().await?;
                }
                drop(input);
                while let Some(msg) = output.next().await {
                    match msg.map_err(engine_err)? {
                        bollard::container::LogOutput::StdOut { message } => {
                            out.stdout.extend_from_slice(&message)
                        }
                        bollard::container::LogOutput::StdErr { message } => {
                            out.stderr.extend_from_slice(&message)
                        }
                        bollard::container::LogOutput::Console { message } => {
                            out.stdout.extend_from_slice(&message)
                        }
                        _ => {}
                    }
                }
            }
            let inspected = self
                .docker
                .inspect_exec(&created.id)
                .await
                .map_err(engine_err)?;
            out.exit_code = inspected.exit_code.unwrap_or(-1);
            Ok(out)
        };
        match req.timeout {
            Some(t) => tokio::time::timeout(t, fut)
                .await
                .map_err(|_| VmmError::Timeout {
                    name: self.container.clone(),
                    secs: t.as_secs(),
                    detail: "exec timed out".into(),
                })?,
            None => fut.await,
        }
    }
}

/// Gives every binding of `bindings` without a host port one this machine
/// has free on loopback now (see
/// [`ContainerRuntime::forwards_ports_by_number`]).
/// Starts of one container that may meet a taken host port
/// ([`is_port_conflict`]).
const PORT_CONFLICT_ATTEMPTS: u32 = 3;

/// Whether an engine error says a published host port was taken when the
/// container started.
fn is_port_conflict(msg: &str) -> bool {
    msg.contains("failed to bind host port")
        || msg.contains("port is already allocated")
        || (msg.contains("programming external connectivity")
            && msg.contains("address already in use"))
}

fn pick_host_ports(bindings: &mut HashMap<String, Option<Vec<PortBinding>>>) -> Result<()> {
    // Held until all are chosen, so one create never picks a port twice.
    let mut held = Vec::new();
    for b in bindings.values_mut().flatten().flatten() {
        if b.host_port.as_deref().is_some_and(|p| !p.is_empty()) {
            continue;
        }
        let l = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        b.host_port = Some(l.local_addr()?.port().to_string());
        held.push(l);
    }
    Ok(())
}

/// One warm-pool member container ([`ContainerRuntime::pool_members`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PoolMemberInfo {
    /// Container name ([`crate::POOL_MEMBER_PREFIX`]...).
    pub name: String,
    /// Running (or paused).
    pub running: bool,
    /// Its shape is kept warm on purpose ([`POOL_KEEP`]).
    pub keep: bool,
}

/// The Docker create body for a sandbox container. The guest environment
/// (`StartSpec::env`, e.g. the SDK-minted `CUA_ENV_TOKEN`) goes into the
/// container environment, where cua-spacesd / the image's
/// `ensure-env-token.sh` pick it up.
fn create_body(
    name: &str,
    image: &str,
    spec: &StartSpec,
    base_labels: HashMap<String, String>,
    runtime: String,
) -> ContainerCreateBody {
    let mut labels = base_labels;
    labels.insert(LABEL_SANDBOX.into(), "true".into());
    labels.insert(LABEL_NAME.into(), name.into());
    labels.insert(LABEL_BACKEND.into(), "container".into());
    labels.insert(LABEL_MANAGED.into(), "true".into());
    labels.insert(LABEL_KIND.into(), "sandbox".into());
    labels.insert(LABEL_CREATED.into(), host::now_secs().to_string());
    labels.insert(LABEL_HOME.into(), home_tag());
    // The caller's labels win (a build container sets `ai.cua.kind=build`,
    // an ephemeral sandbox `ai.cua.ephemeral=true`).
    labels.extend(spec.labels.iter().map(|(k, v)| (k.clone(), v.clone())));
    let mut bindings = HashMap::new();
    let mut exposed = Vec::new();
    // Sidecars share this container's network namespace, so their ports are
    // published here (a `container:` network cannot publish its own).
    let mut ports: Vec<u16> = spec.ports.clone();
    for p in spec.sidecars.iter().flat_map(|s| s.ports.iter()) {
        if !ports.contains(p) {
            ports.push(*p);
        }
    }
    for p in &ports {
        let key = format!("{p}/tcp");
        exposed.push(key.clone());
        bindings.insert(
            key,
            Some(vec![PortBinding {
                host_ip: Some("127.0.0.1".into()),
                host_port: Some(String::new()),
            }]),
        );
    }
    ContainerCreateBody {
        image: Some(image.to_string()),
        env: (!spec.env.is_empty())
            .then(|| spec.env.iter().map(|(k, v)| format!("{k}={v}")).collect()),
        // `command` replaces the image's ENTRYPOINT and CMD, like a
        // Kubernetes `command`.
        entrypoint: spec.command.clone().filter(|c| !c.is_empty()),
        cmd: spec
            .command
            .as_ref()
            .filter(|c| !c.is_empty())
            .map(|_| Vec::new()),
        labels: Some(labels),
        exposed_ports: (!exposed.is_empty()).then_some(exposed),
        // No image HEALTHCHECK: cua has its own readiness and never reads
        // Docker's health, and the check is a `docker exec` into every
        // Space on its interval (the Linux images: every 10 s), which under
        // gVisor costs a sandbox exec each time (445 idle Spaces: ~44
        // execs/s; turning it off took 256 gVisor Spaces on an M2 Max
        // from 184 to 355 rollout steps/s).
        healthcheck: Some(no_healthcheck()),
        // An injected cua-spacesd runs as root; it starts the image's
        // command as the image's user.
        user: spec.inject_spacesd.as_ref().map(|_| "0".to_string()),
        host_config: Some(HostConfig {
            // Under gVisor, SYS_ADMIN lets cua-spacesd mount Cua Volume
            // (FUSE) in the guest. The capability applies inside gVisor's
            // own kernel, not the host's; runc containers do not get it.
            cap_add: matches!(runtime.as_str(), "runsc" | snapshot::WRAPPER_RUNTIME)
                .then(|| vec!["SYS_ADMIN".to_string()]),
            runtime: Some(runtime),
            port_bindings: Some(bindings),
            nano_cpus: Some(i64::from(spec.cpus) * 1_000_000_000),
            memory: Some((spec.memory_mb as i64) * 1024 * 1024),
            shm_size: Some(shm_size_bytes(spec.memory_mb)),
            extra_hosts: sidecar_hosts(spec),
            device_requests: gpu_requests(spec),
            binds: mount_binds(spec),
            // When the machine runs out of memory, a Space goes before the
            // engine or the host's own agent (which reports and recovers).
            oom_score_adj: Some(SPACE_OOM_SCORE_ADJ),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// The run config (`ENTRYPOINT`, `CMD`, `User`, `WorkingDir`) of an image
/// as the engine reports it.
fn run_config_of(cfg: Option<&bollard::models::ImageConfig>) -> crate::OciRunConfig {
    let Some(c) = cfg else {
        return crate::OciRunConfig::default();
    };
    crate::OciRunConfig {
        entrypoint: c.entrypoint.clone().unwrap_or_default(),
        cmd: c.cmd.clone().unwrap_or_default(),
        env: c.env.clone().unwrap_or_default(),
        working_dir: c.working_dir.clone().filter(|w| !w.is_empty()),
        user: c.user.clone().filter(|u| !u.is_empty()),
    }
}

/// `host:guest[:ro]` binds for [`StartSpec::mounts`] (`None` without any).
fn mount_binds(spec: &StartSpec) -> Option<Vec<String>> {
    (!spec.mounts.is_empty()).then(|| {
        spec.mounts
            .iter()
            .map(|m| {
                format!(
                    "{}:{}{}",
                    m.host.display(),
                    m.guest,
                    if m.read_only { ":ro" } else { "" }
                )
            })
            .collect()
    })
}

/// Docker's way to turn off an image's HEALTHCHECK.
fn no_healthcheck() -> bollard::models::HealthConfig {
    bollard::models::HealthConfig {
        test: Some(vec!["NONE".into()]),
        ..Default::default()
    }
}

/// The engine's device requests for [`StartSpec::gpu`]: every NVIDIA GPU
/// (`--gpus all`) for [`crate::gpu::NVIDIA`].
fn gpu_requests(spec: &StartSpec) -> Option<Vec<bollard::models::DeviceRequest>> {
    (spec.gpu.as_deref() == Some(crate::gpu::NVIDIA)).then(|| {
        vec![bollard::models::DeviceRequest {
            driver: Some("nvidia".into()),
            count: Some(-1),
            capabilities: Some(vec![vec!["gpu".into()]]),
            ..Default::default()
        }]
    })
}

/// `/dev/shm` of a sandbox container: half its memory limit, at most
/// 2 GiB and at least Docker's 64 MiB. Docker's 64 MiB default crashes
/// browsers (Chromium and Firefox keep renderer buffers there); the size is
/// a ceiling, not an allocation, and tmpfs pages count against the
/// container's memory limit, so it never raises the real cap.
/// A container CPU limit is a ceiling, and Docker refuses one above the CPUs
/// the engine has ("range of CPUs is from 0.01 to N"). Cap it there: the
/// container gets every CPU the engine has, which is all it could use anyway.
/// `engine_cpus <= 0` (unknown) leaves the request as is.
fn cap_nano_cpus(requested: i64, engine_cpus: i64) -> i64 {
    if engine_cpus > 0 {
        requested.min(engine_cpus * 1_000_000_000)
    } else {
        requested
    }
}

pub fn shm_size_bytes(memory_mb: u64) -> i64 {
    const MIB: i64 = 1024 * 1024;
    let half = (memory_mb as i64).saturating_mul(MIB) / 2;
    half.clamp(64 * MIB, 2048 * MIB)
}

/// The hostname sidecars reach the sandbox at.
pub const MAIN_HOSTNAME: &str = "main";

/// `/etc/hosts` entries naming every container of a sidecar group, so the
/// sandbox reaches a sidecar at its name and a sidecar reaches the sandbox
/// at [`MAIN_HOSTNAME`]. They all share the sandbox's network
/// namespace, so every name is loopback. Sidecars joined with
/// `container:<sandbox>` networking use the sandbox's `/etc/hosts` (Docker
/// refuses `--add-host` on them), so the entries go on the sandbox only.
/// `None` without sidecars, so plain sandboxes are unchanged.
fn sidecar_hosts(spec: &StartSpec) -> Option<Vec<String>> {
    if spec.sidecars.is_empty() {
        return None;
    }
    Some(
        std::iter::once(MAIN_HOSTNAME)
            .chain(spec.sidecars.iter().map(|s| s.name.as_str()))
            .map(|h| format!("{h}:127.0.0.1"))
            .collect(),
    )
}

/// Why a sandbox with sidecars cannot run on gVisor under Docker.
pub const SIDECARS_NEED_RUNC: &str = "sidecars share a network namespace, which gVisor can't do \
     under Docker. Pass runtime='runc' to run this group on runc";

/// Normalizes a user-facing container runtime name: `runc`, or `runsc`
/// (also `gvisor`). `None` for anything else.
pub fn normalize_container_runtime(name: &str) -> Option<&'static str> {
    match name.trim().to_ascii_lowercase().as_str() {
        "runc" => Some("runc"),
        "runsc" | "gvisor" => Some("runsc"),
        _ => None,
    }
}

/// The group's OCI runtime. `explicit` is the user's choice (`runc` /
/// `runsc`), `resolved` what the engine would pick (`runsc` when gVisor is
/// available). runc only when chosen explicitly or when gVisor is not
/// available at all; sidecars that would land on gVisor fail fast.
pub fn choose_group_runtime(
    explicit: Option<&str>,
    resolved: &str,
    sidecars: bool,
) -> Result<String> {
    let runtime = match explicit.map(|e| normalize_container_runtime(e).ok_or(e)) {
        Some(Ok("runc")) => "runc".to_string(),
        Some(Ok(_)) if resolved != "runsc" => {
            return Err(VmmError::missing(
                "gVisor (runsc)",
                "runtime='gvisor' was requested but the engine has no runsc",
            ));
        }
        Some(Ok(_)) => "runsc".to_string(),
        Some(Err(other)) => {
            return Err(VmmError::invalid(format!(
                "unknown container runtime {other:?} (use runc or gvisor)"
            )));
        }
        None => resolved.to_string(),
    };
    if sidecars && runtime == "runsc" {
        return Err(VmmError::Unsupported {
            backend: BackendKind::Container.as_str(),
            op: SIDECARS_NEED_RUNC,
        });
    }
    Ok(runtime)
}

/// Docker container name of sidecar `sidecar` of sandbox `sandbox`.
pub fn sidecar_container_name(sandbox: &str, sidecar: &str) -> String {
    format!("{sandbox}-sc-{sidecar}")
}

/// The Docker create body for a sidecar: the sandbox's network namespace,
/// its own image, command and environment, and the default resources.
fn sidecar_body(sandbox: &str, sc: &SidecarSpec, runtime: String) -> ContainerCreateBody {
    let mut labels = HashMap::new();
    labels.insert(LABEL_SANDBOX.to_string(), "true".to_string());
    labels.insert(LABEL_SIDECAR_OF.to_string(), sandbox.to_string());
    labels.insert("cua.sandbox.sidecar".to_string(), sc.name.clone());
    labels.insert(LABEL_MANAGED.to_string(), "true".to_string());
    labels.insert(LABEL_KIND.to_string(), "sidecar".to_string());
    labels.insert(LABEL_CREATED.to_string(), host::now_secs().to_string());
    labels.insert(LABEL_HOME.to_string(), home_tag());
    ContainerCreateBody {
        image: Some(sc.image.clone()),
        env: (!sc.env.is_empty()).then(|| sc.env.iter().map(|(k, v)| format!("{k}={v}")).collect()),
        entrypoint: sc.command.clone().filter(|c| !c.is_empty()),
        cmd: sc
            .command
            .as_ref()
            .filter(|c| !c.is_empty())
            .map(|_| Vec::new()),
        labels: Some(labels),
        host_config: Some(HostConfig {
            runtime: Some(runtime),
            network_mode: Some(format!("container:{sandbox}")),
            nano_cpus: Some(SIDECAR_NANO_CPUS),
            memory: Some(SIDECAR_MEMORY_BYTES),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Tries at one container image pull: a network drop mid-pull is common on
/// home and mobile networks, and a second try continues from the layers the
/// engine already has.
const PULL_ATTEMPTS: u32 = 3;

/// The pause before try `attempt` + 1 (2 s, then 4 s).
fn pull_retry_delay(attempt: u32) -> std::time::Duration {
    std::time::Duration::from_secs(2u64 << attempt.saturating_sub(1).min(3))
}

/// Whether a pull's error is the network dropping (worth another try), not
/// the image, the credentials or the disk (which a retry cannot fix).
fn pull_error_is_transient(message: &str) -> bool {
    let l = message.to_ascii_lowercase();
    const PERMANENT: &[&str] = &[
        "not found",
        "manifest unknown",
        "unauthorized",
        "denied",
        "authentication required",
        "no space left",
        "invalid reference",
        "no matching manifest",
        "toomanyrequests",
        "rate limit",
    ];
    if PERMANENT.iter().any(|p| l.contains(p)) {
        return false;
    }
    const TRANSIENT: &[&str] = &[
        "timeout",
        "timed out",
        "connection reset",
        "connection refused",
        "connection closed",
        "broken pipe",
        "unexpected eof",
        "unexpected end of",
        "tls handshake",
        "temporary failure",
        "no such host",
        "service unavailable",
        "bad gateway",
        "gateway timeout",
        "502",
        "503",
        "504",
        "i/o error",
        "network is unreachable",
        "eof",
    ];
    TRANSIENT.iter().any(|t| l.contains(t))
}

/// Share of a container image pull spent downloading layers (the rest is
/// extracting them), from a measured cold pull of `linux:24.04-slim` on an
/// Apple silicon Mac (Docker 29, containerd image store): about 6 s down,
/// 7 s extracting.
const DOWNLOAD_SHARE: f64 = 0.5;

/// How the engine's pull progress names a layer: the first 12 hex digits
/// of its digest.
fn layer_id(digest: &str) -> String {
    let hex = digest.rsplit(':').next().unwrap_or(digest);
    hex.chars().take(12).collect()
}

/// One pull fraction from the download's and the extraction's (each by
/// bytes; unknown until a layer reports its size).
fn pull_fraction(download: Option<f64>, extract: Option<f64>) -> Option<f64> {
    download.map(|d| DOWNLOAD_SHARE * d + (1.0 - DOWNLOAD_SHARE) * extract.unwrap_or(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_hosts_can_require_gvisor_by_environment() {
        assert!(require_gvisor_from(Some("1")));
        assert!(require_gvisor_from(Some(" true ")));
        assert!(!require_gvisor_from(Some("0")));
        assert!(!require_gvisor_from(Some("")));
        assert!(!require_gvisor_from(None));
    }

    #[tokio::test]
    async fn concurrent_pulls_of_one_image_take_turns() {
        let a = pull_gate("registry.example/img:1");
        let b = pull_gate("registry.example/img:1");
        let other = pull_gate("registry.example/img:2");
        assert!(std::sync::Arc::ptr_eq(&a, &b));
        assert!(!std::sync::Arc::ptr_eq(&a, &other));
        let held = a.lock().await;
        assert!(b.try_lock().is_err(), "a second pull of the image waits");
        assert!(other.try_lock().is_ok(), "another image does not");
        drop(held);
        assert!(b.try_lock().is_ok());
    }

    fn inspected(
        status: bollard::models::ContainerStateStatusEnum,
        finished: &str,
        code: i64,
        oom: bool,
    ) -> bollard::models::ContainerInspectResponse {
        bollard::models::ContainerInspectResponse {
            state: Some(bollard::models::ContainerState {
                status: Some(status),
                finished_at: Some(finished.into()),
                exit_code: Some(code),
                oom_killed: Some(oom),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn a_stopped_container_says_how_it_ended() {
        use bollard::models::ContainerStateStatusEnum as S;
        let at = "2026-10-15T20:40:01.60761462Z";
        // What the engine recorded for a Space the guest kernel killed.
        assert_eq!(
            ContainerRuntime::exit_from(&inspected(S::EXITED, at, 137, true)),
            Some(ExitRecord {
                exit_code: 137,
                oom_killed: true
            })
        );
        assert_eq!(
            ContainerRuntime::exit_from(&inspected(S::DEAD, at, 1, false)),
            Some(ExitRecord {
                exit_code: 1,
                oom_killed: false
            })
        );
        // Running, or created and never started: nothing ended.
        assert_eq!(
            ContainerRuntime::exit_from(&inspected(S::RUNNING, at, 0, false)),
            None
        );
        assert_eq!(
            ContainerRuntime::exit_from(&inspected(S::EXITED, "0001-01-01T00:00:00Z", 0, false)),
            None
        );
    }

    #[test]
    fn a_dropped_network_is_retried_but_a_bad_image_or_disk_is_not() {
        for m in [
            "error pulling image: net/http: TLS handshake timeout",
            "read tcp 10.0.0.2:5000: connection reset by peer",
            "Get \"https://registry-1.docker.io/v2/\": dial tcp: lookup registry-1.docker.io: no such host",
            "unexpected EOF",
            "Error response from daemon: received unexpected HTTP status: 503 Service Unavailable",
        ] {
            assert!(pull_error_is_transient(m), "{m}");
        }
        for m in [
            "manifest for cua/x:1 not found: manifest unknown",
            "pull access denied for x, repository does not exist or may require authorization",
            "unauthorized: authentication required",
            "write /var/lib/docker/tmp: no space left on device",
            "toomanyrequests: You have reached your pull rate limit (timeout)",
            "boom",
        ] {
            assert!(!pull_error_is_transient(m), "{m}");
        }
    }

    #[test]
    fn the_pull_pauses_grow_and_stay_short() {
        assert_eq!(pull_retry_delay(1), std::time::Duration::from_secs(2));
        assert_eq!(pull_retry_delay(2), std::time::Duration::from_secs(4));
        assert!(pull_retry_delay(9) <= std::time::Duration::from_secs(16));
    }

    #[test]
    fn bulk_timeout_defaults_and_overrides() {
        assert_eq!(bulk_timeout_from(None), Duration::from_secs(1800));
        assert_eq!(bulk_timeout_from(Some("7200")), Duration::from_secs(7200));
        assert_eq!(bulk_timeout_from(Some("0")), Duration::from_secs(1800));
        assert_eq!(bulk_timeout_from(Some("abc")), Duration::from_secs(1800));
        assert!(bulk_timeout() > Duration::from_secs(120));
    }

    #[test]
    fn manifest_layers_give_a_pull_its_total_and_count_what_exists() {
        let a = format!("sha256:{}", "a1".repeat(32));
        let b = format!("sha256:{}", "b2".repeat(32));
        assert_eq!(layer_id(&a), "a1a1a1a1a1a1");
        let mut down = crate::progress::Bytes::default();
        for (d, size) in [(&a, 300u64), (&b, 100u64)] {
            down.update(&layer_id(d), 0, size);
        }
        assert_eq!(down.totals(), (0, 400), "the total from the start");
        // The engine already has `a` ("Already exists"): it counts as done.
        down.complete(&layer_id(&a));
        down.update(&layer_id(&b), 50, 100);
        assert_eq!(down.totals(), (350, 400));
    }

    #[test]
    fn a_pull_moves_through_download_then_extraction() {
        let mut down = crate::progress::Bytes::default();
        let mut ext = crate::progress::Bytes::default();
        assert_eq!(
            pull_fraction(down.fraction(), ext.fraction()),
            None,
            "no size yet"
        );
        down.update("a", 50, 100);
        assert_eq!(pull_fraction(down.fraction(), ext.fraction()), Some(0.25));
        down.complete("a");
        ext.update("a", 50, 100);
        assert_eq!(pull_fraction(down.fraction(), ext.fraction()), Some(0.75));
        assert_eq!(ext.fraction_of("a"), Some(0.5));
        assert_eq!(ext.fraction_of("b"), None);
        ext.complete("a");
        assert_eq!(pull_fraction(down.fraction(), ext.fraction()), Some(1.0));
    }

    #[test]
    fn mounts_become_binds_read_only_when_asked() {
        let mut spec = StartSpec::new(
            "sb",
            ImageSource::Oci {
                reference: "img:1".into(),
            },
        );
        let plain = create_body("sb", "img:1", &spec, HashMap::new(), "runsc".into());
        assert_eq!(plain.host_config.unwrap().binds, None);
        spec.mounts = vec![
            crate::Mount {
                host: "/srv/cua/jobs/cj-1".into(),
                guest: "/cua/job".into(),
                read_only: false,
            },
            crate::Mount {
                host: "/srv/cua/tools".into(),
                guest: "/cua/tools".into(),
                read_only: true,
            },
        ];
        let body = create_body("sb", "img:1", &spec, HashMap::new(), "runsc".into());
        assert_eq!(
            body.host_config.unwrap().binds.unwrap(),
            [
                "/srv/cua/jobs/cj-1:/cua/job",
                "/srv/cua/tools:/cua/tools:ro"
            ]
        );
        // Checked before anything is created: absolute existing host
        // directories, absolute guest paths, each guest path once.
        let e = crate::types::check_mounts(BackendKind::Container, &spec.mounts)
            .unwrap_err()
            .to_string();
        assert!(e.contains("existing directory"), "{e}");
        let dir = tempfile::tempdir().unwrap();
        let ok = vec![crate::Mount {
            host: dir.path().to_path_buf(),
            guest: "/cua/job".into(),
            read_only: false,
        }];
        crate::types::check_mounts(BackendKind::Container, &ok).unwrap();
        let mut twice = ok.clone();
        twice.push(ok[0].clone());
        assert!(
            crate::types::check_mounts(BackendKind::Container, &twice)
                .unwrap_err()
                .to_string()
                .contains("two mounts at /cua/job")
        );
        let mut relative = ok.clone();
        relative[0].guest = "cua/job".into();
        assert!(crate::types::check_mounts(BackendKind::Container, &relative).is_err());
    }

    #[test]
    fn sidecars_share_the_sandbox_netns_and_publish_on_it() {
        let sc = SidecarSpec {
            name: "db".into(),
            image: "redis:7-alpine".into(),
            command: Some(vec!["redis-server".into(), "--port".into(), "6380".into()]),
            env: [("A".to_string(), "1".to_string())].into(),
            ports: vec![6380],
        };
        let spec = StartSpec::new("sb", ImageSource::Existing)
            .port(8000)
            .sidecar(sc.clone());
        let main = create_body("sb", "img:1", &spec, HashMap::new(), "runc".into());
        let bindings = main.host_config.clone().unwrap().port_bindings.unwrap();
        assert!(bindings.contains_key("8000/tcp") && bindings.contains_key("6380/tcp"));
        let body = sidecar_body("sb", &sc, "runc".into());
        let hc = body.host_config.unwrap();
        assert_eq!(hc.network_mode.as_deref(), Some("container:sb"));
        assert!(
            hc.extra_hosts.is_none(),
            "a container: network takes no hosts"
        );
        // Names resolve on every container of the group.
        assert_eq!(
            main.host_config.as_ref().unwrap().extra_hosts,
            Some(vec![
                "main:127.0.0.1".to_string(),
                "db:127.0.0.1".to_string()
            ])
        );
        assert!(
            hc.port_bindings.is_none(),
            "a container: network publishes nothing"
        );
        assert_eq!(hc.memory, Some(SIDECAR_MEMORY_BYTES));
        let labels = body.labels.unwrap();
        assert_eq!(labels.get(LABEL_SIDECAR_OF).map(String::as_str), Some("sb"));
        assert!(
            !labels.contains_key(LABEL_BACKEND),
            "sidecars are not listed as sandboxes"
        );
        assert_eq!(body.entrypoint.unwrap()[0], "redis-server");
        assert_eq!(body.env.unwrap(), vec!["A=1".to_string()]);
        assert_eq!(sidecar_container_name("sb", "db"), "sb-sc-db");
    }

    #[test]
    fn sidecars_never_fall_back_to_runc_silently() {
        // The engine has gVisor: sidecars fail unless runc is explicit.
        let err = choose_group_runtime(None, "runsc", true).unwrap_err();
        assert!(matches!(err, VmmError::Unsupported { .. }), "{err}");
        assert!(err.to_string().contains("runtime='runc'"), "{err}");
        assert_eq!(
            choose_group_runtime(Some("runc"), "runsc", true).unwrap(),
            "runc"
        );
        assert!(choose_group_runtime(Some("gvisor"), "runsc", true).is_err());
        // No sidecars: gVisor stays the default; explicit runc is honored.
        assert_eq!(choose_group_runtime(None, "runsc", false).unwrap(), "runsc");
        assert_eq!(
            choose_group_runtime(Some("runc"), "runsc", false).unwrap(),
            "runc"
        );
        assert_eq!(
            choose_group_runtime(Some("gvisor"), "runsc", false).unwrap(),
            "runsc"
        );
        // An engine without gVisor already runs runc (the documented
        // fallback); explicit gVisor there is an error.
        assert_eq!(choose_group_runtime(None, "runc", true).unwrap(), "runc");
        assert!(choose_group_runtime(Some("gvisor"), "runc", false).is_err());
        assert!(choose_group_runtime(Some("kata"), "runsc", false).is_err());
    }

    #[test]
    fn credentials_scope_to_their_registry() {
        let c = RegistryCredentials::new("u", "p").for_registry("localhost:5000");
        assert!(c.applies_to(registry_of("localhost:5000/app:1")));
        assert!(!c.applies_to(registry_of("python:3.12-slim")));
        let hub = RegistryCredentials::new("u", "p").for_registry("index.docker.io");
        assert!(hub.applies_to(registry_of("library/python")));
        let dbg = format!("{:?}", RegistryCredentials::new("u", "s3cret"));
        assert!(
            dbg.contains("<redacted>") && !dbg.contains("s3cret"),
            "{dbg}"
        );
    }

    #[test]
    fn a_cpu_limit_is_capped_at_the_engine_cpus() {
        assert_eq!(cap_nano_cpus(4_000_000_000, 2), 2_000_000_000);
        assert_eq!(cap_nano_cpus(1_000_000_000, 2), 1_000_000_000);
        assert_eq!(cap_nano_cpus(4_000_000_000, 0), 4_000_000_000);
    }

    #[test]
    fn shm_is_half_the_memory_within_64_mib_and_2_gib() {
        const MIB: i64 = 1024 * 1024;
        assert_eq!(shm_size_bytes(4096), 2048 * MIB);
        assert_eq!(shm_size_bytes(16384), 2048 * MIB);
        assert_eq!(shm_size_bytes(2048), 1024 * MIB);
        assert_eq!(shm_size_bytes(64), 64 * MIB);
    }

    #[test]
    fn only_gvisor_sandboxes_can_mount_the_volume() {
        let spec = StartSpec::new("sb", ImageSource::Existing).port(3211);
        let gvisor = create_body("sb", "img:1", &spec, HashMap::new(), "runsc".into());
        assert_eq!(
            gvisor.host_config.unwrap().cap_add,
            Some(vec!["SYS_ADMIN".to_string()])
        );
        let runc = create_body("sb", "img:1", &spec, HashMap::new(), "runc".into());
        assert_eq!(runc.host_config.unwrap().cap_add, None);
    }

    #[test]
    fn a_gpu_container_asks_the_engine_for_every_nvidia_gpu() {
        let mut spec = StartSpec::new("sb", ImageSource::Existing);
        let plain = create_body("sb", "img:1", &spec, HashMap::new(), "runc".into());
        assert_eq!(plain.host_config.unwrap().device_requests, None);
        spec.gpu = Some(crate::gpu::NVIDIA.into());
        let gpu = create_body("sb", "img:1", &spec, HashMap::new(), "runc".into());
        let req = gpu.host_config.unwrap().device_requests.unwrap();
        assert_eq!(req.len(), 1);
        assert_eq!(req[0].driver.as_deref(), Some("nvidia"));
        assert_eq!(req[0].count, Some(-1), "all of them");
        assert_eq!(req[0].capabilities, Some(vec![vec!["gpu".to_string()]]));
    }

    #[test]
    fn a_taken_host_port_is_told_apart() {
        assert!(is_port_conflict(
            "Docker responded with status code 500: failed to set up container networking: driver failed programming external connectivity on endpoint s (x): failed to bind host port 127.0.0.1:60052/tcp: address already in use"
        ));
        assert!(is_port_conflict(
            "Bind for 127.0.0.1:5000 failed: port is already allocated"
        ));
        assert!(!is_port_conflict("no such image: x"));
        assert!(!is_port_conflict("OCI runtime create failed"));
    }

    #[test]
    fn create_body_carries_the_env_token() {
        let spec = StartSpec::new("sb", ImageSource::Existing)
            .env("CUA_ENV_TOKEN", "tok123")
            .port(3211);
        let body = create_body("sb", "img:1", &spec, HashMap::new(), "runsc".into());
        assert_eq!(
            body.healthcheck.as_ref().and_then(|h| h.test.clone()),
            Some(vec!["NONE".to_string()]),
            "the image's HEALTHCHECK is off (an exec into every Space every 10 s)"
        );
        let env = body.env.unwrap();
        assert!(env.contains(&"CUA_ENV_TOKEN=tok123".to_string()), "{env:?}");
        let hc = body.host_config.unwrap();
        assert_eq!(hc.runtime.as_deref(), Some("runsc"));
        // Default 2 GiB of memory: /dev/shm is 1 GiB, not Docker's 64 MiB.
        assert_eq!(hc.shm_size, Some(1024 * 1024 * 1024));
        assert!(hc.port_bindings.unwrap().contains_key("3211/tcp"));
        let plain = StartSpec::new("sb", ImageSource::Existing);
        let bare = create_body("sb", "img:1", &plain, HashMap::new(), "runc".into());
        assert!(bare.env.is_none());
        assert!(
            bare.host_config.as_ref().unwrap().extra_hosts.is_none(),
            "no sidecars, no host entries"
        );
        assert!(
            bare.entrypoint.is_none() && bare.cmd.is_none(),
            "the image decides"
        );
    }

    #[test]
    fn everything_the_sdk_creates_is_labelled() {
        let mut spec = StartSpec::new("sb", ImageSource::Existing);
        spec.labels
            .insert(LABEL_EPHEMERAL.to_string(), "true".to_string());
        let body = create_body("sb", "img:1", &spec, HashMap::new(), "runc".into());
        let l = body.labels.unwrap();
        assert_eq!(l.get(LABEL_MANAGED).map(String::as_str), Some("true"));
        assert_eq!(l.get(LABEL_KIND).map(String::as_str), Some("sandbox"));
        assert_eq!(l.get(LABEL_EPHEMERAL).map(String::as_str), Some("true"));
        assert!(
            l.get(LABEL_CREATED)
                .is_some_and(|c| c.parse::<u64>().is_ok())
        );
        // A build container names its own kind.
        let mut b = StartSpec::new("cua-build-ctr-1", ImageSource::Existing);
        b.labels.insert(LABEL_KIND.to_string(), "build".to_string());
        let body = create_body(
            "cua-build-ctr-1",
            "img:1",
            &b,
            HashMap::new(),
            "runc".into(),
        );
        assert_eq!(
            body.labels.unwrap().get(LABEL_KIND).map(String::as_str),
            Some("build")
        );
        let sc = SidecarSpec {
            name: "db".into(),
            image: "postgres:16".into(),
            command: None,
            env: Default::default(),
            ports: vec![],
        };
        let l = sidecar_body("sb", &sc, "runc".into()).labels.unwrap();
        assert_eq!(l.get(LABEL_MANAGED).map(String::as_str), Some("true"));
        assert_eq!(l.get(LABEL_KIND).map(String::as_str), Some("sidecar"));
        // Images the backend writes carry the label through a LABEL change.
        assert_eq!(image_kind(CKPT_REPO, "sb-rebind"), "intermediate");
        assert_eq!(image_kind(CKPT_REPO, "fork-x"), "intermediate");
        assert_eq!(image_kind(CKPT_REPO, "mine"), "checkpoint");
        assert_eq!(image_kind("cua-vmm/build", "cua-b-1"), "build-image");
        assert_eq!(
            managed_label_change(CKPT_REPO, "mine"),
            format!(
                "LABEL ai.cua.managed=true ai.cua.kind=checkpoint ai.cua.home={}",
                home_tag()
            )
        );
        assert_eq!(home_tag().len(), 12);
        assert_ne!(
            home_tag_for(std::path::Path::new("/a/.cua")),
            home_tag_for(std::path::Path::new("/b/.cua"))
        );
    }

    #[test]
    fn an_injected_spacesd_starts_as_root() {
        let mut spec = StartSpec::new("sb", ImageSource::Existing);
        assert_eq!(
            create_body("sb", "img:1", &spec, HashMap::new(), "runsc".into()).user,
            None,
            "the image's own user"
        );
        spec.inject_spacesd = Some("/host/cua-spacesd-linux-aarch64".into());
        spec.command = Some(crate::inject::wrapper_command(
            &crate::OciRunConfig {
                user: Some("node".into()),
                ..Default::default()
            },
            None,
        ));
        let body = create_body("sb", "img:1", &spec, HashMap::new(), "runsc".into());
        assert_eq!(body.user.as_deref(), Some("0"));
        assert_eq!(
            body.entrypoint
                .as_deref()
                .and_then(|e| e.first())
                .map(String::as_str),
            Some(crate::inject::GUEST_PATH)
        );
    }

    #[test]
    fn an_images_config_becomes_a_run_config() {
        let cfg = bollard::models::ImageConfig {
            entrypoint: Some(vec!["/entry".into()]),
            cmd: Some(vec!["serve".into()]),
            user: Some("app".into()),
            working_dir: Some("".into()),
            env: Some(vec!["A=1".into()]),
            ..Default::default()
        };
        let run = run_config_of(Some(&cfg));
        assert_eq!(run.entrypoint, ["/entry"]);
        assert_eq!(run.cmd, ["serve"]);
        assert_eq!(run.user.as_deref(), Some("app"));
        assert_eq!(run.working_dir, None, "an empty WorkingDir is none");
        assert_eq!(run_config_of(None), crate::OciRunConfig::default());
    }

    #[test]
    fn create_body_command_replaces_entrypoint_and_cmd() {
        let spec = StartSpec::new("sb", ImageSource::Existing)
            .command(["python", "-m", "srv"])
            .env("K", "v");
        let body = create_body("sb", "img:1", &spec, HashMap::new(), "runsc".into());
        assert_eq!(
            body.entrypoint.as_deref(),
            Some(&["python".to_string(), "-m".into(), "srv".into()][..])
        );
        assert_eq!(body.cmd.as_deref(), Some(&[][..]));
        assert_eq!(body.env.unwrap(), vec!["K=v".to_string()]);
    }

    #[test]
    fn splits_tags_and_digests() {
        assert_eq!(split_tag("alpine"), ("alpine".into(), "latest".into()));
        assert_eq!(split_tag("alpine:3.20"), ("alpine".into(), "3.20".into()));
        assert_eq!(
            split_tag("localhost:5000/x"),
            ("localhost:5000/x".into(), "latest".into())
        );
        assert_eq!(
            split_tag("localhost:5000/x:1"),
            ("localhost:5000/x".into(), "1".into())
        );
        assert_eq!(
            split_tag("r.io/a@sha256:ab"),
            ("r.io/a@sha256:ab".into(), String::new())
        );
    }

    #[test]
    fn endpoints_prefer_ipv4_bindings() {
        let mut ports = HashMap::new();
        ports.insert(
            "8080/tcp".to_string(),
            Some(vec![
                PortBinding {
                    host_ip: Some("::".into()),
                    host_port: Some("1".into()),
                },
                PortBinding {
                    host_ip: Some("127.0.0.1".into()),
                    host_port: Some("32768".into()),
                },
            ]),
        );
        ports.insert(
            "53/udp".to_string(),
            Some(vec![PortBinding {
                host_ip: None,
                host_port: Some("9".into()),
            }]),
        );
        let i = bollard::models::ContainerInspectResponse {
            id: Some("abc".into()),
            network_settings: Some(bollard::models::NetworkSettings {
                ports: Some(ports),
                ..Default::default()
            }),
            ..Default::default()
        };
        let ep = ContainerRuntime::endpoints_from(&i);
        assert_eq!(ep.ports, BTreeMap::from([(8080, 32768)]));
        assert_eq!(ep.container_id.as_deref(), Some("abc"));
    }
}
