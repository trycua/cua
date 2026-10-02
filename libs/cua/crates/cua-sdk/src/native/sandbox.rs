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

/// Portable lifecycle phase, the same words local and in the cloud.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SandboxPhase {
    /// Being created (cloud: capacity is being provisioned).
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
    /// an existing machine, or a cloud sandbox this client did not create).
    pub kind: String,
    /// The engine that runs it: `gvisor`, `runc`, `qemu`, `lume` or
    /// `kubevirt` (empty when not known).
    pub runtime: String,
    /// `runtime_type` as persisted (`fleet`, `direct`, `gvisor`, `qemu`,
    /// ...); prefer `location`, `kind` and `runtime`.
    pub runtime_type: String,
    /// Status.
    pub status: SandboxStatus,
    /// Provider's word for an unknown status.
    pub status_detail: Option<String>,
    /// No state file; torn down on delete.
    pub ephemeral: bool,
    /// Declared services: name → guest port (0 = only the name is known).
    pub services: HashMap<String, u16>,
    /// Service base URLs by name (Fleet: gateway URLs that need the Fleet
    /// bearer).
    pub endpoints: HashMap<String, String>,
    /// Image, when known.
    pub image: Option<String>,
    /// Qualified ref, the same kind of value local and in the cloud:
    /// `local:<name>`, `cloud:<name>` or `direct:<host:port>`. Every call
    /// that takes a sandbox name accepts it.
    pub id: String,
    /// Portable phase: provisioning, starting, ready or stopped.
    pub phase: SandboxPhase,
    /// Where it runs: `local`, `cloud`, `direct` (a machine by address) or
    /// `relay`.
    pub location: String,
    /// When it expires unless kept alive (unix seconds), when known.
    pub expires_at_unix: Option<i64>,
    /// Provider internals (cloud: `pool`, `namespace`, `claim`; local:
    /// `backend`, `container_id`). Not part of the portable API.
    pub provider_details: HashMap<String, String>,
    /// The image as resolved and pinned at create time (digest, variant).
    /// `None` for direct connections, named pools, images not resolved from
    /// a registry, and the daemon topology.
    #[uniffi(default = None)]
    pub image_info: Option<ImageInfo>,
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

/// Advanced options for cloud sandboxes (warm capacity, limits, TTL, a
/// dedicated pool). Leave unset for the defaults. Setting them with `on`
/// unset means the cloud; with `on = "local"` they are an error.
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct CloudOptions {
    /// Keep one warm replica of this image ready (faster next start).
    #[uniffi(default = None)]
    pub warm: Option<bool>,
    /// Most sandboxes of this image at once (default 10).
    #[uniffi(default = None)]
    pub max_pool_size: Option<u32>,
    /// Seconds the sandbox outlives this process without a keep-alive
    /// (renewed while held; default 15 min).
    #[uniffi(default = None)]
    pub claim_ttl_seconds: Option<u32>,
    /// Dedicated capacity: claim from this existing pool. The sandbox
    /// fields given alongside (image, command, env, services, sidecars,
    /// cpus, memory) must match its template, or creation fails with
    /// `PoolSpecMismatch` and a diff.
    #[uniffi(default = None)]
    pub pool: Option<String>,
    /// With `pool`: update the pool's template to the given fields instead
    /// of failing on a mismatch (`Fleet.apply` semantics for the template;
    /// the pool's capacity is kept). Managed pools refuse it.
    #[uniffi(default = false)]
    pub apply: bool,
}

/// An extra container next to a sandbox (a compose-style sidecar),
/// addressed by name on every runtime: the sandbox reaches it at its
/// `name` (on its `ports`), it reaches the sandbox at `main`, and
/// `services` may name its ports (`{"db": 5432}`). Local containers run it
/// in the sandbox's network namespace (`localhost` works too; needs
/// `runtime="runc"`); cloud gVisor sandboxes run it in the same pod; cloud
/// VM (KubeVirt) sandboxes run it in a companion pod named in the guest's
/// `/etc/hosts`. Local VM sandboxes refuse sidecars. With sidecars the
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
    pub(crate) fn from_core(s: cua_sandbox_core::Sidecar) -> Self {
        Self {
            image: s.image,
            command: s.command,
            env: s.env.into_iter().collect(),
            ports: s.ports,
            name: Some(s.name),
        }
    }

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

