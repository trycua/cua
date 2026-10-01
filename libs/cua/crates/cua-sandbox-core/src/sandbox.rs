//! [`Sandboxes`] (the provider-dispatching manager) and [`Sandbox`].

use crate::{
    Error, Result,
    http::{HttpClient, HttpResponse, RequestBody, ServiceEndpoint, StreamingResponse},
    runtime::{
        ImageInfo, InstanceStatus, LocalEndpoints, LocalInstance, LocalRuntime, LocalStartSpec,
    },
    state::{LocalState, SandboxState, StateStore, python_utc_now, registry_image_dict},
};
use cua_fleet::{
    AcquireOpts, BoundSandbox, ClaimOptions, FleetClient, ManagedClaim, PoolManager, PoolSpecKey,
    RuntimeKind,
};
pub use cua_fleet::{BuildFile, BuildSpec, ImageLayer, RegistryCredentials, Sidecar};
use cua_spacesd_client::{ConnectOptions, SpacesdClient};

use crate::placement::{self, Kind, On, Runtime};
use crate::refs::SandboxRef;

mod contrib;

/// The guest port of `service` in `services`.
pub(crate) fn service_port(services: &BTreeMap<String, u16>, service: &str) -> Result<u16> {
    match services.get(service) {
        Some(p) if *p > 0 => Ok(*p),
        _ => Err(Error::InvalidArgument(format!(
            "no service {service:?} with a known port (declared: {:?})",
            services.keys().collect::<Vec<_>>()
        ))),
    }
}
use serde_json::{Map, Value};
use std::{
    collections::BTreeMap,
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::net::{TcpListener, TcpStream};

/// How long [`Sandboxes::create`] waits, at most, for the cua-spacesd of an
/// image that declares one to answer `Health` (within `ready_timeout`) when
/// the caller did not choose the budget ([`CreateOptions::ready_timeout_given`]);
/// a chosen `ready_timeout` governs that wait too.
pub const SPACESD_READY_TIMEOUT: Duration = Duration::from_secs(120);

/// Default spacesd port.
pub const ENV_PORT: u16 = 3211;

/// Which provider backs a sandbox.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProviderKind {
    /// Fleet pool/claim.
    Fleet,
    /// Local VM or container runtime.
    Local,
    /// Direct URL.
    Direct,
    /// A third-party platform ([`crate::Provider`]); `runtime_type` names
    /// which (`e2b`, `daytona`, ...).
    Contrib,
}

/// Coarse lifecycle status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Running / bound.
    Running,
    /// Paused or scaled to zero.
    Suspended,
    /// Stopped.
    Stopped,
    /// Starting.
    Provisioning,
    /// Unknown (with the provider's word for it).
    Unknown(String),
}

impl From<InstanceStatus> for Status {
    fn from(s: InstanceStatus) -> Self {
        match s {
            InstanceStatus::Running => Status::Running,
            InstanceStatus::Paused => Status::Suspended,
            InstanceStatus::Stopped => Status::Stopped,
            InstanceStatus::Provisioning => Status::Provisioning,
            InstanceStatus::Unknown(u) => Status::Unknown(u),
        }
    }
}

/// A user-declared readiness probe. None is implied: with no probes, a
/// sandbox is ready as soon as its provider reports it running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Probe {
    /// A TCP connect to this guest port succeeds (through the Fleet
    /// gateway: the port's service answers any HTTP status below 500).
    Tcp(u16),
    /// `GET path` on this guest port returns 2xx (or `status` when set).
    Http {
        /// Guest port.
        port: u16,
        /// Path.
        path: String,
        /// Exact status to wait for.
        status: Option<u16>,
    },
}

/// How long a local resume waits for the probes its create waited for.
pub const RESUME_READY_TIMEOUT: Duration = Duration::from_secs(180);

/// Probes as the state file keeps them.
fn probes_to_json(probes: &[Probe]) -> Value {
    Value::Array(
        probes
            .iter()
            .map(|p| match p {
                Probe::Tcp(port) => serde_json::json!({"tcp": port}),
                Probe::Http { port, path, status } => {
                    serde_json::json!({"http": {"port": port, "path": path, "status": status}})
                }
            })
            .collect(),
    )
}

/// [`probes_to_json`] back; entries it does not know are skipped.
fn probes_from_json(v: Value) -> Vec<Probe> {
    let port = |v: &Value| v.as_u64().and_then(|p| u16::try_from(p).ok());
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|p| {
                    if let Some(t) = p.get("tcp").and_then(port) {
                        return Some(Probe::Tcp(t));
                    }
                    let h = p.get("http")?;
                    Some(Probe::Http {
                        port: h.get("port").and_then(port)?,
                        path: h.get("path")?.as_str()?.to_string(),
                        status: h.get("status").and_then(port),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Fleet-specific create options.
///
/// Without `pool`, the sandbox is claimed from a managed, autoscaled pool
/// keyed by image and shape ([`cua_fleet::PoolManager`]): pools are reused,
/// never deleted with the sandbox, and garbage-collected when idle.
#[derive(Clone, Debug, Default)]
pub struct FleetOptions {
    /// Claim from this existing (user-owned) pool instead of a managed one.
    pub pool: Option<String>,
    /// Runtime of the managed pool. `None` defaults from the image's
    /// registry manifest (a containerDisk boots on KubeVirt, a container
    /// rootfs runs on gVisor); an explicit runtime must match it
    /// (`cua_fleet::resolve_runtime`).
    pub runtime: Option<RuntimeKind>,
    /// Start a new managed pool with one warm replica (default: the
    /// manager's `CUA_FLEET_WARM`). Ignored for existing pools, whose
    /// replicas KEDA owns.
    pub warm: Option<bool>,
    /// Autoscaling ceiling of the managed pool.
    pub max_pool_size: Option<u32>,
    /// Deprecated: initial replicas of a new managed pool (`> 0` means
    /// warm).
    pub replicas: Option<u32>,
    /// Claim TTL in seconds (managed pools: renewed by a heartbeat while the
    /// handle lives; default `CUA_FLEET_CLAIM_TTL`, 15 min).
    pub ttl_seconds_after_created: Option<u32>,
    /// With `pool`: update the pool's template to the sandbox fields given
    /// here (command, env, sidecars, image, ...) instead of refusing a pool
    /// whose template differs ([`cua_fleet::Error::PoolSpecMismatch`]).
    pub apply: bool,
    /// `cpus` was given explicitly (compared against a named pool).
    pub cpus_given: bool,
    /// `memory_mb` was given explicitly (compared against a named pool).
    pub memory_given: bool,
}

/// Guest network of a sandbox.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum NetworkMode {
    /// Outbound network (the default, like a Docker container). Local QEMU
    /// guests use user-mode NAT with loopback-only port forwards.
    #[default]
    Default,
    /// No outbound network: local QEMU guests only (`restrict=on`); the
    /// published ports still reach the guest. Containers, Lume and cloud
    /// sandboxes refuse it ([`Error::Unsupported`]).
    None,
}

impl NetworkMode {
    /// Parses `default` / `none` (case-insensitive; empty = `default`).
    pub fn parse(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "default" => Ok(Self::Default),
            "none" => Ok(Self::None),
            other => Err(Error::InvalidArgument(format!(
                "network {other:?}: expected \"default\" or \"none\""
            ))),
        }
    }

    /// The spelling [`NetworkMode::parse`] accepts.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::None => "none",
        }
    }
}

/// Options for [`Sandboxes::create`].
#[derive(Clone, Debug)]
pub struct CreateOptions {
    /// Provider.
    pub provider: ProviderKind,
    /// Name. `None` = ephemeral (no state file; Fleet: the claim is
    /// released with the handle, the managed pool stays for reuse).
    pub name: Option<String>,
    /// Image reference.
    pub image: String,
    /// Guest OS (`linux`, `macos`, `windows`).
    pub os: String,
    /// vCPUs.
    pub cpus: u32,
    /// Memory (MiB).
    pub memory_mb: u64,
    /// Grow a local VM's disk to this many GiB (`None`: the image's size).
    /// Local containers and cloud sandboxes refuse it
    /// ([`Error::InvalidArgument`]).
    pub disk_gb: Option<u32>,
    /// Extra guest ports to publish (local) or expose as `port-<n>`
    /// services (Fleet).
    pub ports: Vec<u16>,
    /// Named services (name → guest port). `env` defaults to 3211.
    pub services: BTreeMap<String, u16>,
    /// Readiness probes.
    pub wait_for: Vec<Probe>,
    /// Total readiness budget, the cua-spacesd wait included.
    pub ready_timeout: Duration,
    /// Whether the caller chose `ready_timeout` (`cua sb create
    /// --ready-timeout`, the SDK's `ready_timeout_ms`). Chosen, it bounds
    /// the cua-spacesd wait too; the default budget caps that wait at
    /// [`SPACESD_READY_TIMEOUT`], so an image that declares cua-spacesd but
    /// never starts it fails in minutes, not after the whole budget.
    pub ready_timeout_given: bool,
    /// Guest environment. Local: the container environment, cloud-init
    /// (Linux VMs) or `lume ssh` after boot (macOS VMs: `~/.cua/env.sh`
    /// sourced by shells, plus `launchctl setenv`); Windows VMs have no
    /// provisioning channel ([`Error::Unsupported`]). Fleet: the template's
    /// `env` with `processMode: Run` (gVisor and KubeVirt), hashed into the
    /// managed-pool key.
    pub env: BTreeMap<String, String>,
    /// The sandbox's command (argv), replacing the image's entrypoint. Local
    /// containers: ENTRYPOINT (with an empty CMD); local Linux VMs: run by
    /// cloud-init; Fleet: the template's `command` with `processMode: Run`
    /// (gVisor and KubeVirt), hashed into the managed-pool key. macOS
    /// guests: [`Error::Unsupported`].
    pub command: Option<Vec<String>>,
    /// spacesd token for [`Sandbox::spacesd`].
    pub env_token: Option<String>,
    /// Fleet options. With the local provider, `fleet.pool` (and an empty
    /// image) names a Fleet pool or template to run locally.
    pub fleet: FleetOptions,
    /// Boot firmware for local VMs (`bios` / `efi`); `None` = auto.
    pub firmware: Option<String>,
    /// Extra containers next to the sandbox, addressed by name on every
    /// runtime: the sandbox reaches a sidecar at its name (on its ports) and
    /// a sidecar reaches the sandbox at `main`; name their ports in
    /// `services` to reach them from outside. Local containers: extra
    /// containers in the sandbox's netns (`localhost` works too); Fleet
    /// gVisor: extra containers in the pod; Fleet KubeVirt: a companion
    /// gVisor pod named in the guest's `/etc/hosts`. Local VMs (QEMU/Lume):
    /// [`Error::Unsupported`]. With sidecars the service names `main`,
    /// `sidecars` and `sc` are reserved.
    pub sidecars: Vec<Sidecar>,
    /// Credentials for a private `image` (and sidecar images on the same
    /// registry). Local: used for the pull; Fleet: stored as a
    /// `cua-registry-*` pull Secret in the sandbox's namespace. Resolver
    /// and manifest reads use them too. Never logged.
    pub registry_credentials: Option<RegistryCredentials>,
    /// Image layers to build on top of `image` (`from` is `image`). Fleet: a
    /// remote build through the images API; local: a build into the
    /// container engine ([`LocalRuntime::build_image`]). Both are cached by
    /// the same content hash. VM images take no layers
    /// ([`Error::UnsupportedImage`]).
    pub build: Option<BuildSpec>,
    /// What kind of machine: [`Kind::Auto`] (the default) decides from
    /// the image (see [`crate::placement`]).
    pub kind: Kind,
    /// Which engine: [`Runtime::Auto`] (the default) picks the safest one
    /// the location offers for the kind (gVisor before runc; QEMU, or Lume
    /// for macOS; KubeVirt for cloud VMs). An engine the location does not
    /// offer for the kind is [`Error::InvalidPlacement`]. A local sandbox
    /// with sidecars needs `runc` where gVisor would run (separate gVisor
    /// containers cannot share a network namespace); nothing falls back
    /// silently.
    pub runtime: Runtime,
    /// Deprecated spelling of `runtime` for local containers (`runc` or
    /// `gvisor`); used when `runtime` is `Auto`. Cloud sandboxes ignore it.
    pub container_runtime: Option<String>,
    /// Guest network: outbound by default; [`NetworkMode::None`] cuts
    /// egress (local QEMU VMs only; everything else refuses it).
    pub network: NetworkMode,
    /// The process that owns an ephemeral local sandbox (recorded in its
    /// lease; the sandbox is reaped once that process is gone). `None`:
    /// this process. A daemon passes its client's pid.
    pub owner_pid: Option<u32>,
    /// [`ProviderKind::Contrib`]: the provider's location word (`e2b`).
    pub contrib: Option<String>,
    /// Keep a named sandbox whose readiness check fails (its Fleet claim,
    /// VM or container, and its state file) for debugging. `false` (the
    /// default): a failed create deletes what it made, named or not.
    /// Ephemeral sandboxes are always deleted.
    pub keep_on_failure: bool,
    /// A GPU option of the runtime it runs on (`paravirtual` for a macOS
    /// VM on Lume, `virgl` for QEMU, `nvidia` for a container, a provider's
    /// GPU type; `auto` or empty: the runtime's first). `None`: no GPU. See
    /// [`crate::gpu`] and [`LocalRuntime::gpu_support`].
    pub gpu: Option<String>,
    /// The name an ephemeral local or contrib sandbox gets (default
    /// `cua-eph-<hex>`); [`Sandboxes::create_cancellable`] sets it so it
    /// knows what to clean up.
    pub ephemeral_name: Option<String>,
}

impl CreateOptions {
    /// Defaults for `provider` + `image`.
    pub fn new(provider: ProviderKind, image: impl Into<String>) -> Self {
        Self {
            provider,
            name: None,
            image: image.into(),
            os: "linux".into(),
            cpus: 2,
            memory_mb: 4096,
            disk_gb: None,
            ports: vec![],
            services: BTreeMap::new(),
            wait_for: vec![],
            ready_timeout: Duration::from_secs(600),
            ready_timeout_given: false,
            env: BTreeMap::new(),
            command: None,
            env_token: None,
            fleet: FleetOptions::default(),
            firmware: None,
            sidecars: vec![],
            registry_credentials: None,
            build: None,
            kind: Kind::Auto,
            runtime: Runtime::Auto,
            container_runtime: None,
            network: NetworkMode::Default,
            owner_pid: None,
            contrib: None,
            keep_on_failure: false,
            gpu: None,
            ephemeral_name: None,
        }
    }

    /// Adds a sidecar.
    pub fn sidecar(mut self, sidecar: Sidecar) -> Self {
        self.sidecars.push(sidecar);
        self
    }

    /// The credentials, scoped to `image`'s registry when unscoped.
    fn scoped_credentials(&self) -> Option<RegistryCredentials> {
        self.registry_credentials.clone().map(|c| match c.registry {
            Some(_) => c,
            None => {
                // `container:` / `vm:` / `docker:` / `lume:` pick a local
                // backend; the registry is the reference's.
                let image = self.image.trim();
                let reference = match image.split_once(':') {
                    Some((p, r))
                        if ["container", "docker", "vm", "lume"].contains(&p)
                            && !r.starts_with("//") =>
                    {
                        r
                    }
                    _ => image,
                };
                c.for_registry(cua_fleet::registry_of(reference))
            }
        })
    }

    /// Whether the managed cloud capacity for this sandbox starts warm by
    /// default: the canonical images (`ghcr.io/trycua/{linux,windows,macos}`)
    /// do, anything else does not. An explicit `fleet.warm` (or
    /// `CUA_FLEET_WARM`) always wins. Warm is written as an explicit floor
    /// (`minPoolSize: 1`); Fleet has no server-side warm floor.
    pub fn default_warm(&self) -> Option<bool> {
        self.fleet
            .warm
            .or(self.fleet.replicas.map(|r| r > 0))
            .or_else(|| {
                // A user-chosen warm (CUA_FLEET_WARM or `cloud.warm`) applies
                // through the pool manager's config instead.
                if crate::settings::Settings::load()
                    .ok()
                    .and_then(|s| s.lookup("CUA_FLEET_WARM"))
                    .is_some_and(|v| !v.trim().is_empty())
                {
                    return None;
                }
                cua_fleet::is_canonical_image(&self.image).then_some(true)
            })
    }

    /// The location this create runs on (`None` for direct).
    pub fn on(&self) -> Option<On> {
        match self.provider {
            ProviderKind::Fleet => Some(On::Cloud),
            ProviderKind::Local => Some(On::Local),
            ProviderKind::Direct => None,
            ProviderKind::Contrib => self.contrib.clone().map(On::Provider),
        }
    }

    /// Checks `kind` and `runtime` against the location and folds them
    /// into the provider options: the older spellings (`container_runtime`,
    /// `fleet.runtime`, an image prefix such as `vm:`) become `runtime` /
    /// `kind` when those are `auto`, and contradictions are
    /// [`Error::InvalidPlacement`].
    pub fn apply_placement(&mut self) -> Result<()> {
        let Some(on) = self.on() else {
            if self.kind != Kind::Auto || self.runtime != Runtime::Auto {
                placement::validate(&On::Direct(String::new()), self.kind, &self.runtime)?;
            }
            return Ok(());
        };
        if self.runtime == Runtime::Auto {
            match on {
                On::Local => {
                    if let Some(r) = self.container_runtime.as_deref().filter(|r| !r.is_empty()) {
                        self.runtime = Runtime::parse(r)?;
                    }
                }
                _ => {
                    if let Some(k) = &self.fleet.runtime {
                        self.runtime = match k {
                            RuntimeKind::Kubevirt => Runtime::Kubevirt,
                            RuntimeKind::Gvisor => Runtime::Gvisor,
                            other => Runtime::Other(cua_fleet::runtime_name(other).to_string()),
                        };
                    }
                }
            }
        }
        // A backend prefix on a local image (`vm:`, `container:`, `lume:`)
        // is an older way to say the kind and runtime.
        if on == On::Local
            && let Some((prefix, _)) = local_prefix(&self.image)
        {
            let (k, r) = match prefix {
                "container" | "docker" => (Kind::Container, Runtime::Auto),
                "lume" => (Kind::Vm, Runtime::Lume),
                _ => (Kind::Vm, Runtime::Qemu),
            };
            if self.kind != Kind::Auto && self.kind != k {
                return Err(placement::PlacementError::new(
                    placement::Axis::Kind,
                    self.kind.as_str(),
                    format!("the image prefix {prefix}: runs a {k}, not a {}", self.kind),
                    vec!["auto".into(), k.to_string()],
                )
                .into());
            }
            self.kind = k;
            if self.runtime == Runtime::Auto {
                self.runtime = r;
            }
        }
        let implied = placement::validate(&on, self.kind, &self.runtime)?;
        if self.kind == Kind::Auto {
            self.kind = implied;
        }
        match on {
            On::Local => {
                if matches!(self.runtime, Runtime::Gvisor | Runtime::Runc) {
                    self.container_runtime = Some(self.runtime.to_string());
                }
            }
            _ => {
                let rt = match (&self.runtime, self.kind) {
                    (Runtime::Gvisor, _) | (Runtime::Auto, Kind::Container) => {
                        Some(RuntimeKind::Gvisor)
                    }
                    (Runtime::Kubevirt, _) | (Runtime::Auto, Kind::Vm) => {
                        Some(RuntimeKind::Kubevirt)
                    }
                    _ => None,
                };
                if rt.is_some() {
                    self.fleet.runtime = rt;
                }
            }
        }
        Ok(())
    }

