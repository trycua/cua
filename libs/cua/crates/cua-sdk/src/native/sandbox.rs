//! Daemon-agnostic sandboxes: one API over the embedded runtime and the
//! daemon client.

use super::{Backend, SpacesdClient, ms, run};
use crate::types::{ExitInfo, ImageFormat, ProcessOutput, Screenshot};
use crate::{CuaError, Result};
use cua_daemon::{CreateRequest, SandboxRecord};
use cua_sandbox_core::{Probe, ProviderKind as CoreProvider, Status};
use std::{collections::HashMap, sync::Arc};

/// Coarse lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SandboxStatus {
    /// Running (or bound).
    Running,
    /// Suspended or scaled to zero.
    Suspended,
    /// Stopped.
    Stopped,
    /// Starting.
    Provisioning,
    /// Unknown; see `SandboxInfo.status_detail`.
    Unknown,
}

/// Portable lifecycle phase, the same words on every provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SandboxPhase {
    /// Being created.
    Provisioning,
    /// Booting; readiness probes have not passed yet.
    Starting,
    /// Running and ready.
    Ready,
    /// Stopped or suspended.
    Stopped,
}

impl SandboxPhase {
    fn of(status: SandboxStatus) -> Self {
        match status {
            SandboxStatus::Running => SandboxPhase::Ready,
            SandboxStatus::Provisioning => SandboxPhase::Provisioning,
            SandboxStatus::Suspended | SandboxStatus::Stopped => SandboxPhase::Stopped,
            SandboxStatus::Unknown => SandboxPhase::Starting,
        }
    }
}

/// The image a sandbox runs, as resolved and pinned at create time.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ImageInfo {
    /// The reference as requested, normalised
    /// (`docker.io/library/python:3.12-slim`).
    pub reference: String,
    /// `registry/repo@sha256:...` of the variant that runs.
    pub pinned_ref: String,
    /// The digest in `pinned_ref`.
    pub digest: String,
    /// `rootfs`, `containerdisk` or `lume`.
    pub variant: String,
    /// Architecture that runs (`amd64`/`arm64`), when known.
    pub arch: Option<String>,
    /// Guest OS: `linux`, `windows` or `macos`.
    pub os: String,
    /// Whether it runs emulated (no build for the requested architecture).
    pub emulated: bool,
    /// Whether the image carries cua-spacesd (`ai.cua.spacesd` label or a
    /// published 3211); `None` when it does not say.
    #[uniffi(default = None)]
    pub spacesd: Option<bool>,
}

impl From<cua_sandbox_core::ImageInfo> for ImageInfo {
    fn from(i: cua_sandbox_core::ImageInfo) -> Self {
        Self {
            reference: i.reference,
            pinned_ref: i.pinned_ref,
            digest: i.digest,
            variant: i.variant,
            arch: i.arch,
            os: i.os,
            emulated: i.emulated,
            spacesd: i.spacesd,
        }
    }
}

/// A sandbox as listed.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SandboxInfo {
    /// Name.
    pub name: String,
    /// What kind of machine: `container` or `vm` (empty when not known:
    /// an existing machine).
    pub kind: String,
    /// The engine that runs it: `gvisor`, `runc`, `qemu`, `lume` or a
    /// provider's own (empty when not known).
    pub runtime: String,
    /// `runtime_type` as persisted (`direct`, `gvisor`, `qemu`, ...); prefer
    /// `location`, `kind` and `runtime`.
    pub runtime_type: String,
    /// Status.
    pub status: SandboxStatus,
    /// Provider's word for an unknown status.
    pub status_detail: Option<String>,
    /// No state file; torn down on delete.
    pub ephemeral: bool,
    /// Declared services: name → guest port (0 = only the name is known).
    pub services: HashMap<String, u16>,
    /// Service base URLs by name.
    pub endpoints: HashMap<String, String>,
    /// Image, when known.
    pub image: Option<String>,
    /// Qualified ref: `local:<name>`, `direct:<host:port>` or
    /// `<provider>:<name>`. Every call that takes a sandbox name accepts it.
    pub id: String,
    /// Portable phase: provisioning, starting, ready or stopped.
    pub phase: SandboxPhase,
    /// Where it runs: `local`, `direct` (a machine by address), `relay` or a
    /// provider.
    pub location: String,
    /// When it expires unless kept alive (unix seconds), when known.
    pub expires_at_unix: Option<i64>,
    /// Provider internals (local: `backend`, `container_id`). Not part of
    /// the portable API.
    pub provider_details: HashMap<String, String>,
    /// The image as resolved and pinned at create time (digest, variant).
    /// `None` for direct connections, images not resolved from a registry,
    /// and the daemon topology.
    #[uniffi(default = None)]
    pub image_info: Option<ImageInfo>,
    /// Why a local sandbox stopped without being asked to, in plain words
    /// ("out of memory", "killed", "exited with code 1"); `None` otherwise.
    #[uniffi(default = None)]
    pub stop_reason: Option<String>,
}

impl From<SandboxRecord> for SandboxInfo {
    fn from(r: SandboxRecord) -> Self {
        let (status, detail) = match r.status {
            Status::Running => (SandboxStatus::Running, None),
            Status::Suspended => (SandboxStatus::Suspended, None),
            Status::Stopped => (SandboxStatus::Stopped, None),
            Status::Provisioning => (SandboxStatus::Provisioning, None),
            Status::Unknown(u) => (SandboxStatus::Unknown, Some(u)),
        };
        SandboxInfo {
            id: r.id,
            phase: SandboxPhase::of(status),
            location: r.location,
            expires_at_unix: r.expires_at.map(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0)
            }),
            provider_details: r.provider_details.into_iter().collect(),
            name: r.name,
            kind: r.kind,
            runtime: r.runtime,
            runtime_type: r.runtime_type,
            status,
            status_detail: detail,
            ephemeral: r.ephemeral,
            services: r.services.into_iter().collect(),
            endpoints: r.endpoints.into_iter().collect(),
            image: r.image,
            image_info: r.image_info.map(Into::into),
            stop_reason: r.stop_reason,
        }
    }
}

/// A user-declared readiness probe. With no probes a sandbox is ready as
/// soon as its provider reports it running; nothing assumes a guest daemon.
///
/// Name a declared service (`service = "mcp"`) or a guest port.
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct ReadinessProbe {
    /// Guest port (0 with `service`).
    #[uniffi(default = 0)]
    pub port: u16,
    /// HTTP path; `None` is a TCP connect probe.
    #[uniffi(default = None)]
    pub http_path: Option<String>,
    /// Exact HTTP status to wait for; `None` accepts any 2xx.
    #[uniffi(default = None)]
    pub http_status: Option<u16>,
    /// A declared service whose guest port to probe (instead of `port`).
    #[uniffi(default = None)]
    pub service: Option<String>,
}

impl ReadinessProbe {
    /// A TCP probe on a declared service.
    pub fn tcp(service: impl Into<String>) -> Self {
        Self {
            service: Some(service.into()),
            ..Default::default()
        }
    }

    /// `GET path` on a declared service returns 2xx.
    pub fn http(service: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            service: Some(service.into()),
            http_path: Some(path.into()),
            ..Default::default()
        }
    }

    /// The core probe; `services` resolves `service` (`env` defaults to
    /// 3211).
    pub(crate) fn resolve(&self, services: &HashMap<String, u16>) -> Result<Probe> {
        let port = match self.service.as_deref().filter(|s| !s.is_empty()) {
            Some(name) => match services.get(name).copied().filter(|p| *p > 0) {
                Some(p) => p,
                None if name == "env" => cua_sandbox_core::ENV_PORT,
                None => {
                    return Err(CuaError::InvalidArgument(format!(
                        "readiness probe names service {name:?}, which has no known port \
                         (declared: {:?})",
                        services.keys().collect::<Vec<_>>()
                    )));
                }
            },
            None => self.port,
        };
        if port == 0 {
            return Err(CuaError::InvalidArgument(
                "probe needs a port > 0 or a declared service".into(),
            ));
        }
        Ok(match &self.http_path {
            None => Probe::Tcp(port),
            Some(path) => Probe::Http {
                port,
                path: if path.starts_with('/') {
                    path.clone()
                } else {
                    format!("/{path}")
                },
                status: self.http_status,
            },
        })
    }
}

/// An extra container next to a sandbox (a compose-style sidecar),
/// addressed by name on every runtime: the sandbox reaches it at its
/// `name` (on its `ports`), it reaches the sandbox at `main`, and
/// `services` may name its ports (`{"db": 5432}`). Local containers run it
/// in the sandbox's network namespace (`localhost` works too; needs
/// `runtime="runc"`). Local VM sandboxes refuse sidecars. With sidecars the
/// service names `main`, `sidecars` and `sc` are reserved.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Container {
    /// Image reference.
    pub image: String,
    /// argv replacing the image's entrypoint.
    #[uniffi(default = None)]
    pub command: Option<Vec<String>>,
    /// Environment (plain values, not secrets).
    #[uniffi(default)]
    pub env: HashMap<String, String>,
    /// Ports it listens on.
    #[uniffi(default = [])]
    pub ports: Vec<u16>,
    /// Name, unique within the sandbox, and its hostname (not `main`).
    /// Unset: from the image (`redis`).
    #[uniffi(default = None)]
    pub name: Option<String>,
}

impl Container {
    pub(crate) fn to_core(&self) -> cua_sandbox_core::Sidecar {
        let mut s = cua_sandbox_core::Sidecar::new(self.image.clone());
        if let Some(n) = self.name.clone().filter(|n| !n.is_empty()) {
            s.name = n;
        }
        s.command = self.command.clone().filter(|c| !c.is_empty());
        s.env = self.env.clone().into_iter().collect();
        s.ports = self.ports.clone();
        s
    }
}

/// Credentials for a private registry image, used for the pull. Never
/// logged or persisted.
#[derive(Clone, PartialEq, Eq, uniffi::Enum)]
pub enum RegistrySecret {
    /// A user name and password (or access token).
    Basic {
        /// User name.
        username: String,
        /// Password or token.
        password: String,
        /// Registry host; `None` means the image's.
        registry: Option<String>,
    },
    /// Read from environment variables of this process at create time.
    FromEnv {
        /// Variable holding the user name.
        username_var: String,
        /// Variable holding the password or token.
        password_var: String,
        /// Registry host; `None` means the image's.
        registry: Option<String>,
    },
    /// A private Amazon ECR image: a login token from the AWS CLI
    /// (`aws ecr get-login-password`) with the caller's AWS credentials.
    AwsEcr {
        /// Region; `None` takes it from the image's ECR host.
        region: Option<String>,
    },
}

impl std::fmt::Debug for RegistrySecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegistrySecret::Basic {
                username, registry, ..
            } => f
                .debug_struct("Basic")
                .field("username", username)
                .field("password", &"<redacted>")
                .field("registry", registry)
                .finish(),
            RegistrySecret::FromEnv {
                username_var,
                password_var,
                registry,
            } => f
                .debug_struct("FromEnv")
                .field("username_var", username_var)
                .field("password_var", password_var)
                .field("registry", registry)
                .finish(),
            RegistrySecret::AwsEcr { region } => {
                f.debug_struct("AwsEcr").field("region", region).finish()
            }
        }
    }
}

impl RegistrySecret {
    /// The credentials for `image`.
    pub(crate) async fn credentials(
        &self,
        image: &str,
    ) -> Result<cua_sandbox_core::RegistryCredentials> {
        use cua_sandbox_core::RegistryCredentials as C;
        let scope = |c: C, registry: &Option<String>| match registry.as_deref() {
            Some(r) if !r.is_empty() => c.for_registry(r),
            _ => c,
        };
        match self {
            RegistrySecret::Basic {
                username,
                password,
                registry,
            } => {
                if username.is_empty() || password.is_empty() {
                    return Err(CuaError::InvalidArgument(
                        "RegistrySecret needs a username and a password".into(),
                    ));
                }
                Ok(scope(C::new(username, password), registry))
            }
            RegistrySecret::FromEnv {
                username_var,
                password_var,
                registry,
            } => {
                let read = |var: &str| {
                    std::env::var(var)
                        .ok()
                        .filter(|v| !v.is_empty())
                        .ok_or_else(|| {
                            CuaError::InvalidArgument(format!(
                                "RegistrySecret.from_env: ${var} is not set"
                            ))
                        })
                };
                Ok(scope(
                    C::new(read(username_var)?, read(password_var)?),
                    registry,
                ))
            }
            RegistrySecret::AwsEcr { region } => {
                let host = cua_image::registry_of(image).to_string();
                let region = region
                    .clone()
                    .filter(|r| !r.is_empty())
                    .or_else(|| cua_image::auth::ecr_region(&host).map(str::to_string))
                    .ok_or_else(|| {
                        CuaError::InvalidArgument(format!(
                            "RegistrySecret.aws_ecr: {image} is not a private ECR image \
                             (<account>.dkr.ecr.<region>.amazonaws.com); pass the region"
                        ))
                    })?;
                let token = cua_image::auth::aws_ecr_login(&region).await.ok_or_else(|| {
                    CuaError::InvalidArgument(format!(
                        "RegistrySecret.aws_ecr: `aws ecr get-login-password --region {region}` \
                         failed (install the AWS CLI and sign in)"
                    ))
                })?;
                Ok(C::new("AWS", token).for_registry(host))
            }
        }
    }
}

/// One image build step (as in `images.cua.ai/v1alpha1` recipes).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum ImageLayer {
    /// `apt-get install`.
    AptInstall {
        /// Packages.
        packages: Vec<String>,
    },
    /// `pip install`.
    PipInstall {
        /// Packages.
        packages: Vec<String>,
    },
    /// `uv pip install`.
    UvInstall {
        /// Packages.
        packages: Vec<String>,
    },
    /// A shell command.
    Run {
        /// Command.
        command: String,
    },
    /// An app from the cua catalog (VM images only).
    AppInstall {
        /// App id.
        app_id: String,
    },
}