/// Credentials for a private registry image. Local sandboxes pull with
/// them; cloud sandboxes store them as the sandbox's registry pull Secret.
/// Never logged or persisted.
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

/// Image layers built on top of a sandbox's image. Cloud: a remote build
/// (Fleet images API). Local: a build into the container engine (container
/// images only; VM images take no layers here). Both are cached by the same
/// content hash (`cua-b-<hash>`), so an identical spec never rebuilds.
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
    /// Where it runs: `local`, `cloud`, `direct:<addr>` (an existing
    /// machine running cua-spacesd) or a registered provider. `None` (the
    /// default): the cloud when `cloud` options (or the deprecated flat
    /// cloud fields) are set, else the user default (`default.on` in
    /// `~/.cua/config.toml`, `CUA_DEFAULT_ON`, else `local`). `local`
    /// together with `cloud` is an `InvalidArgument`.
    #[uniffi(default = None)]
    pub on: Option<String>,
    /// What kind of machine: `auto`, `container` or `vm`. `None`: the user
    /// default (`default.kind`, else `auto`: macOS and Windows images are
    /// VMs, an image with a container rootfs is a container, a disk-only
    /// image is a VM).
    #[uniffi(default = None)]
    pub kind: Option<String>,
    /// Which engine: `auto` or one the location offers for the kind (local
    /// `gvisor`, `runc`, `qemu`, `lume`; cloud `gvisor`, `kubevirt`). `None`:
    /// the user default (`default.runtime`, else `auto`, the safest
    /// available). A local sandbox with sidecars needs `runc` where gVisor
    /// would run. A combination that does not exist is `InvalidPlacement`,
    /// listing the valid values.
    #[uniffi(default = None)]
    pub runtime: Option<String>,
    /// Image reference. Locally, `pool:<name>` runs that cloud pool's
    /// template image.
    #[uniffi(default = "")]
    pub image: String,
    /// Name. `None`: ephemeral (cloud: released with the sandbox; the
    /// managed capacity stays for reuse).
    #[uniffi(default = None)]
    pub name: Option<String>,
    /// spacesd token.
    #[uniffi(default = None)]
    pub token: Option<String>,
    /// Fleet: claim from this existing (user-owned) pool. Unset: a managed,
    /// autoscaled pool keyed by image and shape. Local (with an empty
    /// image): run this Fleet pool's template locally.
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
    /// Extra guest ports (local: published; Fleet: `port-<n>` services).
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
    /// Guest environment (local: container environment or cloud-init;
    /// cloud: the template environment with `processMode: Run`, on gVisor
    /// and KubeVirt; values are not secrets).
    #[uniffi(default)]
    pub env: HashMap<String, String>,
    /// Deprecated: initial replicas of a new managed pool (> 0 = warm).
    #[uniffi(default = None)]
    pub fleet_replicas: Option<u32>,
    /// Deprecated: use `cloud.claim_ttl_seconds`.
    #[uniffi(default = None)]
    pub fleet_ttl_seconds: Option<u32>,
    /// Deprecated: use `cloud.warm`.
    #[uniffi(default = None)]
    pub warm: Option<bool>,
    /// Deprecated: use `cloud.max_pool_size`.
    #[uniffi(default = None)]
    pub max_pool_size: Option<u32>,
    /// The sandbox's command (argv), replacing the image's entrypoint.
    /// Same meaning local (container ENTRYPOINT, or run by cloud-init in a
    /// Linux VM) and in the cloud (the template command with
    /// `processMode: Run`, on gVisor and KubeVirt).
    #[uniffi(default = None)]
    pub command: Option<Vec<String>>,
    /// Advanced cloud options (warm capacity, limits, TTL, runtime, a
    /// dedicated pool). They win over the deprecated flat fields.
    #[uniffi(default = None)]
    pub cloud: Option<CloudOptions>,
    /// Sidecar containers, addressed by name (see [`Container`]).
    #[uniffi(default = [])]
    pub sidecars: Vec<Container>,
    /// Credentials for a private `image` (and sidecar images on its
    /// registry).
    #[uniffi(default = None)]
    pub registry_secret: Option<RegistrySecret>,
    /// Image layers built on top of `image` (cloud: a remote build; local:
    /// a build into the container engine; both cached by content).
    #[uniffi(default = None)]
    pub build: Option<ImageBuild>,
    /// Guest network: `"default"` (unset) gives outbound network, like a
    /// Docker container; `"none"` cuts it while published ports still work.
    /// Only local QEMU VMs honour `"none"`; containers, Lume and cloud
    /// sandboxes reject it.
    #[uniffi(default = None)]
    pub network: Option<String>,
    /// Binaries to inject once the sandbox is up, so tests run the build
    /// under test and not the copy the image bundles (see [`Overlay`]; the
    /// same as calling [`Sandbox::overlay`] after create). Needs
    /// cua-spacesd in the image, or a local container.
    #[uniffi(default = [])]
    pub overlays: Vec<super::Overlay>,
    /// Keep a named sandbox whose readiness check fails (its cloud claim,
    /// VM or container, and its record) to debug it. `false` (the
    /// default): a failed create deletes what it made and releases the
    /// claim. Ephemeral sandboxes are always deleted.
    #[uniffi(default = false)]
    pub keep_on_failure: bool,
    /// A GPU option of the runtime it runs on (`Spaces.gpu_support` lists them):
    /// `paravirtual` (a macOS VM on Lume: GPU acceleration, experimental),
    /// `virgl` (QEMU on Linux), `nvidia` (a runc container on Linux), a
    /// contrib provider's GPU type; `auto` picks the runtime's own. `None`
    /// (the default): no GPU.
    #[uniffi(default = None)]
    pub gpu: Option<String>,
}

