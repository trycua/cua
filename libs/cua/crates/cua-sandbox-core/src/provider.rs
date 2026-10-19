//! Pluggable sandbox providers: third-party platforms (E2B, Daytona,
//! Modal, ...) behind a location word, `--on e2b`.
//!
//! The core owns everything provider-independent: resolving and pinning the
//! image (variant, architecture, whether it carries cua-spacesd), refusing
//! what a provider cannot run from its [`ProviderCapabilities`] before any
//! API call, the spacesd token, readiness probes, state files and refs
//! (`e2b:<name>`). A [`Provider`] only maps that onto its platform: create
//! from a pinned registry image (building and caching a template or snapshot
//! when the platform needs one), report status, expose guest ports and
//! delete. Implementations live in the `cua-contrib` crate, one cargo
//! feature each, and are registered with
//! [`SandboxesBuilder::provider`](crate::SandboxesBuilder::provider).

use crate::{Error, ProviderKind, RegistryCredentials, Result, ServiceEndpoint, Status};
use std::{collections::BTreeMap, time::Duration};

/// Every contrib location word the refs grammar reserves, whether or not
/// this build includes the provider (`e2b:<name>` always parses, and a build
/// without the provider says how to get it).
pub const CONTRIB_LOCATIONS: &[&str] = &[
    "e2b",
    "daytona",
    "modal",
    "cloudflare",
    "vercel",
    "morph",
    "runloop",
    "fly",
    "blaxel",
    "codesandbox",
    "northflank",
];

/// Whether `word` is a contrib location (see [`CONTRIB_LOCATIONS`]).
pub fn is_contrib_location(word: &str) -> bool {
    CONTRIB_LOCATIONS.contains(&word)
}

/// Your own cloud accounts as locations (`--on aws`): sandboxes (and the
/// Spaces on them) run there with your credentials (see [`crate::byoc`]).
/// `modal` is also a contrib word: with the `byoc` providers built, Modal
/// sandboxes join the relay instead of using Modal's tunnels.
pub const CLOUD_LOCATIONS: &[&str] = &["aws", "gcp", "modal"];

/// Whether `word` names your own cloud (see [`CLOUD_LOCATIONS`]).
pub fn is_cloud_location(word: &str) -> bool {
    CLOUD_LOCATIONS.contains(&word)
}

/// Whether `word` is a provider location: a contrib platform or your own
/// cloud (`<word>:<name>` refs).
pub fn is_provider_location(word: &str) -> bool {
    is_contrib_location(word) || is_cloud_location(word)
}

/// How a sandbox runs on a provider (the `--kind` axis).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RunKind {
    /// A container rootfs (the image's `rootfs` variant).
    Container,
    /// A VM disk (the image's `containerdisk` variant).
    Vm,
}

impl RunKind {
    /// `container` / `vm`.
    pub fn as_str(self) -> &'static str {
        match self {
            RunKind::Container => "container",
            RunKind::Vm => "vm",
        }
    }
}

/// How a provider gets a registry image onto its platform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageMode {
    /// Pulls the pinned registry reference at create time.
    Direct,
    /// Needs a build step first (a template or snapshot); the provider
    /// caches it by the image digest and the build inputs.
    Template,
}

/// How guest ports are reachable from outside.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortExposure {
    /// Not at all.
    None,
    /// HTTPS URLs through the provider's proxy (HTTP/1.1 and WebSocket;
    /// cua-spacesd answers gRPC-Web there).
    Https,
}

/// What a provider can do. The core checks these before calling it, so an
/// impossible request fails with a typed error that names the reason.
#[derive(Clone, Debug)]
pub struct ProviderCapabilities {
    /// Run kinds, preferred first.
    pub kinds: Vec<RunKind>,
    /// The engine the platform runs sandboxes on (`firecracker`, `gvisor`,
    /// ...): the `runtime` of the placement model, reported in
    /// `provider_details`.
    pub runtime: &'static str,
    /// OCI architectures it runs (`amd64`, `arm64`).
    pub arches: Vec<&'static str>,
    /// Registry image handling.
    pub image_mode: ImageMode,
    /// Port exposure.
    pub ports: PortExposure,
    /// A command replacing the image's entrypoint.
    pub command: bool,
    /// Guest environment reaches the image's entrypoint (and so the
    /// spacesd token is delivered at create; otherwise the core installs
    /// one with the driver's bootstrap `Init`).
    pub env_to_entrypoint: bool,
    /// Private registry credentials.
    pub private_registry: bool,
    /// Suspend and resume.
    pub suspend: bool,
    /// Largest vCPU count, when capped.
    pub max_cpus: Option<u32>,
    /// Largest memory in MiB, when capped.
    pub max_memory_mb: Option<u64>,
    /// The environment variables its credentials come from (first set wins;
    /// never printed).
    pub credential_env: &'static [&'static str],
    /// GPU types it can attach ([`ProviderCreate::gpu`]); empty: none.
    /// A type the account cannot use now (no quota, over budget) is listed
    /// with `supported: false` and the reason.
    pub gpus: Vec<crate::gpu::GpuOption>,
}