impl ImageLayer {
    fn to_core(&self) -> cua_image::spec::ImageLayer {
        use cua_image::spec::ImageLayer as L;
        match self.clone() {
            ImageLayer::AptInstall { packages } => L::AptInstall { packages },
            ImageLayer::PipInstall { packages } => L::PipInstall { packages },
            ImageLayer::UvInstall { packages } => L::UvInstall { packages },
            ImageLayer::Run { command } => L::Run { command },
            ImageLayer::AppInstall { app_id } => L::AppInstall { app_id },
        }
    }
}

/// A local file copied into a built image.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct BuildFile {
    /// Local path.
    pub source: String,
    /// Absolute path in the image.
    pub destination: String,
}

/// Image layers built on top of a sandbox's image. Local: a build into the
/// container engine (container images only; VM images take no layers here),
/// cached by a content hash (`cua-b-<hash>`), so an identical spec never
/// rebuilds.
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct ImageBuild {
    /// Steps, in order.
    #[uniffi(default = [])]
    pub layers: Vec<ImageLayer>,
    /// Image environment.
    #[uniffi(default)]
    pub env: HashMap<String, String>,
    /// Ports the image exposes.
    #[uniffi(default = [])]
    pub ports: Vec<u16>,
    /// Local files copied in.
    #[uniffi(default = [])]
    pub files: Vec<BuildFile>,
    /// Build budget (default one hour).
    #[uniffi(default = None)]
    pub timeout_ms: Option<u32>,
}

impl ImageBuild {
    pub(crate) fn to_core(&self) -> cua_sandbox_core::BuildSpec {
        cua_sandbox_core::BuildSpec {
            from: String::new(),
            layers: self.layers.iter().map(ImageLayer::to_core).collect(),
            env: self.env.clone().into_iter().collect(),
            ports: self.ports.clone(),
            files: self
                .files
                .iter()
                .map(|f| cua_sandbox_core::BuildFile {
                    source: f.source.clone().into(),
                    destination: f.destination.clone(),
                })
                .collect(),
            timeout: self.timeout_ms.map(super::millis),
        }
    }
}

/// Options for [`Sandboxes::create`].
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SandboxCreateOptions {
    /// Where it runs: `local`, `direct:<addr>` (an existing machine running
    /// cua-spacesd) or a registered provider. `None` (the default): the
    /// user default (`default.on` in `~/.cua/config.toml`,
    /// `CUA_DEFAULT_ON`, else `local`). `cloud` fails with the "Cua Cloud has
    /// ended" error.
    #[uniffi(default = None)]
    pub on: Option<String>,
    /// What kind of machine: `auto`, `container` or `vm`. `None`: the user
    /// default (`default.kind`, else `auto`: macOS and Windows images are
    /// VMs, an image with a container rootfs is a container, a disk-only
    /// image is a VM).
    #[uniffi(default = None)]
    pub kind: Option<String>,
    /// Which engine: `auto` or one the location offers for the kind (local
    /// `gvisor`, `runc`, `qemu`, `lume`). `None`:
    /// the user default (`default.runtime`, else `auto`, the safest
    /// available). A local sandbox with sidecars needs `runc` where gVisor
    /// would run. A combination that does not exist is `InvalidPlacement`,
    /// listing the valid values.
    #[uniffi(default = None)]
    pub runtime: Option<String>,
    /// Image reference. A `pool:<name>` image fails with the "Cua Cloud has
    /// ended" error.
    #[uniffi(default = "")]
    pub image: String,
    /// Name. `None`: ephemeral (no state file; deleted with the sandbox).
    #[uniffi(default = None)]
    pub name: Option<String>,
    /// spacesd token.
    #[uniffi(default = None)]
    pub token: Option<String>,
    /// Create from this pool: locally, a pool made with
    /// [`Sandboxes::create_pool`] (the sandbox gets the pool's image and
    /// shape and takes one of its ready sandboxes in about 0.1 s; `image`
    /// may be left empty).
    #[uniffi(default = None)]
    pub pool: Option<String>,
    /// Guest OS (`linux`, `macos`, `windows`).
    #[uniffi(default = None)]
    pub os: Option<String>,
    /// vCPUs.
    #[uniffi(default = None)]
    pub cpus: Option<u32>,
    /// Memory (MiB).
    #[uniffi(default = None)]
    pub memory_mb: Option<u64>,
    /// Extra guest ports (local: published).
    #[uniffi(default = [])]
    pub ports: Vec<u16>,
    /// Named services (name → guest port). `env` defaults to 3211.
    #[uniffi(default)]
    pub services: HashMap<String, u16>,
    /// Readiness probes.
    #[uniffi(default = [])]
    pub wait_for: Vec<ReadinessProbe>,
    /// Readiness budget (default 10 min).
    #[uniffi(default = None)]
    pub ready_timeout_ms: Option<u32>,
    /// Guest environment (local: container environment or cloud-init; values
    /// are not secrets).
    #[uniffi(default)]
    pub env: HashMap<String, String>,
    /// Lifetime backstop in seconds: the platform deletes the sandbox this
    /// long after it stops being kept alive (a provider sandbox such as
    /// `on = "e2b"`: its lifetime). `None`: the platform's default.
    #[uniffi(default = None)]
    pub ttl_seconds: Option<u32>,
    /// The sandbox's command (argv), replacing the image's entrypoint.
    /// Same meaning local (container ENTRYPOINT, or run by cloud-init in a
    /// Linux VM) and in the cloud (the template command with
    /// `processMode: Run`, on gVisor and KubeVirt).
    #[uniffi(default = None)]
    pub command: Option<Vec<String>>,
    /// Sidecar containers, addressed by name (see [`Container`]).
    #[uniffi(default = [])]
    pub sidecars: Vec<Container>,
    /// Credentials for a private `image` (and sidecar images on its
    /// registry).
    #[uniffi(default = None)]
    pub registry_secret: Option<RegistrySecret>,
    /// Image layers built on top of `image` (a build into the local
    /// container engine, cached by content).
    #[uniffi(default = None)]
    pub build: Option<ImageBuild>,
    /// Guest network: `"default"` (unset) gives outbound network, like a
    /// Docker container; `"none"` cuts it while published ports still work.
    /// Only local QEMU VMs honour `"none"`; containers and Lume reject it.
    #[uniffi(default = None)]
    pub network: Option<String>,
    /// Binaries to inject once the sandbox is up, so tests run the build
    /// under test and not the copy the image bundles (see [`Overlay`]; the
    /// same as calling [`Sandbox::overlay`] after create). Needs
    /// cua-spacesd in the image, or a local container.
    #[uniffi(default = [])]
    pub overlays: Vec<super::Overlay>,
    /// Keep a named sandbox whose readiness check fails (its VM or
    /// container, and its record) to debug it. `false` (the default): a
    /// failed create deletes what it made. Ephemeral sandboxes are always
    /// deleted.
    #[uniffi(default = false)]
    pub keep_on_failure: bool,
    /// A GPU option of the runtime it runs on (`Spaces.gpu_support` lists them):
    /// `paravirtual` (a macOS VM on Lume: GPU acceleration, experimental),
    /// `virgl` (QEMU on Linux), `nvidia` (a runc container on Linux), a
    /// contrib provider's GPU type; `auto` picks the runtime's own. `None`
    /// (the default): no GPU.
    #[uniffi(default = None)]
    pub gpu: Option<String>,
    /// A Space without a desktop: only cua-spacesd runs (exec, files,
    /// processes, MCP), with no display, window manager or audio. For shell
    /// and RL workloads it starts several times faster and fits many more
    /// per machine. Images without cua-spacesd are unaffected; a `command`
    /// of your own is kept.
    #[uniffi(default = false)]
    pub headless: bool,
    /// Whether an image without cua-spacesd gets a static one injected, so
    /// any OCI image is a full Space (local containers and microVMs: exec,
    /// files, ports). `None` (the default): yes. `Some(false)` runs the
    /// image as it is.
    #[uniffi(default = None)]
    pub spacesd: Option<bool>,
    /// A size profile: defaults for what is left unset. `"rollout"`: 1
    /// vCPU, 1024 MiB and `headless` (a local VM gets no display hardware,
    /// so many more fit on one machine), for RL rollouts and agent episodes
    /// that mostly wait. `None` or `"default"`: 2 vCPUs, 4096 MiB.
    #[uniffi(default = None)]
    pub profile: Option<String>,
    /// A fork source (a "bake" sandbox): a local QEMU VM's guest RAM is a
    /// shared file, so its checkpoints take milliseconds and forks of them
    /// share its pages ([`Sandboxes::checkpoint`], [`Sandboxes::fork`]).
    /// Other kinds ignore it. `None` (the default): not one.
    #[uniffi(default = None)]
    pub forkable: Option<bool>,
}

impl Default for SandboxCreateOptions {
    fn default() -> Self {
        Self::auto("")
    }
}

impl SandboxCreateOptions {
    /// Options for `image` where `on` says (`local`, `direct:<addr>`, a
    /// provider), everything else default.
    pub fn new(on: impl Into<String>, image: impl Into<String>) -> Self {
        Self {
            on: Some(on.into()),
            ..Self::auto(image)
        }
    }

    /// Options for `image` with the default location (see
    /// [`SandboxCreateOptions::on`]).
    pub fn auto(image: impl Into<String>) -> Self {
        Self {
            on: None,
            kind: None,
            runtime: None,
            image: image.into(),
            name: None,
            token: None,
            os: None,
            cpus: None,
            memory_mb: None,
            ports: vec![],
            services: HashMap::new(),
            wait_for: vec![],
            ready_timeout_ms: None,
            env: HashMap::new(),
            pool: None,
            ttl_seconds: None,
            command: None,
            sidecars: vec![],
            registry_secret: None,
            build: None,
            network: None,
            overlays: vec![],
            keep_on_failure: false,
            gpu: None,
            headless: false,
            spacesd: None,
            profile: None,
            forkable: None,
        }
    }

    /// The location, kind and runtime this create runs with, each with
    /// where it came from: explicit values, else the user defaults
    /// ([`cua_sandbox_core::settings`]).
    pub fn placement(&self) -> Result<cua_sandbox_core::settings::Resolved> {
        use cua_sandbox_core::placement::{Kind, On, Runtime};
        let text = |v: &Option<String>| {
            v.as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_string)
        };
        let place =
            |e: cua_sandbox_core::placement::PlacementError| CuaError::InvalidPlacement(e.message);
        let on = text(&self.on)
            .map(|o| On::parse(&o))
            .transpose()
            .map_err(place)?;
        let kind = text(&self.kind)
            .map(|k| Kind::parse(&k))
            .transpose()
            .map_err(place)?;
        let runtime = text(&self.runtime)
            .map(|r| Runtime::parse(&r))
            .transpose()
            .map_err(place)?;
        let settings = cua_sandbox_core::settings::Settings::load()
            .map_err(|e| CuaError::InvalidArgument(e.to_string()))?;
        let r = settings
            .resolve(on, kind, runtime)
            .map_err(|e| CuaError::InvalidArgument(e.to_string()))?;
        if r.on == On::Cloud {
            return Err(CuaError::Unsupported(cua_sandbox_core::CLOUD_ENDED.into()));
        }
        cua_sandbox_core::placement::validate(&r.on, r.kind, &r.runtime).map_err(place)?;
        Ok(r)
    }

    fn to_request(&self) -> Result<CreateRequest> {
        use cua_sandbox_core::placement::On;
        let placement = self.placement()?;
        let pool = self.pool.clone().filter(|p| !p.trim().is_empty());
        let (provider, url) = match &placement.on {
            On::Local => (Some(CoreProvider::Local), None),
            On::Direct(a) => (Some(CoreProvider::Direct), Some(a.clone())),
            On::Provider(_) => (Some(CoreProvider::Contrib), None),
            // `placement()` refused the cloud above.
            On::Relay(_) | On::Host(_) | On::Auto | On::Cloud => (None, None),
        };
        Ok(CreateRequest {
            // Resolved here, in the caller's process: a daemon never applies
            // its own environment to this request.
            location: Some(placement.on.to_string()),
            kind: Some(placement.kind.to_string()),
            runtime: Some(placement.runtime.to_string()),
            provider,
            url,
            name: self.name.clone(),
            image: self.image.clone(),
            token: self.token.clone(),
            pool,
            os: self.os.clone(),
            cpus: self.cpus,
            memory_mb: self.memory_mb,
            ports: self.ports.clone(),
            services: self.services.clone().into_iter().collect(),
            wait_for: self
                .wait_for
                .iter()
                .map(|p| p.resolve(&self.services))
                .collect::<Result<_>>()?,
            ready_timeout: self.ready_timeout_ms.map(super::millis),
            env: self.env.clone().into_iter().collect(),
            command: self.command.clone().filter(|c| !c.is_empty()),
            labels: Default::default(),
            sidecars: self.sidecars.iter().map(Container::to_core).collect(),
            // Resolved (env, AWS CLI) in `create`, in this process.
            registry_credentials: None,
            build: self
                .build
                .as_ref()
                .map(ImageBuild::to_core)
                .filter(|b| !b.is_empty()),
            container_runtime: None,
            network: self.network.clone().filter(|n| !n.is_empty()),
            // An ephemeral sandbox a daemon holds for this process is reaped
            // once this process is gone.
            owner_pid: Some(std::process::id()),
            keep_on_failure: self.keep_on_failure,
            gpu: self.gpu.clone().filter(|g| !g.trim().is_empty()),
            headless: self.headless,
            spacesd: self.spacesd,
            profile: self.profile.clone().filter(|p| !p.trim().is_empty()),
            forkable: self.forkable.unwrap_or(false),
        })
    }
}

/// A sandbox listing and what it left out.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SandboxListing {
    /// Sandboxes, each with its `location`.
    pub sandboxes: Vec<SandboxInfo>,
    /// Sources left out of the listing.
    pub warnings: Vec<String>,
}