    /// Sets the name.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Adds a probe.
    pub fn wait_for(mut self, probe: Probe) -> Self {
        self.wait_for.push(probe);
        self
    }

    /// Adds a service.
    pub fn service(mut self, name: impl Into<String>, port: u16) -> Self {
        self.services.insert(name.into(), port);
        self
    }

    /// Sets the command (argv).
    pub fn command<I, S>(mut self, argv: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.command = Some(argv.into_iter().map(Into::into).collect());
        self
    }

    /// Adds a readiness probe on a declared service (see
    /// [`Self::service_probe`]).
    pub fn wait_for_service(mut self, service: &str, http_path: Option<&str>) -> Result<Self> {
        let probe = self.service_probe(service, http_path)?;
        self.wait_for.push(probe);
        Ok(self)
    }

    /// A probe on the guest port of a declared service: TCP, or `GET
    /// http_path` returning 2xx.
    pub fn service_probe(&self, service: &str, http_path: Option<&str>) -> Result<Probe> {
        let port = service_port(&self.all_services(), service)?;
        Ok(match http_path {
            None => Probe::Tcp(port),
            Some(p) => Probe::Http {
                port,
                path: if p.starts_with('/') {
                    p.to_string()
                } else {
                    format!("/{p}")
                },
                status: None,
            },
        })
    }

    /// The managed-pool key of these options (image, runtime, shape,
    /// services, command, env).
    /// The runtime is resolved from the image's manifest when unset, and
    /// checked against it when set (`cua_fleet::resolve_runtime`).
    pub async fn fleet_pool_key(&self) -> Result<PoolSpecKey> {
        Ok(self.fleet_pool_key_resolved().await?.0)
    }

    /// [`CreateOptions::fleet_pool_key`], plus the image as the resolver
    /// pinned it (`None` when the registry could not be read).
    pub async fn fleet_pool_key_resolved(&self) -> Result<(PoolSpecKey, Option<ImageInfo>)> {
        // The one image rule: the variant the runtime runs, digest-pinned
        // (read with the registry credentials, when given).
        let creds = self.scoped_credentials();
        let image = cua_fleet::resolve_fleet_image_with(
            self.fleet.runtime.clone(),
            &self.image,
            creds.as_ref(),
        )
        .await?;
        let runtime = image.runtime.clone();
        let vm = matches!(runtime, RuntimeKind::Kubevirt);
        // command / env run on both runtimes (`processMode: Run`, written
        // by the template); sidecars too (a companion pod on KubeVirt).
        let command = self.command.clone().filter(|c| !c.is_empty());
        let env: BTreeMap<String, String> = self
            .env
            .iter()
            .filter(|(k, _)| !matches!(k.as_str(), "CUA_ENV_TOKEN" | "CUA_SPACESD_TOKEN"))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let mut services = self.all_services();
        // `env` (3211) only for images that carry cua-spacesd: a plain
        // image gets no env service (unless asked for).
        if !self.services.contains_key("env")
            && image.resolved.as_ref().and_then(|r| r.spacesd) == Some(false)
        {
            services.remove("env");
        }
        let mut key = PoolSpecKey::new(&image.image)
            .runtime(runtime)
            .resources(Some(self.cpus), Some(self.memory_mb as u32))
            .services(services);
        key.efi = self.os.eq_ignore_ascii_case("windows");
        key.command = command;
        key.env = env;
        key.sidecars = self.sidecars.clone();
        // A pull secret only when a pod image lives on the credentials'
        // registry (a remote build's output is Fleet's own).
        if let Some(c) = &creds
            && std::iter::once(&image.image)
                .chain(self.sidecars.iter().map(|s| &s.image))
                .any(|i| c.applies_to(cua_fleet::registry_of(i)))
        {
            let registry = c.registry.clone().unwrap_or_default();
            key.pull_secret = Some(cua_fleet::registry_secret_name(&registry, &c.username));
        }
        // A pod replica is ready once the first probed port listens, so a
        // warm replica never binds before its server is up.
        if !vm {
            key.readiness_tcp_port = self.wait_for.first().map(probe_port);
        }
        let info = image.resolved.as_ref().map(ImageInfo::from);
        Ok((key, info))
    }

    /// The sandbox fields these options set explicitly, as the shared
    /// model: what a named pool's template is compared with (or updated
    /// to, with `fleet.apply`). Defaults are left unset, so they are not
    /// compared.
    pub fn pool_sandbox_spec(&self) -> cua_fleet::SandboxSpec {
        let mut services = self.services.clone();
        for p in &self.ports {
            if !services.values().any(|v| v == p) {
                services.insert(format!("port-{p}"), *p);
            }
        }
        cua_fleet::SandboxSpec {
            image: self.image.trim().to_string(),
            command: self.command.clone().filter(|c| !c.is_empty()),
            args: None,
            env: self
                .env
                .iter()
                .filter(|(k, _)| !matches!(k.as_str(), "CUA_ENV_TOKEN" | "CUA_SPACESD_TOKEN"))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            services,
            readiness: None,
            cpu: self.fleet.cpus_given.then_some(self.cpus),
            memory_mb: self.fleet.memory_given.then_some(self.memory_mb as u32),
            efi: false,
            sidecars: self.sidecars.clone(),
            registry_secret: self.scoped_credentials().map(|c| {
                cua_fleet::registry_secret_name(
                    c.registry.as_deref().unwrap_or_default(),
                    &c.username,
                )
            }),
            process_mode: None,
            claim_secrets: false,
        }
    }

    /// The sidecar rules every provider shares (Fleet's admission): valid,
    /// unique names other than `main`, and with sidecars the service names
    /// `main`, `sidecars` and `sc` are reserved.
    pub fn validate_sidecars(&self) -> Result<()> {
        if self.sidecars.is_empty() {
            return Ok(());
        }
        cua_fleet::validate_sidecars(&self.sidecars, &[])?;
        cua_fleet::check_reserved_service_names(self.services.keys(), true)?;
        Ok(())
    }

    fn all_services(&self) -> BTreeMap<String, u16> {
        let mut s = self.services.clone();
        s.entry("env".into()).or_insert(ENV_PORT);
        // Sidecar ports are reachable like the sandbox's own.
        for p in self
            .ports
            .iter()
            .chain(self.sidecars.iter().flat_map(|c| c.ports.iter()))
        {
            if !s.values().any(|v| v == p) {
                s.insert(format!("port-{p}"), *p);
            }
        }
        s
    }
}

/// Listing row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxInfo {
    /// Qualified ref (`local:<name>`, `cloud:<name>`, `direct:<host:port>`).
    pub id: String,
    /// Name.
    pub name: String,
    /// Provider.
    pub provider: ProviderKind,
    /// `runtime_type` as persisted.
    pub runtime_type: String,
    /// Status.
    pub status: Status,
}

// One per sandbox handle; size does not matter here.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug)]
enum Target {
    Fleet {
        pool: String,
        bound: BoundSandbox,
        /// Managed claim (heartbeat); released or detached with the last
        /// handle.
        lease: Option<Arc<Lease>>,
        /// The pool's runtime, when known.
        runtime: Option<RuntimeKind>,
    },
    Local {
        backend: String,
        endpoints: LocalEndpoints,
    },
    Direct {
        endpoint: cua_spacesd_client::Endpoint,
    },
    Contrib {
        provider: ContribHandle,
        instance: crate::ProviderInstance,
    },
}

/// A registered contrib provider (Debug without its internals).
#[derive(Clone)]
struct ContribHandle(Arc<dyn crate::Provider>);

impl std::fmt::Debug for ContribHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Provider({})", self.0.name())
    }
}

/// A managed claim shared by a sandbox handle's clones, and the latest
/// shutdown time a `keep_alive` asked for (unix seconds, 0 = none).
struct Lease(
    std::sync::Mutex<Option<ManagedClaim>>,
    std::sync::atomic::AtomicI64,
);

impl std::fmt::Debug for Lease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Lease")
    }
}

impl Lease {
    fn new(claim: ManagedClaim) -> Arc<Self> {
        Arc::new(Self(
            std::sync::Mutex::new(Some(claim)),
            std::sync::atomic::AtomicI64::new(0),
        ))
    }

    /// When the claim expires if nothing renews it after now: the heartbeat
    /// keeps it at now + TTL while the handle holds it.
    fn expires_at(&self) -> Option<std::time::SystemTime> {
        let extended = self.1.load(std::sync::atomic::Ordering::SeqCst);
        let held = self
            .0
            .lock()
            .unwrap()
            .as_ref()
            .map(|c| std::time::SystemTime::now() + c.claim_ttl);
        let extended =
            (extended > 0).then(|| std::time::UNIX_EPOCH + Duration::from_secs(extended as u64));
        match (held, extended) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        }
    }

    fn take(&self) -> Option<ManagedClaim> {
        self.0.lock().unwrap().take()
    }
}

struct Inner {
    fleet: Option<FleetClient>,
    pools: Option<PoolManager>,
    local: Option<Arc<dyn LocalRuntime>>,
    /// Contrib and cloud providers by location word.
    providers: BTreeMap<&'static str, Arc<dyn crate::Provider>>,
    /// Your clouds: connect, test, sweep ([`crate::byoc`]).
    clouds: Option<Arc<dyn crate::byoc::CloudManager>>,
    state: StateStore,
    http: HttpClient,
}

/// Creates, finds and manages sandboxes across providers. Cheap to clone.
#[derive(Clone)]
pub struct Sandboxes {
    inner: Arc<Inner>,
}

/// Builder for [`Sandboxes`].
#[derive(Default)]
pub struct SandboxesBuilder {
    fleet: Option<FleetClient>,
    pools: Option<PoolManager>,
    local: Option<Arc<dyn LocalRuntime>>,
    providers: BTreeMap<&'static str, Arc<dyn crate::Provider>>,
    clouds: Option<Arc<dyn crate::byoc::CloudManager>>,
    state_dir: Option<PathBuf>,
}

impl SandboxesBuilder {
    /// Enables the Fleet provider.
    pub fn fleet(mut self, fleet: FleetClient) -> Self {
        self.fleet = Some(fleet);
        self
    }

    /// Uses this pool manager for managed Fleet pools (share one per
    /// process). Default: a manager over the Fleet client with
    /// [`crate::settings::auto_pool_config`] (environment, then `config.toml`), its home next to the state directory.
    pub fn pool_manager(mut self, pools: PoolManager) -> Self {
        self.pools = Some(pools);
        self
    }

    /// Enables the local provider.
    pub fn local(mut self, runtime: Arc<dyn LocalRuntime>) -> Self {
        self.local = Some(runtime);
        self
    }

    /// Registers a contrib provider (`--on <name>`); a later one with the
    /// same name replaces it.
    pub fn provider(mut self, provider: Arc<dyn crate::Provider>) -> Self {
        self.providers.insert(provider.name(), provider);
        self
    }

    /// The service behind `cua cloud` ([`crate::byoc`]).
    pub fn clouds(mut self, clouds: Arc<dyn crate::byoc::CloudManager>) -> Self {
        self.clouds = Some(clouds);
        self
    }

    /// Overrides `~/.cua/sandboxes`.
    pub fn state_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.state_dir = Some(dir.into());
        self
    }

    /// Builds.
    pub fn build(self) -> Sandboxes {
        let pools = self.pools.or_else(|| {
            let fleet = self.fleet.clone()?;
            let mut cfg = crate::settings::auto_pool_config();
            if let Some(dir) = &self.state_dir {
                cfg = cfg.with_state_dir(dir);
            }
            Some(PoolManager::new(fleet, cfg))
        });
        Sandboxes {
            inner: Arc::new(Inner {
                fleet: self.fleet,
                pools,
                local: self.local,
                providers: self.providers,
                clouds: self.clouds,
                state: self.state_dir.map(StateStore::new).unwrap_or_default(),
                http: HttpClient::new(),
            }),
        }
    }
}

/// Suspend, resume and restart of a cloud sandbox: Fleet has no per-claim
/// pause, stop or restart, and scaling the pool would touch every sandbox
/// in it.
fn fleet_lifecycle_unsupported(op: &str) -> Error {
    Error::Unsupported {
        provider: ProviderKind::Fleet,
        op: format!(
            "{op} of a cloud sandbox (Fleet cannot suspend a single sandbox; hold it with \
             keep_alive, or delete it and create a new one)"
        ),
    }
}

impl Sandboxes {
    /// A builder.
    pub fn builder() -> SandboxesBuilder {
        SandboxesBuilder::default()
    }

    /// The state store.
    pub fn state(&self) -> &StateStore {
        &self.inner.state
    }

    fn fleet(&self) -> Result<&FleetClient> {
        self.inner
            .fleet
            .as_ref()
            .ok_or(Error::ProviderNotConfigured(ProviderKind::Fleet))
    }

    /// The managed-pool manager (Fleet only).
    pub fn pools(&self) -> Result<&PoolManager> {
        self.inner
            .pools
            .as_ref()
            .ok_or(Error::ProviderNotConfigured(ProviderKind::Fleet))
    }

    /// The registered contrib provider `name`: [`crate::provider::not_built`]
    /// for a contrib word this build lacks, `InvalidArgument` otherwise.
    pub fn contrib_provider(&self, name: &str) -> Result<&Arc<dyn crate::Provider>> {
        self.inner.providers.get(name).ok_or_else(|| {
            if crate::provider::is_provider_location(name) {
                crate::provider::not_built(name)
            } else {
                Error::InvalidArgument(format!(
                    "unknown provider {name:?} (contrib providers: {})",
                    crate::CONTRIB_LOCATIONS.join(", ")
                ))
            }
        })
    }

    /// Your clouds (`cua cloud`), when this build has them.
    pub fn clouds(&self) -> Result<&Arc<dyn crate::byoc::CloudManager>> {
        self.inner
            .clouds
            .as_ref()
            .ok_or_else(crate::byoc::not_built)
    }