/// The image the core resolved for a provider.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderImage {
    /// The reference as requested, normalised.
    pub reference: String,
    /// `registry/repo@sha256:...` of the variant to run; the requested
    /// reference when the registry could not be read.
    pub pinned_ref: String,
    /// The digest (empty when unresolved).
    pub digest: String,
    /// What runs.
    pub kind: RunKind,
    /// Architecture (`amd64`, `arm64`).
    pub arch: String,
    /// Whether the image carries cua-spacesd (`None`: unknown).
    pub spacesd: Option<bool>,
}

/// What the core asks a provider to create.
#[derive(Clone, Debug)]
pub struct ProviderCreate {
    /// Sandbox name (unique per provider account for named sandboxes;
    /// `cua-eph-<hex>` for ephemeral ones).
    pub name: String,
    /// Image.
    pub image: ProviderImage,
    /// vCPUs.
    pub cpus: u32,
    /// Memory (MiB).
    pub memory_mb: u64,
    /// Guest environment (includes `CUA_ENV_TOKEN` when the provider
    /// delivers it to the entrypoint).
    pub env: BTreeMap<String, String>,
    /// Command replacing the entrypoint.
    pub command: Option<Vec<String>>,
    /// Guest ports to expose.
    pub ports: Vec<u16>,
    /// Lifetime backstop: the platform deletes (or stops) the sandbox after
    /// this unless kept alive. `None`: the provider's longest.
    pub ttl: Option<Duration>,
    /// Labels / metadata to tag it with (`cua.name`, `cua.managed`).
    pub labels: BTreeMap<String, String>,
    /// Credentials for a private image.
    pub registry_credentials: Option<RegistryCredentials>,
    /// Overall budget (template builds included).
    pub timeout: Duration,
    /// The engine the placement chose (`--runtime`; [`crate::placement::Runtime::Auto`]:
    /// the provider's default). Providers with one engine ignore it.
    pub runtime: crate::placement::Runtime,
    /// A GPU type from [`ProviderCapabilities::gpus`]; `None`: no GPU.
    pub gpu: Option<String>,
}

/// A sandbox on a provider.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderInstance {
    /// The provider's id.
    pub id: String,
    /// The cua name (label), when known.
    pub name: String,
    /// Status.
    pub status: Status,
    /// Endpoints of guest ports the provider resolved (for platforms whose
    /// URLs need an API call); see [`Provider::endpoint`].
    pub endpoints: BTreeMap<u16, ServiceEndpoint>,
    /// Provider internals for `provider_details`. Keys starting with `_`
    /// are private state (access tokens) the core never shows.
    pub details: BTreeMap<String, String>,
}

impl ProviderInstance {
    /// An instance with `id`, `name` and `status`.
    pub fn new(id: impl Into<String>, name: impl Into<String>, status: Status) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            status,
            endpoints: BTreeMap::new(),
            details: BTreeMap::new(),
        }
    }
}