/// An HTTP header.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct HttpHeader {
    /// Name.
    pub name: String,
    /// Value.
    pub value: String,
}

/// Where a sandbox service is reachable from this process.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ServiceEndpoint {
    /// Base URL (append the path).
    pub url: String,
    /// Headers every request needs.
    pub headers: Vec<HttpHeader>,
}

/// An HTTP response from a sandbox service.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct HttpResponse {
    /// Status.
    pub status: u16,
    /// Headers.
    pub headers: Vec<HttpHeader>,
    /// Body.
    pub body: Vec<u8>,
}

/// Options for [`Sandbox::viewer_url`].
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ViewerOptions {
    /// Lifetime of the link in seconds (60 to 86400). Default 3600.
    #[uniffi(default = None)]
    pub ttl_seconds: Option<u32>,
    /// Watch only: no input, clipboard, files or microphone.
    #[uniffi(default = false)]
    pub view_only: bool,
    /// Two-way clipboard sync.
    #[uniffi(default = true)]
    pub clipboard: bool,
    /// Guest directory for uploads and folder sharing (`~` is the desktop
    /// user's home). `None` means `~`; an empty string turns files off.
    #[uniffi(default = None)]
    pub files_root: Option<String>,
    /// Allow the microphone (also subject to the sandbox's uplink policy).
    #[uniffi(default = true)]
    pub microphone: bool,
}

impl Default for ViewerOptions {
    fn default() -> Self {
        Self {
            ttl_seconds: None,
            view_only: false,
            clipboard: true,
            files_root: None,
            microphone: true,
        }
    }
}

/// A link to the sandbox desktop in the cua-spacesd HTML5 viewer.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ViewerLink {
    /// Open this in any modern browser. The credential is in the fragment
    /// (`#ticket=`), which browsers never send to a server.
    pub url: String,
    /// When the link stops working for new connections (unix seconds).
    pub expires_at_unix: i64,
}

/// A shareable URL of a sandbox service that expires.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PublicUrl {
    /// Id for `Sandbox.revoke_public_url`.
    pub id: String,
    /// The URL.
    pub url: String,
    /// When it stops working (unix seconds).
    pub expires_at_unix: i64,
    /// Service name.
    pub service: String,
    /// Provider internals (local: upstream).
    pub provider_details: HashMap<String, String>,
}

impl From<cua_daemon::shares::PublicUrl> for PublicUrl {
    fn from(u: cua_daemon::shares::PublicUrl) -> Self {
        PublicUrl {
            id: u.id,
            url: u.url,
            expires_at_unix: u
                .expires_at
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
            service: u.service,
            provider_details: u.provider_details.into_iter().collect(),
        }
    }
}

/// A media bridge for webviews (daemon mode).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MediaBridge {
    /// Loopback `ws://` URL carrying a short-lived bridge ticket. Speaks
    /// rcdp wire v2 unchanged; no env token is exposed.
    pub ws_url: String,
    /// The bridge ticket (also accepted as subprotocol
    /// `cua.ticket.<ticket>`).
    pub ticket: String,
    /// `cua.env.v1.OpenMediaResponse` as proto3 JSON (upstream ticket
    /// removed).
    pub open_media_response_json: String,
}

/// Creates, finds and manages sandboxes.
#[derive(uniffi::Object)]
pub struct Sandboxes {
    pub(crate) backend: Backend,
}

impl Sandboxes {
    /// Creates without applying overlays.
    async fn create_bare(
        &self,
        options: SandboxCreateOptions,
        progress: Option<cua_sandbox_core::progress::Sink>,
    ) -> Result<Arc<Sandbox>> {
        let mut req = options.to_request()?;
        let backend = self.backend.clone();
        let secret = options.registry_secret.clone();
        let rec = run(async move {
            // Credentials are read here (this process's environment and AWS
            // CLI), then travel only to the runtime that uses them.
            if let Some(s) = secret {
                req.registry_credentials = Some(s.credentials(&req.image).await?);
            }
            Ok(match (&backend, progress) {
                (Backend::Embedded(rt), progress) => {
                    cua_sandbox_core::progress::carry(progress, rt.create(req)).await?
                }
                (Backend::Daemon(d), Some(progress)) => d.create_reporting(req, progress).await?,
                (Backend::Daemon(d), None) => d.create(req).await?,
            })
        })
        .await?;
        Ok(self.handle(rec.into()))
    }

    /// Rust hosts: a handle for a known sandbox without reattaching (for
    /// lifecycle calls on a suspended one).
    pub async fn by_name(&self, name: String) -> Result<Arc<Sandbox>> {
        let info = self.get(name).await?;
        Ok(self.handle(info))
    }

    fn handle(&self, info: SandboxInfo) -> Arc<Sandbox> {
        Arc::new(Sandbox {
            backend: self.backend.clone(),
            info,
        })
    }
}

#[uniffi::export]
impl Sandboxes {
    /// Creates (local, your cloud, a provider) or connects (direct) a sandbox
    /// and waits for the provider and every readiness probe. `on: "cloud"`
    /// fails with the "Cua Cloud has ended" error.
    pub async fn create(&self, options: SandboxCreateOptions) -> Result<Arc<Sandbox>> {
        self.create_reported(options, None).await
    }
}

impl Sandboxes {
    /// Rust hosts: [`Sandboxes::create`], passing what the create does (the
    /// image download with its bytes, creating, booting, the readiness
    /// probes) to `progress` as it happens. Through a daemon that predates
    /// streaming creates, `progress` hears nothing.
    pub async fn create_reporting(
        &self,
        options: SandboxCreateOptions,
        progress: cua_sandbox_core::progress::Sink,
    ) -> Result<Arc<Sandbox>> {
        self.create_reported(options, Some(progress)).await
    }

    async fn create_reported(
        &self,
        options: SandboxCreateOptions,
        progress: Option<cua_sandbox_core::progress::Sink>,
    ) -> Result<Arc<Sandbox>> {
        let facts = super::telemetry::CreateFacts::of(&options);
        let mut options = options;
        // A local sandbox's cua-spacesd reports aggregate health counts only
        // when the host's telemetry is on (and the caller did not decide).
        if facts.is_local() && cua_telemetry::global().is_enabled() {
            options
                .env
                .entry("CUA_SPACESD_TELEMETRY".into())
                .or_insert_with(|| "1".into());
        }
        let r = self.create_recorded(options, progress).await;
        facts.record(&r);
        r
    }

    async fn create_recorded(
        &self,
        options: SandboxCreateOptions,
        progress: Option<cua_sandbox_core::progress::Sink>,
    ) -> Result<Arc<Sandbox>> {
        let overlays = options.overlays.clone();
        for o in &overlays {
            o.validate()?;
        }
        let ready_timeout_ms = options.ready_timeout_ms;
        let sandbox = self.create_bare(options, progress).await?;
        if overlays.is_empty() {
            return Ok(sandbox);
        }
        // The overlays are part of what was asked for: a sandbox that
        // cannot run the build under test is deleted, not returned.
        match sandbox.clone().overlay(overlays, ready_timeout_ms).await {
            Ok(_) => Ok(sandbox),
            Err(e) => {
                let _ = sandbox.delete().await;
                Err(e)
            }
        }
    }
}

#[uniffi::export]
impl Sandboxes {
    /// Connects directly to a machine by URL (`http(s)://host:port`, bare
    /// `host:port` or a relay URL) with an optional spacesd token. No daemon is assumed and nothing is probed.
    pub async fn connect_url(
        &self,
        url: String,
        token: Option<String>,
        name: Option<String>,
    ) -> Result<Arc<Sandbox>> {
        let addr = url.trim().to_string();
        if addr.is_empty() {
            return Err(CuaError::InvalidArgument("connect_url needs a URL".into()));
        }
        self.create(SandboxCreateOptions {
            name,
            token,
            ..SandboxCreateOptions::new(format!("direct:{addr}"), "")
        })
        .await
    }

    /// Reattaches to a sandbox by ref (`local:<name>`, `cloud:<name>`,
    /// `direct:<host:port>`, a legacy id) or by a name that is unique across
    /// locations (else `AmbiguousSandbox`, listing the qualified refs).
    pub async fn connect(&self, name: String) -> Result<Arc<Sandbox>> {
        let started = std::time::Instant::now();
        let backend = self.backend.clone();
        let rec = run(async move {
            Ok(match &backend {
                Backend::Embedded(rt) => rt.connect(&name).await?,
                Backend::Daemon(d) => d.connect(&name).await?,
            })
        })
        .await;
        super::telemetry::api("sandbox.connect", started, &rec);
        Ok(self.handle(rec?.into()))
    }

    /// Sandboxes, each tagged with its `location`. `None` (the default):
    /// all of them, local and direct, whatever the default location is.
    /// `"local"` or `"direct"`: only those. Cua Cloud has ended, so
    /// `"cloud"` lists none.
    pub async fn list(&self, location: Option<String>) -> Result<Vec<SandboxInfo>> {
        let listing = self.list_with_warnings(location).await?;
        for w in &listing.warnings {
            tracing::warn!("{w}");
        }
        Ok(listing.sandboxes)
    }

    /// [`Sandboxes::list`] with what the listing left out.
    pub async fn list_with_warnings(&self, location: Option<String>) -> Result<SandboxListing> {
        let backend = self.backend.clone();
        let p = location_filter(location.as_deref())?;
        // Cua Cloud has ended: it has no sandboxes to list.
        if location.as_deref().is_some_and(asks_for_cloud) {
            return Ok(SandboxListing {
                sandboxes: vec![],
                warnings: vec![],
            });
        }
        let listing = run(async move {
            Ok(match &backend {
                Backend::Embedded(rt) => rt.list_filtered(p).await?,
                Backend::Daemon(d) => d.list_filtered(p).await?,
            })
        })
        .await?;
        Ok(SandboxListing {
            sandboxes: listing.sandboxes.into_iter().map(Into::into).collect(),
            warnings: listing.warnings,
        })
    }

    /// Deprecated: [`Sandboxes::list`] with no location lists everything.
    pub async fn list_all(&self) -> Result<Vec<SandboxInfo>> {
        self.list(None).await
    }

    /// Cancels the create of the named sandbox `name` that is still
    /// running (in this process or the daemon): the work in flight stops
    /// (an image download, a boot) and what the create made is
    /// deleted; a sandbox of that name that existed before is never
    /// touched. Finished image downloads stay cached. Returns what was
    /// removed, or `None` when no create of that name was running. The
    /// create itself fails with `Cancelled`.
    pub async fn cancel_create(&self, name: String) -> Result<Option<String>> {
        let backend = self.backend.clone();
        run(async move {
            Ok(match &backend {
                Backend::Embedded(rt) => rt.cancel_create(&name).await?,
                Backend::Daemon(d) => d.cancel_create(&name).await?,
            })
        })
        .await
    }

    /// One sandbox's info, by ref or unique name.
    pub async fn get(&self, name: String) -> Result<SandboxInfo> {
        let backend = self.backend.clone();
        Ok(run(async move {
            Ok(match &backend {
                Backend::Embedded(rt) => rt.record(&name).await?,
                Backend::Daemon(d) => d.get(&name).await?,
            })
        })
        .await?
        .into())
    }

    /// Deletes a sandbox by ref or unique name.
    pub async fn delete(&self, name: String) -> Result<()> {
        let backend = self.backend.clone();
        // The location word of a ref (`cloud:box`); a bare name is `other`.
        let location = name
            .split_once(':')
            .map(|(l, _)| l.to_string())
            .unwrap_or_default();
        let r = run(async move { delete(&backend, &name).await }).await;
        super::telemetry::sandbox_deleted(&location, &r);
        r
    }
}

/// A named checkpoint of a sandbox, and the machine that holds it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, uniffi::Record)]
pub struct CheckpointSummary {
    /// The checkpoint's name.
    pub name: String,
    /// The sandbox it was taken from.
    pub source: String,
    /// The engine that holds it (`firecracker`, `gvisor`, `qemu`, ...).
    pub runtime_type: String,
    /// RFC 3339 creation time.
    pub created_at: String,
    /// The source's image.
    pub image: String,
    /// Pinned: a fork source to keep (`delete_checkpoint` refuses it and
    /// `prune_checkpoints` leaves it until it is unpinned).
    pub pinned: bool,
    /// Its forks map its RAM copy-on-write and share the pages they do not
    /// write (a QEMU memory checkpoint).
    pub shared_memory: bool,
    /// Taken to fork a running sandbox many times ([`Sandboxes::fork_many`]
    /// of a sandbox): deleted with the last of its forks.
    pub release_with_forks: bool,
    /// How many of its forks still exist (for `release_with_forks`).
    pub forks: u32,
    /// Where it is: empty for this machine, else the name of the machine of
    /// yours that holds it (a checkpoint of a Space that machine provides).
    /// Its forks start there (`Spaces.create` with image
    /// `checkpoint:<name>`, or `fork_many`), or on another machine after a
    /// verified copy.
    pub host: String,
}

impl From<cua_sandbox_core::CheckpointRecord> for CheckpointSummary {
    fn from(c: cua_sandbox_core::CheckpointRecord) -> Self {
        Self {
            image: c
                .template
                .image
                .get("registry")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            name: c.name,
            source: c.source,
            runtime_type: c.runtime_type,
            created_at: c.created_at,
            pinned: c.pinned,
            shared_memory: c.shared_memory,
            release_with_forks: c.release_with_forks,
            forks: c.holders.len() as u32,
            host: String::new(),
        }
    }
}

impl From<cua_proto::daemon::v1::Checkpoint> for CheckpointSummary {
    fn from(c: cua_proto::daemon::v1::Checkpoint) -> Self {
        Self {
            name: c.name,
            source: c.source,
            runtime_type: c.runtime_type,
            created_at: c
                .created_at
                .map(|t| {
                    let at = std::time::UNIX_EPOCH
                        + std::time::Duration::new(t.seconds.max(0) as u64, t.nanos.max(0) as u32);
                    humantime::format_rfc3339_seconds(at).to_string()
                })
                .unwrap_or_default(),
            image: c.image,
            pinned: c.pinned,
            shared_memory: c.shared_memory,
            release_with_forks: c.release_with_forks,
            forks: c.forks,
            host: c.host,
        }
    }
}