    /// The public provider details a persisted provider sandbox was saved
    /// with (empty for other sandboxes).
    pub fn persisted_details(&self, name: &str) -> BTreeMap<String, String> {
        let Some(SandboxState::Local(l)) = self.inner.state.load(name) else {
            return BTreeMap::new();
        };
        l.extra
            .get(contrib::DETAILS_KEY)
            .and_then(Value::as_object)
            .map(|o| {
                o.iter()
                    .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The persisted cloud sandbox that joined the relay as `machine`: its
    /// provider word (`aws`) and where it runs, if it is one this home
    /// created.
    pub fn cloud_of_relay_machine(&self, machine: &str) -> Option<(String, String)> {
        self.inner.state.list_all().into_iter().find_map(|s| {
            let SandboxState::Local(l) = &s else {
                return None;
            };
            if l.extra
                .get(crate::byoc::DETAIL_RELAY_MACHINE)
                .and_then(Value::as_str)
                != Some(machine)
            {
                return None;
            }
            let (word, _) = contrib::contrib_of(&s)?;
            let place = l
                .extra
                .get(crate::byoc::DETAIL_PLACE)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            Some((word.to_string(), place))
        })
    }

    /// The name of the persisted provider sandbox (`aws`, `gcp`, `modal`)
    /// that joined the relay as `machine`, if any.
    pub fn by_relay_machine(&self, machine: &str) -> Result<Option<String>> {
        for s in self.inner.state.list_all() {
            if let SandboxState::Local(l) = &s
                && contrib::contrib_of(&s).is_some()
                && l.extra
                    .get(crate::byoc::DETAIL_RELAY_MACHINE)
                    .and_then(Value::as_str)
                    == Some(machine)
            {
                return Ok(Some(l.name.clone()));
            }
        }
        Ok(None)
    }

    /// The contrib providers this build registered, by name.
    pub fn contrib_providers(&self) -> Vec<Arc<dyn crate::Provider>> {
        self.inner.providers.values().cloned().collect()
    }

    fn local(&self) -> Result<&Arc<dyn LocalRuntime>> {
        self.inner
            .local
            .as_ref()
            .ok_or(Error::ProviderNotConfigured(ProviderKind::Local))
    }

    /// Creates a sandbox and waits until the provider reports it running
    /// and every probe in `options.wait_for` passes.
    pub async fn create(&self, mut options: CreateOptions) -> Result<Sandbox> {
        use crate::progress::{Phase, Progress, report};
        let started = Instant::now();
        report(Progress::phase(Phase::Preparing).detail(&options.image));
        options.apply_placement()?;
        options.validate_sidecars()?;
        if options.disk_gb.is_some_and(|g| g > 0) && options.provider != ProviderKind::Local {
            return Err(Error::InvalidArgument(
                "disk_gb sizes local VMs only: cloud sandboxes boot the image's disk".into(),
            ));
        }
        let options = self.resolve_fleet_template(options).await?;
        let options = self.build_image(options).await?;
        let sandbox = match options.provider {
            ProviderKind::Fleet => self.create_fleet(&options).await?,
            ProviderKind::Local => self.create_local(&options).await?,
            ProviderKind::Contrib => self.create_contrib(&options).await?,
            ProviderKind::Direct => {
                return Err(Error::InvalidArgument(
                    "direct sandboxes are not created; use Sandboxes::connect_url".into(),
                ));
            }
        };
        let remaining = options.ready_timeout.saturating_sub(started.elapsed());
        if !options.wait_for.is_empty() {
            report(Progress::phase(Phase::WaitingForServices));
        }
        let mut ready = sandbox.wait_ready(&options.wait_for, remaining).await;
        // The image declares cua-spacesd: ready includes it answering
        // `Health` (bounded), so the first guest call does not race its
        // start. Images that do not declare it are never waited on.
        if ready.is_ok() && sandbox.declares_spacesd(&options) {
            let remaining = options.ready_timeout.saturating_sub(started.elapsed());
            report(Progress::phase(Phase::WaitingForServices).detail("cua-spacesd"));
            ready = sandbox
                .wait_spacesd(spacesd_budget(&options, remaining))
                .await;
        }
        if let Err(e) = ready {
            // A failed create releases what it made (a named sandbox's claim
            // too), unless the caller keeps it to debug.
            if sandbox.ephemeral || !options.keep_on_failure {
                let id = sandbox.id();
                if let Err(d) = sandbox.clone().delete().await {
                    tracing::warn!(sandbox = %id, error = %d, "could not delete a sandbox that failed readiness");
                }
            } else {
                tracing::warn!(sandbox = %sandbox.id(), "sandbox failed readiness and is kept (keep_on_failure)");
            }
            return Err(e);
        }
        Ok(sandbox)
    }

    /// [`Sandboxes::create`] that stops when `cancel` fires, and when this
    /// future is dropped (a closed request, a Ctrl-C): the work in flight
    /// stops (an image download, a boot, a claim) and what this create made
    /// is deleted: its instance and state, its Fleet claim, the provider's
    /// sandbox. Nothing that existed before is touched: a name that was
    /// already in use is never deleted. Finished image downloads stay
    /// cached (a later create resumes from them); partial files do not.
    /// On cancel it returns [`Error::Cancelled`] once the clean-up is done.
    pub async fn create_cancellable(
        &self,
        mut options: CreateOptions,
        cancel: crate::CancellationToken,
    ) -> Result<Sandbox> {
        let plan = self.discard_plan(&mut options).await;
        let mut guard = DiscardGuard {
            mgr: self.clone(),
            plan: Some(plan),
        };
        let create = Box::pin(self.create(options));
        let result = tokio::select! {
            r = create => r,
            _ = cancel.cancelled() => {
                let plan = guard.plan.take().expect("armed");
                let what = self.discard(plan).await;
                return Err(Error::Cancelled(what));
            }
        };
        guard.plan = None;
        result
    }

    /// [`Sandboxes::create_cancellable`] that [`Sandboxes::cancel_create`]
    /// can stop by the sandbox's name (a named create; an ephemeral one is
    /// cancelled by dropping it). A second create of a name that is being
    /// created is refused.
    pub async fn create_tracked(&self, options: CreateOptions) -> Result<Sandbox> {
        let Some(name) = options.name.clone() else {
            return Box::pin(self.create_cancellable(options, crate::CancellationToken::new()))
                .await;
        };
        let key = (self.inner.state.dir().to_path_buf(), name.clone());
        let token = crate::CancellationToken::new();
        let (tx, rx) = tokio::sync::watch::channel(None);
        {
            let mut all = CREATING.lock().unwrap_or_else(|e| e.into_inner());
            all.retain(|c| c.done.borrow().is_none());
            if all.iter().any(|c| c.key == key) {
                return Err(Error::InvalidArgument(format!(
                    "{name} is already being created; wait for it or cancel it"
                )));
            }
            all.push(Creating {
                key: key.clone(),
                token: token.clone(),
                done: rx,
            });
        }
        let result = Box::pin(self.create_cancellable(options, token)).await;
        let _ = tx.send(Some(match &result {
            Err(Error::Cancelled(m)) => Some(m.clone()),
            _ => None,
        }));
        CREATING
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|c| c.done.borrow().is_none());
        result
    }

    /// Stops the [`Sandboxes::create_tracked`] of `name` running in this
    /// process and waits until what it made is deleted. `Ok(None)`: no
    /// create of that name is running (it finished, failed or was
    /// cancelled already). `Ok(Some(what))`: it was cancelled; `what` says
    /// what was removed.
    pub async fn cancel_create(&self, name: &str) -> Result<Option<String>> {
        let key = (self.inner.state.dir().to_path_buf(), name.to_string());
        let found = CREATING
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|c| c.key == key && c.done.borrow().is_none())
            .map(|c| (c.token.clone(), c.done.clone()));
        let Some((token, mut done)) = found else {
            return Ok(None);
        };
        token.cancel();
        loop {
            if let Some(outcome) = done.borrow().clone() {
                return Ok(Some(
                    outcome.unwrap_or_else(|| format!("{name} finished before the cancel")),
                ));
            }
            if done.changed().await.is_err() {
                return Ok(Some(format!("cancelled {name}")));
            }
        }
    }

    /// What a cancelled create of `o` would delete, decided before it
    /// starts (an ephemeral sandbox gets its name now).
    async fn discard_plan(&self, o: &mut CreateOptions) -> DiscardPlan {
        let named = o.name.is_some();
        if !named && matches!(o.provider, ProviderKind::Local | ProviderKind::Contrib) {
            o.ephemeral_name
                .get_or_insert_with(|| format!("cua-eph-{:08x}", rand_u32()));
        }
        let name = o.name.clone().or_else(|| o.ephemeral_name.clone());
        // Only a name nothing used before is ours to delete.
        let fresh = match (&o.provider, &name) {
            (ProviderKind::Local, Some(n)) => match self.local() {
                Ok(rt) => {
                    self.inner.state.load(n).is_none()
                        && matches!(rt.status(n).await, Err(crate::RuntimeError::NotFound(_)))
                }
                Err(_) => false,
            },
            (ProviderKind::Contrib, Some(n)) if named => {
                match o
                    .contrib
                    .as_deref()
                    .and_then(|w| self.contrib_provider(w).ok())
                {
                    Some(p) => p
                        .list()
                        .await
                        .is_ok_and(|all| all.iter().all(|i| &i.name != n)),
                    None => false,
                }
            }
            (ProviderKind::Contrib, Some(_)) => true,
            (ProviderKind::Fleet, Some(n)) => {
                self.inner.state.load(n).is_none()
                    && match self.fleet() {
                        Ok(f) => matches!(
                            tokio::time::timeout(Duration::from_secs(10), f.find_claims(n)).await,
                            Ok(Ok(c)) if c.is_empty()
                        ),
                        Err(_) => false,
                    }
            }
            _ => false,
        };
        DiscardPlan {
            provider: o.provider,
            name,
            fresh,
            contrib: o.contrib.clone(),
        }
    }

    /// Deletes what a cancelled create made (see [`DiscardPlan`]) once the
    /// clean-up its drop guards started has finished; says what it did.
    async fn discard(&self, plan: DiscardPlan) -> String {
        let settled = crate::cleanup::settle(DISCARD_SETTLE).await;
        if !settled {
            tracing::warn!("clean-up of a cancelled create is still running");
        }
        let Some(name) = plan.name.clone() else {
            // An ephemeral cloud claim: its lease released it when the
            // create was dropped.
            return "nothing was left".into();
        };
        if !plan.fresh {
            return format!("{name} existed before this create and was left as it was");
        }
        let removed = match plan.provider {
            ProviderKind::Local => {
                let _ = self.inner.state.remove_lease(&name);
                self.delete_local_instance(&name).await
            }
            ProviderKind::Fleet => match self.delete(&name).await {
                Ok(()) => Ok(true),
                Err(Error::NotFound(_)) => Ok(false),
                Err(e) => Err(e),
            },
            ProviderKind::Contrib => {
                let word = plan.contrib.clone().unwrap_or_default();
                match self.contrib_provider(&word) {
                    Ok(p) => p.abort_create(&name).await.map(|()| {
                        let _ = self.inner.state.delete(&name);
                        true
                    }),
                    Err(e) => Err(e),
                }
            }
            ProviderKind::Direct => Ok(false),
        };
        match removed {
            Ok(true) => format!("removed {name}"),
            Ok(false) => format!("{name} was never made"),
            Err(e) => {
                tracing::warn!(sandbox = %name, error = %e, "could not remove a cancelled create");
                format!("could not remove {name}: {e}")
            }
        }
    }

    /// Local provider + `pool:<name>` image (or `fleet.pool`): the Fleet
    /// template's image, firmware, resources, services and probes.
    async fn resolve_fleet_template(&self, mut o: CreateOptions) -> Result<CreateOptions> {
        if o.provider != ProviderKind::Local {
            return Ok(o);
        }
        let Some(name) = crate::fleet_local::template_name(&o).map(str::to_string) else {
            return Ok(o);
        };
        let plan = crate::fleet_local::local_from_fleet_template(self.fleet()?, &name).await?;
        tracing::info!(template = %plan.template, image = %plan.image, "running a Fleet template locally");
        plan.apply(&mut o);
        o.fleet.pool = None;
        Ok(o)
    }

    /// Image layers: a remote build on Fleet, or a build into this
    /// machine's container engine locally (both cached by the same content
    /// hash, `cua-b-<hash>`), after which the sandbox runs the built image.
    async fn build_image(&self, mut o: CreateOptions) -> Result<CreateOptions> {
        let Some(mut spec) = o.build.take().filter(|b| !b.is_empty()) else {
            return Ok(o);
        };
        match o.provider {
            ProviderKind::Fleet => {
                if o.fleet.pool.is_some() {
                    return Err(Error::InvalidArgument(
                        "image layers build a new image; an existing pool keeps its own \
                         (omit the pool)"
                            .into(),
                    ));
                }
                if spec.from.is_empty() {
                    spec.from = o.image.clone();
                }
                let creds = o.scoped_credentials();
                let built = self
                    .fleet()?
                    .build_image(&spec, creds.as_ref(), None)
                    .await?;
                tracing::info!(image = %built.reference, cached = built.cached, "built image");
                // Credentials stay for sidecars on the private registry.
                o.registry_credentials = creds;
                o.image = built.reference;
                // A remote build's output is a container rootfs.
                o.fleet.runtime.get_or_insert(RuntimeKind::Gvisor);
                Ok(o)
            }
            ProviderKind::Local => {
                if spec.from.is_empty() {
                    spec.from = o.image.clone();
                }
                spec.validate()?;
                let creds = o.scoped_credentials();
                let built = self
                    .local()?
                    .build_image(&spec, creds.as_ref())
                    .await
                    .map_err(|e| match e {
                        crate::RuntimeError::UnsupportedImage(m) => Error::UnsupportedImage(m),
                        crate::RuntimeError::Unsupported { backend, op } => Error::Unsupported {
                            provider: ProviderKind::Local,
                            op: format!("{op} ({backend})"),
                        },
                        other => other.into(),
                    })?;
                tracing::info!(image = %built, "built image locally");
                o.registry_credentials = creds;
                o.image = built;
                Ok(o)
            }
            other => Err(Error::Unsupported {
                provider: other,
                op: "image layers".into(),
            }),
        }
    }

    async fn create_fleet(&self, o: &CreateOptions) -> Result<Sandbox> {
        if o.gpu.is_some() {
            return Err(Error::Unsupported {
                provider: ProviderKind::Fleet,
                op: format!("a GPU ({})", FLEET_NO_GPU),
            });
        }
        if o.network == NetworkMode::None {
            return Err(Error::Unsupported {
                provider: ProviderKind::Fleet,
                op: "network=\"none\" (only local QEMU VMs run without outbound network)".into(),
            });
        }
        let fleet = self.fleet()?;
        crate::progress::report(crate::progress::Progress::phase(
            crate::progress::Phase::Creating,
        ));
        let ephemeral = o.name.is_none();
        // The spacesd token of a managed-pool sandbox, delivered through the
        // claim's Secret (see `fleet_claim_token`); `None` elsewhere.
        let mut claim_token: Option<String> = None;
        let (bound, lease, services, image_info, runtime) = match &o.fleet.pool {
            // An explicit, user-owned pool: plain claim, the pool is never
            // touched.
            Some(pool) => {
                // Sandbox fields with a named pool: they must match its
                // template (never silently ignored), or `apply` updates it.
                let requested = o.pool_sandbox_spec();
                if o.fleet.apply && is_managed_pool(pool) {
                    return Err(Error::InvalidArgument(format!(
                        "{pool} is a managed pool (its template is its key); apply=True \
                         updates only pools you own (omit the pool to get a sandbox with \
                         these fields)"
                    )));
                }
                if requested != cua_fleet::SandboxSpec::default() {
                    if o.fleet.apply {
                        let creds = requested
                            .registry_secret
                            .as_ref()
                            .and_then(|_| o.scoped_credentials());
                        fleet
                            .apply_pool_template(pool, &requested, creds.as_ref())
                            .await?;
                    } else {
                        fleet.check_pool_spec(pool, &requested).await?;
                    }
                }
                let handle = fleet.get_pool(pool).await?;
                let pool_runtime = match handle.runtime.clone() {
                    Some(r) => Some(r),
                    None => fleet.pool_runtime(&handle.pool).await,
                };
                let claim_opts = ClaimOptions {
                    // A named sandbox's claim carries its name, so it
                    // reattaches.
                    name: o.name.clone(),
                    ttl_seconds_after_created: o.fleet.ttl_seconds_after_created,
                    ..Default::default()
                };
                let bound = fleet.acquire(&handle.pool, claim_opts).await?;
                // A pre-existing pool's services are its template's, not
                // the create options' defaults (`env` -> 3211).
                let services = fleet_services(fleet, &handle.pool, &bound).await;
                // The template's image, pinned (cached per pool); never
                // fails the claim.
                let creds = o.scoped_credentials();
                let image_info =
                    match crate::pool_image::pool_image_info(fleet, pool, creds.as_ref()).await {
                        Ok(i) => i,
                        Err(e) => {
                            tracing::debug!(pool, error = %e, "pool template image not read");
                            None
                        }
                    };
                (bound, None, services, image_info, pool_runtime)
            }
            None => {
                let warm = o.default_warm();
                let (mut key, image_info) = o.fleet_pool_key_resolved().await?;
                claim_token = fleet_claim_token(o, image_info.as_ref())?;
                key.claim_secrets = claim_token.is_some();
                let registry_credentials = key
                    .pull_secret
                    .as_ref()
                    .and_then(|_| o.scoped_credentials());
                let opts = AcquireOpts {
                    name: o.name.clone(),
                    warm,
                    max_pool_size: o.fleet.max_pool_size,
                    claim_ttl: o
                        .fleet
                        .ttl_seconds_after_created
                        .filter(|t| *t > 0)
                        .map(|t| Duration::from_secs(u64::from(t))),
                    labels: BTreeMap::new(),
                    bind_deadline: Some(o.ready_timeout),
                    registry_credentials,
                    claim_token: claim_token.clone(),
                };
                let services = key.services.clone();
                let pool_runtime = Some(key.runtime.clone());
                let mut claim = self.pools()?.acquire(key, opts).await?;
                // An ephemeral claim goes with its handle; a named one
                // outlives this process until its TTL (reattach with
                // `connect`, extend with `keep_alive`).
                claim.set_release_on_drop(ephemeral);
                (
                    claim.sandbox.clone(),
                    Some(Lease::new(claim)),
                    services,
                    image_info,
                    pool_runtime,
                )
            }
        };
        let pool = bound.namespace.clone();
        let name = o.name.clone().unwrap_or_else(|| bound.claim.clone());
        if !ephemeral {
            self.inner.state.save_fleet_claim(&bound.claim, &pool)?;
            if o.fleet.pool.is_some() {
                // A named pool's resolved template image, for a reattach.
                if let Some(i) = &image_info {
                    let mut fields = Map::new();
                    fields.insert(IMAGE_INFO_KEY.into(), serde_json::to_value(i)?);
                    self.inner.state.update(&bound.claim, fields)?;
                }
            } else {
                // What `resume` needs to claim the same shape again.
                let mut fields = managed_fields(o);
                if let Some(i) = &image_info {
                    fields.insert(IMAGE_INFO_KEY.into(), serde_json::to_value(i)?);
                }
                if let Some(t) = &claim_token {
                    fields.insert("env_token".into(), Value::String(t.clone()));
                }
                self.inner.state.update(&bound.claim, fields)?;
                if !o.env.is_empty() || claim_token.is_some() {
                    // The environment and the token are secrets.
                    self.inner.state.restrict(&bound.claim)?;
                }
            }
        }
        Ok(Sandbox {
            mgr: self.clone(),
            name,
            ephemeral,
            env_token: claim_token.or_else(|| o.env_token.clone()),
            bootstrap_token: Default::default(),
            services,
            image_info,
            target: Target::Fleet {
                pool,
                bound,
                lease,
                runtime,
            },
        })
    }

    async fn create_local(&self, o: &CreateOptions) -> Result<Sandbox> {
        let rt = self.local()?;
        let services = o.all_services();
        let mut ports: Vec<u16> = services.values().copied().chain(o.ports.clone()).collect();
        ports.sort_unstable();
        ports.dedup();
        let ephemeral = o.name.is_none();
        let name = o
            .name
            .clone()
            .or_else(|| o.ephemeral_name.clone())
            .unwrap_or_else(|| format!("cua-eph-{:08x}", rand_u32()));
        let env_token = local_env_token(o);
        if ephemeral {
            // Before anything exists: a crash from here on leaves a lease
            // whose process is gone, and the instance is reaped.
            self.inner
                .state
                .write_lease(&name, o.owner_pid.unwrap_or_else(std::process::id))?;
        }
        let spec = LocalStartSpec {
            ephemeral,
            name: name.clone(),
            image: o.image.clone(),
            os: o.os.clone(),
            cpus: o.cpus,
            memory_mb: o.memory_mb,
            disk_size_gb: o.disk_gb.filter(|g| *g > 0),
            ports,
            env: guest_env(o, env_token.as_deref()),
            command: o.command.clone().filter(|c| !c.is_empty()),
            ready_timeout: o.ready_timeout,
            firmware: o.firmware.clone(),
            sidecars: o.sidecars.clone(),
            registry_credentials: o.scoped_credentials(),
            container_runtime: o.container_runtime.clone(),
            kind: (o.kind != Kind::Auto).then(|| o.kind.to_string()),
            runtime: (o.runtime != Runtime::Auto).then(|| o.runtime.to_string()),
            restrict_network: o.network == NetworkMode::None,
            gpu: o.gpu.clone().filter(|g| !g.trim().is_empty()),
            probes: o
                .wait_for
                .iter()
                .map(|p| match p {
                    Probe::Tcp(port) => crate::LocalProbe {
                        port: *port,
                        http_path: None,
                    },
                    // The backend's HTTP check accepts 2xx/3xx; an exact
                    // status is left to `wait_ready`, the backend waits for
                    // the port.
                    Probe::Http { port, path, status } => crate::LocalProbe {
                        port: *port,
                        http_path: status
                            .is_none_or(|s| (200..400).contains(&s))
                            .then(|| path.clone()),
                    },
                })
                .collect(),
        };
        let started = rt.start_resolved(&spec).await;
        if ephemeral && started.is_err() {
            // A failed start can leave a created instance (overlay, stopped
            // container) behind: nothing will hold a handle to it.
            match rt.delete(&name).await {
                Ok(()) | Err(crate::RuntimeError::NotFound(_)) => {
                    let _ = self.inner.state.remove_lease(&name);
                }
                Err(e) => {
                    tracing::warn!(sandbox = %name, error = %e, "could not remove a failed ephemeral sandbox; it will be reaped")
                }
            }
        }
        let (inst, image_info) = started.map_err(|e| match e {
            crate::RuntimeError::UnsupportedImage(m) => Error::UnsupportedImage(m),
            crate::RuntimeError::Unsupported { backend, op } => Error::Unsupported {
                provider: ProviderKind::Local,
                op: format!("{op} ({backend})"),
            },
            other => other.into(),
        })?;
        // The runtime publishes 3211 only for images that carry
        // cua-spacesd (or when it cannot tell): report `env` only then.
        let mut services = services;
        if !o.services.contains_key("env")
            && let Some(p) = services.get("env")
            && !inst.endpoints.ports.contains_key(p)
        {
            services.remove("env");
        }
        if !ephemeral {
            self.save_local_state(
                &name,
                &inst,
                o,
                &recorded_os(&o.os, image_info.as_ref()),
                &services,
                env_token.as_deref(),
            )?;
            if let Some(i) = &image_info {
                let mut fields = Map::new();
                fields.insert(IMAGE_INFO_KEY.into(), serde_json::to_value(i)?);
                self.inner.state.update(&name, fields)?;
            }
        }
        Ok(Sandbox {
            mgr: self.clone(),
            name,
            ephemeral,
            env_token,
            bootstrap_token: Default::default(),
            services,
            image_info,
            target: Target::Local {
                backend: inst.backend,
                endpoints: inst.endpoints,
            },
        })
    }

    fn save_local_state(
        &self,
        name: &str,
        inst: &LocalInstance,
        o: &CreateOptions,
        os: &str,
        services: &BTreeMap<String, u16>,
        env_token: Option<&str>,
    ) -> Result<()> {
        let (backend, ep) = (inst.backend.as_str(), &inst.endpoints);
        let api_port = ep
            .ports
            .get(&ENV_PORT)
            .copied()
            .or_else(|| ep.ports.values().next().copied())
            .unwrap_or(ENV_PORT);
        let mut extra = Map::new();
        extra.insert("services".into(), serde_json::to_value(services)?);
        // What readiness meant, so a resume waits for it too.
        if !o.wait_for.is_empty() {
            extra.insert("ready_probes".into(), probes_to_json(&o.wait_for));
        }
        // `connect(name)` from another process needs it; the state file is
        // owner-only (StateStore writes every file 0600).
        if let Some(t) = env_token {
            extra.insert("env_token".into(), Value::String(t.into()));
        }
        // Console sources for `cua sb logs`.
        if let Some(l) = &ep.serial_log {
            extra.insert("serial_log".into(), Value::String(l.clone()));
        }
        if let Some(c) = &ep.container_id {
            extra.insert("container_id".into(), Value::String(c.clone()));
        }
        let port_of = |s: &Option<String>| {
            s.as_deref()
                .and_then(|a| a.rsplit(':').next())
                .and_then(|p| p.parse::<u16>().ok())
        };
        self.inner.state.save(&SandboxState::Local(LocalState {
            name: name.into(),
            runtime_type: backend.into(),
            image: registry_image_dict(&o.image, os, Some("vm")),
            host: ep.host.clone(),
            api_port,
            exposed_ports: Some(ep.ports.iter().map(|(g, h)| (g.to_string(), *h)).collect()),
            vnc_port: port_of(&ep.vnc),
            qmp_port: port_of(&ep.qmp),
            os_type: Some(os.to_string()),
            memory_mb: Some(o.memory_mb),
            cpu_count: Some(o.cpus),
            status: "running".into(),
            created_at: python_utc_now(),
            extra,
            ..Default::default()
        }))
    }

    /// Connects to a reachable machine by URL (`http(s)://host:port`, bare
    /// `host:port`, a Fleet service URL or a relay URL). No daemon is
    /// assumed and nothing is probed.
    pub fn connect_url(&self, url: &str, token: Option<String>) -> Result<Sandbox> {
        self.connect_url_named(url, token, None)
    }

    /// [`Self::connect_url`] under a display name (default: the host). The
    /// ref stays `direct:<host:port>`.
    pub fn connect_url_named(
        &self,
        url: &str,
        token: Option<String>,
        name: Option<&str>,
    ) -> Result<Sandbox> {
        let endpoint = cua_spacesd_client::Endpoint::parse(url)
            .map_err(|e| Error::InvalidArgument(e.to_string()))?;
        Ok(Sandbox {
            mgr: self.clone(),
            name: name
                .filter(|n| !n.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| endpoint.host().to_string()),
            ephemeral: true,
            env_token: token,
            bootstrap_token: Default::default(),
            services: [("env".to_string(), endpoint.port())].into(),
            image_info: None,
            target: Target::Direct { endpoint },
        })
    }

    /// The GPU options on `provider` (a contrib provider: `contrib`, its
    /// word): each local runtime's ([`LocalRuntime::gpu_support`]), a
    /// contrib provider's GPU types, none on Cua Cloud.
    pub async fn gpu_support(
        &self,
        provider: ProviderKind,
        contrib: Option<&str>,
    ) -> Vec<crate::gpu::GpuSupport> {
        match provider {
            ProviderKind::Local => match self.local() {
                Ok(rt) => rt.gpu_support().await,
                Err(_) => vec![],
            },
            ProviderKind::Fleet => vec![crate::gpu::GpuSupport::none("fleet", FLEET_NO_GPU)],
            ProviderKind::Contrib => contrib
                .and_then(|w| self.contrib_provider(w).ok())
                .map(|p| {
                    let gpus = p.capabilities().gpus;
                    vec![if gpus.is_empty() {
                        crate::gpu::GpuSupport::none(
                            p.name(),
                            format!("{} offers no GPU sandboxes through cua", p.name()),
                        )
                    } else {
                        crate::gpu::GpuSupport {
                            runtime: p.name().into(),
                            options: gpus,
                            reason: String::new(),
                        }
                    }]
                })
                .unwrap_or_default(),
            ProviderKind::Direct => vec![],
        }
    }

    /// Whether `name` is taken locally: a recorded sandbox, or an instance
    /// the local runtime has.
    pub async fn local_name_in_use(&self, name: &str) -> bool {
        if self.inner.state.load(name).is_some() {
            return true;
        }
        match self.local() {
            Ok(rt) => !matches!(rt.status(name).await, Err(crate::RuntimeError::NotFound(_))),
            Err(_) => false,
        }
    }

    /// Deletes local instance `name` straight through the local runtime,
    /// whether or not this machine recorded it: an instance whose create
    /// was cut off (its process died between starting it and recording
    /// it). `Ok(false)` when the runtime has no such instance.
    pub async fn delete_local_instance(&self, name: &str) -> Result<bool> {
        let rt = self.local()?;
        match rt.status(name).await {
            Err(crate::RuntimeError::NotFound(_)) => {
                let _ = self.inner.state.delete(name);
                Ok(false)
            }
            _ => {
                rt.delete(name).await?;
                let _ = self.inner.state.delete(name);
                Ok(true)
            }
        }
    }

    /// Reconnects to a named sandbox from its state file.
    pub async fn connect(&self, name: &str) -> Result<Sandbox> {
        let state = self
            .inner
            .state
            .load(name)
            .ok_or_else(|| Error::NotFound(name.into()))?;
        match state {
            SandboxState::Fleet(f) => {
                if f.status == "suspended" {
                    return Err(Error::InvalidArgument(format!(
                        "sandbox {} is suspended; resume it first",
                        f.name
                    )));
                }
                let fleet = self.fleet()?;
                let bound = fleet.attach_claim(&f.pool_name, &f.name).await?;
                // A claim on a managed pool has a TTL: keep it alive while
                // this handle lives (dropping the handle detaches).
                let lease = if is_managed_pool(&f.pool_name) {
                    self.pools()
                        .ok()
                        .map(|p| Lease::new(p.adopt(bound.clone(), None)))
                } else {
                    None
                };
                let (services, runtime) = match fleet.get_pool(&f.pool_name).await {
                    Ok(p) => (
                        fleet_services(fleet, &p.pool, &bound).await,
                        fleet.pool_runtime(&p.pool).await,
                    ),
                    Err(_) => (
                        bound.services.iter().map(|s| (s.clone(), 0u16)).collect(),
                        None,
                    ),
                };
                Ok(Sandbox {
                    mgr: self.clone(),
                    name: f.name.clone(),
                    ephemeral: false,
                    env_token: f
                        .extra
                        .get("env_token")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    bootstrap_token: Default::default(),
                    services,
                    image_info: image_info_of(&f.extra),
                    target: Target::Fleet {
                        pool: f.pool_name,
                        bound,
                        lease,
                        runtime,
                    },
                })
            }
            s @ SandboxState::Local(_) if contrib::contrib_of(&s).is_some() => {
                let SandboxState::Local(l) = s else {
                    unreachable!()
                };
                self.connect_contrib(l).await
            }
            SandboxState::Local(l) if l.runtime_type == "direct" => {
                let url = l
                    .extra
                    .get("url")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Error::InvalidArgument("direct state without url".into()))?;
                let mut sb = self.connect_url(url, None)?;
                sb.name = l.name;
                Ok(sb)
            }
            SandboxState::Local(l) => {
                let rt = self.local()?;
                let status = rt.status(&l.name).await?;
                if status != InstanceStatus::Running {
                    return Err(Error::InvalidArgument(format!(
                        "sandbox {} is {status:?}; resume it first",
                        l.name
                    )));
                }
                let endpoints = rt.endpoints(&l.name).await?;
                let services = l
                    .extra
                    .get("services")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_else(|| [("env".to_string(), ENV_PORT)].into());
                Ok(Sandbox {
                    mgr: self.clone(),
                    name: l.name,
                    ephemeral: false,
                    env_token: l
                        .extra
                        .get("env_token")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    bootstrap_token: Default::default(),
                    services,
                    image_info: image_info_of(&l.extra),
                    target: Target::Local {
                        backend: l.runtime_type,
                        endpoints,
                    },
                })
            }
        }
    }

