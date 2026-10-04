//! One daemon-agnostic `Sandbox` over these providers:
//!
//! - **Local** (a VM or container runtime behind [`LocalRuntime`], which
//!   `cua-vmm` implements through a thin adapter),
//! - **Direct** (`url` + optional token; any reachable machine, including a
//!   relay URL),
//! - **Contrib** (a third-party platform behind [`Provider`], registered by
//!   the `cua-contrib` crate: `--on e2b`, `--on daytona`, ...).
//!
//! Readiness is "the provider reports running" plus optional user probes
//! ([`Probe::Tcp`], [`Probe::Http`]). Nothing in create, readiness or
//! teardown assumes cua-spacesd, computer-server or any MCP server;
//! [`Sandbox::spacesd`] *attaches* to cua-spacesd when present and fails with
//! [`Error::SpacesdNotAvailable`] otherwise.
//!
//! State files (`~/.cua/sandboxes/<name>.json`) use cua-sandbox's format.
//!
//! Cua Cloud (`on="cloud"`, the Fleet provider) has closed: creating,
//! reaching or listing cloud sandboxes fails with [`Error::CloudClosed`];
//! records of earlier cloud sandboxes stay readable and `delete` removes
//! them.

pub mod build;
pub mod byoc;
pub mod http;
pub mod mcp;
pub mod placement;
/// Clean-up a dropped (cancelled) create hands off ([`cleanup::settle`]).
pub use cua_vmm::cleanup;
/// GPU options per runtime ([`gpu::GpuSupport`]).
pub use cua_vmm::gpu;
/// Create progress: what a create is doing (pulling, booting, waiting for
/// the guest), reported to the [`progress::scope`] that started it.
pub use cua_vmm::progress;

/// Deletes the runtimes cua set up itself under `$CUA_HOME/runtimes`: the
/// built-in Lume, and the built-in Linux runtime's VM with its disks. A
/// runtime the user installed is never touched; a built-in one is set up
/// again the next time a Space needs it.
pub async fn remove_builtin_runtimes() -> std::result::Result<(), String> {
    let lume = cua_vmm::lume::builtin::remove().map_err(|e| e.to_string());
    let linux = cua_vmm::managed::remove().await.map_err(|e| e.to_string());
    lume.and(linux)
}
/// Cancels a create ([`Sandboxes::create_cancellable`]).
pub use tokio_util::sync::CancellationToken;
pub mod power;
pub mod provider;
pub mod proxy;
pub mod refs;
mod runtime;
mod sandbox;
pub mod settings;
pub mod sidecar;
pub mod state;
#[cfg(feature = "testing")]
pub mod testing;

pub use build::{BuildFile, BuildSpec, ImageLayer, RegistryCredentials};
pub use cua_spacesd_client;
pub use http::{HttpResponse, RequestBody, ServiceEndpoint, StreamingResponse};
pub use mcp::{McpClient, McpConfig};
pub use power::{PowerControl, PowerState};
pub use provider::{
    CLOUD_LOCATIONS, CONTRIB_LOCATIONS, ImageMode, PortExposure, Provider, ProviderCapabilities,
    ProviderCreate, ProviderImage, ProviderInstance, RunKind, is_cloud_location,
    is_contrib_location,
};
pub use refs::{Location, SandboxRef};
pub use runtime::{
    GuestDisplay, GuestOutput, GuestScreenshot, ImageInfo, InstanceStatus, LocalEndpoints,
    LocalInstance, LocalProbe, LocalRuntime, LocalStartSpec, LocalSummary, RuntimeError,
    RuntimeResult,
};
pub use sandbox::{
    ConnectOptionsOverride, CreateOptions, ENV_PORT, Forward, ForwardVia, MISSING, NetworkMode,
    PortTarget, Probe, ProviderKind, SPACESD_READY_TIMEOUT, Sandbox, SandboxInfo, Sandboxes,
    SandboxesBuilder, Service, Status, Tunnel, placement_of_backend,
};
pub use sidecar::Sidecar;