/// Checkpoints and forks of sandboxes (the CLI's `cua sb checkpoint`
/// and `cua sb fork`).
#[uniffi::export]
impl Sandboxes {
    /// Saves sandbox `name`'s state as `checkpoint` (memory and disk on a
    /// microVM or a QEMU VM; a container's filesystem). It keeps running.
    /// `pin` (default false) pins the new checkpoint (see
    /// [`Sandboxes::pin_checkpoint`]). A QEMU checkpoint's forks share its
    /// RAM copy-on-write ([`CheckpointSummary::shared_memory`]). `name` may
    /// also be a Space another machine of yours provides (its name or id):
    /// that machine takes the checkpoint and keeps it
    /// ([`CheckpointSummary::host`]).
    #[uniffi::method(default(pin = false))]
    pub async fn checkpoint(
        &self,
        name: String,
        checkpoint: String,
        pin: bool,
    ) -> Result<CheckpointSummary> {
        let backend = self.backend.clone();
        run(async move {
            Ok(match &backend {
                Backend::Embedded(rt) => rt
                    .checkpoint_anywhere(&name, &checkpoint, pin)
                    .await?
                    .into(),
                Backend::Daemon(d) => d.checkpoint_with(&name, &checkpoint, pin).await?.into(),
            })
        })
        .await
    }

    /// Pins (`pinned`) or unpins a checkpoint. A pinned checkpoint is a
    /// fork source to keep: `delete_checkpoint` refuses it and
    /// `prune_checkpoints` leaves it until it is unpinned.
    pub async fn pin_checkpoint(&self, name: String, pinned: bool) -> Result<CheckpointSummary> {
        let backend = self.backend.clone();
        run(async move {
            Ok(match &backend {
                Backend::Embedded(rt) => rt.pin_checkpoint(&name, pinned).await?.into(),
                Backend::Daemon(d) => d.pin_checkpoint(&name, pinned).await?.into(),
            })
        })
        .await
    }

    /// Deletes every checkpoint that is not pinned (forks made from them
    /// keep running); returns their names.
    pub async fn prune_checkpoints(&self) -> Result<Vec<String>> {
        let backend = self.backend.clone();
        run(async move {
            Ok(match &backend {
                Backend::Embedded(rt) => rt.prune_checkpoints().await?,
                Backend::Daemon(d) => d.prune_checkpoints().await?,
            })
        })
        .await
    }

    /// Starts `name` from a sandbox (running or stopped) or a checkpoint.
    pub async fn fork(&self, source: String, name: String) -> Result<Arc<Sandbox>> {
        let backend = self.backend.clone();
        let info: SandboxInfo = run(async move {
            Ok(match &backend {
                Backend::Embedded(rt) => rt.fork(&source, &name, Default::default()).await?,
                Backend::Daemon(d) => d.fork(&source, &name).await?,
            })
        })
        .await?
        .into();
        Ok(self.handle(info))
    }

    /// Every checkpoint: this machine's (oldest first), then those your
    /// machines that provide Spaces hold, each name once
    /// ([`CheckpointSummary::host`]).
    pub async fn checkpoints(&self) -> Result<Vec<CheckpointSummary>> {
        let backend = self.backend.clone();
        run(async move {
            Ok(match &backend {
                Backend::Embedded(rt) => rt
                    .checkpoints_everywhere()
                    .await
                    .into_iter()
                    .map(Into::into)
                    .collect(),
                Backend::Daemon(d) => d.checkpoints().await?.into_iter().map(Into::into).collect(),
            })
        })
        .await
    }

    /// Deletes a checkpoint, here and on every machine of yours that holds
    /// a copy (forks made from it keep running). A pinned checkpoint is
    /// refused until it is unpinned.
    pub async fn delete_checkpoint(&self, name: String) -> Result<()> {
        let backend = self.backend.clone();
        run(async move {
            match &backend {
                Backend::Embedded(rt) => rt.delete_checkpoint_everywhere(&name).await?,
                Backend::Daemon(d) => d.delete_checkpoint(&name).await?,
            }
            Ok(())
        })
        .await
    }

    /// Creates a pool: sandboxes of `image` (and `options`' shape) kept
    /// ready on this machine by `cua daemon`, so that a create from the
    /// pool (`SandboxCreateOptions.pool`) takes one in about 0.1 s.
    ///
    /// - `min`: kept ready at all times; one starts again as soon as one is
    ///   created from the pool.
    /// - `max`: the most kept ready (not counting those created from the
    ///   pool); above `min` only while creates come faster than sandboxes
    ///   start. Default: `min`.
    /// - `idle_ms`: after this long without a create from the pool it keeps
    ///   none ready, until the next create (which starts on the default tier
    ///   and refills it). Default: never.
    ///
    /// Pools run on this machine (`on = "local"`, the default). Without a
    /// pool, the daemon keeps sandboxes ready only where repeated creates of
    /// a shape pay for it; a pool asks for them explicitly. Creating a pool
    /// with the name of an existing one updates it.
    pub async fn create_pool(
        &self,
        image: String,
        options: PoolCreateOptions,
    ) -> Result<SandboxPool> {
        let backend = self.backend.clone();
        let on = options
            .on
            .clone()
            .map(|o| o.trim().to_string())
            .filter(|o| !o.is_empty())
            .unwrap_or_else(|| "local".into());
        if on != "local" {
            return Err(CuaError::Unsupported(format!(
                "pools run on this machine only today: create it with on=\"local\", not {on:?}"
            )));
        }
        let mut create = SandboxCreateOptions::new("local", image);
        create.kind = options.kind.clone();
        create.runtime = options.runtime.clone();
        create.os = options.os.clone();
        create.cpus = options.cpus;
        create.memory_mb = options.memory_mb;
        create.env = options.env.clone();
        create.ports = options.ports.clone();
        create.services = options.services.clone();
        create.command = options.command.clone();
        create.headless = options.headless;
        let req = create.to_request()?;
        let name = options.name.clone();
        let (min, max) = (options.min, options.max.unwrap_or(options.min));
        let idle = options.idle_ms.map(std::time::Duration::from_millis);
        run(async move {
            let p = match &backend {
                Backend::Embedded(rt) => {
                    rt.create_pool(name.as_deref(), req, min, max, idle).await?
                }
                Backend::Daemon(d) => d.create_pool(name.as_deref(), req, min, max, idle).await?,
            };
            Ok(SandboxPool::from(p))
        })
        .await
    }

    /// Pool `name`, with what it holds now.
    pub async fn get_pool(&self, name: String) -> Result<SandboxPool> {
        let backend = self.backend.clone();
        run(async move {
            let p = match &backend {
                Backend::Embedded(rt) => rt.get_pool(&name)?,
                Backend::Daemon(d) => d.get_pool(&name).await?,
            };
            Ok(SandboxPool::from(p))
        })
        .await
    }

    /// Every pool, with what each holds now.
    pub async fn list_pools(&self) -> Result<Vec<SandboxPool>> {
        let backend = self.backend.clone();
        run(async move {
            let all = match &backend {
                Backend::Embedded(rt) => rt.list_pools()?,
                Backend::Daemon(d) => d.list_pools().await?,
            };
            Ok(all.into_iter().map(SandboxPool::from).collect())
        })
        .await
    }

    /// Deletes pool `name`: its ready sandboxes go; sandboxes created from
    /// it stay. `false` when there was none.
    pub async fn delete_pool(&self, name: String) -> Result<bool> {
        let backend = self.backend.clone();
        run(async move {
            Ok(match &backend {
                Backend::Embedded(rt) => rt.delete_pool(&name)?,
                Backend::Daemon(d) => d.delete_pool(&name).await?,
            })
        })
        .await
    }
}

/// What a pool's sandboxes are and how many it keeps ready
/// ([`Sandboxes::create_pool`]). The shape fields mean what they mean in
/// [`SandboxCreateOptions`].
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PoolCreateOptions {
    /// Pool name (lower-case letters, digits and `-`). `None`: a new name.
    /// The name of an existing pool updates it.
    #[uniffi(default = None)]
    pub name: Option<String>,
    /// Where its sandboxes run: `local` (the default; the only location
    /// with pools today).
    #[uniffi(default = None)]
    pub on: Option<String>,
    /// What kind of machine: `auto`, `container`, `vm` or `microvm`.
    #[uniffi(default = None)]
    pub kind: Option<String>,
    /// Which engine: `auto` or one the location offers for the kind.
    #[uniffi(default = None)]
    pub runtime: Option<String>,
    /// Guest OS (`linux`, `macos`, `windows`).
    #[uniffi(default = None)]
    pub os: Option<String>,
    /// vCPUs of each sandbox.
    #[uniffi(default = None)]
    pub cpus: Option<u32>,
    /// Memory of each sandbox (MiB).
    #[uniffi(default = None)]
    pub memory_mb: Option<u64>,
    /// Guest environment.
    #[uniffi(default)]
    pub env: HashMap<String, String>,
    /// Extra guest ports to publish.
    #[uniffi(default = [])]
    pub ports: Vec<u16>,
    /// Named services (name → guest port).
    #[uniffi(default)]
    pub services: HashMap<String, u16>,
    /// The sandboxes' command (argv), replacing the image's entrypoint (as
    /// [`SandboxCreateOptions::command`]).
    #[uniffi(default = None)]
    pub command: Option<Vec<String>>,
    /// Sandboxes without a desktop (as [`SandboxCreateOptions::headless`]).
    #[uniffi(default = false)]
    pub headless: bool,
    /// Sandboxes kept ready at all times (refilled right after each create
    /// from the pool).
    #[uniffi(default = 1)]
    pub min: u32,
    /// The most kept ready, not counting those created from the pool
    /// (default: `min`).
    #[uniffi(default = None)]
    pub max: Option<u32>,
    /// After this long without a create from the pool it keeps none ready,
    /// until the next create (default: never).
    #[uniffi(default = None)]
    pub idle_ms: Option<u64>,
}

impl Default for PoolCreateOptions {
    fn default() -> Self {
        Self {
            name: None,
            on: None,
            kind: None,
            runtime: None,
            os: None,
            cpus: None,
            memory_mb: None,
            env: HashMap::new(),
            ports: vec![],
            services: HashMap::new(),
            command: None,
            headless: false,
            min: 1,
            max: None,
            idle_ms: None,
        }
    }
}

/// A pool ([`Sandboxes::create_pool`]).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SandboxPool {
    /// Name.
    pub name: String,
    /// Image as given.
    pub image: String,
    /// Where its sandboxes run (`local`).
    pub location: String,
    /// Kind and runtime as given (empty: auto).
    pub kind: String,
    pub runtime: String,
    /// Each sandbox's CPUs and memory (MiB).
    pub cpus: u32,
    pub memory_mb: u64,
    /// Sandboxes kept ready at all times, and at most.
    pub min: u32,
    pub max: u32,
    /// After this long without a create it keeps none ready (`None`: never).
    pub idle_ms: Option<u64>,
    /// Ready now, and starting.
    pub ready: u32,
    pub starting: u32,
    /// Why it holds what it holds, or why it cannot.
    pub status: String,
}

impl From<cua_daemon::PoolInfo> for SandboxPool {
    fn from(p: cua_daemon::PoolInfo) -> Self {
        Self {
            name: p.name,
            image: p.image,
            location: p.location,
            kind: p.kind,
            runtime: p.runtime,
            cpus: p.cpus,
            memory_mb: p.memory_mb,
            min: p.min,
            max: p.max,
            idle_ms: p.idle.map(|d| d.as_millis() as u64),
            ready: p.ready,
            starting: p.starting,
            status: p.status,
        }
    }
}

impl Sandboxes {
    /// The sandboxes this machine knows (state files, local backends,
    /// connected URLs) of `provider` (all when `None`). Rust only; the CLI's
    /// `cua do ls` uses it.
    pub async fn list_known(&self, location: Option<&str>) -> Result<Vec<SandboxInfo>> {
        let backend = self.backend.clone();
        let p = location_filter(location)?;
        let listing = run(async move {
            Ok(match &backend {
                Backend::Embedded(rt) => rt.list_filtered(p).await?,
                Backend::Daemon(d) => d.list_filtered(p).await?,
            })
        })
        .await?;
        Ok(listing.sandboxes.into_iter().map(Into::into).collect())
    }
}

/// Whether a `location` filter asks for Cua Cloud, which has ended.
fn asks_for_cloud(location: &str) -> bool {
    cua_sandbox_core::Location::parse(location.trim()) == Some(cua_sandbox_core::Location::Cloud)
}

/// A `location` filter (`local`, `direct`; empty or `None`: all).
fn location_filter(location: Option<&str>) -> Result<Option<CoreProvider>> {
    let Some(l) = location.map(str::trim).filter(|l| !l.is_empty()) else {
        return Ok(None);
    };
    match cua_sandbox_core::Location::parse(l) {
        Some(cua_sandbox_core::Location::Local) => Ok(Some(CoreProvider::Local)),
        // The caller returns an empty listing before using the filter.
        Some(cua_sandbox_core::Location::Cloud) => Ok(None),
        Some(cua_sandbox_core::Location::Direct) => Ok(Some(CoreProvider::Direct)),
        _ => Err(CuaError::InvalidArgument(format!(
            "location {l:?}: expected local or direct"
        ))),
    }
}

async fn delete(backend: &Backend, name: &str) -> Result<()> {
    match backend {
        Backend::Embedded(rt) => Ok(rt.delete(name).await?),
        Backend::Daemon(d) => Ok(d.delete(name).await?),
    }
}

/// A sandbox handle.
#[derive(uniffi::Object)]
pub struct Sandbox {
    backend: Backend,
    info: SandboxInfo,
}