    /// Connects to the sandbox a qualified ref names (a bare name is looked
    /// up with [`Self::resolve_ref`] first): `local:` and a persisted
    /// `cloud:` reattach from their state file, any other `cloud:` claim of
    /// the account is attached by name, `direct:` reuses a remembered
    /// connection to that address or connects to it. `relay:` machines are
    /// Spaces ([`Error::Unsupported`] here).
    pub async fn connect_ref(&self, wanted: &SandboxRef) -> Result<Sandbox> {
        let r = match wanted {
            SandboxRef::Bare { .. } => self.resolve_ref(wanted).await?,
            other => other.clone(),
        };
        match &r {
            SandboxRef::Local { name } => match self.inner.state.load(name) {
                Some(s @ SandboxState::Local(_)) if state_ref(&s) == r => self.connect(name).await,
                Some(_) => Err(Error::NotFound(r.to_string())),
                None => self.connect(name).await,
            },
            SandboxRef::Cloud { name, namespace } => match self.inner.state.load(name) {
                Some(SandboxState::Fleet(_)) => self.connect(name).await,
                _ => self.connect_cloud(name, namespace.as_deref()).await,
            },
            SandboxRef::Direct { authority } => {
                let named = self
                    .inner
                    .state
                    .list_all()
                    .into_iter()
                    .find(|s| s.runtime_type() == "direct" && state_ref(s) == r);
                match named {
                    Some(s) => self.connect(s.name()).await,
                    None => self.connect_url(&format!("http://{authority}"), None),
                }
            }
            SandboxRef::Contrib { name, .. } => match self.inner.state.load(name) {
                Some(s) if state_ref(&s) == r => self.connect(name).await,
                _ => Err(Error::NotFound(r.to_string())),
            },
            SandboxRef::Relay { .. } => Err(Error::Unsupported {
                provider: ProviderKind::Direct,
                op: format!(
                    "{r}: relay machines are Spaces; open it with the Spaces API or `cua mcp` \
                     (space {r})"
                ),
            }),
            SandboxRef::Bare { name } => self.connect(name).await,
        }
    }

    /// Attaches to the account's cloud sandbox (Fleet claim) `name`, whether
    /// or not this machine created it. `namespace` is a lookup hint; without
    /// it the claim is found across the account's pools. The handle holds
    /// no lease: dropping it leaves the claim alone, `delete` releases it.
    pub async fn connect_cloud(&self, name: &str, namespace: Option<&str>) -> Result<Sandbox> {
        let fleet = self.fleet()?;
        let namespace = match namespace {
            Some(ns) => ns.to_string(),
            None => {
                let mut found = fleet.find_claims(name).await?;
                match found.len() {
                    0 => return Err(Error::NotFound(format!("cloud:{name}"))),
                    1 => found.remove(0).metadata.namespace,
                    _ => {
                        return Err(Error::InvalidArgument(format!(
                            "cloud:{name} names claims in {} pools (created before names were \
                             unique); delete one of them",
                            found.len()
                        )));
                    }
                }
            }
        };
        let bound = fleet.attach_claim(&namespace, name).await?;
        let (services, runtime) = match fleet.get_pool(&namespace).await {
            Ok(p) => (
                fleet_services(fleet, &p.pool, &bound).await,
                fleet.pool_runtime(&p.pool).await,
            ),
            Err(_) => (
                bound.services.iter().map(|s| (s.clone(), 0u16)).collect(),
                None,
            ),
        };
        Ok(Sandbox {
            mgr: self.clone(),
            name: name.to_string(),
            ephemeral: false,
            env_token: None,
            bootstrap_token: Default::default(),
            services,
            target: Target::Fleet {
                pool: namespace,
                bound,
                lease: None,
                runtime,
            },
            // Attached, not created here: the resolved image is not known.
            image_info: None,
        })
    }

    /// The image this machine recorded when it created cloud sandbox `name`
    /// in pool `pool` (the requested reference and its resolved digest), from
    /// the state file alone; `None` for a claim created elsewhere.
    pub fn recorded_cloud_image(&self, name: &str, pool: &str) -> Option<ImageInfo> {
        match self.inner.state.load(name)? {
            SandboxState::Fleet(f) if f.pool_name == pool => image_info_of(&f.extra),
            _ => None,
        }
    }

    /// The qualified refs of every sandbox this machine knows (state files
    /// and the local runtime's instances), without asking Fleet.
    pub async fn known_refs(&self) -> Result<Vec<SandboxRef>> {
        Ok(self
            .known_named()
            .await?
            .into_iter()
            .map(|(_, r)| r)
            .collect())
    }

    /// [`Self::known_refs`] with each sandbox's display name.
    async fn known_named(&self) -> Result<Vec<(String, SandboxRef)>> {
        let mut out: Vec<(String, SandboxRef)> = self
            .inner
            .state
            .list_all()
            .iter()
            .map(|s| (s.name().to_string(), state_ref(s)))
            .collect();
        if let Some(rt) = &self.inner.local {
            for i in rt.list().await? {
                let r = SandboxRef::Local {
                    name: i.name.clone(),
                };
                if !out.iter().any(|(_, k)| *k == r) {
                    out.push((i.name, r));
                }
            }
        }
        Ok(out)
    }

    /// Resolves a ref to a qualified one. A qualified ref comes back as is
    /// (a persisted cloud sandbox gains its namespace hint). A bare name is
    /// searched across every location: the sandboxes this machine knows
    /// and, when Fleet is configured, the account's live cloud sandboxes
    /// (bounded by [`LOOKUP_CLOUD_TIMEOUT`]; a Fleet failure only leaves
    /// them out). It must match exactly one, else
    /// [`Error::AmbiguousSandbox`] lists the qualified candidates.
    pub async fn resolve_ref(&self, wanted: &SandboxRef) -> Result<SandboxRef> {
        self.resolve_ref_among(wanted, vec![]).await
    }

    /// [`Self::resolve_ref`], also considering `extra` refs (sandboxes a
    /// caller holds that have no state file, such as ephemeral handles).
    pub async fn resolve_ref_among(
        &self,
        wanted: &SandboxRef,
        extra: Vec<(String, SandboxRef)>,
    ) -> Result<SandboxRef> {
        if wanted.is_qualified() {
            // Keep the persisted namespace hint of a cloud sandbox.
            let known = self
                .inner
                .state
                .list_all()
                .iter()
                .map(state_ref)
                .find(|k| k == wanted);
            return Ok(known.unwrap_or_else(|| wanted.clone()));
        }
        let mut known = extra;
        known.extend(self.known_named().await?);
        if let (SandboxRef::Bare { name }, Some(fleet)) = (wanted, &self.inner.fleet) {
            match tokio::time::timeout(LOOKUP_CLOUD_TIMEOUT, fleet.find_claims(name)).await {
                Ok(Ok(claims)) => known.extend(claims.into_iter().map(|c| {
                    (
                        c.metadata.name.clone(),
                        SandboxRef::Cloud {
                            name: c.metadata.name,
                            namespace: Some(c.metadata.namespace),
                        },
                    )
                })),
                Ok(Err(e)) => tracing::debug!(name, error = %e, "cloud sandboxes not searched"),
                Err(_) => tracing::debug!(name, "cloud sandbox search timed out"),
            }
        }
        crate::refs::resolve_named(wanted, known)
    }

    /// Persists a direct connection under `name` so `connect(name)` works.
    pub fn remember_direct(&self, name: &str, url: &str) -> Result<()> {
        let endpoint = cua_spacesd_client::Endpoint::parse(url)
            .map_err(|e| Error::InvalidArgument(e.to_string()))?;
        let mut extra = Map::new();
        extra.insert("url".into(), Value::String(url.into()));
        self.inner.state.save(&SandboxState::Local(LocalState {
            name: name.into(),
            runtime_type: "direct".into(),
            image: Value::Null,
            host: endpoint.host().into(),
            api_port: endpoint.port(),
            status: "running".into(),
            created_at: python_utc_now(),
            extra,
            ..Default::default()
        }))
    }

    /// Every known sandbox: state files plus instances the local runtime
    /// manages.
    pub async fn list(&self) -> Result<Vec<SandboxInfo>> {
        let mut out: BTreeMap<String, SandboxInfo> = BTreeMap::new();
        for s in self.inner.state.list_all() {
            let provider = provider_of(&s);
            let id = state_ref(&s).to_string();
            out.insert(
                id.clone(),
                SandboxInfo {
                    id,
                    name: s.name().into(),
                    provider,
                    runtime_type: s.runtime_type().into(),
                    status: status_word(s.status()),
                },
            );
        }
        if let Some(rt) = &self.inner.local {
            let listed = rt.list().await?;
            // Local state files whose VM or container is gone: `missing`,
            // so `ls` shows the stale record and `rm` clears it. Only a
            // definite not-found counts; an unreachable engine keeps the
            // recorded status.
            for s in self.inner.state.list_all() {
                if provider_of(&s) != ProviderKind::Local
                    || listed.iter().any(|i| i.name == s.name())
                {
                    continue;
                }
                if let Err(crate::RuntimeError::NotFound(_)) =
                    rt.status_on(s.name(), s.runtime_type()).await
                    && let Some(row) = out.get_mut(&state_ref(&s).to_string())
                {
                    row.status = Status::Unknown(MISSING.into());
                }
            }
            for i in listed {
                let id = format!("local:{}", i.name);
                // The state file says which runtime ran it (`gvisor`); the
                // backend listing only knows the backend (`container`).
                let runtime_type = match out.get(&id) {
                    Some(known) if !known.runtime_type.is_empty() => known.runtime_type.clone(),
                    _ => i.backend,
                };
                out.insert(
                    id.clone(),
                    SandboxInfo {
                        id,
                        name: i.name,
                        provider: ProviderKind::Local,
                        runtime_type,
                        status: i.status.into(),
                    },
                );
            }
        }
        Ok(out.into_values().collect())
    }