impl Default for SandboxCreateOptions {
    fn default() -> Self {
        Self::auto("")
    }
}

impl SandboxCreateOptions {
    /// Options for `image` where `on` says (`local`, `cloud`,
    /// `direct:<addr>`), everything else default.
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
            pool: None,
            os: None,
            cpus: None,
            memory_mb: None,
            ports: vec![],
            services: HashMap::new(),
            wait_for: vec![],
            ready_timeout_ms: None,
            env: HashMap::new(),
            fleet_replicas: None,
            fleet_ttl_seconds: None,
            warm: None,
            max_pool_size: None,
            command: None,
            cloud: None,
            sidecars: vec![],
            registry_secret: None,
            build: None,
            network: None,
            overlays: vec![],
            keep_on_failure: false,
            gpu: None,
        }
    }

    fn cloud_options(&self) -> bool {
        self.cloud.is_some()
            || self.fleet_replicas.is_some()
            || self.fleet_ttl_seconds.is_some()
            || self.warm.is_some()
            || self.max_pool_size.is_some()
    }

    /// The location, kind and runtime this create runs with, each with
    /// where it came from: explicit values, else the cloud when cloud
    /// options are set, else the user defaults
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
        let mut on = text(&self.on)
            .map(|o| On::parse(&o))
            .transpose()
            .map_err(place)?;
        if on.is_none() && self.cloud_options() {
            on = Some(On::Cloud);
        }
        if on == Some(On::Local) && self.cloud.is_some() {
            return Err(CuaError::InvalidArgument(
                "cloud options were passed for a local sandbox: drop `cloud`, or run it in the \
                 cloud (on=\"cloud\" / local=False)"
                    .into(),
            ));
        }
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
        cua_sandbox_core::placement::validate(&r.on, r.kind, &r.runtime).map_err(place)?;
        Ok(r)
    }

    fn to_request(&self) -> Result<CreateRequest> {
        use cua_sandbox_core::placement::On;
        let placement = self.placement()?;
        let cloud = self.cloud.clone().unwrap_or_default();
        let (provider, url) = match &placement.on {
            On::Local => (Some(CoreProvider::Local), None),
            On::Cloud => (Some(CoreProvider::Fleet), None),
            On::Direct(a) => (Some(CoreProvider::Direct), Some(a.clone())),
            On::Provider(_) => (Some(CoreProvider::Contrib), None),
            On::Relay(_) | On::Host(_) => (None, None),
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
            pool: cloud.pool.or_else(|| self.pool.clone()),
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
            fleet_runtime: None,
            fleet_replicas: self.fleet_replicas,
            fleet_ttl_seconds: cloud.claim_ttl_seconds.or(self.fleet_ttl_seconds),
            fleet_warm: cloud.warm.or(self.warm),
            fleet_max_pool_size: cloud.max_pool_size.or(self.max_pool_size),
            fleet_apply: cloud.apply,
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
        })
    }
}