#[uniffi::export]
impl Sandbox {
    /// Name.
    pub fn name(&self) -> String {
        self.info.name.clone()
    }

    /// Qualified ref (`local:<name>`, `cloud:<name>`, `direct:<host:port>`),
    /// the same kind of value local and in the cloud; it round-trips through
    /// `Sandboxes.connect`.
    pub fn id(&self) -> String {
        self.info.id.clone()
    }

    /// Where it runs: `local`, `cloud`, `direct` or `relay`.
    pub fn location(&self) -> String {
        self.info.location.clone()
    }

    /// What kind of machine: `container` or `vm` (empty when not known).
    pub fn kind(&self) -> String {
        self.info.kind.clone()
    }

    /// The engine that runs it (`gvisor`, `runc`, `qemu`, `lume`,
    /// `kubevirt`; empty when not known).
    pub fn runtime(&self) -> String {
        self.info.runtime.clone()
    }

    /// `runtime_type`.
    pub fn runtime_type(&self) -> String {
        self.info.runtime_type.clone()
    }

    /// Whether the sandbox is torn down on delete and has no state file.
    pub fn is_ephemeral(&self) -> bool {
        self.info.ephemeral
    }

    /// Declared services.
    pub fn services(&self) -> HashMap<String, u16> {
        self.info.services.clone()
    }

    /// Info as of creation / connection.
    pub fn info(&self) -> SandboxInfo {
        self.info.clone()
    }

    /// The image this sandbox runs, as resolved and pinned at create time:
    /// the digest and variant that actually ran. `None` for direct (URL)
    /// connections, images not resolved from a registry, and the daemon
    /// topology.
    pub fn image_info(&self) -> Option<ImageInfo> {
        self.info.image_info.clone()
    }

    /// Fresh info (status from the provider).
    pub async fn refresh(&self) -> Result<SandboxInfo> {
        let backend = self.backend.clone();
        let name = self.info.id.clone();
        Ok(run(async move {
            Ok(match &backend {
                Backend::Embedded(rt) => rt.record(&name).await?,
                Backend::Daemon(d) => d.get(&name).await?,
            })
        })
        .await?
        .into())
    }

    /// Attaches to cua-spacesd inside the sandbox (the `env` service or
    /// guest port 3211). Fails with `SpacesdNotAvailable` when none
    /// answers; sandbox lifecycle never depends on it.
    pub async fn spacesd(&self, probe_timeout_ms: Option<u32>) -> Result<Arc<SpacesdClient>> {
        let backend = self.backend.clone();
        let name = self.info.id.clone();
        let location = self.info.location.clone();
        let timeout = probe_timeout_ms.map(super::millis);
        run(async move {
            match &backend {
                Backend::Embedded(rt) => {
                    let a = rt.env(&name, timeout).await?;
                    Ok(Arc::new(
                        SpacesdClient::new(a.client, a.ws_headers).at(&location),
                    ))
                }
                Backend::Daemon(d) => {
                    let ep = d.env_endpoint(&name, timeout).await?;
                    let mut o = cua_spacesd_client::ConnectOptions::parse(&ep.url)?
                        .transport(cua_spacesd_client::TransportPreference::Native)
                        .probe(false);
                    o.token = Some(ep.token.clone());
                    let client = cua_spacesd_client::SpacesdClient::connect(o).await?;
                    Ok(Arc::new(
                        SpacesdClient::new(
                            client,
                            vec![("authorization".into(), format!("Bearer {}", ep.token))],
                        )
                        .at(&location),
                    ))
                }
            }
        })
        .await
    }

    /// A browser link to this sandbox's desktop in the cua-spacesd HTML5
    /// viewer (`/viewer` on the `env` service): video, audio, input,
    /// clipboard, file drop and folder sharing, in any modern browser. The
    /// link carries a scoped viewer ticket, never the sandbox token. Needs
    /// cua-spacesd in the sandbox.
    #[uniffi::method(default(options = None))]
    pub async fn viewer_url(&self, options: Option<ViewerOptions>) -> Result<ViewerLink> {
        let started = std::time::Instant::now();
        let r: Result<ViewerLink> = async move {
            let o = options.unwrap_or_default();
            let ttl = o.ttl_seconds.unwrap_or(3600);
            if !(60..=86_400).contains(&ttl) {
                return Err(CuaError::InvalidArgument(
                    "ttl_seconds must be between 60 and 86400".into(),
                ));
            }
            let guest = self.spacesd(None).await?;
            let base = self.service("env".into())?.url().await?;
            run(async move {
                use cua_spacesd_client::pb;
                let files_root = if o.view_only {
                    String::new()
                } else {
                    o.files_root.unwrap_or_else(|| "~".into())
                };
                let minted = guest
                    .client
                    .system()
                    .create_viewer_ticket(pb::CreateViewerTicketRequest {
                        ttl: Some(cua_proto::wkt::Duration {
                            seconds: i64::from(ttl),
                            nanos: 0,
                        }),
                        policy: if o.view_only {
                            pb::SessionPolicy::ViewOnly
                        } else {
                            pb::SessionPolicy::AllowActivation
                        } as i32,
                        clipboard: o.clipboard && !o.view_only,
                        files_root,
                        audio_uplink: o.microphone && !o.view_only,
                        principal: None,
                    })
                    .await
                    .map_err(cua_spacesd_client::Error::from)?
                    .into_inner();
                Ok(ViewerLink {
                    url: format!("{}{}", base.trim_end_matches('/'), minted.viewer_path),
                    expires_at_unix: minted.expires_at.map(|t| t.seconds).unwrap_or_default(),
                })
            })
            .await
        }
        .await;
        super::telemetry::api("sandbox.viewer_url", started, &r);
        r
    }

    /// Coding agents inside this sandbox (needs cua-spacesd): run a harness
    /// over ACP, stream normalized events, follow up, interrupt, collect
    /// results; runs outlive this handle.
    pub async fn agents(&self) -> Result<Arc<super::Agents>> {
        let guest = self.spacesd(None).await?;
        super::Agents::over(guest.client.clone(), &self.info.location).await
    }

    /// A named service (for example `"server"` or `"env"`).
    pub fn service(&self, name: String) -> Result<Arc<Service>> {
        if !self.info.services.contains_key(&name) && !self.info.endpoints.contains_key(&name) {
            // Direct and local sandboxes accept declared services only.
            if !self.info.services.is_empty() {
                return Err(CuaError::NotFound(format!(
                    "sandbox {} has no service {name:?} (known: {:?})",
                    self.info.name,
                    self.info.services.keys().collect::<Vec<_>>()
                )));
            }
        }
        Ok(Arc::new(Service {
            backend: self.backend.clone(),
            sandbox: self.info.id.clone(),
            name,
        }))
    }

    /// Where the MCP server behind `service` is (URL of its endpoint at
    /// `path`, default `/mcp`, plus the headers every request needs), for
    /// any MCP client. Embedded: the service route itself (loopback).
    /// Daemon: the daemon's streaming passthrough with its loopback bearer.
    #[uniffi::method(default(path = None))]
    pub async fn mcp_config(
        &self,
        service: String,
        path: Option<String>,
    ) -> Result<super::McpConfig> {
        self.service(service.clone())?.mcp_config(path).await
    }

    /// An MCP client (the official Rust SDK) for the MCP server behind
    /// `service` at `path` (default `/mcp`).
    #[uniffi::method(default(path = None))]
    pub async fn mcp(
        &self,
        service: String,
        path: Option<String>,
    ) -> Result<Arc<super::McpClient>> {
        self.service(service)?.mcp(path).await
    }

    /// A shareable URL for `service` that stops working after `ttl_seconds`
    /// (60 s to 24 h, default 1 h): a loopback URL with its own token, served
    /// by the cua daemon (started if needed; `CUA_BIN` names the CLI).
    #[uniffi::method(default(ttl_seconds = None, label = None))]
    pub async fn public_url(
        &self,
        service: String,
        ttl_seconds: Option<u32>,
        label: Option<String>,
    ) -> Result<PublicUrl> {
        let backend = self.backend.clone();
        let name = self.info.id.clone();
        let ttl = ttl_seconds.map(super::secs);
        run(async move {
            let u = match &backend {
                Backend::Embedded(rt) => rt.public_url(&name, &service, ttl, label).await?,
                Backend::Daemon(d) => d.public_url(&name, &service, ttl, label).await?,
            };
            Ok(u.into())
        })
        .await
    }

    /// Revokes a URL from `public_url`.
    pub async fn revoke_public_url(&self, id: String) -> Result<()> {
        let backend = self.backend.clone();
        let name = self.info.id.clone();
        run(async move {
            match &backend {
                Backend::Embedded(rt) => Ok(rt.revoke_public_url(&name, &id).await?),
                Backend::Daemon(d) => Ok(d.revoke_public_url(&name, &id).await?),
            }
        })
        .await
    }

    /// Forwards a loopback port to guest `port`: a TCP forward locally (and
    /// over cua-spacesd's tunnel when the image has it). `url()` is a
    /// loopback URL.
    pub async fn forward(&self, port: u16) -> Result<Arc<PortForward>> {
        let started = std::time::Instant::now();
        let r: Result<Arc<PortForward>> = async move {
            let backend = self.backend.clone();
            let name = self.info.id.clone();
            let info = run({
                let backend = backend.clone();
                async move {
                    Ok(match &backend {
                        Backend::Embedded(rt) => rt.forward(&name, port).await?,
                        Backend::Daemon(d) => d.forward(&name, port).await?,
                    })
                }
            })
            .await?;
            Ok(Arc::new(PortForward {
                backend: Some(backend),
                info,
                tunnel: std::sync::Mutex::new(None),
                closed: std::sync::atomic::AtomicBool::new(false),
            }))
        }
        .await;
        super::telemetry::api("sandbox.forward", started, &r);
        r
    }

    /// Waits until every probe passes.
    pub async fn wait_ready(&self, probes: Vec<ReadinessProbe>, timeout_ms: u32) -> Result<()> {
        let probes = probes
            .iter()
            .map(|p| p.resolve(&self.info.services))
            .collect::<Result<Vec<_>>>()?;
        let backend = self.backend.clone();
        let name = self.info.id.clone();
        let t = super::millis(timeout_ms);
        run(async move {
            match &backend {
                Backend::Embedded(rt) => Ok(rt.wait_ready(&name, &probes, t).await?),
                Backend::Daemon(d) => Ok(d.wait_ready(&name, &probes, t).await?),
            }
        })
        .await
    }

    /// Extends the sandbox's lease by `seconds`, for a provider that has one.
    pub async fn keep_alive(&self, seconds: u32) -> Result<()> {
        let backend = self.backend.clone();
        let name = self.info.id.clone();
        let d = super::secs(seconds);
        run(async move {
            match &backend {
                Backend::Embedded(rt) => Ok(rt.keep_alive(&name, d).await?),
                Backend::Daemon(c) => Ok(c.keep_alive(&name, d).await?),
            }
        })
        .await
    }

    /// Gives this local sandbox a lifetime of `seconds` from now, replacing
    /// any earlier one: the cua daemon then deletes it (`on_expire`
    /// `"delete"`, the default) or suspends it (`"suspend"`), whether or
    /// not this process is still running. Needs the cua daemon (an
    /// embedded runtime records the lifetime; the next daemon enforces it).
    pub async fn set_lifetime(&self, seconds: u32, on_expire: Option<String>) -> Result<()> {
        let action = match on_expire.as_deref() {
            None => cua_sandbox_core::LocalExpiry::Delete,
            Some(s) => cua_sandbox_core::LocalExpiry::parse(s).ok_or_else(|| {
                CuaError::InvalidArgument(format!(
                    "on_expire {s:?}: expected \"delete\" or \"suspend\""
                ))
            })?,
        };
        let backend = self.backend.clone();
        let name = self.info.id.clone();
        let d = super::secs(seconds);
        run(async move {
            match &backend {
                Backend::Embedded(rt) => Ok(rt.set_lifetime(&name, d, action).await?),
                Backend::Daemon(c) => Ok(c.set_lifetime(&name, d, action).await?),
            }
        })
        .await
    }

    /// When this sandbox's lifetime ends ([`Sandbox::set_lifetime`]), as
    /// Unix seconds; `None` when it has none.
    pub async fn expires_at_unix(&self) -> Result<Option<i64>> {
        let info = self.refresh().await?;
        Ok(info.expires_at_unix)
    }

    /// Suspends.
    pub async fn suspend(&self) -> Result<()> {
        self.lifecycle(Op::Suspend).await
    }

    /// Resumes.
    pub async fn resume(&self) -> Result<()> {
        self.lifecycle(Op::Resume).await
    }

    /// Restarts.
    pub async fn restart(&self) -> Result<()> {
        self.lifecycle(Op::Restart).await
    }

    /// Gives a local VM another shape: `cpus` vCPUs, `memory_mb` MiB of
    /// memory and a disk grown to `disk_gb` GiB (each `None` keeps it). The
    /// VM stops and boots again from its own disk, so its memory and
    /// running processes do not survive; its files do. Returns once it is
    /// ready again. A disk never shrinks. Containers, Fleet and other
    /// providers' sandboxes are refused (`Unsupported`).
    pub async fn resize(
        &self,
        cpus: Option<u32>,
        memory_mb: Option<u64>,
        disk_gb: Option<u32>,
    ) -> Result<()> {
        let shape = cua_sandbox_core::Resize {
            cpus,
            memory_mb,
            disk_gb,
            probes: Vec::new(),
        };
        let backend = self.backend.clone();
        let name = self.info.id.clone();
        run(async move {
            match &backend {
                Backend::Embedded(rt) => Ok(rt.resize(&name, &shape).await?),
                Backend::Daemon(d) => Ok(d.resize(&name, &shape).await?),
            }
        })
        .await
    }

    /// Deletes (local: removes it; direct: forgets it; your cloud: deletes
    /// the machine).
    pub async fn delete(&self) -> Result<()> {
        let backend = self.backend.clone();
        let name = self.info.id.clone();
        let r = run(async move { delete(&backend, &name).await }).await;
        super::telemetry::sandbox_deleted(&self.info.location, &r);
        r
    }