    /// One sandbox's info (live status for local sandboxes).
    pub async fn get(&self, name: &str) -> Result<SandboxInfo> {
        let state = self.inner.state.load(name);
        match &state {
            Some(s) if contrib::contrib_of(s).is_some() => {
                let (provider, id) = self.contrib_target(s)?;
                let status = match provider.get(&id).await {
                    Ok(i) => i.status,
                    Err(Error::NotFound(_)) => Status::Unknown("gone".into()),
                    Err(e) => return Err(e),
                };
                Ok(SandboxInfo {
                    id: state_ref(s).to_string(),
                    name: name.into(),
                    provider: ProviderKind::Contrib,
                    runtime_type: s.runtime_type().into(),
                    status,
                })
            }
            Some(SandboxState::Local(l)) if l.runtime_type != "direct" => {
                let status = match self.local()?.status_on(name, &l.runtime_type).await {
                    Ok(s) => s.into(),
                    Err(crate::RuntimeError::NotFound(_)) => Status::Unknown(MISSING.into()),
                    Err(e) => return Err(e.into()),
                };
                Ok(SandboxInfo {
                    id: format!("local:{name}"),
                    name: name.into(),
                    provider: ProviderKind::Local,
                    runtime_type: l.runtime_type.clone(),
                    status,
                })
            }
            Some(s) => Ok(SandboxInfo {
                id: state_ref(s).to_string(),
                name: name.into(),
                provider: if s.runtime_type() == "fleet" {
                    ProviderKind::Fleet
                } else {
                    ProviderKind::Direct
                },
                runtime_type: s.runtime_type().into(),
                status: status_word(s.status()),
            }),
            None => {
                if let Some(rt) = &self.inner.local
                    && let Some(i) = rt.list().await?.into_iter().find(|i| i.name == name)
                {
                    return Ok(SandboxInfo {
                        id: format!("local:{}", i.name),
                        name: i.name,
                        provider: ProviderKind::Local,
                        runtime_type: i.backend,
                        status: i.status.into(),
                    });
                }
                Err(Error::NotFound(name.into()))
            }
        }
    }

    fn kind_of(&self, name: &str) -> Result<(ProviderKind, SandboxState)> {
        let s = self
            .inner
            .state
            .load(name)
            .ok_or_else(|| Error::NotFound(name.into()))?;
        Ok((provider_of(&s), s))
    }

    /// Suspends by name (local: pause; Fleet: scale the pool to 0, as
    /// cua-sandbox does).
    pub async fn suspend(&self, name: &str) -> Result<()> {
        match self.kind_of(name)? {
            (ProviderKind::Local, _) => {
                self.local()?.suspend(name).await?;
                self.inner.state.set_status(name, "suspended")
            }
            // Fleet has no per-claim pause or stop, and a pool's replicas
            // are shared by every claim in it: never scale one.
            (ProviderKind::Fleet, _) => Err(fleet_lifecycle_unsupported("suspend")),
            (ProviderKind::Contrib, s) => {
                let (provider, id) = self.contrib_target(&s)?;
                provider.suspend(&id).await?;
                self.inner.state.set_status(name, "suspended")
            }
            (kind, _) => Err(Error::Unsupported {
                provider: kind,
                op: "suspend".into(),
            }),
        }
    }

    /// Resumes by name.
    pub async fn resume(&self, name: &str) -> Result<()> {
        match self.kind_of(name)? {
            (ProviderKind::Local, _) => {
                self.local()?.resume(name).await?;
                self.inner.state.set_status(name, "running")?;
                // A resumed VM reports running once it has an address; what
                // its create waited for (a Space's cua-spacesd: 20 to 40 s
                // later in a macOS guest) comes after. Resume waits for the
                // same probes, bounded, so the next call is not a transport
                // error.
                let probes = self
                    .inner
                    .state
                    .load_raw(name)
                    .and_then(|m| m.get("ready_probes").cloned())
                    .map(probes_from_json)
                    .unwrap_or_default();
                if !probes.is_empty() {
                    let sb = self.connect(name).await?;
                    sb.wait_ready(&probes, RESUME_READY_TIMEOUT).await?;
                }
                Ok(())
            }
            // A running claim needs no resume (it reattaches); Fleet cannot
            // bring back a suspended one.
            (ProviderKind::Fleet, SandboxState::Fleet(f)) => {
                if f.status != "suspended"
                    && self
                        .fleet()?
                        .attach_claim(&f.pool_name, &f.name)
                        .await
                        .is_ok()
                {
                    return Ok(());
                }
                Err(fleet_lifecycle_unsupported("resume"))
            }
            (ProviderKind::Contrib, s) => {
                let (provider, id) = self.contrib_target(&s)?;
                provider.resume(&id).await?;
                self.inner.state.set_status(name, "running")
            }
            (kind, _) => Err(Error::Unsupported {
                provider: kind,
                op: "resume".into(),
            }),
        }
    }

    /// Restarts by name (stop + resume locally; not supported on Fleet).
    pub async fn restart(&self, name: &str) -> Result<()> {
        match self.kind_of(name)? {
            (ProviderKind::Local, _) => {
                let rt = self.local()?;
                rt.stop(name).await?;
                rt.resume(name).await?;
                self.inner.state.set_status(name, "running")
            }
            (ProviderKind::Fleet, _) => Err(fleet_lifecycle_unsupported("restart")),
            (kind, _) => Err(Error::Unsupported {
                provider: kind,
                op: "restart".into(),
            }),
        }
    }

    /// How the sandbox `name` turns off and on again (`None`: it cannot,
    /// or there is no such sandbox): a local instance as its runtime says
    /// ([`LocalRuntime::power_control`]), a contrib one as its provider
    /// says ([`crate::Provider::power`]). Fleet claims and direct
    /// sandboxes cannot. Reads only the state file.
    pub fn power_control(&self, name: &str) -> Option<crate::PowerControl> {
        let s = self.inner.state.load(name)?;
        match provider_of(&s) {
            ProviderKind::Local => self.inner.local.as_ref()?.power_control(s.runtime_type()),
            ProviderKind::Contrib => {
                let (word, _) = contrib::contrib_of(&s)?;
                self.inner.providers.get(word)?.power()
            }
            _ => None,
        }
    }

    /// Whether the sandbox `name` is on, as cua last recorded it (`None`:
    /// not recorded, or no such sandbox). Reads only the state file.
    pub fn power_state(&self, name: &str) -> Option<crate::PowerState> {
        let s = self.inner.state.load(name)?;
        crate::PowerState::parse(s.status())
    }

    fn power_unsupported(&self, name: &str) -> Error {
        match self.kind_of(name) {
            Ok((ProviderKind::Fleet, _)) => fleet_lifecycle_unsupported("turning off"),
            Ok((kind, s)) => Error::Unsupported {
                provider: kind,
                op: format!(
                    "turning {name} off and on ({} cannot suspend or stop it)",
                    s.runtime_type()
                ),
            },
            Err(e) => e,
        }
    }

    /// Turns the sandbox `name` off the way its provider can
    /// ([`Self::power_control`]): suspends it, keeping its memory, or stops
    /// it, keeping its disk. Returns the state it is left in.
    pub async fn power_off(&self, name: &str) -> Result<crate::PowerState> {
        let Some(control) = self.power_control(name) else {
            return Err(self.power_unsupported(name));
        };
        self.power_off_as(name, control).await
    }

    /// [`Self::power_off`] with `control` instead of the provider's own:
    /// a host stops (frees) a container it would otherwise suspend. A
    /// sandbox with no power control at all is refused.
    pub async fn power_off_as(
        &self,
        name: &str,
        control: crate::PowerControl,
    ) -> Result<crate::PowerState> {
        if self.power_control(name).is_none() {
            return Err(self.power_unsupported(name));
        }
        match control {
            crate::PowerControl::Suspend => self.suspend(name).await?,
            crate::PowerControl::Stop => {
                match self.kind_of(name)? {
                    (ProviderKind::Local, _) => self.local()?.stop(name).await?,
                    (ProviderKind::Contrib, s) => {
                        let (provider, id) = self.contrib_target(&s)?;
                        provider.stop(&id).await?;
                    }
                    _ => return Err(self.power_unsupported(name)),
                }
                self.inner.state.set_status(name, "stopped")?;
            }
        }
        Ok(control.off_state())
    }

    /// Turns the sandbox `name` on again: resumes a suspended one, boots a
    /// stopped one, and leaves a running one as it is. A local instance is
    /// waited for as [`Self::resume`] waits (its readiness probes, bounded).
    pub async fn power_on(&self, name: &str) -> Result<()> {
        let Some(control) = self.power_control(name) else {
            return Err(self.power_unsupported(name));
        };
        match self.kind_of(name)? {
            (ProviderKind::Local, s) => {
                let status = self.local()?.status_on(name, s.runtime_type()).await?;
                if status == InstanceStatus::Running {
                    return self.inner.state.set_status(name, "running");
                }
                self.resume(name).await
            }
            (ProviderKind::Contrib, s) => {
                let (provider, id) = self.contrib_target(&s)?;
                match control {
                    crate::PowerControl::Suspend => provider.resume(&id).await?,
                    crate::PowerControl::Stop => provider.start(&id).await?,
                };
                self.inner.state.set_status(name, "running")
            }
            _ => Err(self.power_unsupported(name)),
        }
    }

    /// Deletes by name: releases the claim / removes the instance, then the
    /// state file.
    ///
    /// Idempotent about the backing resource: a state file whose VM,
    /// container or claim is already gone is removed and the delete
    /// succeeds. A local instance with no state file (an orphan the local
    /// runtime still lists) is removed from the runtime.
    pub async fn delete(&self, name: &str) -> Result<()> {
        let Some(state) = self.inner.state.load(name) else {
            return match &self.inner.local {
                Some(rt) => match rt.delete(name).await {
                    Ok(()) => Ok(()),
                    Err(crate::RuntimeError::NotFound(_)) => Err(Error::NotFound(name.into())),
                    Err(e) => Err(e.into()),
                },
                None => Err(Error::NotFound(name.into())),
            };
        };
        match (provider_of(&state), state) {
            (ProviderKind::Local, _) => {
                match self.local()?.delete(name).await {
                    Ok(()) | Err(crate::RuntimeError::NotFound(_)) => {}
                    Err(e) => return Err(e.into()),
                }
                // As `Sandbox::delete`: an ephemeral sandbox's lease goes too.
                self.inner.state.remove_lease(name)?;
            }
            (ProviderKind::Fleet, SandboxState::Fleet(f)) => {
                // Idempotent: Fleet's 404 (claim already gone) is success.
                self.fleet()?.release(&f.pool_name, &f.name).await?;
            }
            (ProviderKind::Contrib, s) => {
                let (provider, id) = self.contrib_target(&s)?;
                match provider.delete(&id).await {
                    Ok(()) | Err(Error::NotFound(_)) => {}
                    Err(e) => return Err(e),
                }
            }
            _ => {}
        }
        self.inner.state.delete(name)
    }

    /// Extends a Fleet claim's lease.
    pub async fn keep_alive(&self, name: &str, duration: Duration) -> Result<()> {
        match self.kind_of(name)? {
            (ProviderKind::Fleet, SandboxState::Fleet(f)) => {
                self.fleet()?
                    .keep_alive(&f.pool_name, &f.name, duration)
                    .await?;
                Ok(())
            }
            (ProviderKind::Contrib, s) => {
                let (provider, id) = self.contrib_target(&s)?;
                provider.keep_alive(&id, duration).await
            }
            (kind, _) => Err(Error::Unsupported {
                provider: kind,
                op: "keep_alive".into(),
            }),
        }
    }
}

impl SandboxState {
    /// The qualified ref of this persisted sandbox (`local:`, `cloud:`, or
    /// `direct:<host:port>` for a remembered connection).
    pub fn sandbox_ref(&self) -> SandboxRef {
        state_ref(self)
    }
}

/// The qualified ref of a persisted sandbox.
fn state_ref(s: &SandboxState) -> SandboxRef {
    if let Some((provider, _)) = contrib::contrib_of(s) {
        return SandboxRef::Contrib {
            provider: provider.to_string(),
            name: s.name().to_string(),
        };
    }
    match s {
        SandboxState::Fleet(f) => SandboxRef::Cloud {
            name: f.name.clone(),
            namespace: Some(f.pool_name.clone()),
        },
        SandboxState::Local(l) if l.runtime_type == "direct" => {
            let authority = l
                .extra
                .get("url")
                .and_then(Value::as_str)
                .and_then(|u| cua_spacesd_client::Endpoint::parse(u).ok())
                .map(|e| crate::refs::authority(e.host(), e.port()))
                .unwrap_or_else(|| crate::refs::authority(&l.host, l.api_port));
            SandboxRef::Direct { authority }
        }
        SandboxState::Local(l) => SandboxRef::Local {
            name: l.name.clone(),
        },
    }
}

/// The provider of a persisted sandbox.
fn provider_of(s: &SandboxState) -> ProviderKind {
    if contrib::contrib_of(s).is_some() {
        return ProviderKind::Contrib;
    }
    match s.runtime_type() {
        "fleet" => ProviderKind::Fleet,
        "direct" => ProviderKind::Direct,
        _ => ProviderKind::Local,
    }
}

/// How long a bare-name lookup waits for Fleet before it resolves without
/// the account's live cloud sandboxes.
pub const LOOKUP_CLOUD_TIMEOUT: Duration = Duration::from_secs(5);

/// State-file fields of a sandbox on a managed pool.
/// The kind and runtime of a local backend label (`runtime_type`:
/// `gvisor`, `container`/`docker`/`runc`, `qemu`, `qemu-docker`, `lume`).
pub fn placement_of_backend(backend: &str) -> (Kind, Runtime) {
    match backend.trim().to_ascii_lowercase().as_str() {
        "gvisor" | "runsc" => (Kind::Container, Runtime::Gvisor),
        "container" | "docker" | "runc" | "managed" => (Kind::Container, Runtime::Runc),
        "qemu" | "qemu-docker" | "qemu_docker" | "qemu-baremetal" => (Kind::Vm, Runtime::Qemu),
        "lume" => (Kind::Vm, Runtime::Lume),
        _ => (Kind::Auto, Runtime::Auto),
    }
}

/// The local backend prefix of an image string (`container:`, `docker:`,
/// `vm:`, `disk:`, `lume:`) and the reference after it.
pub(crate) fn local_prefix(image: &str) -> Option<(&str, &str)> {
    let image = image.trim();
    match image.split_once(':') {
        Some((p, r))
            if ["container", "docker", "vm", "disk", "lume"].contains(&p)
                && !r.starts_with("//") =>
        {
            Some((p, r))
        }
        _ => None,
    }
}

fn managed_fields(o: &CreateOptions) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("managed".into(), Value::Bool(true));
    m.insert("image".into(), Value::String(o.image.clone()));
    m.insert("os".into(), Value::String(o.os.clone()));
    m.insert(
        "fleet_runtime".into(),
        serde_json::to_value(&o.fleet.runtime).unwrap_or(Value::Null),
    );
    m.insert("cpus".into(), Value::from(o.cpus));
    m.insert("memory_mb".into(), Value::from(o.memory_mb));
    m.insert(
        "services".into(),
        serde_json::to_value(&o.services).unwrap_or(Value::Null),
    );
    m.insert(
        "ports".into(),
        serde_json::to_value(&o.ports).unwrap_or(Value::Null),
    );
    if let Some(t) = o.fleet.ttl_seconds_after_created {
        m.insert("claim_ttl".into(), Value::from(t));
    }
    if let Some(n) = o.fleet.max_pool_size {
        m.insert("max_pool_size".into(), Value::from(n));
    }
    if let Some(c) = o.command.as_ref().filter(|c| !c.is_empty()) {
        m.insert(
            "command".into(),
            serde_json::to_value(c).unwrap_or(Value::Null),
        );
    }
    if !o.env.is_empty() {
        m.insert(
            "env".into(),
            serde_json::to_value(&o.env).unwrap_or(Value::Null),
        );
    }
    if !o.wait_for.is_empty() {
        m.insert(
            "readiness_tcp_port".into(),
            Value::from(probe_port(&o.wait_for[0])),
        );
    }
    m
}

/// Whether a pool is SDK-managed (`cua-auto-*`): shared, autoscaled by
/// KEDA, never suspended or deleted through a sandbox.
fn is_managed_pool(pool: &str) -> bool {
    pool.starts_with(cua_fleet::autopool::AUTO_POOL_PREFIX)
}

/// The status word of a local record whose VM or container is gone.
pub const MISSING: &str = "missing";

fn status_word(s: &str) -> Status {
    match s {
        "running" => Status::Running,
        "suspended" => Status::Suspended,
        "stopped" => Status::Stopped,
        other => Status::Unknown(other.into()),
    }
}

/// State-file key of the resolved [`ImageInfo`] (additive; older files
/// have none).
const IMAGE_INFO_KEY: &str = "image_info";

/// The [`ImageInfo`] a state file recorded, if any.
/// The guest OS a local sandbox's state records: the one its image resolved
/// to, else the requested one. A plain registry reference (not an alias such
/// as `macos`) leaves [`CreateOptions::os`] at its `linux` default, so a macOS
/// image created by reference (the Spaces app, the SDK) was recorded as
/// `linux`.
fn recorded_os(requested: &str, image: Option<&ImageInfo>) -> String {
    image
        .map(|i| i.os.trim())
        .filter(|os| !os.is_empty())
        .unwrap_or(requested)
        .to_string()
}

fn image_info_of(extra: &Map<String, Value>) -> Option<ImageInfo> {
    extra
        .get(IMAGE_INFO_KEY)
        .and_then(|v| serde_json::from_value(v.clone()).ok())
}

fn rand_u32() -> u32 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() ^ (d.as_secs() as u32))
        .unwrap_or(0);
    nanos.wrapping_mul(2_654_435_761) ^ std::process::id()
}

