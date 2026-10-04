//! OCI container backend over the Docker Engine API (`bollard`).
//!
//! Runs rootfs images such as the Fleet gVisor `docker-*` tags. Isolation is
//! chosen per engine:
//!
//! 1. `runsc` (gVisor) when the engine has it registered;
//! 2. otherwise, when [`ContainerConfig::allow_install_runsc`] is set and the
//!    engine is provisionable (Colima VM, native Linux), install it and retry;
//! 3. otherwise `runc`, reported as [`Isolation::Runc`] so callers can surface
//!    the weaker sandbox (or fail when [`ContainerConfig::require_gvisor`]).
//!
//! [`DockerExec`] is the agentless guest-exec path for containers.

pub mod engine;
pub mod gvisor;

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
/// Sidecar defaults, the same as a Fleet pod sidecar's (500m / 512Mi).
const SIDECAR_NANO_CPUS: i64 = 500_000_000;
const SIDECAR_MEMORY_BYTES: i64 = 512 * 1024 * 1024;
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
}

impl Default for ContainerConfig {
    fn default() -> Self {
        Self {
            endpoint: None,
            prefer_gvisor: true,
            allow_install_runsc: false,
            require_gvisor: false,
            stop_timeout: 10,
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
            Some(uri) => EngineEndpoint { kind: engine::classify(uri), uri: uri.clone(), source: "config".into() },
            None => engine::discover().ok_or_else(|| {
                VmmError::missing(
                    "a container engine",
                    "no Docker-API socket found (DOCKER_HOST, docker context, ~/.colima, ~/.orbstack, \
                     ~/.docker/run, /var/run/docker.sock). Install one (macOS: `brew install colima docker && colima start`) \
                     or let cua boot its managed runtime VM",
                )
            })?,
        };
        let docker = connect_uri(&endpoint.uri)?;
        docker.ping().await.map_err(|e| {
            VmmError::missing(
                format!("container engine at {}", endpoint.uri),
                format!(
                    "engine did not answer ping ({e}); is it running? (Colima: `colima start`)"
                ),
            )
        })?;
        Ok(Self {
            docker,
            endpoint,
            cfg,
            oci_runtime: Mutex::new(None),
            engine_cpus: Mutex::new(None),
        })
    }

    pub fn endpoint(&self) -> &EngineEndpoint {
        &self.endpoint
    }