/// A sandbox listing and what it left out.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SandboxListing {
    /// Sandboxes, each with its `location`.
    pub sandboxes: Vec<SandboxInfo>,
    /// Sources left out, for example "cloud sandboxes not listed: ...".
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
    /// Provider internals (cloud: namespace, claim; local: upstream).
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
    /// rcdp wire v2 unchanged; no Fleet bearer or env token is exposed.
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
    async fn create_bare(&self, options: SandboxCreateOptions) -> Result<Arc<Sandbox>> {
        let placement = options.placement()?;
        let mut req = options.to_request()?;
        let backend = self.backend.clone();
        let secret = options.registry_secret.clone();
        let rec = run(async move {
            // Credentials are read here (this process's environment and AWS
            // CLI), then travel only to the runtime that uses them.
            if let Some(s) = secret {
                req.registry_credentials = Some(s.credentials(&req.image).await?);
            }
            Ok(match &backend {
                Backend::Embedded(rt) => rt.create(req).await?,
                Backend::Daemon(d) => d.create(req).await?,
            })
        })
        .await
        .map_err(|e| match e {
            // The cloud came from a default: say how to sign in or go back.
            CuaError::ProviderNotConfigured(m) => {
                match cua_sandbox_core::settings::cloud_default_hint(&placement.on_source) {
                    Some(hint) => CuaError::ProviderNotConfigured(format!("{m}; {hint}")),
                    None => CuaError::ProviderNotConfigured(m),
                }
            }
            other => other,
        })?;
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
    /// Creates (Fleet, local) or connects (direct) a sandbox and waits for
    /// the provider and every readiness probe.
    pub async fn create(&self, options: SandboxCreateOptions) -> Result<Arc<Sandbox>> {
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
        let r = self.create_recorded(options).await;
        facts.record(&r);
        r
    }
}