    /// Opens a media session and returns a loopback WebSocket bridge for a
    /// webview (daemon mode). `open_media_json` is a
    /// `cua.env.v1.OpenMediaRequest` in proto3 JSON (`None`: primary display).
    pub async fn open_media_bridge(&self, open_media_json: Option<String>) -> Result<MediaBridge> {
        let backend = self.backend.clone();
        let name = self.info.id.clone();
        run(async move {
            match &backend {
                Backend::Daemon(d) => {
                    let r = d.open_media_bridge(&name, open_media_json).await?;
                    Ok(MediaBridge {
                        ws_url: r.ws_url,
                        ticket: r.ticket,
                        open_media_response_json: r.open_media_response_json,
                    })
                }
                Backend::Embedded(_) => Err(CuaError::Unsupported(
                    "media bridges are served by `cua daemon`; use Cua.connect, or \
                     SpacesdClient.open_media in-process"
                        .into(),
                )),
            }
        })
        .await
    }
}

enum Op {
    Suspend,
    Resume,
    Restart,
}

impl Sandbox {
    async fn lifecycle(&self, op: Op) -> Result<()> {
        let backend = self.backend.clone();
        let name = self.info.id.clone();
        run(async move {
            match (&backend, op) {
                (Backend::Embedded(rt), Op::Suspend) => Ok(rt.suspend(&name).await?),
                (Backend::Embedded(rt), Op::Resume) => Ok(rt.resume(&name).await?),
                (Backend::Embedded(rt), Op::Restart) => Ok(rt.restart(&name).await?),
                (Backend::Daemon(d), Op::Suspend) => Ok(d.suspend(&name).await?),
                (Backend::Daemon(d), Op::Resume) => Ok(d.resume(&name).await?),
                (Backend::Daemon(d), Op::Restart) => Ok(d.restart(&name).await?),
            }
        })
        .await
    }
}

/// The guest display of a sandbox without cua-spacesd
/// (`Sandbox.guest_display`). An object, not a record, so no binding's
/// default string form (repr, str, description, toString, Debug) can show
/// the VNC password: `url()` is masked, and only the explicit
/// `url_with_password()` returns it.
#[derive(uniffi::Object)]
pub struct GuestDisplay {
    inner: cua_sandbox_core::GuestDisplay,
}

impl std::fmt::Debug for GuestDisplay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // cua_sandbox_core::GuestDisplay's Debug is masked.
        self.inner.fmt(f)
    }
}

impl std::fmt::Display for GuestDisplay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.inner.redacted_url())
    }
}

impl From<cua_sandbox_core::GuestDisplay> for GuestDisplay {
    fn from(inner: cua_sandbox_core::GuestDisplay) -> Self {
        GuestDisplay { inner }
    }
}

#[uniffi::export]
impl GuestDisplay {
    /// The display URL (`vnc://HOST:PORT`) with any password masked as `****`.
    /// Safe to print and log.
    pub fn url(&self) -> String {
        self.inner.redacted_url()
    }

    /// SENSITIVE: the display URL with the VNC password
    /// (`vnc://:PASSWORD@HOST:PORT`), for handing to a VNC client. Never
    /// print or log it.
    pub fn url_with_password(&self) -> String {
        self.inner.url.clone()
    }

    /// How the display is reached (`vnc`).
    pub fn via(&self) -> String {
        self.inner.via.clone()
    }

    /// A host command that opens a viewer on it (`lume attach <vm>`);
    /// empty when there is none. It handles the password itself.
    pub fn open_command(&self) -> Vec<String> {
        self.inner.open_command.clone().unwrap_or_default()
    }
}

fn core_err(e: cua_sandbox_core::Error) -> CuaError {
    cua_daemon::Error::from(e).into()
}

/// The agentless fallback in the daemon topology: the daemon runs on this
/// machine, and Lume's `lume ssh` and VNC endpoint are host facilities, so
/// the client reaches them itself for a local sandbox.
fn daemon_local(info: &SandboxInfo, op: &str) -> Result<cua_daemon::local::VmmLocal> {
    if info.location != "local" {
        return Err(CuaError::Unsupported(format!(
            "{op} without cua-spacesd is only available for local Lume sandboxes \
             ({} is {})",
            info.name, info.location
        )));
    }
    Ok(cua_daemon::local::VmmLocal::default())
}

/// Agentless access for sandboxes without cua-spacesd (Rust hosts; the
/// exported methods are below).
impl Sandbox {
    /// `Sandbox.guest_sh`, forwarding output to `sink` as it arrives;
    /// returns the exit code.
    pub async fn guest_sh_streaming(
        &self,
        line: String,
        timeout_ms: Option<u32>,
        sink: tokio::sync::mpsc::Sender<cua_sandbox_core::GuestOutput>,
    ) -> Result<i64> {
        use cua_sandbox_core::LocalRuntime as _;
        let backend = self.backend.clone();
        let info = self.info.clone();
        let timeout = Some(super::millis(timeout_ms.unwrap_or(120_000)));
        run(async move {
            match &backend {
                Backend::Embedded(rt) => rt
                    .handle(&info.id)
                    .await?
                    .guest_exec(&line, timeout, sink)
                    .await
                    .map_err(core_err),
                Backend::Daemon(_) => daemon_local(&info, "exec")?
                    .guest_exec(&info.name, &line, timeout, sink)
                    .await
                    .map_err(|e| core_err(e.into())),
            }
        })
        .await
    }
}

#[uniffi::export]
impl Sandbox {
    /// Whether this sandbox has no cua-spacesd: it declares no `env`
    /// service, or its image says it does not carry one. `spacesd()` fails
    /// for these; `guest_sh`, `guest_screenshot` and `guest_display` reach a
    /// local Lume sandbox without it.
    pub fn lacks_spacesd(&self) -> bool {
        !self.info.services.contains_key("env")
            || self.info.image_info.as_ref().and_then(|i| i.spacesd) == Some(false)
    }

    /// Runs `line` with `/bin/sh -c` in the guest without cua-spacesd, to
    /// completion (default timeout 120 s). Local macOS Lume sandboxes only:
    /// over `lume ssh`, stdout and stderr kept apart, no stdin. Others fail
    /// with `Unsupported`.
    #[uniffi::method(default(timeout_ms = None))]
    pub async fn guest_sh(&self, line: String, timeout_ms: Option<u32>) -> Result<ProcessOutput> {
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        let run = self.guest_sh_streaming(line, timeout_ms, tx);
        let collect = async {
            let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
            while let Some(c) = rx.recv().await {
                match c {
                    cua_sandbox_core::GuestOutput::Stdout(b) => stdout.extend(b),
                    cua_sandbox_core::GuestOutput::Stderr(b) => stderr.extend(b),
                }
            }
            (stdout, stderr)
        };
        let (code, (stdout, stderr)) = tokio::join!(run, collect);
        let code = code?;
        Ok(ProcessOutput {
            exit: ExitInfo {
                success: code == 0,
                code: i32::try_from(code).ok(),
                signal: None,
                signal_number: None,
                signal_name: None,
                timed_out: false,
                error: None,
            },
            stdout,
            stderr,
            pty: vec![],
        })
    }

    /// Captures the guest framebuffer as PNG without cua-spacesd. Local
    /// Lume sandboxes only: over the VM's VNC endpoint.
    pub async fn guest_screenshot(&self) -> Result<Screenshot> {
        use cua_sandbox_core::LocalRuntime as _;
        let backend = self.backend.clone();
        let info = self.info.clone();
        let s = run(async move {
            match &backend {
                Backend::Embedded(rt) => rt
                    .handle(&info.id)
                    .await?
                    .guest_screenshot()
                    .await
                    .map_err(core_err),
                Backend::Daemon(_) => daemon_local(&info, "screenshots")?
                    .guest_screenshot(&info.name)
                    .await
                    .map_err(|e| core_err(e.into())),
            }
        })
        .await?;
        Ok(Screenshot {
            image: s.png,
            format: ImageFormat::Png,
            width: s.width,
            height: s.height,
            scale: 1.0,
            screenshot_id: String::new(),
        })
    }

    /// The guest display without cua-spacesd: the VM's VNC endpoint and a
    /// host command that opens it (`lume attach`). Local Lume sandboxes
    /// only.
    pub async fn guest_display(&self) -> Result<Arc<GuestDisplay>> {
        use cua_sandbox_core::LocalRuntime as _;
        let backend = self.backend.clone();
        let info = self.info.clone();
        run(async move {
            match &backend {
                Backend::Embedded(rt) => rt
                    .handle(&info.id)
                    .await?
                    .guest_display()
                    .await
                    .map(|d| Arc::new(d.into()))
                    .map_err(core_err),
                Backend::Daemon(_) => daemon_local(&info, "a display")?
                    .guest_display(&info.name)
                    .await
                    .map(|d| Arc::new(d.into()))
                    .map_err(|e| core_err(e.into())),
            }
        })
        .await
    }
}

/// A named service of a sandbox.
#[derive(uniffi::Object)]
pub struct Service {
    backend: Backend,
    sandbox: String,
    name: String,
}

#[uniffi::export]
impl Service {
    /// Service name.
    pub fn name(&self) -> String {
        self.name.clone()
    }

    /// A URL for the service usable from this machine with no credentials
    /// (no trailing slash): the published loopback port locally.
    pub async fn url(&self) -> Result<String> {
        let backend = self.backend.clone();
        let (sb, svc) = (self.sandbox.clone(), self.name.clone());
        run(async move {
            match &backend {
                Backend::Embedded(rt) => Ok(rt.service_url(&sb, &svc).await?),
                Backend::Daemon(d) => Ok(d.service_url(&sb, &svc).await?),
            }
        })
        .await
    }

    /// A shareable URL for this service that stops working after
    /// `ttl_seconds` (60 s to 24 h, default 1 h): the same as
    /// `Sandbox.public_url(name, ...)`: a loopback URL with its own token,
    /// served by the cua daemon.
    #[uniffi::method(default(ttl_seconds = None, label = None))]
    pub async fn public_url(
        &self,
        ttl_seconds: Option<u32>,
        label: Option<String>,
    ) -> Result<PublicUrl> {
        let backend = self.backend.clone();
        let (sb, svc) = (self.sandbox.clone(), self.name.clone());
        let ttl = ttl_seconds.map(super::secs);
        run(async move {
            let u = match &backend {
                Backend::Embedded(rt) => rt.public_url(&sb, &svc, ttl, label).await?,
                Backend::Daemon(d) => d.public_url(&sb, &svc, ttl, label).await?,
            };
            Ok(u.into())
        })
        .await
    }

    /// Where this service is reachable from this process: base URL plus
    /// the headers every request needs. Any HTTP client can use it (SSE,
    /// long-lived streams and every method pass through unchanged).
    pub async fn endpoint(&self) -> Result<ServiceEndpoint> {
        let (backend, sb, svc) = (
            self.backend.clone(),
            self.sandbox.clone(),
            self.name.clone(),
        );
        let e = run(async move { super::mcp::service_endpoint(&backend, &sb, &svc).await }).await?;
        Ok(ServiceEndpoint {
            url: e.url,
            headers: e
                .headers
                .into_iter()
                .map(|(name, value)| HttpHeader { name, value })
                .collect(),
        })
    }

    /// The MCP endpoint of this service at `path` (default `/mcp`).
    #[uniffi::method(default(path = None))]
    pub async fn mcp_config(&self, path: Option<String>) -> Result<super::McpConfig> {
        let (backend, sb, svc) = (
            self.backend.clone(),
            self.sandbox.clone(),
            self.name.clone(),
        );
        run(async move {
            let e = super::mcp::service_endpoint(&backend, &sb, &svc).await?;
            Ok(cua_sandbox_core::McpConfig::of(&e, path.as_deref()).into())
        })
        .await
    }

    /// An MCP client (the official Rust SDK) for this service's endpoint at
    /// `path` (default `/mcp`).
    #[uniffi::method(default(path = None))]
    pub async fn mcp(&self, path: Option<String>) -> Result<Arc<super::McpClient>> {
        let (backend, sb, svc) = (
            self.backend.clone(),
            self.sandbox.clone(),
            self.name.clone(),
        );
        run(async move {
            let e = super::mcp::service_endpoint(&backend, &sb, &svc).await?;
            super::McpClient::connect(cua_sandbox_core::McpConfig::of(&e, path.as_deref())).await
        })
        .await
    }

    /// One HTTP request to `path` on the service. Credentials are attached by
    /// the runtime. `headers` are sent as given (for example `content-type`,
    /// or `accept` and `mcp-session-id` for MCP).
    #[uniffi::method(default(headers = None))]
    pub async fn request(
        &self,
        method: String,
        path: String,
        body: Option<Vec<u8>>,
        timeout_ms: Option<u32>,
        headers: Option<Vec<HttpHeader>>,
    ) -> Result<HttpResponse> {
        let backend = self.backend.clone();
        let (sb, svc) = (self.sandbox.clone(), self.name.clone());
        let t = ms(timeout_ms, 30_000);
        let headers: Vec<(String, String)> = headers
            .unwrap_or_default()
            .into_iter()
            .map(|h| (h.name, h.value))
            .collect();
        run(async move {
            match &backend {
                Backend::Embedded(rt) => {
                    let r = rt
                        .service_request(&sb, &svc, &method, &path, &headers, body, t)
                        .await?;
                    Ok(HttpResponse {
                        status: r.status,
                        headers: r
                            .headers
                            .into_iter()
                            .map(|(name, value)| HttpHeader { name, value })
                            .collect(),
                        body: r.body,
                    })
                }
                Backend::Daemon(d) => {
                    let r = d
                        .service_request(&sb, &svc, &method, &path, &headers, body, t)
                        .await?;
                    Ok(HttpResponse {
                        status: r.status,
                        headers: r
                            .headers
                            .into_iter()
                            .map(|(name, value)| HttpHeader { name, value })
                            .collect(),
                        body: r.body,
                    })
                }
            }
        })
        .await
    }
}