/// Where a guest port can be reached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PortTarget {
    /// Plain TCP.
    Addr {
        /// Host.
        host: String,
        /// Port.
        port: u16,
    },
    /// HTTP(S) only, through the Fleet gateway.
    Url(String),
}

/// A sandbox handle.
#[derive(Clone)]
pub struct Sandbox {
    mgr: Sandboxes,
    name: String,
    ephemeral: bool,
    env_token: Option<String>,
    /// Token this process installed on a bootstrap-mode spacesd.
    bootstrap_token: Arc<std::sync::Mutex<Option<String>>>,
    /// Logical service → guest port (0 when only the name is known).
    services: BTreeMap<String, u16>,
    /// The image as resolved and pinned at create time (`None`: direct,
    /// or not resolved from a registry). A named pool's is its template's.
    image_info: Option<ImageInfo>,
    target: Target,
}

impl std::fmt::Debug for Sandbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sandbox")
            .field("name", &self.name)
            .field("provider", &self.provider())
            .field("ephemeral", &self.ephemeral)
            .finish()
    }
}

impl Sandbox {
    /// Name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The image this sandbox runs, as resolved and pinned at create time
    /// (digest and variant). A claim on a named pool reports its template's
    /// image (with empty `pinned_ref`/`digest` when the registry could not
    /// be read). `None` for direct connections and images not resolved
    /// from a registry.
    pub fn image_info(&self) -> Option<&ImageInfo> {
        self.image_info.as_ref()
    }

    /// Provider.
    pub fn provider(&self) -> ProviderKind {
        match self.target {
            Target::Fleet { .. } => ProviderKind::Fleet,
            Target::Local { .. } => ProviderKind::Local,
            Target::Direct { .. } => ProviderKind::Direct,
            Target::Contrib { .. } => ProviderKind::Contrib,
        }
    }

    /// Runtime identifier (`fleet`, `direct`, or the local backend name).
    pub fn runtime_type(&self) -> &str {
        match &self.target {
            Target::Fleet { .. } => "fleet",
            Target::Local { backend, .. } => backend,
            Target::Direct { .. } => "direct",
            Target::Contrib { provider, .. } => provider.0.name(),
        }
    }

    /// Whether the sandbox has no state file (and is torn down on delete).
    pub fn is_ephemeral(&self) -> bool {
        self.ephemeral
    }

    /// The qualified ref, the same kind of value local and in the cloud:
    /// `local:<name>`, `cloud:<name>` or `direct:<host:port>` (Fleet
    /// claims, pools and namespaces stay in [`Self::provider_details`]).
    pub fn id(&self) -> String {
        self.sandbox_ref().to_string()
    }

    /// [`Self::id`] as a [`SandboxRef`].
    pub fn sandbox_ref(&self) -> SandboxRef {
        match &self.target {
            Target::Fleet { pool, bound, .. } => SandboxRef::Cloud {
                name: bound.claim.clone(),
                namespace: Some(pool.clone()),
            },
            Target::Local { .. } => SandboxRef::Local {
                name: self.name.clone(),
            },
            Target::Direct { endpoint } => SandboxRef::Direct {
                authority: crate::refs::authority(endpoint.host(), endpoint.port()),
            },
            Target::Contrib { provider, .. } => SandboxRef::Contrib {
                provider: provider.0.name().into(),
                name: self.name.clone(),
            },
        }
    }