/// A third-party sandbox platform.
#[async_trait::async_trait]
pub trait Provider: Send + Sync + 'static {
    /// Location word and ref prefix (`e2b` → `--on e2b`, `e2b:<name>`); one
    /// of [`CONTRIB_LOCATIONS`].
    fn name(&self) -> &'static str;

    /// What it can do.
    fn capabilities(&self) -> ProviderCapabilities;

    /// Whether credentials are present (no I/O). The error names the
    /// environment variable or `cua auth provider set <name>`.
    fn check_configured(&self) -> Result<()>;

    /// Creates a sandbox and returns once the platform reports it running
    /// (template builds included). On failure nothing is left behind.
    ///
    /// Cancel-safe: the core may drop this future at any await (a
    /// cancelled create) and then calls [`Provider::abort_create`] with the
    /// same name; report byte progress of any upload or build through
    /// [`crate::progress`] ([`crate::progress::Meter`]).
    async fn create(&self, spec: &ProviderCreate) -> Result<ProviderInstance>;

    /// Removes whatever a cancelled [`Provider::create`] of `name` made
    /// (the sandbox, a half-built template it made only for this create).
    /// Idempotent. Default: deletes every sandbox [`Provider::list`] shows
    /// under that name.
    async fn abort_create(&self, name: &str) -> Result<()> {
        for i in self.list().await? {
            if i.name == name {
                self.delete(&i.id).await?;
            }
        }
        Ok(())
    }

    /// One sandbox by provider id ([`Error::NotFound`] when gone).
    async fn get(&self, id: &str) -> Result<ProviderInstance>;

    /// The sandboxes this SDK created on the account (label `cua.managed`).
    async fn list(&self) -> Result<Vec<ProviderInstance>>;

    /// Deletes (idempotent: a missing sandbox is `Ok`).
    async fn delete(&self, id: &str) -> Result<()>;

    /// Where guest `port` is reachable: a base URL and the headers every
    /// request needs. No I/O: platforms whose URLs need an API call resolve
    /// them into [`ProviderInstance::endpoints`] in `create` / `get`.
    fn endpoint(&self, instance: &ProviderInstance, port: u16) -> Result<ServiceEndpoint>;

    /// Suspends (pause / snapshot).
    async fn suspend(&self, _id: &str) -> Result<()> {
        Err(unsupported(self.name(), "suspend"))
    }

    /// Resumes a suspended sandbox.
    async fn resume(&self, _id: &str) -> Result<ProviderInstance> {
        Err(unsupported(self.name(), "resume"))
    }

    /// Extends the lifetime backstop to `duration` from now.
    async fn keep_alive(&self, _id: &str, _duration: Duration) -> Result<()> {
        Err(unsupported(self.name(), "keep_alive"))
    }

    /// Whether its sandboxes join the cua.ai relay as machines of the
    /// signed-in account (your own cloud, [`crate::byoc`]): Spaces shows
    /// such a sandbox as `relay:<machine>`.
    fn joins_relay(&self) -> bool {
        false
    }

    /// How the SDK reaches guest `port` when a URL and static headers are
    /// not enough (a relay gateway whose bearer refreshes): the spacesd
    /// connect options. `None` (the default): [`Provider::endpoint`].
    fn connect_options(
        &self,
        _instance: &ProviderInstance,
        _port: u16,
    ) -> Option<Result<cua_spacesd_client::ConnectOptions>> {
        None
    }

    /// How a sandbox on this platform turns off and on again (`None`: it
    /// cannot). Default: [`crate::PowerControl::Suspend`] when
    /// [`ProviderCapabilities::suspend`] says so. A platform that stops
    /// and starts instances (a cloud VM) returns [`crate::PowerControl::Stop`]
    /// and implements [`Provider::stop`] and [`Provider::start`].
    fn power(&self) -> Option<crate::PowerControl> {
        self.capabilities()
            .suspend
            .then_some(crate::PowerControl::Suspend)
    }

    /// Stops a sandbox, keeping its disk ([`crate::PowerControl::Stop`]).
    async fn stop(&self, _id: &str) -> Result<()> {
        Err(unsupported(self.name(), "stop"))
    }

    /// Starts a stopped sandbox again.
    async fn start(&self, _id: &str) -> Result<ProviderInstance> {
        Err(unsupported(self.name(), "start"))
    }
}

/// [`Error::Unsupported`] for a contrib provider.
pub fn unsupported(provider: &str, op: impl Into<String>) -> Error {
    Error::Unsupported {
        provider: ProviderKind::Contrib,
        op: format!("{} on {provider}", op.into()),
    }
}

/// The error for `--on <word>` when this build has no such provider.
pub fn not_built(word: &str) -> Error {
    if is_cloud_location(word) && !is_contrib_location(word) {
        return crate::byoc::not_built();
    }
    Error::Unsupported {
        provider: ProviderKind::Contrib,
        op: format!(
            "--on {word}: this build has no {word} provider (contrib providers are opt-in: build \
             the cua CLI or SDK with `--features contrib`; `cua auth provider ls` lists the \
             providers this cua implements)"
        ),
    }
}