/// What every call that needs Cua Cloud (`on="cloud"`, cloud sandbox refs,
/// Fleet pools) says now that it has closed.
pub const CLOUD_CLOSED: &str = "Cua Cloud has closed: cloud sandboxes (on=\"cloud\", cloud:<name>) \
     are no longer available. Run sandboxes locally, on a machine you run (Sandbox.connect(url)), \
     or in your own cloud account (on=aws|gcp|modal); see \
     https://cua.ai/docs/cua-sdk/guides/your-cloud";
pub use state::{EphemeralLease, FleetState, LEASE_DIR, LocalState, SandboxState, StateStore};

/// Errors.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Bad input.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// A location, kind or runtime that does not exist, or a combination
    /// of them (or with the image) that does not; the error lists the
    /// valid values ([`placement::PlacementError::valid`]).
    #[error("invalid placement: {0}")]
    InvalidPlacement(#[from] placement::PlacementError),
    /// No sandbox with this name.
    #[error("sandbox {0} not found")]
    NotFound(String),
    /// A bare name matches sandboxes in more than one location; use one of
    /// the qualified `candidates` (`local:<name>`, `cloud:<name>`, ...).
    #[error("ambiguous sandbox name: {message}")]
    AmbiguousSandbox {
        /// The bare name.
        name: String,
        /// The qualified refs it matches.
        candidates: Vec<String>,
        /// The rendered message.
        message: String,
    },
    /// The provider was not configured on this [`Sandboxes`].
    #[error("{}", provider_not_configured(.0))]
    ProviderNotConfigured(ProviderKind),
    /// A contrib provider has no credentials (the message names the
    /// environment variable or `cua auth provider set <name>`).
    #[error("{0}")]
    ContribNotConfigured(String),
    /// The provider cannot do this.
    #[error("{op} is not supported by the {provider:?} provider")]
    Unsupported {
        /// Provider.
        provider: ProviderKind,
        /// Operation.
        op: String,
    },
    /// No cua-spacesd answered on the sandbox's env port / service.
    #[error("cua-spacesd is not available in sandbox {sandbox}: {reason}")]
    SpacesdNotAvailable {
        /// Sandbox name.
        sandbox: String,
        /// Probe failure.
        reason: String,
    },
    /// The image cannot run on the chosen provider (for example a Fleet
    /// macOS template locally). The message names the alternative.
    #[error("{0}")]
    UnsupportedImage(String),
    /// A readiness probe or provider wait timed out.
    #[error("timed out: {0}")]
    Timeout(String),
    /// HTTP failure.
    #[error("http: {0}")]
    Http(String),
    /// Cua Cloud has closed ([`CLOUD_CLOSED`]): the call needed a cloud
    /// sandbox. Use a local sandbox, a machine you run, or your own cloud.
    #[error("{}", CLOUD_CLOSED)]
    CloudClosed,
    /// Local runtime.
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
    /// env client (other than "not available").
    #[error(transparent)]
    Env(#[from] cua_spacesd_client::Error),
    /// MCP client (rmcp) failure.
    #[error("mcp: {0}")]
    Mcp(String),
    /// Your own cloud account (AWS, Google Cloud, Modal) refused or failed a
    /// call: a missing permission, a quota, a region without the machine
    /// type. The message names the cloud's own error.
    #[error("your cloud: {0}")]
    Cloud(String),
    /// I/O.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// JSON.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// The create was cancelled; what it made is gone (the message says
    /// what, and what stays for a later create).
    #[error("cancelled: {0}")]
    Cancelled(String),
}

fn provider_not_configured(p: &ProviderKind) -> String {
    match p {
        ProviderKind::Fleet => CLOUD_CLOSED.into(),
        ProviderKind::Contrib => "no contrib provider is configured".into(),
        other => format!("provider {other:?} is not configured"),
    }
}

/// Result alias.
pub type Result<T, E = Error> = std::result::Result<T, E>;