impl Sandboxes {
    async fn create_recorded(&self, options: SandboxCreateOptions) -> Result<Arc<Sandbox>> {
        let overlays = options.overlays.clone();
        for o in &overlays {
            o.validate()?;
        }
        let ready_timeout_ms = options.ready_timeout_ms;
        let sandbox = self.create_bare(options).await?;
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
    /// `host:port`, a Fleet service URL or a relay URL) with an optional
    /// spacesd token. No daemon is assumed and nothing is probed.
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
    /// all of them, local, direct and the account's live cloud sandboxes,
    /// whatever the default location is. `"local"`, `"cloud"` or
    /// `"direct"`: only those.
    /// Cloud sandboxes never fail the listing: without Fleet credentials
    /// they are left out silently, and when Fleet fails or takes longer than
    /// 5 s they are left out with a warning (logged here;
    /// [`Sandboxes::list_with_warnings`] returns it).
    pub async fn list(&self, location: Option<String>) -> Result<Vec<SandboxInfo>> {
        let listing = self.list_with_warnings(location).await?;
        for w in &listing.warnings {
            tracing::warn!("{w}");
        }
        Ok(listing.sandboxes)
    }

    /// [`Sandboxes::list`] with what the listing left out (for example
    /// "cloud sandboxes not listed: <reason>").
    pub async fn list_with_warnings(&self, location: Option<String>) -> Result<SandboxListing> {
        let backend = self.backend.clone();
        let p = location_filter(location.as_deref())?;
        let listing = run(async move {
            Ok(match &backend {
                Backend::Embedded(rt) => rt.list_filtered(p, true).await?,
                Backend::Daemon(d) => d.list_filtered(p, true).await?,
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
    /// (an image download, a boot, a claim) and what the create made is
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

impl Sandboxes {
    /// The sandboxes this machine knows (state files, local backends,
    /// connected URLs, Fleet claims it holds) of `provider` (all when
    /// `None`), without asking Fleet for live claims. Rust only; the CLI's
    /// `cua do ls` uses it.
    pub async fn list_known(&self, location: Option<&str>) -> Result<Vec<SandboxInfo>> {
        let backend = self.backend.clone();
        let p = location_filter(location)?;
        let listing = run(async move {
            Ok(match &backend {
                Backend::Embedded(rt) => rt.list_filtered(p, false).await?,
                Backend::Daemon(d) => d.list_filtered(p, false).await?,
            })
        })
        .await?;
        Ok(listing.sandboxes.into_iter().map(Into::into).collect())
    }
}

/// A `location` filter (`local`, `cloud`, `direct`; empty or `None`: all).
fn location_filter(location: Option<&str>) -> Result<Option<CoreProvider>> {
    let Some(l) = location.map(str::trim).filter(|l| !l.is_empty()) else {
        return Ok(None);
    };
    match cua_sandbox_core::Location::parse(l) {
        Some(cua_sandbox_core::Location::Local) => Ok(Some(CoreProvider::Local)),
        Some(cua_sandbox_core::Location::Cloud) => Ok(Some(CoreProvider::Fleet)),
        Some(cua_sandbox_core::Location::Direct) => Ok(Some(CoreProvider::Direct)),
        _ => Err(CuaError::InvalidArgument(format!(
            "location {l:?}: expected local, cloud or direct"
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
    /// connections, claims on a named pool, images not resolved from a
    /// registry, and the daemon topology.
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
        let timeout = probe_timeout_ms.map(super::millis);
        run(async move {
            match &backend {
                Backend::Embedded(rt) => {
                    let a = rt.env(&name, timeout).await?;
                    let fleet = !a.ws_headers.is_empty();
                    let client = SpacesdClient::new(a.client, a.ws_headers);
                    if !fleet {
                        return Ok(Arc::new(client));
                    }
                    // A cloud Space: the Fleet bearer in the headers expires
                    // within minutes, so each socket re-reads the attachment,
                    // which re-mints it (cached until it expires).
                    let (rt, name) = (rt.clone(), name.clone());
                    let refresh: super::spacesd::WsHeaderRefresh = Arc::new(move || {
                        let (rt, name) = (rt.clone(), name.clone());
                        Box::pin(async move { Ok(rt.env(&name, timeout).await?.ws_headers) })
                    });
                    Ok(Arc::new(client.with_ws_refresh(refresh)))
                }
                Backend::Daemon(d) => {
                    let ep = d.env_endpoint(&name, timeout).await?;
                    let mut o = cua_spacesd_client::ConnectOptions::parse(&ep.url)?
                        .transport(cua_spacesd_client::TransportPreference::Native)
                        .probe(false);
                    o.token = Some(ep.token.clone());
                    let client = cua_spacesd_client::SpacesdClient::connect(o).await?;
                    Ok(Arc::new(SpacesdClient::new(
                        client,
                        vec![("authorization".into(), format!("Bearer {}", ep.token))],
                    )))
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
        super::Agents::over(guest.client.clone()).await
    }

    /// A named service (for example `"server"` or `"env"`).
    pub fn service(&self, name: String) -> Result<Arc<Service>> {
        if !self.info.services.contains_key(&name) && !self.info.endpoints.contains_key(&name) {
            // Fleet only knows service names; direct/local accept declared
            // services only. Let the backend decide when unknown locally.
            if self.info.location != "cloud" && !self.info.services.is_empty() {
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
    /// any MCP client. Embedded: the service route itself (loopback, or the
    /// Fleet gateway with a fresh bearer and claim header). Daemon: the
    /// daemon's streaming passthrough with its loopback bearer.
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
    /// (60 s to 24 h, default 1 h). Cloud: a Fleet signed service URL.
    /// Local: a loopback URL with its own token, served by the cua daemon
    /// (started if needed; `CUA_BIN` names the CLI).
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
    /// over cua-spacesd's tunnel when the image has it); in the cloud
    /// without cua-spacesd, a loopback HTTP/WebSocket proxy through the
    /// Fleet gateway. Either way `url()` is a loopback URL.
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
                backend,
                info,
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

    /// Extends a Fleet lease by `seconds`.
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

    /// Managed Fleet claims: stop renewing the claim and forget the handle;
    /// the claim runs until its current shutdown time. Embedded runtime
    /// only (a daemon holds its claims by design).
    pub fn detach(&self) -> Result<()> {
        match &self.backend {
            Backend::Embedded(rt) => {
                let _guard = super::runtime().enter();
                rt.detach(&self.info.id);
                Ok(())
            }
            Backend::Daemon(_) => Err(CuaError::Unsupported(
                "detach: the cua daemon holds Fleet claims; use delete or keep_alive".into(),
            )),
        }
    }

    /// Deletes (Fleet: releases the claim and an ephemeral pool).
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
    /// (no trailing slash): the published loopback port locally, a signed
    /// service URL (1 h, renewed on later calls) in the cloud.
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
    /// `Sandbox.public_url(name, ...)`. Cloud: a Fleet signed service URL.
    /// Local: a loopback URL with its own token, served by the cua daemon.
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

    /// One HTTP request to `path` on the service. Credentials (Fleet bearer
    /// and claim) are attached by the runtime. `headers` are sent as given
    /// (for example `content-type`, or `accept` and `mcp-session-id` for
    /// MCP over Fleet); `authorization` is refused on Fleet, where the
    /// gateway bearer owns it.
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
    backend: Backend,
    info: cua_daemon::ForwardInfo,
    closed: std::sync::atomic::AtomicBool,
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

    /// URL to use (loopback URL, or the Fleet gateway URL).
    pub fn url(&self) -> Option<String> {
        self.info.url.clone()
    }

    /// Stops the forward.
    pub async fn close(&self) -> Result<()> {
        if self.closed.swap(true, std::sync::atomic::Ordering::SeqCst) || self.info.id.is_empty() {
            return Ok(());
        }
        let backend = self.backend.clone();
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
        if self.closed.swap(true, std::sync::atomic::Ordering::SeqCst) || self.info.id.is_empty() {
            return;
        }
        let backend = self.backend.clone();
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
    /// configured); cloud options imply the cloud; `local` plus cloud
    /// options is an error. Runs with a private CUA_HOME and no default
    /// overrides in the environment.
    #[test]
    fn the_location_defaults_and_cloud_options_imply_the_cloud() {
        use cua_sandbox_core::placement::{Kind, On, Runtime};
        let _env = super::super::test_env::EnvGuard::isolated();
        let mut o = SandboxCreateOptions::auto("python:3.12-slim");
        assert_eq!(o.placement().unwrap().on, On::Local);
        assert_eq!(SandboxCreateOptions::default().on, None);
        let r = o.to_request().unwrap();
        assert_eq!(r.location.as_deref(), Some("local"));
        assert_eq!(r.provider, Some(CoreProvider::Local));
        // Cloud options imply the cloud.
        o.cloud = Some(CloudOptions::default());
        assert_eq!(o.placement().unwrap().on, On::Cloud);
        assert_eq!(o.to_request().unwrap().provider, Some(CoreProvider::Fleet));
        // ...and contradict an explicit local.
        o.on = Some("local".into());
        assert!(matches!(o.placement(), Err(CuaError::InvalidArgument(_))));
        // A deprecated flat cloud field implies the cloud too.
        let mut o = SandboxCreateOptions::auto("x");
        o.warm = Some(true);
        assert_eq!(o.placement().unwrap().on, On::Cloud);
        // direct:<addr> carries the address.
        let o = SandboxCreateOptions::new("direct:127.0.0.1:3211", "");
        let r = o.to_request().unwrap();
        assert_eq!(r.url.as_deref(), Some("127.0.0.1:3211"));
        assert_eq!(r.location.as_deref(), Some("direct:127.0.0.1:3211"));
        // kind and runtime go out resolved.
        let mut o = SandboxCreateOptions::new("cloud", "img");
        o.kind = Some("vm".into());
        let p = o.placement().unwrap();
        assert_eq!((p.kind, p.runtime), (Kind::Vm, Runtime::Auto));
        assert_eq!(o.to_request().unwrap().kind.as_deref(), Some("vm"));
    }

    #[test]
    fn invalid_placements_are_typed_and_list_the_valid_values() {
        let _env = super::super::test_env::EnvGuard::isolated();
        let mut o = SandboxCreateOptions::new("cloud", "img");
        o.runtime = Some("qemu".into());
        let e = o.placement().unwrap_err();
        let CuaError::InvalidPlacement(m) = &e else {
            panic!("{e:?}")
        };
        assert!(m.ends_with("valid runtime: auto, gvisor, kubevirt"), "{m}");
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
            "cloud",
        )
        .unwrap();
        let o = SandboxCreateOptions::auto("img");
        let p = o.placement().unwrap();
        assert_eq!(p.on, On::Cloud);
        assert!(matches!(
            p.on_source,
            cua_sandbox_core::settings::Source::Config(_)
        ));
        env.set("CUA_DEFAULT_ON", "local");
        assert_eq!(o.placement().unwrap().on, On::Local);
        let o = SandboxCreateOptions::new("cloud", "img");
        assert_eq!(o.placement().unwrap().on, On::Cloud);
    }

    #[test]
    fn on_words_pick_the_provider_and_the_location() {
        let mut o = SandboxCreateOptions::auto("ghcr.io/trycua/linux:24.04");
        o.on = Some("e2b".into());
        let r = o.to_request().unwrap();
        assert_eq!(r.provider, Some(CoreProvider::Contrib));
        assert_eq!(r.location.as_deref(), Some("e2b"));
        o.on = Some("cloud".into());
        assert_eq!(o.to_request().unwrap().provider, Some(CoreProvider::Fleet));
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
    fn cloud_options_win_over_the_deprecated_flat_fields() {
        let mut o = SandboxCreateOptions::new("cloud", "python:3.12-slim");
        o.warm = Some(false);
        o.max_pool_size = Some(2);
        o.command = Some(vec!["python".into(), "/srv.py".into()]);
        o.services.insert("mcp".into(), 8765);
        o.wait_for = vec![ReadinessProbe::tcp("mcp")];
        let r = o.to_request().unwrap();
        assert_eq!(r.fleet_warm, Some(false));
        assert_eq!(r.fleet_max_pool_size, Some(2));
        assert_eq!(r.command, o.command);
        assert_eq!(r.wait_for, vec![Probe::Tcp(8765)]);
        o.cloud = Some(CloudOptions {
            warm: Some(true),
            max_pool_size: Some(5),
            claim_ttl_seconds: Some(600),
            pool: None,
            apply: true,
        });
        o.runtime = Some("gvisor".into());
        let r = o.to_request().unwrap();
        assert!(r.fleet_apply);
        assert_eq!(
            (r.fleet_warm, r.fleet_max_pool_size, r.fleet_ttl_seconds),
            (Some(true), Some(5), Some(600))
        );
        assert_eq!(r.runtime.as_deref(), Some("gvisor"));
        // An empty argv is no command.
        o.command = Some(vec![]);
        assert_eq!(o.to_request().unwrap().command, None);
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
/// clouds (`aws`, `gcp`). A word is listed whether or not this build
/// includes that provider (see [`contrib_providers_built`]).
#[uniffi::export]
pub fn sandbox_locations() -> Vec<String> {
    let mut out: Vec<String> = ["local", "cloud"]
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
/// (`space://fleet/<ns>/<claim>`, `fleet:<ns>:<claim>`, `url:<addr>`, a
/// URL), or a bare name.
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