    /// Where the sandbox runs: `local`, `cloud`, `direct` (a machine
    /// reached by address) or a contrib provider (`e2b`).
    pub fn location(&self) -> &'static str {
        if let Target::Contrib { provider, .. } = &self.target {
            return provider.0.name();
        }
        self.sandbox_ref()
            .location()
            .map(crate::Location::as_str)
            .unwrap_or("local")
    }

    /// When the sandbox expires unless kept alive (cloud sandboxes held by
    /// this handle: renewed while held). `None`: no expiry known.
    pub fn expires_at(&self) -> Option<std::time::SystemTime> {
        match &self.target {
            Target::Fleet { lease: Some(l), .. } => l.expires_at(),
            // A provider sandbox that ends itself (your cloud's time limit).
            Target::Contrib { instance, .. } => instance
                .details
                .get("expires")
                .and_then(|t| humantime::parse_rfc3339(t).ok()),
            _ => None,
        }
    }

    /// Provider internals for debugging and advanced use (Fleet: `pool`,
    /// `namespace`, `claim`, `sandbox`; local: `backend`, `container_id`,
    /// `host`; direct: `url`).
    pub fn provider_details(&self) -> BTreeMap<String, String> {
        let mut d = BTreeMap::new();
        match &self.target {
            Target::Fleet { pool, bound, .. } => {
                d.insert("provider".into(), "fleet".into());
                d.insert("pool".into(), pool.clone());
                d.insert("namespace".into(), bound.namespace.clone());
                d.insert("claim".into(), bound.claim.clone());
                d.insert("sandbox".into(), bound.name.clone());
                d.insert("managed".into(), is_managed_pool(pool).to_string());
            }
            Target::Local { backend, endpoints } => {
                d.insert("provider".into(), "local".into());
                d.insert("backend".into(), backend.clone());
                d.insert("host".into(), endpoints.host.clone());
                if let Some(c) = &endpoints.container_id {
                    d.insert("container_id".into(), c.clone());
                }
                if let Some(l) = &endpoints.serial_log {
                    d.insert("serial_log".into(), l.clone());
                }
                if let Some(v) = &endpoints.vnc {
                    d.insert("vnc".into(), v.clone());
                }
            }
            Target::Direct { endpoint } => {
                d.insert("provider".into(), "direct".into());
                d.insert(
                    "url".into(),
                    endpoint.base_url().as_str().trim_end_matches('/').into(),
                );
            }
            Target::Contrib { provider, instance } => {
                // `_`-prefixed details are the provider's private state
                // (access tokens), never shown.
                d.extend(
                    instance
                        .details
                        .iter()
                        .filter(|(k, _)| !k.starts_with('_'))
                        .map(|(k, v)| (k.clone(), v.clone())),
                );
                d.insert("provider".into(), provider.0.name().into());
                d.insert("id".into(), instance.id.clone());
                // The engine this instance runs on when the provider offers a
                // choice and reported it, else the provider's one.
                d.entry("runtime".into())
                    .or_insert_with(|| provider.0.capabilities().runtime.into());
            }
        }
        d
    }

    /// The kind and runtime this sandbox runs as, when known
    /// ([`Kind::Auto`] / [`Runtime::Auto`] otherwise).
    pub fn placement(&self) -> (Kind, Runtime) {
        match &self.target {
            Target::Fleet { runtime, .. } => match runtime {
                Some(RuntimeKind::Gvisor) => (Kind::Container, Runtime::Gvisor),
                Some(RuntimeKind::Kubevirt) => (Kind::Vm, Runtime::Kubevirt),
                _ => (Kind::Auto, Runtime::Auto),
            },
            Target::Local { backend, .. } => placement_of_backend(backend),
            Target::Direct { .. } => (Kind::Auto, Runtime::Auto),
            Target::Contrib { provider, instance } => {
                let caps = provider.0.capabilities();
                let kind = match caps.kinds.first() {
                    Some(crate::RunKind::Container) => Kind::Container,
                    Some(crate::RunKind::Vm) => Kind::Vm,
                    None => Kind::Auto,
                };
                let runtime = instance
                    .details
                    .get("runtime")
                    .map(String::as_str)
                    .unwrap_or(caps.runtime);
                (kind, Runtime::parse(runtime).unwrap_or(Runtime::Auto))
            }
        }
    }

    /// The bound Fleet sandbox, for Fleet sandboxes.
    pub fn fleet_sandbox(&self) -> Option<&BoundSandbox> {
        match &self.target {
            Target::Fleet { bound, .. } => Some(bound),
            _ => None,
        }
    }

    /// Declared services (name → guest port; 0 when only the name is known).
    pub fn services(&self) -> &BTreeMap<String, u16> {
        &self.services
    }

    /// Every reachable guest port.
    pub fn ports(&self) -> BTreeMap<u16, PortTarget> {
        let mut out = BTreeMap::new();
        match &self.target {
            Target::Local { endpoints, .. } => {
                for (g, h) in &endpoints.ports {
                    out.insert(
                        *g,
                        PortTarget::Addr {
                            host: endpoints.host.clone(),
                            port: *h,
                        },
                    );
                }
            }
            _ => {
                for p in self.services.values().filter(|p| **p != 0) {
                    if let Ok(t) = self.port(*p) {
                        out.insert(*p, t);
                    }
                }
            }
        }
        out
    }

    /// Where guest port `port` is reachable.
    pub fn port(&self, port: u16) -> Result<PortTarget> {
        match &self.target {
            Target::Local { endpoints, .. } => endpoints
                .ports
                .get(&port)
                .map(|h| PortTarget::Addr {
                    host: endpoints.host.clone(),
                    port: *h,
                })
                .ok_or_else(|| {
                    Error::InvalidArgument(format!("guest port {port} is not published"))
                }),
            Target::Direct { endpoint } => Ok(PortTarget::Addr {
                host: endpoint.host().into(),
                port,
            }),
            Target::Contrib { provider, instance } => {
                Ok(PortTarget::Url(provider.0.endpoint(instance, port)?.url))
            }
            Target::Fleet { bound, .. } => {
                let service = self
                    .services
                    .iter()
                    .find(|(_, p)| **p == port)
                    .map(|(n, _)| n.clone())
                    .unwrap_or_else(|| format!("port-{port}"));
                Ok(PortTarget::Url(
                    self.mgr.fleet()?.service_url(bound, &service)?,
                ))
            }
        }
    }

    /// A named service.
    pub fn service(&self, name: &str) -> Result<Service> {
        let base = match &self.target {
            Target::Fleet { bound, .. } => self.mgr.fleet()?.service_url(bound, name)?,
            Target::Direct { endpoint } if name == "env" => endpoint
                .base_url()
                .as_str()
                .trim_end_matches('/')
                .to_string(),
            _ => {
                let port = *self.services.get(name).ok_or_else(|| {
                    Error::InvalidArgument(format!(
                        "sandbox {} has no service {name:?} (known: {:?})",
                        self.name,
                        self.services.keys().collect::<Vec<_>>()
                    ))
                })?;
                match self.port(port)? {
                    PortTarget::Addr { host, port } => format!("http://{}", host_port(&host, port)),
                    PortTarget::Url(u) => u,
                }
            }
        };
        Ok(Service {
            sandbox: self.clone(),
            name: name.into(),
            base_url: base,
        })
    }

    /// TCP forwarding.
    pub fn tunnel(&self) -> Tunnel {
        Tunnel {
            sandbox: self.clone(),
        }
    }

    /// Connects to cua-spacesd inside the sandbox (the `env` service, or
    /// guest port 3211). Fails with [`Error::SpacesdNotAvailable`] when no
    /// driver answers the capabilities probe.
    pub async fn spacesd(&self) -> Result<SpacesdClient> {
        self.spacesd_with(ConnectOptionsOverride::default()).await
    }

    /// [`Sandbox::spacesd`] with a shorter probe timeout (tests, UIs).
    pub async fn spacesd_with(&self, o: ConnectOptionsOverride) -> Result<SpacesdClient> {
        let held = self.token();
        let client = self.env_connect(&o, held.clone()).await?;
        if matches!(self.target, Target::Direct { .. }) {
            return Ok(client);
        }
        let local = matches!(self.target, Target::Local { .. });
        // A contrib sandbox whose token was delivered at create is like a
        // local one (verify it took); otherwise like Fleet.
        let delivered = match &self.target {
            Target::Contrib { provider, .. } => provider.0.capabilities().env_to_entrypoint,
            _ => local,
        };
        if held.is_some() && !delivered {
            return Ok(client);
        }
        // A driver in bootstrap mode (no token yet, as Fleet images start,
        // or a local image that ignores the token the SDK delivered)
        // accepts the first authenticated Init: install the held token (or
        // a fresh one) so the sandbox is never left open, remember it, and
        // reconnect with it. The capabilities are cached by the client.
        match client.capabilities().await {
            Ok(caps) if !caps.initialized => {}
            _ => return Ok(client),
        }
        let token = held.unwrap_or_else(mint_env_token);
        client
            .init(cua_spacesd_client::pb::InitRequest {
                token: token.clone(),
                ..Default::default()
            })
            .await?;
        *self.bootstrap_token.lock().unwrap() = Some(token.clone());
        if !self.ephemeral {
            let mut m = Map::new();
            m.insert("env_token".into(), Value::String(token.clone()));
            let state = self.mgr.state();
            state.update(&self.name, m)?;
            state.restrict(&self.name)?;
        }
        self.env_connect(&o, Some(token)).await
    }

    /// Whether this sandbox has no cua-spacesd to reach: it declares no
    /// `env` service, or its image says it carries none
    /// ([`ImageInfo::spacesd`], so the port was never published). The
    /// agentless fallbacks
    /// ([`Sandbox::guest_exec`], [`Sandbox::guest_screenshot`],
    /// [`Sandbox::guest_display`]) are for these sandboxes.
    pub fn lacks_spacesd(&self) -> bool {
        !self.services.contains_key("env")
            || self.image_info.as_ref().and_then(|i| i.spacesd) == Some(false)
    }

    fn agentless_unsupported(&self, op: &str) -> Error {
        Error::Unsupported {
            provider: self.provider(),
            op: format!("{op} without cua-spacesd (local Lume sandboxes only)"),
        }
    }

    /// Runs `script` with `/bin/sh -c` in the guest without cua-spacesd
    /// (local macOS Lume sandboxes: over `lume ssh`), forwarding output to
    /// `sink` as it arrives, and returns the exit code. Other sandboxes fail
    /// with [`Error::Unsupported`] (or the runtime's own `Unsupported`).
    pub async fn guest_exec(
        &self,
        script: &str,
        timeout: Option<Duration>,
        sink: tokio::sync::mpsc::Sender<crate::GuestOutput>,
    ) -> Result<i64> {
        match &self.target {
            Target::Local { .. } => Ok(self
                .mgr
                .local()?
                .guest_exec(&self.name, script, timeout, sink)
                .await?),
            _ => Err(self.agentless_unsupported("exec")),
        }
    }

    /// Captures the guest framebuffer as PNG without cua-spacesd (local
    /// Lume sandboxes: over the VM's VNC endpoint).
    pub async fn guest_screenshot(&self) -> Result<crate::GuestScreenshot> {
        match &self.target {
            Target::Local { .. } => Ok(self.mgr.local()?.guest_screenshot(&self.name).await?),
            _ => Err(self.agentless_unsupported("screenshots")),
        }
    }

    /// The guest display without cua-spacesd (local Lume sandboxes: the
    /// VM's VNC endpoint, opened with `lume attach`).
    pub async fn guest_display(&self) -> Result<crate::GuestDisplay> {
        match &self.target {
            Target::Local { .. } => Ok(self.mgr.local()?.guest_display(&self.name).await?),
            _ => Err(self.agentless_unsupported("a display")),
        }
    }

    /// The spacesd token this handle holds: the one set at create time or
    /// installed on a bootstrap-mode spacesd. `None` until one is known.
    pub fn env_token(&self) -> Option<String> {
        self.token()
    }

    fn token(&self) -> Option<String> {
        self.env_token
            .clone()
            .or_else(|| self.bootstrap_token.lock().unwrap().clone())
    }

    async fn env_connect(
        &self,
        o: &ConnectOptionsOverride,
        token: Option<String>,
    ) -> Result<SpacesdClient> {
        let not_available = |reason: String| Error::SpacesdNotAvailable {
            sandbox: self.name.clone(),
            reason,
        };
        let mut opts = match &self.target {
            Target::Fleet { bound, .. } => {
                if !bound.services.iter().any(|s| s == "env") {
                    return Err(not_available(format!(
                        "the sandbox exposes no `env` service (services: {:?})",
                        bound.services
                    )));
                }
                self.mgr
                    .fleet()?
                    .env_connect_options(bound, "env", token.clone())?
            }
            Target::Local { endpoints, .. } => {
                let port = self.services.get("env").copied().unwrap_or(ENV_PORT);
                let Some(host_port_n) = endpoints.ports.get(&port) else {
                    return Err(not_available(format!("guest port {port} is not published")));
                };
                let mut o = ConnectOptions::parse(&host_port(&endpoints.host, *host_port_n))
                    .map_err(|e| not_available(e.to_string()))?;
                o.token = token.clone();
                o
            }
            Target::Direct { endpoint } => {
                let mut o = ConnectOptions::new(endpoint.clone());
                o.token = token.clone();
                o
            }
            Target::Contrib { provider, instance } => {
                let port = self.services.get("env").copied().unwrap_or(ENV_PORT);
                if !self.services.contains_key("env") {
                    return Err(not_available(format!(
                        "the sandbox exposes no `env` service (services: {:?})",
                        self.services.keys().collect::<Vec<_>>()
                    )));
                }
                if let Some(o) = provider.0.connect_options(instance, port) {
                    let mut o = o.map_err(|e| not_available(e.to_string()))?;
                    o.token = token.clone();
                    o
                } else {
                    let ep = provider
                        .0
                        .endpoint(instance, port)
                        .map_err(|e| not_available(e.to_string()))?;
                    if !ep.headers.is_empty() {
                        return Err(not_available(format!(
                            "{} exposes port {port} only with its own access headers",
                            provider.0.name()
                        )));
                    }
                    let mut o =
                        ConnectOptions::parse(&ep.url).map_err(|e| not_available(e.to_string()))?;
                    o.token = token.clone();
                    o
                }
            }
        };
        if let Some(t) = o.probe_timeout {
            opts.probe_timeout = t;
            opts.connect_timeout = opts.connect_timeout.min(t);
        }
        match SpacesdClient::connect(opts).await {
            Ok(c) => Ok(c),
            Err(cua_spacesd_client::Error::SpacesdNotAvailable { reason, .. }) => {
                Err(not_available(reason))
            }
            Err(e @ cua_spacesd_client::Error::Unauthenticated(_)) => Err(Error::Env(e)),
            Err(e) => Err(not_available(e.to_string())),
        }
    }

    /// Whether readiness includes cua-spacesd: the image declares it
    /// ([`ImageInfo::spacesd`]), the sandbox exposes `env`, and no command
    /// replaces the image's entrypoint (which may be what starts spacesd).
    pub fn declares_spacesd(&self, options: &CreateOptions) -> bool {
        self.image_info.as_ref().and_then(|i| i.spacesd) == Some(true)
            && self.services.contains_key("env")
            && options.command.as_ref().is_none_or(|c| c.is_empty())
    }

    /// Waits until the sandbox's cua-spacesd answers `SystemService.Health`
    /// (any status: the desktop may still be starting), for at most
    /// `timeout`. Fails fast when the instance exits.
    pub async fn wait_spacesd(&self, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        let mut last: String;
        loop {
            let probe = ConnectOptionsOverride {
                probe_timeout: Some(Duration::from_secs(3)),
            };
            match self.spacesd_with(probe).await {
                Ok(client) => match client.health().await {
                    Ok(_) => return Ok(()),
                    Err(e @ cua_spacesd_client::Error::Unauthenticated(_)) => {
                        return Err(Error::Env(e));
                    }
                    Err(e) => last = e.to_string(),
                },
                // A wrong token never heals by waiting.
                Err(e @ Error::Env(cua_spacesd_client::Error::Unauthenticated(_))) => {
                    return Err(e);
                }
                Err(Error::SpacesdNotAvailable { reason, .. }) => last = reason,
                Err(e) => last = e.to_string(),
            }
            if let Some(why) = self.exited().await {
                return Err(Error::Runtime(crate::RuntimeError::Other(format!(
                    "sandbox {} {why} before cua-spacesd answered; see `cua sb logs {}`",
                    self.name, self.name
                ))));
            }
            if Instant::now() >= deadline {
                return Err(Error::Timeout(format!(
                    "sandbox {}: cua-spacesd did not answer within {timeout:?} ({last}); \
                     see `cua sb logs {}`",
                    self.name, self.name
                )));
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    /// Waits until every probe passes or `timeout` elapses.
    pub async fn wait_ready(&self, probes: &[Probe], timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        for probe in probes {
            loop {
                let ok = self.probe_once(probe).await;
                if ok {
                    break;
                }
                // A probe can never pass once the instance is gone: fail
                // fast instead of waiting out the budget.
                if let Some(why) = self.exited().await {
                    return Err(Error::Runtime(crate::RuntimeError::Other(format!(
                        "sandbox {} {why} before probe {probe:?} passed; see `cua sb logs {}`",
                        self.name, self.name
                    ))));
                }
                if Instant::now() >= deadline {
                    return Err(Error::Timeout(format!(
                        "sandbox {} probe {probe:?} did not pass within {timeout:?}",
                        self.name
                    )));
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
        Ok(())
    }

    /// `Some(reason)` when the instance behind this handle has stopped
    /// (local runtimes; Fleet claims report through the gateway).
    async fn exited(&self) -> Option<String> {
        if let Target::Contrib { provider, instance } = &self.target {
            return match provider.0.get(&instance.id).await {
                Ok(i) if i.status == Status::Stopped => Some("exited".into()),
                Err(Error::NotFound(_)) => Some("disappeared".into()),
                _ => None,
            };
        }
        let Target::Local { .. } = &self.target else {
            return None;
        };
        let rt = self.mgr.local().ok()?;
        match rt.status(&self.name).await {
            Ok(InstanceStatus::Stopped) => Some("exited".into()),
            Err(crate::RuntimeError::NotFound(_)) => Some("disappeared".into()),
            _ => None,
        }
    }

    async fn probe_once(&self, probe: &Probe) -> bool {
        let per_try = Duration::from_secs(10);
        if let (Probe::Tcp(guest_port), Target::Local { .. }) = (probe, &self.target) {
            // Ask the runtime first: a host-side connect through a
            // container's published port or QEMU slirp succeeds before the
            // guest listens.
            if let Ok(rt) = self.mgr.local() {
                match rt.guest_tcp_listening(&self.name, *guest_port).await {
                    Ok(Some(listening)) => return listening,
                    Ok(None) => {}
                    Err(_) => return false,
                }
            }
        }
        if let Target::Contrib { provider, instance } = &self.target {
            let Ok(ep) = provider.0.endpoint(instance, probe_port(probe)) else {
                return false;
            };
            let (path, want) = match probe {
                Probe::Tcp(_) => ("/", None),
                Probe::Http { path, status, .. } => (path.as_str(), Some(*status)),
            };
            let resp = self
                .mgr
                .inner
                .http
                .request("GET", &ep.url_for(path), &ep.headers, None, per_try)
                .await;
            return match (resp, want) {
                // Through the provider's proxy a TCP probe means the port's
                // server answers (the proxy's own errors are 5xx).
                (Ok(r), None) => r.status < 500,
                (Ok(r), Some(Some(s))) => r.status == s,
                (Ok(r), Some(None)) => (200..300).contains(&r.status),
                (Err(_), _) => false,
            };
        }
        match (probe, self.port(probe_port(probe))) {
            (Probe::Tcp(_), Ok(PortTarget::Addr { host, port })) => {
                tokio::time::timeout(per_try, tcp_alive(&host, port))
                    .await
                    .unwrap_or(false)
            }
            (Probe::Tcp(_), Ok(PortTarget::Url(url))) => self
                .http_get(&url, per_try)
                .await
                .is_ok_and(|r| r.status < 500),
            (Probe::Http { path, status, .. }, Ok(target)) => {
                let base = match target {
                    PortTarget::Addr { host, port } => format!("http://{}", host_port(&host, port)),
                    PortTarget::Url(u) => u,
                };
                let url = format!("{}{}", base.trim_end_matches('/'), path);
                match self.http_get(&url, per_try).await {
                    Ok(r) => match status {
                        Some(s) => r.status == *s,
                        None => (200..300).contains(&r.status),
                    },
                    Err(_) => false,
                }
            }
            (_, Err(_)) => false,
        }
    }

    async fn http_get(&self, url: &str, timeout: Duration) -> Result<HttpResponse> {
        match &self.target {
            Target::Fleet { bound, .. } => {
                // Route through the SDK so the bearer and claim header apply.
                let fleet = self.mgr.fleet()?;
                let base_prefix = format!(
                    "{}/api/svc/{}/{}-",
                    fleet.config().base_url.trim_end_matches('/'),
                    bound.namespace,
                    bound.name
                );
                let rest = url.strip_prefix(&base_prefix).unwrap_or_default();
                let (service, path) = match rest.find('/') {
                    Some(i) => (&rest[..i], &rest[i..]),
                    None => (rest, "/"),
                };
                let r = fleet
                    .service_request(bound, service, path, "GET", None, Some(timeout))
                    .await?;
                Ok(HttpResponse {
                    status: r.status,
                    headers: r.headers.into_iter().map(|h| (h.name, h.value)).collect(),
                    body: r.body,
                })
            }
            _ => {
                self.mgr
                    .inner
                    .http
                    .request("GET", url, &[], None, timeout)
                    .await
            }
        }
    }

    /// Status from the provider.
    pub async fn status(&self) -> Result<Status> {
        match &self.target {
            Target::Local { .. } => Ok(self.mgr.local()?.status(&self.name).await?.into()),
            Target::Contrib { provider, instance } => {
                Ok(provider.0.get(&instance.id).await?.status)
            }
            Target::Fleet { .. } | Target::Direct { .. } => Ok(Status::Running),
        }
    }

    /// Suspends (named sandboxes; see [`Sandboxes::suspend`]).
    pub async fn suspend(&self) -> Result<()> {
        match &self.target {
            Target::Contrib { provider, instance } if self.ephemeral => {
                provider.0.suspend(&instance.id).await
            }
            Target::Local { .. } if self.ephemeral => {
                Ok(self.mgr.local()?.suspend(&self.name).await?)
            }
            _ => self.mgr.suspend(&self.name).await,
        }
    }

    /// Resumes.
    pub async fn resume(&self) -> Result<()> {
        match &self.target {
            Target::Contrib { provider, instance } if self.ephemeral => {
                provider.0.resume(&instance.id).await.map(|_| ())
            }
            Target::Local { .. } if self.ephemeral => {
                self.mgr.local()?.resume(&self.name).await?;
                Ok(())
            }
            _ => self.mgr.resume(&self.name).await,
        }
    }

    /// Restarts.
    pub async fn restart(&self) -> Result<()> {
        match &self.target {
            Target::Contrib { provider, .. } => {
                Err(crate::provider::unsupported(provider.0.name(), "restart"))
            }
            Target::Local { .. } if self.ephemeral => {
                let rt = self.mgr.local()?;
                rt.stop(&self.name).await?;
                rt.resume(&self.name).await?;
                Ok(())
            }
            _ => self.mgr.restart(&self.name).await,
        }
    }

    /// Extends a Fleet lease.
    pub async fn keep_alive(&self, duration: Duration) -> Result<()> {
        match &self.target {
            Target::Fleet {
                pool, bound, lease, ..
            } => {
                self.mgr
                    .fleet()?
                    .keep_alive(pool, &bound.claim, duration)
                    .await?;
                // The heartbeat must not shorten the lease just granted.
                if let Some(l) = lease {
                    let until = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0)
                        + duration.as_secs() as i64;
                    if let Some(c) = l.0.lock().unwrap().as_ref() {
                        c.extend_until(until);
                    }
                    l.1.fetch_max(until, std::sync::atomic::Ordering::SeqCst);
                }
                Ok(())
            }
            Target::Contrib { provider, instance } => {
                provider.0.keep_alive(&instance.id, duration).await
            }
            _ => Err(Error::Unsupported {
                provider: self.provider(),
                op: "keep_alive".into(),
            }),
        }
    }

    /// Managed Fleet claims: stops renewing and lets the claim run until its
    /// current shutdown time (the handle no longer releases it). No-op
    /// otherwise.
    pub fn detach(&self) {
        if let Target::Fleet { lease: Some(l), .. } = &self.target
            && let Some(c) = l.take()
        {
            c.detach();
        }
    }

    /// Deletes: releases the claim (a managed pool stays for reuse) /
    /// removes the instance, and the state file.
    pub async fn delete(self) -> Result<()> {
        match &self.target {
            Target::Fleet {
                pool, bound, lease, ..
            } => {
                match lease.as_ref().and_then(|l| l.take()) {
                    Some(claim) => claim.release().await?,
                    None => self.mgr.fleet()?.release(pool, &bound.claim).await?,
                }
                self.mgr.inner.state.delete(&bound.claim)?;
            }
            Target::Local { .. } => {
                match self.mgr.local()?.delete(&self.name).await {
                    Ok(()) | Err(crate::RuntimeError::NotFound(_)) => {}
                    Err(e) => return Err(e.into()),
                }
                self.mgr.inner.state.delete(&self.name)?;
                self.mgr.inner.state.remove_lease(&self.name)?;
            }
            Target::Direct { .. } => {
                if !self.ephemeral {
                    self.mgr.inner.state.delete(&self.name)?;
                }
            }
            Target::Contrib { provider, instance } => {
                provider.0.delete(&instance.id).await?;
                if !self.ephemeral {
                    self.mgr.inner.state.delete(&self.name)?;
                }
            }
        }
        Ok(())
    }
}

/// Overrides for [`Sandbox::env_with`].
#[derive(Clone, Debug, Default)]
pub struct ConnectOptionsOverride {
    /// Probe timeout.
    pub probe_timeout: Option<Duration>,
}

/// A claimed Fleet sandbox's services: the pool template's name → port map,
/// restricted to (and completed with) the names the claim reports. Falls back
/// to names with port 0 when the template cannot be read.
async fn fleet_services(
    fleet: &FleetClient,
    pool: &cua_fleet::Pool,
    bound: &BoundSandbox,
) -> BTreeMap<String, u16> {
    let ports = fleet.pool_services(pool).await.unwrap_or_default();
    if bound.services.is_empty() {
        return ports;
    }
    bound
        .services
        .iter()
        .map(|n| (n.clone(), ports.get(n).copied().unwrap_or(0)))
        .collect()
}

/// Connects, then requires the peer to send data or hold the connection
/// open for 400 ms: a forwarder with nothing behind it accepts and then
/// closes straight away.
async fn tcp_alive(host: &str, port: u16) -> bool {
    use tokio::io::AsyncReadExt;
    let Ok(mut s) = TcpStream::connect((host, port)).await else {
        return false;
    };
    let mut buf = [0u8; 64];
    match tokio::time::timeout(Duration::from_millis(400), s.read(&mut buf)).await {
        Err(_) => true,
        Ok(Ok(n)) => n > 0,
        Ok(Err(_)) => false,
    }
}

fn probe_port(p: &Probe) -> u16 {
    match p {
        Probe::Tcp(p) => *p,
        Probe::Http { port, .. } => *port,
    }
}

fn host_port(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// A named service of a sandbox.
#[derive(Clone, Debug)]
pub struct Service {
    sandbox: Sandbox,
    name: String,
    base_url: String,
}

impl Service {
    /// Name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Base URL (Fleet: the gateway URL, which needs the Fleet bearer).
    pub fn url(&self) -> &str {
        &self.base_url
    }

    /// An HTTP request to `path` on the service.
    pub async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<Vec<u8>>,
        timeout: Duration,
    ) -> Result<HttpResponse> {
        self.request_with_headers(method, path, &[], body, timeout)
            .await
    }

    /// Where this service is reachable from this process: the base URL and
    /// the headers every request needs (on Fleet the gateway's bearer and
    /// claim header, minted now; locally and direct, none). Any HTTP client
    /// (an MCP SDK, a browser, curl) can use it as is.
    pub async fn endpoint(&self) -> Result<ServiceEndpoint> {
        let mut endpoint = match &self.sandbox.target {
            Target::Fleet { bound, .. } => {
                let fleet = self.sandbox.mgr.fleet()?;
                let token = fleet.access_token(false).await?;
                ServiceEndpoint {
                    url: self.base_url.trim_end_matches('/').to_string(),
                    headers: vec![
                        ("authorization".into(), format!("Bearer {token}")),
                        ("x-cua-fleet-claim".into(), bound.claim.clone()),
                    ],
                }
            }
            Target::Contrib { provider, instance } => {
                let port =
                    *self.sandbox.services.get(&self.name).ok_or_else(|| {
                        Error::InvalidArgument(format!("no service {:?}", self.name))
                    })?;
                let ep = provider.0.endpoint(instance, port)?;
                ServiceEndpoint {
                    url: ep.url.trim_end_matches('/').to_string(),
                    headers: ep.headers,
                }
            }
            _ => ServiceEndpoint {
                url: self.base_url.trim_end_matches('/').to_string(),
                headers: vec![],
            },
        };
        // The `env` service is cua-spacesd, which requires its token (also
        // for its HTTP side channels such as `/mcp`). The alternate header
        // passes the Fleet gateway, which owns `authorization`.
        if self.name == "env"
            && let Some(token) = self.sandbox.token()
        {
            endpoint.headers.push((
                cua_spacesd_client::transport::ENV_TOKEN_HEADER.into(),
                format!("Bearer {token}"),
            ));
        }
        Ok(endpoint)
    }

    /// A protocol-transparent request: any method, the caller's headers
    /// (hop-by-hop ones dropped) and a streamed body. Returns when the
    /// response head arrives (`head_timeout`), with the body streaming, so
    /// SSE and long-lived responses pass through as the service sends them.
    pub async fn open(
        &self,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: RequestBody,
        head_timeout: Option<Duration>,
    ) -> Result<StreamingResponse> {
        let endpoint = self.endpoint().await?;
        crate::http::open(&endpoint, method, path, headers, body, head_timeout).await
    }

    /// [`Self::request`] with caller headers. On Fleet the gateway bearer
    /// and claim header are attached here (a caller cannot replace them).
    pub async fn request_with_headers(
        &self,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: Option<Vec<u8>>,
        timeout: Duration,
    ) -> Result<HttpResponse> {
        let fut = async {
            let resp = self
                .open(
                    method,
                    path,
                    headers,
                    crate::http::full(body.unwrap_or_default()),
                    None,
                )
                .await?;
            crate::http::collect(resp).await
        };
        tokio::time::timeout(timeout, fut)
            .await
            .map_err(|_| Error::Timeout(format!("{method} {path} on service {}", self.name)))?
    }
}

/// TCP forwarding for a sandbox.
#[derive(Clone, Debug)]
pub struct Tunnel {
    sandbox: Sandbox,
}

/// How a [`Forward`] reaches the guest port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForwardVia {
    /// A host-side TCP proxy to a published port (local runtimes).
    Tcp,
    /// The spacesd's `/tunnel` WebSocket (Fleet and direct sandboxes
    /// whose driver advertises `tunnel.forward`).
    EnvTunnel,
    /// No local listener: the Fleet gateway URL of the port's service
    /// (HTTP and WebSocket only, needs the Fleet bearer). No longer
    /// returned by [`Tunnel::forward`]; kept for callers matching on it.
    GatewayUrl,
    /// A loopback HTTP proxy to the port's service through the Fleet
    /// gateway, which attaches the Fleet bearer and claim: HTTP (streaming
    /// included) and WebSocket upgrades, for images without cua-spacesd.
    GatewayProxy,
}

/// An active forward. Dropping it stops the local listener.
#[derive(Debug)]
pub struct Forward {
    /// Guest port.
    pub guest_port: u16,
    /// Local loopback address accepting connections, when a TCP forward
    /// runs (every mode but [`ForwardVia::GatewayUrl`]).
    pub local_addr: Option<SocketAddr>,
    /// URL to use: `http://<local_addr>`, or the Fleet gateway URL.
    pub url: Option<String>,
    /// How the port is reached.
    pub via: ForwardVia,
    task: Option<tokio::task::JoinHandle<()>>,
    tunnel: Option<cua_spacesd_client::TcpForward>,
    proxy: Option<crate::proxy::HttpProxy>,
}

impl Forward {
    /// Stops the listener and, for an env tunnel, revokes the driver-side
    /// forward.
    pub async fn close(mut self) -> Result<()> {
        if let Some(t) = self.task.take() {
            t.abort();
        }
        if let Some(p) = self.proxy.take() {
            p.close();
        }
        if let Some(tunnel) = self.tunnel.take() {
            tunnel.close().await?;
        }
        Ok(())
    }
}

impl Drop for Forward {
    fn drop(&mut self) {
        if let Some(t) = self.task.take() {
            t.abort();
        }
    }
}

impl Tunnel {
    /// Forwards a loopback port to guest `port`.
    ///
    /// - Local runtimes: a TCP proxy to the published port; a port not
    ///   published at create rides the spacesd's `/tunnel` when the image
    ///   has one offering `tunnel.forward`, else [`Error::InvalidArgument`]
    ///   saying how to publish it.
    /// - Direct and Fleet: a TCP listener whose connections ride the
    ///   spacesd's `/tunnel` WebSocket (through the Fleet gateway on
    ///   Fleet), when the driver advertises `tunnel.forward`.
    /// - Fleet without that capability (or without an `env` service): the
    ///   gateway URL of the port's service instead, HTTP and WebSocket only.
    /// - Direct without it: [`Error::Unsupported`].
    pub async fn forward(&self, port: u16) -> Result<Forward> {
        if port == 0 {
            return Err(Error::InvalidArgument("guest port must be 1-65535".into()));
        }
        match &self.sandbox.target {
            Target::Local { endpoints, .. } if !endpoints.ports.contains_key(&port) => {
                self.forward_unpublished(port).await
            }
            Target::Local { .. } => self.forward_tcp(port).await,
            Target::Direct { .. } => {
                let env = self.sandbox.spacesd().await?;
                self.forward_env(&env, port).await
            }
            Target::Contrib { provider, instance } => {
                // The spacesd tunnel reaches any guest port; without it, a
                // loopback proxy to the provider's URL for the port (HTTP and
                // WebSocket, with the provider's access headers).
                if self.sandbox.services.contains_key("env") {
                    let probe = ConnectOptionsOverride {
                        probe_timeout: Some(Duration::from_secs(15)),
                    };
                    match self.sandbox.spacesd_with(probe).await {
                        Ok(env) => match self.forward_env(&env, port).await {
                            Err(Error::Unsupported { .. }) => {}
                            other => return other,
                        },
                        Err(e) => tracing::debug!(error = %e, "no spacesd; using the provider URL"),
                    }
                }
                let ep = provider.0.endpoint(instance, port)?;
                self.forward_proxy(port, ep).await
            }
            Target::Fleet { bound, .. } => {
                if bound.services.iter().any(|s| s == "env") {
                    let probe = ConnectOptionsOverride {
                        probe_timeout: Some(Duration::from_secs(15)),
                    };
                    match self.sandbox.spacesd_with(probe).await {
                        Ok(env) => match self.forward_env(&env, port).await {
                            Err(Error::Unsupported { .. }) => {}
                            other => return other,
                        },
                        Err(e) => {
                            tracing::debug!(error = %e, "no spacesd; using the gateway URL")
                        }
                    }
                }
                let target = self.sandbox.port(port).map_err(|e| match e {
                    Error::Fleet(cua_fleet::Error::UnknownService { .. }) => Error::Unsupported {
                        provider: ProviderKind::Fleet,
                        op: format!(
                            "forwarding guest port {port} (not a declared service, and the \
                             sandbox's cua-spacesd does not offer \"tunnel.forward\")"
                        ),
                    },
                    e => e,
                })?;
                match target {
                    PortTarget::Url(url) => self.forward_gateway(bound, port, url).await,
                    PortTarget::Addr { .. } => unreachable!("Fleet ports are gateway URLs"),
                }
            }
        }
    }

    /// A local sandbox's guest port that was not published at create time:
    /// the guest's cua-spacesd tunnels it when the image has one that
    /// offers `tunnel.forward`; otherwise the error says how to publish it.
    async fn forward_unpublished(&self, port: u16) -> Result<Forward> {
        let probe = ConnectOptionsOverride {
            probe_timeout: Some(Duration::from_secs(15)),
        };
        let why = match self.sandbox.spacesd_with(probe).await {
            Ok(env) => match self.forward_env(&env, port).await {
                Err(Error::Unsupported { op, .. }) => format!("its cua-spacesd has no {op}"),
                other => return other,
            },
            Err(Error::SpacesdNotAvailable { reason, .. }) => {
                format!("no cua-spacesd to tunnel through ({reason})")
            }
            Err(e) => format!("its cua-spacesd did not answer ({e})"),
        };
        Err(Error::InvalidArgument(unpublished_port_message(
            &self.sandbox.name,
            port,
            &why,
        )))
    }

    /// A loopback HTTP proxy to `base` (a gateway service URL) that attaches
    /// the (refreshed) Fleet bearer and the claim header to every request.
    async fn forward_gateway(
        &self,
        bound: &BoundSandbox,
        port: u16,
        base: String,
    ) -> Result<Forward> {
        let fleet = self.sandbox.mgr.fleet()?.clone();
        let claim = bound.claim.clone();
        let router = move |pq: &str| -> crate::proxy::RouteFuture {
            let (fleet, claim) = (fleet.clone(), claim.clone());
            let url = crate::proxy::join_url(&base, pq);
            Box::pin(async move {
                match fleet.access_token(false).await {
                    Ok(token) => crate::proxy::RouteDecision::Forward(crate::proxy::ProxyRoute {
                        url,
                        headers: vec![
                            ("authorization".into(), format!("Bearer {token}")),
                            (
                                cua_spacesd_client::transport::FLEET_CLAIM_HEADER.into(),
                                claim,
                            ),
                        ],
                    }),
                    Err(e) => crate::proxy::RouteDecision::Refuse(
                        502,
                        format!("cloud credentials unavailable: {e}"),
                    ),
                }
            })
        };
        let proxy =
            crate::proxy::HttpProxy::start(SocketAddr::from(([127, 0, 0, 1], 0)), Arc::new(router))
                .await?;
        let local = proxy.local_addr();
        Ok(Forward {
            guest_port: port,
            local_addr: Some(local),
            url: Some(format!("http://{local}")),
            via: ForwardVia::GatewayProxy,
            task: None,
            tunnel: None,
            proxy: Some(proxy),
        })
    }

    /// A loopback HTTP proxy to a provider's URL for a guest port, adding
    /// its access headers to every request.
    async fn forward_proxy(&self, port: u16, ep: ServiceEndpoint) -> Result<Forward> {
        let router = move |pq: &str| -> crate::proxy::RouteFuture {
            let route = crate::proxy::ProxyRoute {
                url: crate::proxy::join_url(&ep.url, pq),
                headers: ep.headers.clone(),
            };
            Box::pin(async move { crate::proxy::RouteDecision::Forward(route) })
        };
        let proxy =
            crate::proxy::HttpProxy::start(SocketAddr::from(([127, 0, 0, 1], 0)), Arc::new(router))
                .await?;
        let local = proxy.local_addr();
        Ok(Forward {
            guest_port: port,
            local_addr: Some(local),
            url: Some(format!("http://{local}")),
            via: ForwardVia::GatewayProxy,
            task: None,
            tunnel: None,
            proxy: Some(proxy),
        })
    }

    async fn forward_env(&self, env: &SpacesdClient, port: u16) -> Result<Forward> {
        match env
            .forward_tcp(cua_spacesd_client::ForwardOptions::new(port))
            .await
        {
            Ok(tunnel) => {
                let local = tunnel.local_addr();
                Ok(Forward {
                    guest_port: port,
                    local_addr: Some(local),
                    url: Some(format!("http://{local}")),
                    via: ForwardVia::EnvTunnel,
                    task: None,
                    tunnel: Some(tunnel),
                    proxy: None,
                })
            }
            Err(cua_spacesd_client::Error::FeatureUnsupported { details, .. }) => {
                Err(Error::Unsupported {
                    provider: self.sandbox.provider(),
                    op: format!("TCP port forwarding ({})", details.message),
                })
            }
            Err(e) => Err(Error::Env(e)),
        }
    }

    async fn forward_tcp(&self, port: u16) -> Result<Forward> {
        match self.sandbox.port(port)? {
            PortTarget::Url(url) => Ok(Forward {
                guest_port: port,
                local_addr: None,
                url: Some(url),
                via: ForwardVia::GatewayUrl,
                task: None,
                tunnel: None,
                proxy: None,
            }),
            PortTarget::Addr {
                host,
                port: target_port,
            } => {
                let listener = TcpListener::bind("127.0.0.1:0").await?;
                let local = listener.local_addr()?;
                let target = host_port(&host, target_port);
                let task = tokio::spawn(async move {
                    while let Ok((mut inbound, _)) = listener.accept().await {
                        let target = target.clone();
                        tokio::spawn(async move {
                            if let Ok(mut outbound) = TcpStream::connect(&target).await {
                                let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound)
                                    .await;
                            }
                        });
                    }
                });
                Ok(Forward {
                    guest_port: port,
                    local_addr: Some(local),
                    url: Some(format!("http://{local}")),
                    via: ForwardVia::Tcp,
                    task: Some(task),
                    tunnel: None,
                    proxy: None,
                })
            }
        }
    }
}

/// Why a local sandbox's unpublished guest port cannot be forwarded, and
/// how to publish it.
fn unpublished_port_message(name: &str, port: u16, why: &str) -> String {
    format!(
        "guest port {port} of sandbox {name} was not published when it was created, and \
         {why}; publish it by creating the sandbox with `--port {port}` \
         (`cua sb create IMAGE --name {name} --port {port}`; SDK: `ports=[{port}]`)"
    )
}

/// How long create waits for cua-spacesd, given `remaining` of the
/// readiness budget: all of it when the caller chose the budget, else at
/// most [`SPACESD_READY_TIMEOUT`].
fn spacesd_budget(options: &CreateOptions, remaining: Duration) -> Duration {
    if options.ready_timeout_given {
        remaining
    } else {
        remaining.min(SPACESD_READY_TIMEOUT)
    }
}

/// The guest environment for a local start: `o.env` plus `CUA_ENV_TOKEN`
/// (see [`local_env_token`]), so the token the client will present is the one the
/// guest's spacesd is started with (cua-vmm delivers it through cloud-init
/// for Linux VMs, the Lume setup share for macOS guests and the container
/// environment for containers). An explicit
/// `CUA_ENV_TOKEN` in `o.env` wins.
fn guest_env(o: &CreateOptions, token: Option<&str>) -> BTreeMap<String, String> {
    let mut env = o.env.clone();
    if let Some(token) = token.filter(|t| !t.is_empty()) {
        env.entry("CUA_ENV_TOKEN".into())
            .or_insert_with(|| token.to_string());
    }
    env
}

/// The per-claim spacesd token of a managed Fleet sandbox, or `None` when
/// the image carries no cua-spacesd (or is not known to) or is Windows.
///
/// Every Spaces image (the published `linux:24.04` with the pre-rename
/// cua-guestd included) mints a random token of its own at boot unless
/// Fleet mounts the claim's Secret at `/run/cua` (await-token-file mode),
/// and no client could ever learn that token. So the SDK opts the template
/// into claim Secrets and delivers this token with the claim: the one the
/// caller gave (`CUA_SPACESD_TOKEN` / `CUA_ENV_TOKEN` in `o.env`, else
/// `o.env_token`), else a fresh one. It never enters the pool template.
/// A tracked create ([`Sandboxes::create_tracked`]).
struct Creating {
    /// (state dir, name).
    key: (PathBuf, String),
    token: crate::CancellationToken,
    /// `Some(Some(what))` once cancelled, `Some(None)` once it ended
    /// otherwise.
    done: tokio::sync::watch::Receiver<Option<Option<String>>>,
}

static CREATING: std::sync::Mutex<Vec<Creating>> = std::sync::Mutex::new(Vec::new());

/// Why a cloud sandbox cannot have a GPU.
pub const FLEET_NO_GPU: &str = "Cua Cloud has no GPU machines yet";

/// How long a cancelled create waits for its drop guards' clean-up.
const DISCARD_SETTLE: Duration = Duration::from_secs(90);

/// What a cancelled create deletes.
#[derive(Debug, Clone)]
struct DiscardPlan {
    provider: ProviderKind,
    /// The sandbox's name, when known before it exists.
    name: Option<String>,
    /// Nothing had that name before (so it is this create's to delete).
    fresh: bool,
    /// The contrib provider's word.
    contrib: Option<String>,
}

/// Runs a create's discard when its future is dropped before it finished.
struct DiscardGuard {
    mgr: Sandboxes,
    plan: Option<DiscardPlan>,
}

impl Drop for DiscardGuard {
    fn drop(&mut self) {
        if let Some(plan) = self.plan.take() {
            let mgr = self.mgr.clone();
            // Spawned, not awaited: the guards it waits on are spawned too.
            if let Ok(rt) = tokio::runtime::Handle::try_current() {
                rt.spawn(async move {
                    let what = mgr.discard(plan).await;
                    tracing::info!(what, "a dropped create was cleaned up");
                });
            }
        }
    }
}

fn fleet_claim_token(o: &CreateOptions, image: Option<&ImageInfo>) -> Result<Option<String>> {
    if o.os.eq_ignore_ascii_case("windows") || image.and_then(|i| i.spacesd) != Some(true) {
        return Ok(None);
    }
    let token = o
        .env
        .get("CUA_SPACESD_TOKEN")
        .or_else(|| o.env.get("CUA_ENV_TOKEN"))
        .or(o.env_token.as_ref())
        .filter(|t| !t.is_empty())
        .cloned()
        .unwrap_or_else(cua_fleet::claim_secrets::generate_claim_token);
    cua_fleet::claim_secrets::validate_claim_token(&token)?;
    Ok(Some(token))
}

/// A fresh per-sandbox spacesd token (128 random bits, hex).
pub(crate) fn mint_env_token() -> String {
    format!("{:032x}", rand::random::<u128>())
}

/// The spacesd token of a new local sandbox: `CUA_ENV_TOKEN` in
/// `o.env` (it is what the guest gets), else `o.env_token`, else a fresh
/// one for guests the local backends deliver it to: containers (their
/// environment), Linux VMs (cloud-init: `/run/cua/env-token`, root `0600`,
/// the Fleet claim-token contract) and macOS VMs (the Lume setup share).
/// Windows VMs have no delivery channel: `None` (a driver there starts in
/// bootstrap mode and [`Sandbox::spacesd`] installs a token with `Init`).
fn local_env_token(o: &CreateOptions) -> Option<String> {
    let given = o
        .env
        .get("CUA_SPACESD_TOKEN")
        .or_else(|| o.env.get("CUA_ENV_TOKEN"))
        .or(o.env_token.as_ref())
        .filter(|t| !t.is_empty())
        .cloned();
    given.or_else(|| (!o.os.eq_ignore_ascii_case("windows")).then(mint_env_token))
}

#[cfg(test)]
mod placement_tests {
    use super::*;

    fn opts(provider: ProviderKind, image: &str) -> CreateOptions {
        CreateOptions::new(provider, image)
    }

    #[test]
    fn an_unpublished_port_says_how_to_publish_it() {
        let m = unpublished_port_message("dev", 8080, "no cua-spacesd to tunnel through");
        assert!(
            m.contains("guest port 8080 of sandbox dev was not published"),
            "{m}"
        );
        assert!(m.contains("--port 8080"), "{m}");
        assert!(m.contains("no cua-spacesd"), "{m}");
    }

    #[test]
    fn a_chosen_ready_timeout_governs_the_spacesd_wait() {
        let mut o = opts(ProviderKind::Local, "macos:26");
        let left = Duration::from_secs(500);
        assert_eq!(spacesd_budget(&o, left), SPACESD_READY_TIMEOUT);
        assert_eq!(
            spacesd_budget(&o, Duration::from_secs(30)),
            Duration::from_secs(30)
        );
        o.ready_timeout = Duration::from_secs(900);
        o.ready_timeout_given = true;
        assert_eq!(spacesd_budget(&o, left), left);
    }

    #[test]
    fn kind_and_runtime_fold_into_the_provider_options() {
        let mut o = opts(ProviderKind::Local, "python:3.12-slim");
        o.runtime = Runtime::Runc;
        o.apply_placement().unwrap();
        assert_eq!(o.kind, Kind::Container);
        assert_eq!(o.container_runtime.as_deref(), Some("runc"));

        let mut o = opts(ProviderKind::Fleet, "ghcr.io/trycua/linux:24.04");
        o.kind = Kind::Vm;
        o.apply_placement().unwrap();
        assert_eq!(o.fleet.runtime, Some(RuntimeKind::Kubevirt));

        let mut o = opts(ProviderKind::Fleet, "ghcr.io/trycua/linux:24.04");
        o.runtime = Runtime::Gvisor;
        o.apply_placement().unwrap();
        assert_eq!(
            (o.kind, o.fleet.runtime.clone()),
            (Kind::Container, Some(RuntimeKind::Gvisor))
        );

        // Auto stays auto: the image decides later.
        let mut o = opts(ProviderKind::Fleet, "ghcr.io/trycua/linux:24.04");
        o.apply_placement().unwrap();
        assert_eq!((o.kind, o.fleet.runtime.clone()), (Kind::Auto, None));
    }

    #[test]
    fn older_spellings_still_work() {
        let mut o = opts(ProviderKind::Local, "python:3.12-slim");
        o.container_runtime = Some("gvisor".into());
        o.apply_placement().unwrap();
        assert_eq!(
            (o.kind, o.runtime.clone()),
            (Kind::Container, Runtime::Gvisor)
        );

        let mut o = opts(ProviderKind::Local, "vm:ghcr.io/trycua/linux:24.04-disk");
        o.apply_placement().unwrap();
        assert_eq!((o.kind, o.runtime.clone()), (Kind::Vm, Runtime::Qemu));

        let mut o = opts(ProviderKind::Local, "lume:ghcr.io/trycua/macos:26");
        o.apply_placement().unwrap();
        assert_eq!((o.kind, o.runtime.clone()), (Kind::Vm, Runtime::Lume));

        let mut o = opts(ProviderKind::Fleet, "img");
        o.fleet.runtime = Some(RuntimeKind::Kubevirt);
        o.apply_placement().unwrap();
        assert_eq!((o.kind, o.runtime.clone()), (Kind::Vm, Runtime::Kubevirt));

        // The cloud ignores the local-only container_runtime, as before.
        let mut o = opts(ProviderKind::Fleet, "img");
        o.container_runtime = Some("runc".into());
        o.apply_placement().unwrap();
        assert_eq!(o.runtime, Runtime::Auto);
    }

    #[test]
    fn contradictions_are_invalid_placements() {
        let mut o = opts(ProviderKind::Fleet, "img");
        o.runtime = Runtime::Qemu;
        let e = o.apply_placement().unwrap_err();
        let Error::InvalidPlacement(p) = e else {
            panic!("{e:?}")
        };
        assert_eq!(p.valid, ["auto", "gvisor", "kubevirt"]);

        let mut o = opts(ProviderKind::Local, "vm:img");
        o.kind = Kind::Container;
        assert!(matches!(
            o.apply_placement(),
            Err(Error::InvalidPlacement(_))
        ));

        let mut o = opts(ProviderKind::Local, "img");
        o.kind = Kind::Container;
        o.runtime = Runtime::Lume;
        assert!(matches!(
            o.apply_placement(),
            Err(Error::InvalidPlacement(_))
        ));

        let mut o = opts(ProviderKind::Direct, "");
        o.kind = Kind::Vm;
        assert!(matches!(
            o.apply_placement(),
            Err(Error::InvalidPlacement(_))
        ));
    }

    #[test]
    fn backend_labels_map_to_kind_and_runtime() {
        assert_eq!(
            placement_of_backend("gvisor"),
            (Kind::Container, Runtime::Gvisor)
        );
        assert_eq!(
            placement_of_backend("container"),
            (Kind::Container, Runtime::Runc)
        );
        assert_eq!(
            placement_of_backend("qemu-docker"),
            (Kind::Vm, Runtime::Qemu)
        );
        assert_eq!(placement_of_backend("lume"), (Kind::Vm, Runtime::Lume));
        assert_eq!(placement_of_backend("fleet"), (Kind::Auto, Runtime::Auto));
    }
}