    pub fn docker(&self) -> &Docker {
        &self.docker
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
            if let Ok(rts) = connect_uri(&self.endpoint.uri)?.info().await.map(|i| {
                i.runtimes
                    .unwrap_or_default()
                    .into_keys()
                    .collect::<Vec<_>>()
            }) && rts.iter().any(|r| r == "runsc")
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
        let mut stream = self.docker.create_image(Some(opts), None, creds);
        while let Some(item) = stream.next().await {
            // The engine's message never carries the credentials.
            let info = item.map_err(|e| VmmError::Engine(format!("pull {reference}: {e}")))?;
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
            Some("runsc") => Isolation::Gvisor,
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

    /// Create (not start) a container.
    async fn create(
        &self,
        name: &str,
        image: &str,
        spec: &StartSpec,
        base_labels: HashMap<String, String>,
        platform: Option<String>,
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
        let mut body = create_body(name, image, spec, base_labels, runtime);
        if let Some(hc) = body.host_config.as_mut() {
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
        self.docker
            .create_container(Some(opts), body)
            .await
            .map_err(engine_err)?;
        Ok(())
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
            .docker
            .commit_container(opts, bollard::models::ContainerConfig::default())
            .await
            .map_err(engine_err)?;
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
        let mut progress = self.docker.create_image(Some(opts), Some(body), None);
        while let Some(item) = progress.next().await {
            item.map_err(|e| VmmError::Engine(format!("import checkpoint {tag}: {e}")))?;
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

    /// An exec handle for a running container.
    pub fn exec_handle(&self, name: &str) -> DockerExec {
        DockerExec {
            docker: self.docker.clone(),
            container: name.to_string(),
        }
    }
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

fn connect_uri(uri: &str) -> Result<Docker> {
    let v = bollard::API_DEFAULT_VERSION;
    let r = if uri.starts_with("unix://") || uri.starts_with("npipe://") || uri.starts_with('/') {
        Docker::connect_with_socket(uri, 120, v)
    } else if uri.starts_with("tcp://") || uri.starts_with("http://") {
        Docker::connect_with_http(uri, 120, v)
    } else {
        return Err(VmmError::invalid(format!(
            "unsupported engine endpoint '{uri}'"
        )));
    };
    r.map_err(engine_err)
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
        let info = match existing {
            Some(i) => {
                let st = Self::status_from(&i);
                if st == Status::Paused {
                    self.docker
                        .unpause_container(&spec.name)
                        .await
                        .map_err(engine_err)?;
                } else if st != Status::Running {
                    let have = Self::published(&i);
                    if spec.ports.iter().any(|p| !have.contains(p)) {
                        // Port bindings are fixed at create time: recreate from a
                        // commit so filesystem changes survive.
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
                        self.create(&spec.name, &img, spec, labels, None).await?;
                    }
                    self.docker
                        .start_container(&spec.name, None)
                        .await
                        .map_err(engine_err)?;
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
                self.create(&spec.name, reference, spec, HashMap::new(), platform)
                    .await?;
                crate::progress::report(crate::progress::Progress::phase(
                    crate::progress::Phase::Booting,
                ));
                if let Err(e) = self.docker.start_container(&spec.name, None).await {
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
                    return Err(engine_err(e));
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
            self.docker
                .start_container(name, None)
                .await
                .map_err(engine_err)?;
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
        let mut spec = StartSpec::new(new_name, ImageSource::Existing);
        let image = if let Some(i) = self.inspect(source).await? {
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
        let mut labels = HashMap::new();
        labels.insert("cua.vmm.source".to_string(), source.to_string());
        self.create(new_name, &image, &spec, labels, None).await
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
        if Self::isolation_from(&i) == Isolation::Gvisor && Self::status_from(&i) == Status::Running
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
                name: c
                    .labels
                    .as_ref()
                    .and_then(|l| l.get(LABEL_NAME).cloned())
                    .or_else(|| {
                        c.names
                            .and_then(|n| n.first().map(|s| s.trim_start_matches('/').to_string()))
                    })
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

    async fn delete(&self, name: &str) -> Result<()> {
        let opts = RemoveContainerOptions {
            force: true,
            v: true,
            ..Default::default()
        };
        // Sidecars first: they hold the sandbox's network namespace.
        for sc in self.sidecar_names(name).await.unwrap_or_default() {
            let _ = self.docker.remove_container(&sc, Some(opts.clone())).await;
        }
        match self.docker.remove_container(name, Some(opts)).await {
            Ok(()) => {}
            Err(e) if is_not_found(&e) => return Err(VmmError::NotFound(name.into())),
            Err(e) => return Err(engine_err(e)),
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
}

impl ContainerRuntime {
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
                tokio::time::sleep(Duration::from_millis(750)).await;
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
        // Kubernetes `command` (and so a Fleet pod template's).
        entrypoint: spec.command.clone().filter(|c| !c.is_empty()),
        cmd: spec
            .command
            .as_ref()
            .filter(|c| !c.is_empty())
            .map(|_| Vec::new()),
        labels: Some(labels),
        exposed_ports: (!exposed.is_empty()).then_some(exposed),
        host_config: Some(HostConfig {
            // Under gVisor, SYS_ADMIN lets cua-spacesd mount Cua Volume
            // (FUSE) in the guest. The capability applies inside gVisor's
            // own kernel, not the host's; runc containers do not get it.
            cap_add: (runtime == "runsc").then(|| vec!["SYS_ADMIN".to_string()]),
            runtime: Some(runtime),
            port_bindings: Some(bindings),
            nano_cpus: Some(i64::from(spec.cpus) * 1_000_000_000),
            memory: Some((spec.memory_mb as i64) * 1024 * 1024),
            shm_size: Some(shm_size_bytes(spec.memory_mb)),
            extra_hosts: sidecar_hosts(spec),
            device_requests: gpu_requests(spec),
            ..Default::default()
        }),
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

/// The hostname sidecars reach the sandbox at (as on Fleet).
pub const MAIN_HOSTNAME: &str = "main";

/// `/etc/hosts` entries naming every container of a sidecar group, so the
/// sandbox reaches a sidecar at its name and a sidecar reaches the sandbox
/// at [`MAIN_HOSTNAME`], as on Fleet. They all share the sandbox's network
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
/// its own image, command and environment, Fleet's default resources.
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
        // Names resolve on every container of the group, as on Fleet.
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
    fn create_body_carries_the_env_token() {
        let spec = StartSpec::new("sb", ImageSource::Existing)
            .env("CUA_ENV_TOKEN", "tok123")
            .port(3211);
        let body = create_body("sb", "img:1", &spec, HashMap::new(), "runsc".into());
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