/// An active port forward. Closed on `close()` or when dropped.
#[derive(uniffi::Object)]
pub struct PortForward {
    backend: Option<Backend>,
    info: cua_daemon::ForwardInfo,
    /// A forward over a Space's own connection ([`super::Space::forward`]),
    /// relayed by this process.
    tunnel: std::sync::Mutex<Option<cua_spacesd_client::TcpForward>>,
    closed: std::sync::atomic::AtomicBool,
}

impl PortForward {
    /// A forward over a spacesd tunnel this process runs.
    pub(crate) fn tunnel(fwd: cua_spacesd_client::TcpForward) -> Self {
        let local = fwd.local_addr().to_string();
        Self {
            backend: None,
            info: cua_daemon::ForwardInfo {
                id: String::new(),
                guest_port: fwd.guest_port(),
                url: Some(format!("http://{local}")),
                local_addr: Some(local),
            },
            tunnel: std::sync::Mutex::new(Some(fwd)),
            closed: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

#[uniffi::export]
impl PortForward {
    /// Guest port.
    pub fn guest_port(&self) -> u16 {
        self.info.guest_port
    }

    /// Loopback `host:port` accepting connections (local/direct).
    pub fn local_addr(&self) -> Option<String> {
        self.info.local_addr.clone()
    }

    /// URL to use (a loopback URL).
    pub fn url(&self) -> Option<String> {
        self.info.url.clone()
    }

    /// Stops the forward.
    pub async fn close(&self) -> Result<()> {
        if self.closed.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return Ok(());
        }
        let tunnel = self.tunnel.lock().expect("tunnel").take();
        if let Some(fwd) = tunnel {
            return run(async move { Ok(fwd.close().await?) }).await;
        }
        let Some(backend) = self.backend.clone().filter(|_| !self.info.id.is_empty()) else {
            return Ok(());
        };
        let id = self.info.id.clone();
        run(async move {
            match &backend {
                Backend::Embedded(rt) => Ok(rt.close_forward(&id)?),
                Backend::Daemon(d) => Ok(d.close_forward(&id).await?),
            }
        })
        .await
    }
}

impl Drop for PortForward {
    fn drop(&mut self) {
        if self.closed.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        if let Some(fwd) = self.tunnel.lock().expect("tunnel").take() {
            // Inside the SDK runtime, so the driver-side forward is revoked.
            super::runtime().spawn(async move {
                let _ = fwd.close().await;
            });
            return;
        }
        let Some(backend) = self.backend.clone().filter(|_| !self.info.id.is_empty()) else {
            return;
        };
        let id = self.info.id.clone();
        super::runtime().spawn(async move {
            let _ = match &backend {
                Backend::Embedded(rt) => rt.close_forward(&id),
                Backend::Daemon(d) => d.close_forward(&id).await,
            };
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_probes_resolve_against_declared_services() {
        let services: HashMap<String, u16> = [("mcp".to_string(), 8765u16)].into();
        assert_eq!(
            ReadinessProbe::http("mcp", "health")
                .resolve(&services)
                .unwrap(),
            Probe::Http {
                port: 8765,
                path: "/health".into(),
                status: None
            }
        );
        assert_eq!(
            ReadinessProbe::tcp("mcp").resolve(&services).unwrap(),
            Probe::Tcp(8765)
        );
        assert_eq!(
            ReadinessProbe::tcp("env").resolve(&services).unwrap(),
            Probe::Tcp(3211),
            "env defaults to the spacesd port"
        );
        assert!(ReadinessProbe::tcp("nope").resolve(&services).is_err());
        assert!(ReadinessProbe::default().resolve(&services).is_err());
        let by_port = ReadinessProbe {
            port: 22,
            ..Default::default()
        };
        assert_eq!(by_port.resolve(&services).unwrap(), Probe::Tcp(22));
    }

    /// With no `on`, the location is the user default (local unless
    /// configured). Runs with a private CUA_HOME and no default overrides in
    /// the environment.
    #[test]
    fn the_location_defaults_to_the_user_default() {
        use cua_sandbox_core::placement::{Kind, On, Runtime};
        let _env = super::super::test_env::EnvGuard::isolated();
        let mut o = SandboxCreateOptions::auto("python:3.12-slim");
        assert_eq!(o.placement().unwrap().on, On::Local);
        assert_eq!(SandboxCreateOptions::default().on, None);
        let r = o.to_request().unwrap();
        assert_eq!(r.location.as_deref(), Some("local"));
        assert_eq!(r.provider, Some(CoreProvider::Local));
        // A pool name does not move the sandbox anywhere.
        o.pool = Some("agents".into());
        let r = o.to_request().unwrap();
        assert_eq!(
            (r.pool.as_deref(), r.location.as_deref()),
            (Some("agents"), Some("local"))
        );
        // Cua Cloud has ended.
        let e = SandboxCreateOptions::new("cloud", "x")
            .placement()
            .unwrap_err();
        assert_eq!(
            e.to_string(),
            format!("unsupported: {}", cua_sandbox_core::CLOUD_ENDED)
        );
        // direct:<addr> carries the address.
        let o = SandboxCreateOptions::new("direct:127.0.0.1:3211", "");
        let r = o.to_request().unwrap();
        assert_eq!(r.url.as_deref(), Some("127.0.0.1:3211"));
        assert_eq!(r.location.as_deref(), Some("direct:127.0.0.1:3211"));
        // kind and runtime go out resolved.
        let mut o = SandboxCreateOptions::new("local", "img");
        o.kind = Some("vm".into());
        let p = o.placement().unwrap();
        assert_eq!((p.kind, p.runtime), (Kind::Vm, Runtime::Auto));
        assert_eq!(o.to_request().unwrap().kind.as_deref(), Some("vm"));
    }

    #[test]
    fn invalid_placements_are_typed_and_list_the_valid_values() {
        let _env = super::super::test_env::EnvGuard::isolated();
        let mut o = SandboxCreateOptions::new("local", "img");
        o.runtime = Some("kubevirt".into());
        let e = o.placement().unwrap_err();
        let CuaError::InvalidPlacement(m) = &e else {
            panic!("{e:?}")
        };
        assert!(m.contains("valid runtime: auto"), "{m}");
        assert_eq!(e.variant(), "InvalidPlacement");
        let mut o = SandboxCreateOptions::new("local", "img");
        o.kind = Some("container".into());
        o.runtime = Some("lume".into());
        assert!(matches!(o.placement(), Err(CuaError::InvalidPlacement(_))));
        let o = SandboxCreateOptions::new("qemu", "img");
        let e = o.placement().unwrap_err();
        assert!(e.to_string().contains("--runtime qemu"), "{e}");
    }

    /// Precedence: explicit > env > config > built-in.
    #[test]
    fn user_defaults_apply_below_explicit_values() {
        use cua_sandbox_core::placement::On;
        let env = super::super::test_env::EnvGuard::isolated();
        let mut s = cua_sandbox_core::settings::Settings::load().unwrap();
        s.set(
            cua_sandbox_core::settings::key("default.on").unwrap(),
            "aws",
        )
        .unwrap();
        let o = SandboxCreateOptions::auto("img");
        let p = o.placement().unwrap();
        assert_eq!(p.on, On::Provider("aws".into()));
        assert!(matches!(
            p.on_source,
            cua_sandbox_core::settings::Source::Config(_)
        ));
        env.set("CUA_DEFAULT_ON", "local");
        assert_eq!(o.placement().unwrap().on, On::Local);
        let o = SandboxCreateOptions::new("local", "img");
        assert_eq!(o.placement().unwrap().on, On::Local);
        // Cua Cloud has ended.
        let o = SandboxCreateOptions::new("cloud", "img");
        assert!(matches!(o.placement(), Err(CuaError::Unsupported(_))));
    }

    #[test]
    fn on_words_pick_the_provider_and_the_location() {
        let mut o = SandboxCreateOptions::auto("ghcr.io/trycua/linux:24.04");
        o.on = Some("e2b".into());
        let r = o.to_request().unwrap();
        assert_eq!(r.provider, Some(CoreProvider::Contrib));
        assert_eq!(r.location.as_deref(), Some("e2b"));
        o.on = Some("cloud".into());
        assert!(matches!(o.to_request(), Err(CuaError::Unsupported(_))));
        o.on = Some("direct:127.0.0.1:3211".into());
        assert_eq!(
            o.to_request().unwrap().url.as_deref(),
            Some("127.0.0.1:3211")
        );
        o.on = Some("nosuchcloud".into());
        let err = o.to_request().unwrap_err().to_string();
        assert!(err.contains("daytona"), "{err}");
    }

    #[test]
    fn phases_are_portable() {
        assert_eq!(
            SandboxPhase::of(SandboxStatus::Running),
            SandboxPhase::Ready
        );
        assert_eq!(
            SandboxPhase::of(SandboxStatus::Suspended),
            SandboxPhase::Stopped
        );
        assert_eq!(
            SandboxPhase::of(SandboxStatus::Provisioning),
            SandboxPhase::Provisioning
        );
        assert_eq!(
            SandboxPhase::of(SandboxStatus::Unknown),
            SandboxPhase::Starting
        );
    }
}

/// A parsed sandbox ref.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SandboxRefParts {
    /// `local`, `cloud`, `direct`, `relay` or a contrib provider (`e2b`);
    /// `None` for a bare name.
    pub location: Option<String>,
    /// The name, `host:port` or relay machine id.
    pub name: String,
    /// The canonical form (`location:name`, or the bare name).
    pub id: String,
}

/// Every location word `on=` accepts, in display order: `local`, `cloud`,
/// then the contrib providers (`e2b`, `daytona`, `modal`, ...) and your own
/// clouds (`aws`, `gcp`, `hetzner`). A word is listed whether or not this build
/// includes that provider (see [`contrib_providers_built`]).
#[uniffi::export]
pub fn sandbox_locations() -> Vec<String> {
    // Cua Cloud (`cloud`) has ended and is no longer offered.
    let mut out: Vec<String> = ["local"]
        .into_iter()
        .chain(cua_sandbox_core::CONTRIB_LOCATIONS.iter().copied())
        .map(str::to_string)
        .collect();
    for w in cua_sandbox_core::CLOUD_LOCATIONS {
        if !out.iter().any(|o| o == w) {
            out.push((*w).to_string());
        }
    }
    out
}

/// The contrib providers this build includes (empty unless built with the
/// `contrib` feature).
#[uniffi::export]
pub fn contrib_providers_built() -> Vec<String> {
    cua_daemon::contrib_provider_names()
}

/// Parses a sandbox ref: `local:<name>`, `cloud:<name>`,
/// `direct:<host:port>`, `relay:<machine-id>`, a legacy spelling
/// (`space://cloud/<name>`, `url:<addr>`, a URL), or a bare name.
#[uniffi::export]
pub fn parse_sandbox_ref(input: String) -> Result<SandboxRefParts> {
    let r = cua_sandbox_core::SandboxRef::parse(&input)?;
    Ok(SandboxRefParts {
        location: r.location_word().map(str::to_string),
        name: r.name().to_string(),
        id: r.to_string(),
    })
}

/// Qualifies `name` for a lookup: `local = true` narrows a bare name to
/// `local:<name>`, `false` to `cloud:<name>`, `None` keeps it (a bare name
/// is then searched across locations). A qualified ref elsewhere than
/// `local` asks is an `InvalidArgument`.
#[uniffi::export(default(local = None))]
pub fn qualify_sandbox_ref(name: String, local: Option<bool>) -> Result<String> {
    use cua_sandbox_core::{Location, SandboxRef};
    let location = local.map(|l| if l { Location::Local } else { Location::Cloud });
    Ok(SandboxRef::parse(&name)?.narrow(location)?.to_string())
}

/// The qualified candidates an `AmbiguousSandbox` message lists (empty
/// for any other message).
#[uniffi::export]
pub fn ambiguous_sandbox_candidates(message: String) -> Vec<String> {
    message
        .rsplit_once("use one of: ")
        .map(|(_, list)| {
            list.split(',')
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

// -- Batched forks -----------------------------------------------------------

/// A fallback copy of checkpoints. Your machines fetch a checkpoint there
/// when the machine that holds it cannot be reached
/// ([`Sandboxes::publish_checkpoint`] puts it there). Set `s3_bucket` (S3,
/// with each machine's AWS credentials) or `dir` (a folder every machine
/// mounts, such as your Cua Volume).
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct SnapshotFallbackOptions {
    /// S3 bucket.
    #[uniffi(default = "")]
    pub s3_bucket: String,
    /// Key prefix in the bucket (`cua-snapshots/`).
    #[uniffi(default = "")]
    pub s3_prefix: String,
    /// Region (empty: each machine's AWS default).
    #[uniffi(default = "")]
    pub s3_region: String,
    /// An S3-compatible endpoint (empty: AWS).
    #[uniffi(default = "")]
    pub s3_endpoint: String,
    /// A folder every machine mounts (`~` is each machine's home), for
    /// example `~/Cua Volume/snapshots`.
    #[uniffi(default = "")]
    pub dir: String,
}

impl SnapshotFallbackOptions {
    fn to_pb(&self) -> cua_proto::env::v1::SnapshotFallback {
        cua_proto::env::v1::SnapshotFallback {
            s3: (!self.s3_bucket.trim().is_empty()).then(|| cua_proto::env::v1::SnapshotS3 {
                bucket: self.s3_bucket.clone(),
                prefix: self.s3_prefix.clone(),
                region: self.s3_region.clone(),
                endpoint: self.s3_endpoint.clone(),
            }),
            dir: self.dir.clone(),
        }
    }
}

/// Batched fork options. For [`Sandboxes::fork_many`] and
/// [`Sandboxes::fork_groups`].
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct ForkManyOptions {
    /// Where: `local` (this machine, the default), `auto` (your machines
    /// that provide Spaces, this one included, least loaded first) or
    /// `host:<machine>`.
    #[uniffi(default = None)]
    pub on: Option<String>,
    /// Forks per group for [`Sandboxes::fork_many`] (0: all in one group).
    /// A group's forks go to one machine, so they share the checkpoint's
    /// memory there.
    #[uniffi(default = 0)]
    pub group_size: u32,
    /// Forks started at once on one machine (0: 4).
    #[uniffi(default = 0)]
    pub max_parallel_per_host: u32,
    /// Names are `<prefix>-<n>` (empty: `<checkpoint>-<random>`).
    #[uniffi(default = "")]
    pub name_prefix: String,
    /// vCPUs each fork is counted as when placing (0: 1, as forks of one
    /// world are mostly idle).
    #[uniffi(default = 0)]
    pub cpus: u32,
    /// Memory (MiB) each fork is counted as when placing (0: the
    /// checkpoint's).
    #[uniffi(default = 0)]
    pub memory_mb: u64,
    /// Times forks of a machine that failed (or was full) are placed again
    /// on another (0: once).
    #[uniffi(default = 0)]
    pub retries: u32,
    /// Never place failed forks again: report them.
    #[uniffi(default = false)]
    pub no_retry: bool,
    /// Where machines fetch a checkpoint they lack when the one that holds
    /// it cannot be reached.
    #[uniffi(default = None)]
    pub fallback: Option<SnapshotFallbackOptions>,
}

/// A group of forks. The forks of one checkpoint that go to one machine
/// together ([`Sandboxes::fork_groups`]).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ForkGroup {
    /// The checkpoint, or a local sandbox (see [`Sandboxes::fork_many`]).
    pub source: String,
    /// How many forks.
    pub count: u32,
    /// Names are `<prefix>-<n>` (empty: the batch's).
    #[uniffi(default = "")]
    pub name_prefix: String,
}

/// One fork of a batch. Ready, or failed with why.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ForkBatchItem {
    /// Its index in the batch.
    pub index: u32,
    /// Its group.
    pub group: u32,
    /// Its checkpoint.
    pub source: String,
    /// Its name.
    pub name: String,
    /// The machine it runs on (`local` for this one).
    pub host: String,
    /// The Space to connect to (`local:<name>`, `direct:<addr>`,
    /// `relay:<machine>`); empty when it failed.
    pub space: String,
    /// Why it failed; empty when it is ready.
    pub error: String,
    /// `host_failed`, `no_capacity`, `sync_failed`, `fork_failed` or
    /// `no_host` with `error`.
    pub error_code: String,
    /// Which placement it came from (1: the first).
    pub attempt: u32,
    /// Milliseconds from the start of the batch until it was ready.
    pub elapsed_ms: u64,
    /// Milliseconds its own fork took.
    pub fork_ms: u64,
}

impl ForkBatchItem {
    fn of(i: cua_proto::daemon::v1::ForkBatchItem) -> Self {
        Self {
            index: i.index,
            group: i.group,
            source: i.source,
            name: i.name,
            host: i.host,
            space: i.space,
            error: i.error,
            error_code: i.error_code,
            attempt: i.attempt,
            elapsed_ms: i.elapsed_ms,
            fork_ms: i.fork_ms,
        }
    }
}

/// A group's placement. Where its forks went.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ForkBatchPlacement {
    /// The group.
    pub group: u32,
    /// Its checkpoint.
    pub source: String,
    /// The machine.
    pub host: String,
    /// How many of its forks go there.
    pub count: u32,
    /// 1 for the first placement, higher when placed again.
    pub attempt: u32,
    /// The checkpoint is copied there first.
    pub needs_sync: bool,
    /// Why there.
    pub reason: String,
}

/// A checkpoint copy's progress. Copying it to a machine that lacked it.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SnapshotSyncProgress {
    /// The checkpoint.
    pub name: String,
    /// Its digest.
    pub digest: String,
    /// Where the bytes come from now (a machine, `s3`, `volume`).
    pub from: String,
    /// Bytes copied.
    pub bytes_done: u64,
    /// Bytes to copy in all.
    pub bytes_total: u64,
    /// Bytes the machine had already.
    pub bytes_reused: u64,
    /// Finished.
    pub done: bool,
    /// Every file matched its SHA-256.
    pub verified: bool,
}

/// A batch's summary.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ForkBatchSummary {
    /// Forks that are ready.
    pub ok: u32,
    /// Forks that failed.
    pub failed: u32,
    /// Milliseconds the batch took.
    pub elapsed_ms: u64,
    /// Ready forks per machine.
    pub per_host: HashMap<String, u32>,
    /// Most forks starting at once per machine.
    pub peak_parallel: HashMap<String, u32>,
}

/// A batch event.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum ForkBatchEvent {
    /// A group was placed.
    Placement { placement: ForkBatchPlacement },
    /// A checkpoint is being copied to a machine.
    Sync { progress: SnapshotSyncProgress },
    /// A fork is ready, or failed.
    Item { item: ForkBatchItem },
    /// The batch is over (the last event).
    Done { summary: ForkBatchSummary },
}

impl ForkBatchEvent {
    fn of(r: cua_proto::daemon::v1::ForkSandboxBatchResponse) -> Option<Self> {
        use cua_proto::daemon::v1::fork_sandbox_batch_response::Event;
        Some(match r.event? {
            Event::Placement(p) => ForkBatchEvent::Placement {
                placement: ForkBatchPlacement {
                    group: p.group,
                    source: p.source,
                    host: p.host,
                    count: p.count,
                    attempt: p.attempt,
                    needs_sync: p.needs_sync,
                    reason: p.reason,
                },
            },
            Event::Sync(p) => ForkBatchEvent::Sync {
                progress: SnapshotSyncProgress {
                    name: p.name,
                    digest: p.digest,
                    from: p.from,
                    bytes_done: p.bytes_done,
                    bytes_total: p.bytes_total,
                    bytes_reused: p.bytes_reused,
                    done: p.done,
                    verified: p.verified,
                },
            },
            Event::Item(i) => ForkBatchEvent::Item {
                item: ForkBatchItem::of(i),
            },
            Event::Done(s) => ForkBatchEvent::Done {
                summary: ForkBatchSummary {
                    ok: s.ok,
                    failed: s.failed,
                    elapsed_ms: s.elapsed_ms,
                    per_host: s.per_host.into_iter().collect(),
                    peak_parallel: s.peak_parallel.into_iter().collect(),
                },
            },
        })
    }
}

/// A finished batch. Every fork and the summary ([`ForkBatch::collect`]).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ForkBatchResult {
    /// The forks, by index.
    pub items: Vec<ForkBatchItem>,
    /// The summary.
    pub summary: ForkBatchSummary,
}

enum BatchStream {
    #[cfg(feature = "spaces")]
    Embedded(
        tokio::sync::mpsc::Receiver<
            cua_daemon::Result<cua_proto::daemon::v1::ForkSandboxBatchResponse>,
        >,
    ),
    Daemon(Box<tonic::Streaming<cua_proto::daemon::v1::ForkSandboxBatchResponse>>),
    /// Every event was handed out.
    Done,
}

/// What `next` has handed out, so `collect` after a drain still answers.
#[derive(Default)]
struct Seen {
    items: Vec<ForkBatchItem>,
    summary: Option<ForkBatchSummary>,
}

/// A running batch of forks. Its events as they happen
/// ([`ForkBatch::next`]), or all of it ([`ForkBatch::collect`]).
#[derive(uniffi::Object)]
pub struct ForkBatch {
    stream: Arc<tokio::sync::Mutex<BatchStream>>,
    seen: Arc<std::sync::Mutex<Seen>>,
}

#[uniffi::export]
impl ForkBatch {
    /// The next event: a placement, a copy's progress, a fork that is ready
    /// (or failed), and last the summary. `None` after the summary.
    pub async fn next(&self) -> Result<Option<ForkBatchEvent>> {
        let stream = self.stream.clone();
        let seen = self.seen.clone();
        run(async move {
            let mut s = stream.lock().await;
            loop {
                let msg = match &mut *s {
                    #[cfg(feature = "spaces")]
                    BatchStream::Embedded(rx) => match rx.recv().await {
                        Some(r) => Some(r?),
                        None => None,
                    },
                    BatchStream::Daemon(st) => st.message().await?,
                    BatchStream::Done => return Ok(None),
                };
                let Some(msg) = msg else {
                    *s = BatchStream::Done;
                    return Ok(None);
                };
                if let Some(ev) = ForkBatchEvent::of(msg) {
                    {
                        let mut seen = seen.lock().unwrap_or_else(|p| p.into_inner());
                        match &ev {
                            ForkBatchEvent::Item { item } => seen.items.push(item.clone()),
                            ForkBatchEvent::Done { summary } => {
                                seen.summary = Some(summary.clone())
                            }
                            _ => {}
                        }
                    }
                    if matches!(ev, ForkBatchEvent::Done { .. }) {
                        *s = BatchStream::Done;
                    }
                    return Ok(Some(ev));
                }
            }
        })
        .await
    }

    /// Waits for the whole batch: every fork (ready or failed), by index,
    /// and the summary. After some or all of the events were read with
    /// [`ForkBatch::next`], it still returns the whole batch: the forks
    /// already read plus the rest, with the summary.
    pub async fn collect(&self) -> Result<ForkBatchResult> {
        while self.next().await?.is_some() {}
        let seen = self.seen.lock().unwrap_or_else(|p| p.into_inner());
        let Some(summary) = seen.summary.clone() else {
            return Err(CuaError::Internal(
                "the batch ended without its summary".into(),
            ));
        };
        let mut items = seen.items.clone();
        items.sort_by_key(|i| i.index);
        Ok(ForkBatchResult { items, summary })
    }
}

impl ForkManyOptions {
    fn request(&self) -> cua_proto::daemon::v1::ForkSandboxBatchRequest {
        cua_proto::daemon::v1::ForkSandboxBatchRequest {
            on: self.on.clone().unwrap_or_default(),
            group_size: self.group_size,
            max_parallel_per_host: self.max_parallel_per_host,
            name_prefix: self.name_prefix.clone(),
            cpus: self.cpus,
            memory_mb: self.memory_mb,
            retries: self.retries,
            no_retry: self.no_retry,
            fallback: self.fallback.as_ref().map(SnapshotFallbackOptions::to_pb),
            ..Default::default()
        }
    }
}

/// Batched forks.
#[uniffi::export]
impl Sandboxes {
    /// Forks checkpoint `source` `count` times at once and returns the
    /// running batch: each fork is reported as soon as it is ready (or
    /// failed: one failure does not stop the others). `source` may also be
    /// a local sandbox, running or stopped: it is checkpointed once (memory
    /// saved once, shared copy-on-write by the forks on QEMU) and that
    /// checkpoint is deleted with the last of its forks. With `on = "auto"`
    /// the forks go to your machines that provide Spaces: each group
    /// (`group_size` forks) to one machine, groups spread over the least
    /// loaded ones, the checkpoint copied (and verified) to a machine that
    /// lacks it first; a machine that fails has its forks placed again on
    /// another.
    pub async fn fork_many(
        &self,
        source: String,
        count: u32,
        options: Option<ForkManyOptions>,
    ) -> Result<Arc<ForkBatch>> {
        let mut req = options.unwrap_or_default().request();
        req.source = source;
        req.count = count;
        self.start_batch(req).await
    }

    /// [`Sandboxes::fork_many`] for several checkpoints at once: each group
    /// (the forks of one checkpoint that belong together) goes to one
    /// machine, and groups of different checkpoints spread.
    pub async fn fork_groups(
        &self,
        groups: Vec<ForkGroup>,
        options: Option<ForkManyOptions>,
    ) -> Result<Arc<ForkBatch>> {
        let mut req = options.unwrap_or_default().request();
        req.groups = groups
            .into_iter()
            .map(|g| cua_proto::daemon::v1::ForkBatchGroup {
                source: g.source,
                count: g.count,
                name_prefix: g.name_prefix,
            })
            .collect();
        self.start_batch(req).await
    }

    /// Uploads checkpoint `name` to `fallback` (S3, or a folder every
    /// machine mounts), so batches on your other machines copy it from
    /// there when this machine cannot be reached. Files already there are
    /// not uploaded again. Returns the bytes uploaded.
    pub async fn publish_checkpoint(
        &self,
        name: String,
        fallback: SnapshotFallbackOptions,
    ) -> Result<u64> {
        let backend = self.backend.clone();
        run(async move {
            let fb = fallback.to_pb();
            match &backend {
                #[cfg(feature = "spaces")]
                Backend::Embedded(rt) => Ok(rt.publish_checkpoint(&name, &fb).await?),
                #[cfg(not(feature = "spaces"))]
                Backend::Embedded(_) => Err(CuaError::Unsupported(
                    "publishing checkpoints needs the Spaces build of the SDK".into(),
                )),
                Backend::Daemon(d) => Ok(d.publish_checkpoint(&name, fb).await?),
            }
        })
        .await
    }
}

impl Sandboxes {
    async fn start_batch(
        &self,
        req: cua_proto::daemon::v1::ForkSandboxBatchRequest,
    ) -> Result<Arc<ForkBatch>> {
        let backend = self.backend.clone();
        let stream = run(async move {
            Ok(match &backend {
                #[cfg(feature = "spaces")]
                Backend::Embedded(rt) => BatchStream::Embedded(rt.fork_batch(req).await?),
                #[cfg(not(feature = "spaces"))]
                Backend::Embedded(_) => {
                    let _ = req;
                    return Err(CuaError::Unsupported(
                        "batched forks need the Spaces build of the SDK".into(),
                    ));
                }
                Backend::Daemon(d) => BatchStream::Daemon(Box::new(d.fork_batch(req).await?)),
            })
        })
        .await?;
        Ok(Arc::new(ForkBatch {
            seen: Arc::default(),
            stream: Arc::new(tokio::sync::Mutex::new(stream)),
        }))
    }
}
