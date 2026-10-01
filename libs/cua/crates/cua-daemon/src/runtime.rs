//! The SDK runtime shared by the embedded and daemon topologies.

use crate::{Error, Result};
use cua_fleet::{AutoPoolConfig, FleetClient, FleetConfig, PoolManager};
use cua_sandbox_core::{
    CreateOptions, Forward, LocalRuntime, Location, PortTarget, Probe, ProviderKind, Sandbox,
    SandboxRef, Sandboxes, Status,
};
use cua_spacesd_client::{ConnectOptions, SpacesdClient};
use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};

/// How to build a [`Runtime`].
#[derive(Clone, Default)]
pub struct RuntimeConfig {
    /// State directory (default `~/.cua/sandboxes`).
    pub state_dir: Option<PathBuf>,
    /// Managed Fleet pool settings (default [`cua_sandbox_core::settings::auto_pool_config`],
    /// home `~/.cua` or next to `state_dir`).
    pub auto_pools: Option<AutoPoolConfig>,
    /// Fleet configuration. `None` disables the Fleet provider; a config
    /// without credentials also disables it (calls then fail with
    /// `ProviderNotConfigured`).
    pub fleet: Option<FleetConfig>,
    /// A ready Fleet client (tests, foreign HTTP clients, a refreshing
    /// session bearer). Wins over `fleet`.
    pub fleet_client: Option<FleetClient>,
    /// Resolve image tags to digests for managed pools. Default: on, except
    /// with an injected `fleet_client` (tests never reach a registry).
    pub resolve_image_digests: Option<bool>,
    /// Local VM / container runtime. Wins over `vmm` for sandboxes.
    pub local: Option<Arc<dyn LocalRuntime>>,
    /// The cua-vmm backends (local sandboxes and image operations).
    pub vmm: Option<Arc<crate::local::VmmLocal>>,
    /// Probe timeout for spacesd attachment (default 15 s).
    pub env_probe_timeout: Option<Duration>,
    /// Spaces registry directory (default `$CUA_HOME` or `~/.cua`).
    pub spaces_home: Option<PathBuf>,
    /// Teleport sender: read app sessions under this home directory with a
    /// side-effect-free host (tests, CI). `None` uses the real host.
    pub teleport_home: Option<PathBuf>,
    /// Contrib providers (`--on e2b`, ...). `None`: every provider this
    /// build includes (feature `contrib`), configured from the environment.
    pub providers: Option<Vec<Arc<dyn cua_sandbox_core::Provider>>>,
    /// Extensions for this runtime (the Keyvault, the Cua Volume, teleport),
    /// added to the process-wide [`crate::extension::register`]ed ones.
    #[cfg(feature = "spaces")]
    pub extensions: Vec<Arc<dyn crate::extension::DaemonExtension>>,
}

impl std::fmt::Debug for RuntimeConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeConfig")
            .field("state_dir", &self.state_dir)
            .field("fleet", &self.fleet)
            .field("local", &self.local.is_some())
            .finish_non_exhaustive()
    }
}

/// A sandbox creation request (union of the providers' options).
#[derive(Clone, Debug, Default)]
pub struct CreateRequest {
    /// Where: `local`, `cloud`, `direct:<addr>` or a registered provider.
    /// `None`: the older `provider` / `url` fields, else the user default
    /// (`cua_sandbox_core::settings`).
    pub location: Option<String>,
    /// What kind: `auto`, `container`, `vm` (`None`: the user default).
    pub kind: Option<String>,
    /// Which engine: `auto` or an engine the location offers (`None`: the
    /// user default).
    pub runtime: Option<String>,
    /// Deprecated: use `location`.
    pub provider: Option<ProviderKind>,
    /// Name. `None`: ephemeral (Fleet/local) or the URL host (direct).
    pub name: Option<String>,
    /// Image (Fleet, local).
    pub image: String,
    /// Direct URL.
    pub url: Option<String>,
    /// spacesd token.
    pub token: Option<String>,
    /// Existing Fleet pool to claim from.
    pub pool: Option<String>,
    /// Guest OS.
    pub os: Option<String>,
    /// vCPUs.
    pub cpus: Option<u32>,
    /// Memory (MiB).
    pub memory_mb: Option<u64>,
    /// Extra ports.
    pub ports: Vec<u16>,
    /// Named services.
    pub services: BTreeMap<String, u16>,
    /// Readiness probes.
    pub wait_for: Vec<Probe>,
    /// Readiness budget.
    pub ready_timeout: Option<Duration>,
    /// Guest environment.
    pub env: BTreeMap<String, String>,
    /// The sandbox's command (argv), replacing the image's entrypoint.
    pub command: Option<Vec<String>>,
    /// Fleet runtime (`kubevirt`, `gvisor`, ...).
    pub fleet_runtime: Option<String>,
    /// Deprecated: initial replicas of a new managed pool (> 0 = warm).
    pub fleet_replicas: Option<u32>,
    /// Fleet claim TTL.
    pub fleet_ttl_seconds: Option<u32>,
    /// Managed Fleet pools: start new pools warm.
    pub fleet_warm: Option<bool>,
    /// Managed Fleet pools: autoscaling ceiling.
    pub fleet_max_pool_size: Option<u32>,
    /// With `pool`: update its template to the given fields instead of
    /// failing on a mismatch.
    pub fleet_apply: bool,
    /// User labels.
    pub labels: BTreeMap<String, String>,
    /// Sidecar containers sharing the sandbox's network namespace.
    pub sidecars: Vec<cua_sandbox_core::Sidecar>,
    /// Credentials for a private image (`Debug` redacts the password).
    pub registry_credentials: Option<cua_sandbox_core::RegistryCredentials>,
    /// Image layers built on top of `image`.
    pub build: Option<cua_sandbox_core::BuildSpec>,
    /// Local container runtime (`runc` / `gvisor`).
    pub container_runtime: Option<String>,
    /// Guest network, `default` or `none` (`None` = default).
    pub network: Option<String>,
    /// The process that owns an ephemeral sandbox (a daemon client);
    /// `None`: this process.
    pub owner_pid: Option<u32>,
    /// Keep a named sandbox whose readiness check fails (default: delete
    /// it and release its claim).
    pub keep_on_failure: bool,
    /// A GPU option of the runtime (`cua_sandbox_core::gpu`); `None`: no
    /// GPU.
    pub gpu: Option<String>,
}

/// What callers see of a sandbox, identical in both topologies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxRecord {
    /// Qualified ref (`local:<name>`, `cloud:<name>`, `direct:<host:port>`):
    /// what every call accepts, and what listings print.
    pub id: String,
    /// Name.
    pub name: String,
    /// Provider.
    pub provider: ProviderKind,
    /// `runtime_type` (`fleet`, `direct`, local backend).
    pub runtime_type: String,
    /// Status.
    pub status: Status,
    /// No state file; torn down on delete.
    pub ephemeral: bool,
    /// Declared services (name → guest port, 0 = unknown).
    pub services: BTreeMap<String, u16>,
    /// Image, when known.
    pub image: Option<String>,
    /// User labels.
    pub labels: BTreeMap<String, String>,
    /// Reachable endpoints by service name.
    pub endpoints: BTreeMap<String, String>,
    /// Creation time, when known.
    pub created_at: Option<SystemTime>,
    /// `local`, `cloud`, `direct` or `relay`.
    pub location: String,
    /// `container` or `vm`; empty when not known.
    pub kind: String,
    /// `gvisor`, `runc`, `qemu`, `lume` or `kubevirt`; empty when not known.
    pub runtime: String,
    /// When the sandbox expires unless kept alive, when known.
    pub expires_at: Option<SystemTime>,
    /// Provider internals (Fleet pool/namespace/claim, local backend).
    pub provider_details: BTreeMap<String, String>,
    /// The image as resolved and pinned at create time, when known (not
    /// carried over the daemon wire).
    pub image_info: Option<cua_sandbox_core::ImageInfo>,
}

/// How long a listing waits for Fleet before it lists without the cloud
/// sandboxes (and says so).
pub const LIST_CLOUD_TIMEOUT: Duration = Duration::from_secs(5);

/// A sandbox listing and what it could not include.
#[derive(Clone, Debug, Default)]
pub struct Listing {
    /// Sandboxes, each with its `location`.
    pub sandboxes: Vec<SandboxRecord>,
    /// Sources left out, for example "cloud sandboxes not listed: ...".
    pub warnings: Vec<String>,
}

/// The account's live Fleet claims as records (`location` `cloud`).
/// Namespaces this principal may not read are skipped.
async fn cloud_claims(fleet: &FleetClient) -> Result<Vec<SandboxRecord>> {
    let namespaces = fleet
        .sdk()
        .list_namespaces()
        .await
        .map_err(|e| Error::from(cua_fleet::Error::Sdk(e)))?;
    let mut out = Vec::new();
    for ns in namespaces {
        let claims = match fleet.list_claims(&ns.name).await {
            Ok(c) => c,
            Err(cua_fleet::Error::Sdk(
                cua_fleet::SdkError::Status { status: 403, .. }
                | cua_fleet::SdkError::PoolAccessDenied { .. },
            )) => continue,
            Err(e) => return Err(e.into()),
        };
        for claim in claims {
            let v = serde_json::to_value(&claim).unwrap_or_default();
            let Some(name) = v["metadata"]["name"].as_str().map(str::to_string) else {
                continue;
            };
            let phase = v["status"]["phase"]
                .as_str()
                .unwrap_or("")
                .to_ascii_lowercase();
            let status = match phase.as_str() {
                "bound" => Status::Running,
                "" | "pending" => Status::Provisioning,
                "failed" | "released" => Status::Stopped,
                other => Status::Unknown(other.to_string()),
            };
            out.push(SandboxRecord {
                id: format!("cloud:{name}"),
                location: location_of(ProviderKind::Fleet).into(),
                kind: String::new(),
                runtime: String::new(),
                name: name.clone(),
                provider: ProviderKind::Fleet,
                runtime_type: "fleet".into(),
                status,
                ephemeral: false,
                services: BTreeMap::new(),
                image: None,
                labels: BTreeMap::new(),
                endpoints: BTreeMap::new(),
                created_at: None,
                expires_at: None,
                provider_details: BTreeMap::from([
                    ("pool".to_string(), ns.name.clone()),
                    ("namespace".to_string(), ns.name.clone()),
                    ("claim".to_string(), name),
                ]),
                image_info: None,
            });
        }
    }
    Ok(out)
}

/// The names of the contrib providers this build includes (empty without
/// the `contrib` features).
pub fn contrib_provider_names() -> Vec<String> {
    default_providers()
        .iter()
        .map(|p| p.name().to_string())
        .collect()
}

/// The contrib providers this build includes, configured from the
/// environment and `cua auth provider set` (feature `contrib`).
fn default_providers() -> Vec<Arc<dyn cua_sandbox_core::Provider>> {
    #[cfg(feature = "contrib")]
    {
        cua_contrib::providers()
    }
    #[cfg(not(feature = "contrib"))]
    {
        Vec::new()
    }
}

/// The `location` of a sandbox: a provider sandbox's own word (`aws`,
/// `e2b`, the prefix of its id), else [`location_of`].
fn location_word(p: ProviderKind, id: &str) -> String {
    match (p, id.split_once(':')) {
        (ProviderKind::Contrib, Some((word, _))) if !word.is_empty() => word.to_string(),
        _ => location_of(p).to_string(),
    }
}

/// `location` of a provider.
pub fn location_of(p: ProviderKind) -> &'static str {
    match p {
        ProviderKind::Fleet => Location::Cloud,
        ProviderKind::Local => Location::Local,
        ProviderKind::Direct => Location::Direct,
        ProviderKind::Contrib => Location::Contrib,
    }
    .as_str()
}

/// A spacesd attachment: the client and what a WebSocket upgrade to the
/// same spacesd (the `/media` socket) needs.
#[derive(Clone, Debug)]
pub struct SpacesdAttachment {
    /// Connected client.
    pub client: SpacesdClient,
    /// Extra headers for WebSocket upgrades (Fleet bearer + claim).
    pub ws_headers: Vec<(String, String)>,
}

/// WebSocket upgrade headers for a Fleet sandbox: the bearer and the claim.
fn fleet_ws_headers(token: &str, claim: &str) -> Vec<(String, String)> {
    vec![
        ("authorization".into(), format!("Bearer {token}")),
        (
            cua_spacesd_client::transport::FLEET_CLAIM_HEADER.into(),
            claim.to_owned(),
        ),
    ]
}

/// The Fleet claim in WebSocket headers built by [`fleet_ws_headers`].
fn fleet_claim_of(headers: &[(String, String)]) -> Option<&str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(cua_spacesd_client::transport::FLEET_CLAIM_HEADER))
        .map(|(_, v)| v.as_str())
}

/// An active port forward.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForwardInfo {
    /// Id for [`Runtime::close_forward`] (empty: nothing to close).
    pub id: String,
    /// Guest port.
    pub guest_port: u16,
    /// Loopback address.
    pub local_addr: Option<String>,
    /// URL.
    pub url: Option<String>,
}

/// Handles, env connections, forwards and service URLs are keyed by the
/// sandbox's qualified ref (see [`Runtime::resolve`]).
struct Handle {
    sandbox: Sandbox,
    image: Option<String>,
    labels: BTreeMap<String, String>,
    created_at: Option<SystemTime>,
}

struct Inner {
    sandboxes: Sandboxes,
    fleet: Option<FleetClient>,
    fleet_error: Option<String>,
    vmm: Option<Arc<crate::local::VmmLocal>>,
    env_probe_timeout: Duration,
    handles: Mutex<HashMap<String, Handle>>,
    envs: tokio::sync::Mutex<HashMap<String, SpacesdAttachment>>,
    forwards: Mutex<HashMap<String, Forward>>,
    /// Local public URLs this runtime hosts (the daemon's; an embedded
    /// runtime's only when no daemon can be reached).
    shares: crate::shares::Shares,
    /// This runtime is the daemon's: it hosts local public URLs itself.
    share_host: std::sync::atomic::AtomicBool,
    /// Cloud service URLs handed out by `service_url` (signed, reused until
    /// near expiry), by (sandbox, service).
    service_urls: Mutex<HashMap<(String, String), (String, SystemTime)>>,
    #[cfg(feature = "spaces")]
    spaces: cua_spaces::Spaces,
    /// The attached extensions (the Keyvault broker, when the Cua Spaces
    /// extension is registered).
    #[cfg(feature = "spaces")]
    attached: Vec<Arc<dyn crate::extension::AttachedExtension>>,
    /// The names of this runtime's extensions ([`crate::extension`]).
    #[cfg(feature = "spaces")]
    extension_names: Vec<String>,
}

/// The SDK runtime. Cheap to clone.
#[derive(Clone)]
pub struct Runtime {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Runtime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runtime")
            .field("fleet", &self.inner.fleet.is_some())
            .finish_non_exhaustive()
    }
}

impl Runtime {
    /// Builds the runtime. Never performs network I/O (an extension may open
    /// local state, such as the Keyvault header; none unlocks anything).
    pub fn new(config: RuntimeConfig) -> Result<Self> {
        cua_spacesd_client::transport::ensure_crypto_provider();
        let resolve_default = config
            .resolve_image_digests
            .unwrap_or(config.fleet_client.is_none());
        let (fleet, fleet_error) = match (config.fleet_client, config.fleet) {
            (Some(c), _) => (Some(c), None),
            (None, Some(cfg)) if cfg.has_auth() => match FleetClient::connect(cfg) {
                Ok(c) => (Some(c), None),
                Err(e) => (None, Some(e.to_string())),
            },
            (None, _) => (None, None),
        };
        // One pool manager per runtime: in daemon mode every client's
        // sandboxes share its pool cache, locks and claim heartbeats.
        let pools = fleet.as_ref().map(|f| {
            let mut cfg = config
                .auto_pools
                .clone()
                .unwrap_or_else(cua_sandbox_core::settings::auto_pool_config);
            if config.auto_pools.is_none()
                && let Some(dir) = &config.state_dir
            {
                cfg = cfg.with_state_dir(dir);
            }
            let mgr = PoolManager::new(f.clone(), cfg);
            // Tags resolve to digests so a moved tag gets a fresh pool.
            // Injected (test) clients never reach a registry.
            let resolve = std::env::var("CUA_FLEET_RESOLVE_DIGESTS").map_or(true, |v| v != "0");
            if resolve_default && resolve {
                mgr.with_resolver(Arc::new(RegistryDigests::default()))
            } else {
                mgr
            }
        });
        let mut builder = Sandboxes::builder();
        if let Some(f) = &fleet {
            builder = builder.fleet(f.clone());
        }
        if let Some(p) = &pools {
            builder = builder.pool_manager(p.clone());
        }
        let local = config
            .local
            .or_else(|| config.vmm.clone().map(|v| v as Arc<dyn LocalRuntime>));
        if let Some(l) = local {
            builder = builder.local(l);
        }
        let default_set = config.providers.is_none();
        let providers = config.providers.unwrap_or_else(default_providers);
        for p in providers {
            builder = builder.provider(p);
        }
        // Your own clouds (a cloud provider replaces a contrib one of the
        // same name), with the state of the cua home.
        #[cfg(feature = "byoc")]
        if default_set {
            let home = config
                .spaces_home
                .clone()
                .unwrap_or_else(cua_sandbox_core::settings::cua_home);
            builder = cua_byoc::install(builder, &home);
        }
        #[cfg(not(feature = "byoc"))]
        let _ = default_set;
        if let Some(d) = config.state_dir {
            builder = builder.state_dir(d);
        }
        let sandboxes = builder.build();
        #[cfg(feature = "spaces")]
        let (spaces, attached, extension_names) = {
            let mut b = cua_spaces::Spaces::builder().sandboxes(sandboxes.clone());
            if let Some(f) = &fleet {
                b = b.fleet(f.clone());
            }
            if let Some(h) = &config.spaces_home {
                b = b.home(h.clone());
            }
            let (home, home_explicit) = crate::extension::home_of(config.spaces_home.as_ref());
            let cx = crate::extension::ExtensionContext {
                home: &home,
                home_explicit,
                fleet: fleet.as_ref(),
                teleport_home: config.teleport_home.as_deref(),
            };
            let mut extensions = crate::extension::registered();
            for e in config.extensions {
                if !extensions.iter().any(|x| x.name() == e.name()) {
                    extensions.push(e);
                }
            }
            for e in &extensions {
                b = e.configure(b, &cx);
            }
            let spaces = b.build();
            let attached: Vec<_> = extensions
                .iter()
                .filter_map(|e| e.attach(&spaces, &cx))
                .collect();
            let names = extensions.iter().map(|e| e.name().to_string()).collect();
            (spaces, attached, names)
        };
        Ok(Self {
            inner: Arc::new(Inner {
                #[cfg(feature = "spaces")]
                spaces,
                #[cfg(feature = "spaces")]
                attached,
                #[cfg(feature = "spaces")]
                extension_names,
                sandboxes,
                fleet,
                fleet_error,
                vmm: config.vmm,
                env_probe_timeout: config.env_probe_timeout.unwrap_or(Duration::from_secs(15)),
                handles: Mutex::new(HashMap::new()),
                envs: tokio::sync::Mutex::new(HashMap::new()),
                forwards: Mutex::new(HashMap::new()),
                shares: Default::default(),
                share_host: std::sync::atomic::AtomicBool::new(false),
                service_urls: Mutex::new(HashMap::new()),
            }),
        })
    }

    /// Marks this runtime as the daemon's: local public URLs are served by
    /// its own share proxy instead of asking a daemon.
    pub fn mark_share_host(&self) {
        self.inner
            .share_host
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    fn is_share_host(&self) -> bool {
        self.inner
            .share_host
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// The underlying sandbox manager.
    pub fn sandboxes(&self) -> &Sandboxes {
        &self.inner.sandboxes
    }

    /// The Spaces runtime (registry, primitives, MCP tools), sharing this
    /// runtime's sandboxes and Fleet client.
    #[cfg(feature = "spaces")]
    pub fn spaces(&self) -> &cua_spaces::Spaces {
        &self.inner.spaces
    }

    /// The names of this runtime's extensions ([`crate::extension`]).
    #[cfg(feature = "spaces")]
    pub fn extension_names(&self) -> &[String] {
        &self.inner.extension_names
    }

    /// The attached extensions ([`crate::extension`]).
    #[cfg(feature = "spaces")]
    pub fn attached_extensions(&self) -> &[Arc<dyn crate::extension::AttachedExtension>] {
        &self.inner.attached
    }

    /// The first attached extension of type `T`.
    #[cfg(feature = "spaces")]
    pub fn attached<T: crate::extension::AttachedExtension>(&self) -> Option<&T> {
        self.inner
            .attached
            .iter()
            .find_map(|a| a.as_any().downcast_ref::<T>())
    }

    /// The MCP teleport seam the Spaces `teleport_app` tool routes delivery
    /// through, when an extension hosts the Keyvault here. `None` keeps that
    /// tool fail-closed.
    #[cfg(feature = "spaces")]
    pub fn session_broker(&self) -> Option<Arc<dyn cua_spaces::teleport_broker::SessionBroker>> {
        self.inner.attached.iter().find_map(|a| a.session_broker())
    }

    /// The Fleet client, or `ProviderNotConfigured`.
    pub fn fleet(&self) -> Result<&FleetClient> {
        self.inner.fleet.as_ref().ok_or_else(|| {
            Error::ProviderNotConfigured(
                self.inner
                    .fleet_error
                    .clone()
                    .unwrap_or_else(|| cua_fleet::MISSING_CREDENTIALS.into()),
            )
        })
    }

    /// The managed Fleet pool manager, or `ProviderNotConfigured`.
    pub fn pools(&self) -> Result<&PoolManager> {
        self.fleet()?;
        Ok(self.inner.sandboxes.pools()?)
    }

    /// The cua-vmm backends, or `ProviderNotConfigured`.
    pub fn vmm(&self) -> Result<&Arc<crate::local::VmmLocal>> {
        self.inner.vmm.as_ref().ok_or_else(|| {
            Error::ProviderNotConfigured("local runtimes are disabled in this runtime".into())
        })
    }

    // ------------------------------------------------------------ lifecycle

    /// Creates (Fleet/local) or connects (direct) a sandbox.
    pub async fn create(&self, mut req: CreateRequest) -> Result<SandboxRecord> {
        use cua_sandbox_core::placement::{Kind, On, Runtime};
        let non_empty = |s: &Option<String>| {
            s.as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        let location = non_empty(&req.location);
        // Unset location: the older fields (direct for a URL, Fleet when
        // Fleet options are set), else the user default.
        let fleet_options = req.fleet_runtime.as_deref().is_some_and(|r| !r.is_empty())
            || req.fleet_replicas.is_some_and(|r| r > 0)
            || req.fleet_warm.is_some()
            || req.fleet_max_pool_size.is_some_and(|m| m > 0)
            || req.fleet_ttl_seconds.is_some_and(|t| t > 0);
        let legacy = match (req.provider, &req.url) {
            (Some(ProviderKind::Fleet), _) => Some(On::Cloud),
            (Some(ProviderKind::Local), _) => Some(On::Local),
            (Some(ProviderKind::Direct), u) => Some(On::Direct(u.clone().unwrap_or_default())),
            // A contrib provider is named by `location`, never this legacy path.
            (Some(ProviderKind::Contrib), _) => None,
            (None, Some(u)) if !u.is_empty() => Some(On::Direct(u.clone())),
            (None, _) if fleet_options => Some(On::Cloud),
            (None, _) => None,
        };
        let on = match &location {
            Some(l) => Some(On::parse(l)?),
            None => legacy,
        };
        let kind = non_empty(&req.kind).map(|k| Kind::parse(&k)).transpose()?;
        let runtime = non_empty(&req.runtime)
            .map(|r| Runtime::parse(&r))
            .transpose()?;
        // A client that names the location has resolved the defaults: an
        // empty kind or runtime is `auto`. Otherwise this process applies
        // them (a raw request).
        let resolved = if location.is_some() {
            cua_sandbox_core::settings::Resolved {
                on: on.clone().expect("location parsed"),
                on_source: cua_sandbox_core::settings::Source::Explicit,
                kind: kind.unwrap_or_default(),
                kind_source: cua_sandbox_core::settings::Source::Explicit,
                runtime: runtime.unwrap_or_default(),
                runtime_source: cua_sandbox_core::settings::Source::Explicit,
            }
        } else {
            cua_sandbox_core::settings::Settings::load()
                .and_then(|s| s.resolve(on, kind, runtime))
                .map_err(|e| Error::InvalidArgument(e.to_string()))?
        };
        let provider = match &resolved.on {
            On::Local => ProviderKind::Local,
            On::Cloud => ProviderKind::Fleet,
            On::Direct(addr) => {
                if !addr.is_empty() {
                    req.url = Some(addr.clone());
                }
                ProviderKind::Direct
            }
            On::Relay(id) => {
                return Err(Error::Unsupported(format!(
                    "relay:{id}: relay machines are Spaces; open it with the Spaces API or \
                     `cua mcp` (space relay:{id})"
                )));
            }
            On::Host(h) => {
                return Err(Error::Unsupported(format!(
                    "host:{h}: a host provides Spaces; create one with create_space \
                     (on=\"host:{h}\") or `cua spaces create --on {h}`"
                )));
            }
            On::Provider(p) => {
                if let Err(e) = self.inner.sandboxes.contrib_provider(p) {
                    return Err(e.into());
                }
                ProviderKind::Contrib
            }
        };
        if provider == ProviderKind::Fleet
            && self.inner.fleet.is_none()
            && let Some(hint) = cua_sandbox_core::settings::cloud_default_hint(&resolved.on_source)
        {
            return Err(Error::ProviderNotConfigured(format!(
                "{}; {hint}",
                self.inner
                    .fleet_error
                    .clone()
                    .unwrap_or_else(|| cua_fleet::MISSING_CREDENTIALS.into())
            )));
        }
        if provider == ProviderKind::Direct {
            cua_sandbox_core::placement::validate(&resolved.on, resolved.kind, &resolved.runtime)?;
            let url = req
                .url
                .clone()
                .filter(|u| !u.is_empty())
                .ok_or_else(|| Error::InvalidArgument("direct sandboxes need a url".into()))?;
            // One handle per address (`direct:<host:port>`): the latest
            // connection, with its token, replaces an earlier one.
            let rec = self.connect_url(req.name.as_deref(), &url, req.token.clone(), req.labels)?;
            self.inner.envs.lock().await.remove(&rec.id);
            return Ok(rec);
        }
        if req.image.is_empty() && req.pool.is_none() {
            return Err(Error::InvalidArgument("image is required".into()));
        }
        let mut o = CreateOptions::new(provider, req.image.clone());
        o.name = req.name.clone().filter(|n| !n.is_empty());
        if let Some(os) = req.os.filter(|s| !s.is_empty()) {
            o.os = os;
        }
        if let Some(c) = req.cpus.filter(|c| *c > 0) {
            o.cpus = c;
            o.fleet.cpus_given = true;
        }
        if let Some(m) = req.memory_mb.filter(|m| *m > 0) {
            o.memory_mb = m;
            o.fleet.memory_given = true;
        }
        o.ports = req.ports;
        o.services = req.services;
        o.wait_for = req.wait_for;
        if let Some(t) = req.ready_timeout {
            o.ready_timeout = t;
            o.ready_timeout_given = true;
        }
        o.env = req.env;
        o.command = req.command.filter(|c| !c.is_empty());
        o.env_token = req.token;
        o.fleet.pool = req.pool.filter(|p| !p.is_empty());
        if let Some(r) = req.fleet_runtime.filter(|r| !r.is_empty()) {
            o.fleet.runtime = Some(parse_runtime_kind(&r)?);
        }
        o.fleet.replicas = req.fleet_replicas.filter(|r| *r > 0);
        o.fleet.warm = req.fleet_warm;
        o.fleet.max_pool_size = req.fleet_max_pool_size.filter(|m| *m > 0);
        o.fleet.ttl_seconds_after_created = req.fleet_ttl_seconds.filter(|t| *t > 0);
        o.fleet.apply = req.fleet_apply;
        o.sidecars = req.sidecars;
        o.registry_credentials = req.registry_credentials;
        o.build = req.build.filter(|b| !b.is_empty());
        o.container_runtime = req.container_runtime.filter(|r| !r.is_empty());
        if let Some(n) = req.network.as_deref() {
            o.network = cua_sandbox_core::NetworkMode::parse(n)?;
        }
        o.owner_pid = req.owner_pid.filter(|p| *p > 0);
        o.keep_on_failure = req.keep_on_failure;
        o.gpu = req.gpu.filter(|g| !g.trim().is_empty());
        o.kind = resolved.kind;
        o.runtime = resolved.runtime;
        if provider == ProviderKind::Contrib {
            o.contrib = req.location.filter(|c| !c.is_empty());
        }
        // Cancellable by name ([`Runtime::cancel_create`]), and cleaned up
        // when the caller goes away mid-create.
        let sandbox = Box::pin(self.inner.sandboxes.create_tracked(o)).await?;
        let key = sandbox.id();
        let image = (!req.image.is_empty()).then_some(req.image);
        self.insert(key.clone(), sandbox, image, req.labels);
        self.record(&key).await
    }

    fn connect_url(
        &self,
        name: Option<&str>,
        url: &str,
        token: Option<String>,
        labels: BTreeMap<String, String>,
    ) -> Result<SandboxRecord> {
        let named = name.filter(|n| !n.is_empty()).map(str::to_string);
        let sandbox = self
            .inner
            .sandboxes
            .connect_url_named(url, token, named.as_deref())?;
        // A named direct sandbox is remembered in a state file (URL only; the
        // token stays in memory) so `connect(name)` works later.
        if let Some(n) = &named {
            self.inner.sandboxes.remember_direct(n, url)?;
        }
        let key = sandbox.id();
        self.insert(key.clone(), sandbox, None, labels);
        self.cached_record(&key)
            .ok_or_else(|| Error::Internal("sandbox vanished".into()))
    }

    fn insert(
        &self,
        key: String,
        sandbox: Sandbox,
        image: Option<String>,
        labels: BTreeMap<String, String>,
    ) {
        self.inner.handles.lock().unwrap().insert(
            key,
            Handle {
                sandbox,
                image,
                labels,
                created_at: Some(SystemTime::now()),
            },
        );
    }

    /// Resolves any accepted spelling of a sandbox (a qualified ref, a
    /// legacy id, or a bare name) to its qualified ref. A bare name is
    /// searched across the sandboxes this runtime holds, this machine's
    /// state and local runtime, and the account's live cloud sandboxes; it
    /// must match exactly one ([`Error::AmbiguousSandbox`] lists the
    /// qualified candidates otherwise).
    pub async fn resolve(&self, input: &str) -> Result<SandboxRef> {
        let wanted = SandboxRef::parse(input)?;
        if let SandboxRef::Bare { name } = &wanted {
            let held: Vec<(String, SandboxRef)> = {
                let handles = self.inner.handles.lock().unwrap();
                handles
                    .iter()
                    .filter(|(_, h)| h.sandbox.name() == name)
                    .filter_map(|(k, h)| {
                        Some((h.sandbox.name().to_string(), SandboxRef::parse(k).ok()?))
                    })
                    .collect()
            };
            return Ok(self
                .inner
                .sandboxes
                .resolve_ref_among(&wanted, held)
                .await?);
        }
        Ok(self
            .inner
            .sandboxes
            .resolve_ref_among(&wanted, vec![])
            .await?)
    }

    /// The handle key (the canonical qualified ref) for `input`.
    async fn key(&self, input: &str) -> Result<String> {
        if self.inner.handles.lock().unwrap().contains_key(input) {
            return Ok(input.to_string());
        }
        Ok(self.resolve(input).await?.to_string())
    }

    /// Reattaches to a sandbox by ref or name.
    pub async fn connect(&self, name: &str) -> Result<SandboxRecord> {
        let key = self.handle_key(name).await?;
        self.record(&key).await
    }

    /// The handle for a ref or name (cached, else reattached).
    pub async fn handle(&self, name: &str) -> Result<Sandbox> {
        let key = self.handle_key(name).await?;
        self.inner
            .handles
            .lock()
            .unwrap()
            .get(&key)
            .map(|h| h.sandbox.clone())
            .ok_or_else(|| Error::NotFound(format!("sandbox {key} not found")))
    }

    /// Resolves `input` and makes sure a handle is cached under the key.
    async fn handle_key(&self, input: &str) -> Result<String> {
        let key = self.key(input).await?;
        if self.inner.handles.lock().unwrap().contains_key(&key) {
            return Ok(key);
        }
        let r = SandboxRef::parse(&key)?;
        let r = self.inner.sandboxes.resolve_ref_among(&r, vec![]).await?;
        let sandbox = self.inner.sandboxes.connect_ref(&r).await?;
        self.insert(key.clone(), sandbox, None, BTreeMap::new());
        Ok(key)
    }

    fn cached_record(&self, key: &str) -> Option<SandboxRecord> {
        let handles = self.inner.handles.lock().unwrap();
        let h = handles.get(key)?;
        Some(record_of(key, h, Status::Running))
    }

    /// The plain name a state file uses for the sandbox `key` names.
    fn state_name(key: &str) -> String {
        SandboxRef::parse(key)
            .map(|r| r.name().to_string())
            .unwrap_or_else(|_| key.to_string())
    }

    /// Current record (with a fresh status).
    pub async fn record(&self, name: &str) -> Result<SandboxRecord> {
        let key = self.key(name).await?;
        let sandbox = match self.handle(&key).await {
            Ok(s) => s,
            Err(e) => {
                // A suspended managed Fleet sandbox has no claim to attach
                // to, a stopped local VM or container is not connected to
                // until it is resumed (nor is a stopped VM in your cloud), and
                // a local record whose VM or container is gone (`missing`)
                // has nothing to connect to: report any of them from its
                // record.
                if let Ok(info) = self.inner.sandboxes.get(&Self::state_name(&key)).await
                    && (info.status == Status::Suspended
                        || (matches!(info.provider, ProviderKind::Local | ProviderKind::Contrib)
                            && info.status == Status::Stopped)
                        || info.status == Status::Unknown(cua_sandbox_core::MISSING.into()))
                    && info.id == key
                {
                    let (kind, runtime) = placement_words_of(&info.runtime_type);
                    return Ok(SandboxRecord {
                        location: location_word(info.provider, &info.id),
                        id: info.id,
                        kind,
                        runtime,
                        name: info.name,
                        provider: info.provider,
                        runtime_type: info.runtime_type,
                        status: info.status,
                        ephemeral: false,
                        services: BTreeMap::new(),
                        image: None,
                        labels: BTreeMap::new(),
                        endpoints: BTreeMap::new(),
                        created_at: None,
                        expires_at: None,
                        provider_details: BTreeMap::new(),
                        image_info: None,
                    });
                }
                return Err(e);
            }
        };
        let status = sandbox.status().await?;
        let handles = self.inner.handles.lock().unwrap();
        let h = handles
            .get(&key)
            .ok_or_else(|| Error::NotFound(format!("sandbox {key} not found")))?;
        Ok(record_of(&key, h, status))
    }

    /// Every known sandbox: state files, local instances and live handles.
    pub async fn list(&self, provider: Option<ProviderKind>) -> Result<Vec<SandboxRecord>> {
        let mut out: BTreeMap<String, SandboxRecord> = BTreeMap::new();
        for info in self.inner.sandboxes.list().await? {
            let (mut kind, mut runtime) = placement_words_of(&info.runtime_type);
            let mut expires_at = None;
            let mut provider_details = BTreeMap::new();
            // A provider sandbox (your cloud, contrib): its kind, engine and
            // expiry from what it was saved with.
            if info.provider == ProviderKind::Contrib
                && let Ok(p) = self.inner.sandboxes.contrib_provider(&info.runtime_type)
            {
                let caps = p.capabilities();
                provider_details = self.inner.sandboxes.persisted_details(&info.name);
                if kind.is_empty() {
                    kind = caps
                        .kinds
                        .first()
                        .map(|k| k.as_str().to_string())
                        .unwrap_or_default();
                }
                runtime = provider_details
                    .get("runtime")
                    .cloned()
                    .unwrap_or_else(|| caps.runtime.to_string());
                expires_at = provider_details
                    .get("expires")
                    .and_then(|e| humantime::parse_rfc3339(e).ok());
            }
            out.insert(
                info.id.clone(),
                SandboxRecord {
                    location: location_word(info.provider, &info.id),
                    id: info.id,
                    kind,
                    runtime,
                    name: info.name,
                    provider: info.provider,
                    runtime_type: info.runtime_type,
                    status: info.status,
                    ephemeral: false,
                    services: BTreeMap::new(),
                    image: None,
                    labels: BTreeMap::new(),
                    endpoints: BTreeMap::new(),
                    created_at: None,
                    expires_at,
                    provider_details,
                    image_info: None,
                },
            );
        }
        {
            let handles = self.inner.handles.lock().unwrap();
            for (key, h) in handles.iter() {
                let status = out
                    .get(key)
                    .map(|r| r.status.clone())
                    .unwrap_or(Status::Running);
                out.insert(key.clone(), record_of(key, h, status));
            }
        }
        Ok(out
            .into_values()
            .filter(|r| provider.is_none_or(|p| p == r.provider))
            .collect())
    }

    /// The sandbox listing every surface shows: the known sandboxes of
    /// `provider` (all when `None`) and, with `include_cloud` when the
    /// filter allows cloud rows, the account's live Fleet claims, each
    /// tagged with its `location`. The cloud part never fails the listing:
    /// without Fleet credentials it is skipped silently (a warning only when
    /// only cloud rows were asked for), and a Fleet error or a Fleet call
    /// slower than [`LIST_CLOUD_TIMEOUT`] becomes a warning.
    pub async fn list_filtered(
        &self,
        provider: Option<ProviderKind>,
        include_cloud: bool,
    ) -> Result<Listing> {
        let mut out: BTreeMap<String, SandboxRecord> = self
            .list(provider)
            .await?
            .into_iter()
            .map(|r| (r.id.clone(), r))
            .collect();
        let mut warnings = Vec::new();
        if include_cloud && provider.is_none_or(|p| p == ProviderKind::Fleet) {
            let cloud = match self.fleet() {
                Ok(fleet) => {
                    match tokio::time::timeout(LIST_CLOUD_TIMEOUT, cloud_claims(fleet)).await {
                        Ok(r) => r,
                        Err(_) => Err(Error::Timeout(format!(
                            "Fleet did not answer within {}s",
                            LIST_CLOUD_TIMEOUT.as_secs()
                        ))),
                    }
                }
                Err(e) => Err(e),
            };
            match cloud {
                Ok(rows) => {
                    for r in rows {
                        out.entry(r.id.clone()).or_insert(r);
                    }
                }
                Err(Error::ProviderNotConfigured(_)) if provider.is_none() => {}
                Err(e) => warnings.push(format!("cloud sandboxes not listed: {e}")),
            }
        }
        Ok(Listing {
            sandboxes: out.into_values().collect(),
            warnings,
        })
    }

    /// Deletes (Fleet: release + ephemeral pool; local: remove; direct:
    /// forget).
    /// Stops a named create still running here and waits until what it
    /// made is deleted (`None`: no create of that name was running).
    pub async fn cancel_create(&self, name: &str) -> Result<Option<String>> {
        let name = name
            .rsplit_once(':')
            .map(|(_, n)| n)
            .unwrap_or(name)
            .to_string();
        Ok(self.inner.sandboxes.cancel_create(&name).await?)
    }

    pub async fn delete(&self, name: &str) -> Result<()> {
        let key = self.key(name).await?;
        let sandbox = match self.handle(&key).await {
            Ok(s) => s,
            // Suspended (no claim) or expired: delete by record.
            // A local record whose VM or container is gone, or a local
            // instance with no state file: the manager removes what is left.
            Err(e) => {
                let state_name = Self::state_name(&key);
                if self.inner.sandboxes.state().load(&state_name).is_some()
                    || key.starts_with("local:")
                {
                    self.forget(&key).await;
                    return match self.inner.sandboxes.delete(&state_name).await {
                        Err(cua_sandbox_core::Error::NotFound(_)) => Err(e),
                        r => Ok(r?),
                    };
                }
                return Err(e);
            }
        };
        self.inner.envs.lock().await.remove(&key);
        self.inner.handles.lock().unwrap().remove(&key);
        self.inner.shares.revoke_sandbox(&key);
        self.inner
            .service_urls
            .lock()
            .unwrap()
            .retain(|(sb, _), _| sb != &key);
        let direct = sandbox.provider() == ProviderKind::Direct;
        sandbox.delete().await?;
        if direct {
            // Handles from `connect_url` are ephemeral in sandbox-core; drop
            // the state files `remember_direct` wrote for this address.
            let state = self.inner.sandboxes.state();
            for s in state.list_all() {
                if s.runtime_type() == "direct" && s.sandbox_ref().to_string() == key {
                    state.delete(s.name())?;
                }
            }
        }
        Ok(())
    }

    /// Suspends. A managed Fleet sandbox's handle is dropped with it (its
    /// claim is released; `resume` claims a new one).
    pub async fn suspend(&self, name: &str) -> Result<()> {
        let key = self.key(name).await?;
        let sandbox = match self.handle(&key).await {
            Ok(s) => s,
            // Already suspended (no claim to attach to): nothing to do but
            // go through the manager, which knows.
            Err(_) => {
                return Ok(self
                    .inner
                    .sandboxes
                    .suspend(&Self::state_name(&key))
                    .await?);
            }
        };
        sandbox.suspend().await?;
        if sandbox.provider() == ProviderKind::Fleet {
            self.forget(&key).await;
        }
        Ok(())
    }

    /// Resumes (managed Fleet: the new claim is held by this runtime).
    pub async fn resume(&self, name: &str) -> Result<()> {
        let key = self.key(name).await?;
        let state_name = Self::state_name(&key);
        let fleet_state = key.starts_with("cloud:")
            && matches!(
                self.inner.sandboxes.state().load(&state_name),
                Some(cua_sandbox_core::SandboxState::Fleet(_))
            );
        if fleet_state {
            self.forget(&key).await;
            self.inner.sandboxes.resume(&state_name).await?;
            self.connect(&key).await?;
            return Ok(());
        }
        match self.handle(&key).await {
            Ok(sandbox) => Ok(sandbox.resume().await?),
            // A stopped local VM or container is not connected to (connect
            // wants it running): the manager resumes it by its record.
            Err(e) => match self.stopped_local(&key).await {
                Some(state_name) => Ok(self.inner.sandboxes.resume(&state_name).await?),
                None => Err(e),
            },
        }
    }

    /// The state name of `key` when it is a local or provider sandbox (a
    /// cloud VM) with a state file whose instance is not running (stopped,
    /// or suspended by another process), so lifecycle calls go through the
    /// manager instead of a handle.
    async fn stopped_local(&self, key: &str) -> Option<String> {
        if key.starts_with("cloud:") || key.starts_with("direct:") || key.starts_with("relay:") {
            return None;
        }
        let state_name = Self::state_name(key);
        let info = self.inner.sandboxes.get(&state_name).await.ok()?;
        (matches!(info.provider, ProviderKind::Local | ProviderKind::Contrib)
            && info.id == key
            && matches!(info.status, Status::Stopped | Status::Suspended))
        .then_some(state_name)
    }

    /// Stops holding a managed Fleet claim: no more renewals, the claim
    /// runs until its current shutdown time. The handle is dropped. Needs a
    /// Tokio runtime context (the cached env connection is dropped on it).
    pub fn detach(&self, name: &str) {
        // No lookup here (it is sync): a bare name detaches the one handle
        // of that name, a qualified ref its own.
        let name = {
            let handles = self.inner.handles.lock().unwrap();
            if handles.contains_key(name) {
                name.to_string()
            } else {
                let canon = cua_sandbox_core::refs::canonical(name).unwrap_or_else(|_| name.into());
                let named: Vec<&String> = handles
                    .iter()
                    .filter(|(k, h)| **k == canon || h.sandbox.name() == name)
                    .map(|(k, _)| k)
                    .collect();
                match named.as_slice() {
                    [one] => (*one).clone(),
                    _ => canon,
                }
            }
        };
        let name = name.as_str();
        let handle = self.inner.handles.lock().unwrap().remove(name);
        if let Some(h) = handle {
            h.sandbox.detach();
        }
        let rt = self.clone();
        let name = name.to_string();
        if let Ok(t) = tokio::runtime::Handle::try_current() {
            t.spawn(async move {
                rt.inner.envs.lock().await.remove(&name);
            });
        }
    }

    async fn forget(&self, name: &str) {
        self.inner.envs.lock().await.remove(name);
        self.inner.handles.lock().unwrap().remove(name);
    }

    /// Reattaches every running named sandbox on a managed Fleet pool, so
    /// this (daemon) runtime renews their claims again after a restart.
    /// Returns the names held. Claims that expired meanwhile are skipped.
    pub async fn adopt_managed_fleet_sandboxes(&self) -> Vec<String> {
        if self.inner.fleet.is_none() {
            return vec![];
        }
        let mut held = vec![];
        for s in self.inner.sandboxes.state().list_all() {
            let cua_sandbox_core::SandboxState::Fleet(f) = s else {
                continue;
            };
            if !f
                .pool_name
                .starts_with(cua_fleet::autopool::AUTO_POOL_PREFIX)
                || f.status != "running"
            {
                continue;
            }
            match self.connect(&format!("cloud:{}", f.name)).await {
                Ok(_) => held.push(f.name),
                Err(e) => {
                    tracing::info!(sandbox = %f.name, error = %e, "managed Fleet sandbox not reattached")
                }
            }
        }
        held
    }

    /// Restarts.
    pub async fn restart(&self, name: &str) -> Result<()> {
        let key = self.key(name).await?;
        if key.starts_with("cloud:")
            && matches!(
                self.inner.sandboxes.state().load(&Self::state_name(&key)),
                Some(cua_sandbox_core::SandboxState::Fleet(ref f))
                    if f.pool_name.starts_with(cua_fleet::autopool::AUTO_POOL_PREFIX)
            )
        {
            self.suspend(&key).await?;
            return self.resume(&key).await;
        }
        self.inner.envs.lock().await.remove(&key);
        match self.handle(&key).await {
            Ok(sandbox) => Ok(sandbox.restart().await?),
            Err(e) => match self.stopped_local(&key).await {
                Some(state_name) => Ok(self.inner.sandboxes.restart(&state_name).await?),
                None => Err(e),
            },
        }
    }

    /// Extends a Fleet lease.
    pub async fn keep_alive(&self, name: &str, duration: Duration) -> Result<()> {
        Ok(self.handle(name).await?.keep_alive(duration).await?)
    }

    /// Waits for probes.
    pub async fn wait_ready(&self, name: &str, probes: &[Probe], timeout: Duration) -> Result<()> {
        Ok(self.handle(name).await?.wait_ready(probes, timeout).await?)
    }

    /// One HTTP request to a service.
    #[allow(clippy::too_many_arguments)]
    pub async fn service_request(
        &self,
        name: &str,
        service: &str,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: Option<Vec<u8>>,
        timeout: Duration,
    ) -> Result<cua_sandbox_core::HttpResponse> {
        let sandbox = self.handle(name).await?;
        let method = if method.is_empty() { "GET" } else { method };
        Ok(sandbox
            .service(service)?
            .request_with_headers(method, path, headers, body, timeout)
            .await?)
    }

    /// Where a service is reachable from this process (URL and headers).
    pub async fn service_endpoint(
        &self,
        name: &str,
        service: &str,
    ) -> Result<cua_sandbox_core::ServiceEndpoint> {
        Ok(self
            .handle(name)
            .await?
            .service(service)?
            .endpoint()
            .await?)
    }

    /// A URL for `service` usable from this machine with no credentials:
    /// the published loopback port locally, a signed service URL (1 h,
    /// reused until 5 min before it expires) in the cloud.
    pub async fn service_url(&self, name: &str, service: &str) -> Result<String> {
        let name = self.key(name).await?;
        let sandbox = self.handle(&name).await?;
        let svc = sandbox.service(service)?;
        let Some(bound) = sandbox.fleet_sandbox() else {
            return Ok(svc.url().trim_end_matches('/').to_string());
        };
        let key = (name.clone(), service.to_string());
        let now = SystemTime::now();
        if let Some((url, exp)) = self.inner.service_urls.lock().unwrap().get(&key)
            && *exp > now + Duration::from_secs(300)
        {
            return Ok(url.clone());
        }
        let signed = self
            .fleet()?
            .create_signed_service_url(
                bound,
                service,
                Some("cua service url".into()),
                crate::shares::DEFAULT_TTL,
            )
            .await?;
        let exp = parse_rfc3339(&signed.expires_at).unwrap_or(now + crate::shares::DEFAULT_TTL);
        let url = signed.url.trim_end_matches('/').to_string();
        self.inner
            .service_urls
            .lock()
            .unwrap()
            .insert(key, (url.clone(), exp));
        Ok(url)
    }

    /// A shareable URL for `service` that stops working after `ttl` (60 s to
    /// 24 h, default 1 h): a Fleet signed service URL in the cloud; locally a
    /// loopback proxy URL with its own token, hosted by the cua daemon
    /// (started if needed).
    pub async fn public_url(
        &self,
        name: &str,
        service: &str,
        ttl: Option<Duration>,
        label: Option<String>,
    ) -> Result<crate::shares::PublicUrl> {
        let ttl = ttl.unwrap_or(crate::shares::DEFAULT_TTL);
        if ttl < Duration::from_secs(60) || ttl > crate::shares::MAX_TTL {
            return Err(Error::InvalidArgument(format!(
                "ttl must be between 60 and {} seconds",
                crate::shares::MAX_TTL.as_secs()
            )));
        }
        let name = &self.key(name).await?;
        let sandbox = self.handle(name).await?;
        let svc = sandbox.service(service)?;
        if let Some(bound) = sandbox.fleet_sandbox() {
            let s = self
                .fleet()?
                .create_signed_service_url(bound, service, label.clone(), ttl)
                .await?;
            let mut details: BTreeMap<String, String> = [
                ("provider".to_string(), "fleet".to_string()),
                ("namespace".into(), s.namespace.clone()),
                ("claim".into(), s.claim.clone()),
                ("sandbox".into(), s.sandbox.clone()),
            ]
            .into();
            if let Some(l) = s.label.clone() {
                details.insert("label".into(), l);
            }
            return Ok(crate::shares::PublicUrl {
                id: s.id,
                url: s.url,
                expires_at: parse_rfc3339(&s.expires_at).unwrap_or(SystemTime::now() + ttl),
                sandbox: name.to_string(),
                service: service.to_string(),
                provider_details: details,
            });
        }
        let upstream = svc.url().trim_end_matches('/').to_string();
        self.share(&upstream, ttl, name, service).await
    }

    /// Shares a loopback `upstream` for `ttl`: through the daemon when this
    /// runtime is not the daemon's (starting one if needed), else (or when
    /// no daemon can be reached) from this process.
    pub async fn share(
        &self,
        upstream: &str,
        ttl: Duration,
        name: &str,
        service: &str,
    ) -> Result<crate::shares::PublicUrl> {
        #[cfg(feature = "client")]
        if !self.is_share_host() {
            match crate::client::ensure_daemon().await {
                Ok(d) => return d.share(upstream, ttl, name, service).await,
                Err(e) => tracing::warn!(
                    error = %e,
                    "no cua daemon for the public URL; serving it from this process (it stops \
                     working when the process exits)"
                ),
            }
        }
        self.inner.shares.create(upstream, ttl, name, service).await
    }

    /// Serves `upstream` from this runtime's own share proxy (the daemon's
    /// side of [`Self::share`]).
    pub async fn host_share(
        &self,
        upstream: &str,
        ttl: Duration,
        name: &str,
        service: &str,
    ) -> Result<crate::shares::PublicUrl> {
        self.inner.shares.create(upstream, ttl, name, service).await
    }

    /// Revokes a public URL (cloud: the signed URL; local: the share).
    pub async fn revoke_public_url(&self, name: &str, id: &str) -> Result<()> {
        if self.inner.shares.revoke(id) {
            return Ok(());
        }
        let name = &self.key(name).await?;
        #[cfg(feature = "client")]
        if !self.is_share_host()
            && id.starts_with("share-")
            && let Ok(d) = crate::client::existing_daemon().await
        {
            return d.revoke_public_url(name, id).await;
        }
        let sandbox = self.handle(name).await?;
        let Some(bound) = sandbox.fleet_sandbox() else {
            return Err(Error::NotFound(format!("public URL {id} not found")));
        };
        let fleet = self.fleet()?;
        let url = fleet
            .list_signed_service_urls(bound)
            .await?
            .into_iter()
            .find(|u| u.id == id)
            .ok_or_else(|| Error::NotFound(format!("public URL {id} not found")))?;
        Ok(fleet.revoke_signed_service_url(url).await?)
    }

    /// Where a guest port is reachable.
    pub async fn port(&self, name: &str, port: u16) -> Result<PortTarget> {
        Ok(self.handle(name).await?.port(port)?)
    }

    /// Forwards a loopback port to guest `port`.
    pub async fn forward(&self, name: &str, port: u16) -> Result<ForwardInfo> {
        let fwd = self.handle(name).await?.tunnel().forward(port).await?;
        let info = ForwardInfo {
            id: String::new(),
            guest_port: fwd.guest_port,
            local_addr: fwd.local_addr.map(|a| a.to_string()),
            url: fwd.url.clone(),
        };
        if fwd.local_addr.is_none() {
            return Ok(info);
        }
        let id = format!("fwd-{:016x}", rand::random::<u64>());
        self.inner.forwards.lock().unwrap().insert(id.clone(), fwd);
        Ok(ForwardInfo { id, ..info })
    }

    /// Stops a forward.
    pub fn close_forward(&self, id: &str) -> Result<()> {
        self.inner
            .forwards
            .lock()
            .unwrap()
            .remove(id)
            .map(drop)
            .ok_or_else(|| Error::NotFound(format!("forward {id} not found")))
    }

    // ------------------------------------------------------------------ env

    /// Attaches to cua-spacesd in the sandbox (cached per sandbox).
    pub async fn env(
        &self,
        name: &str,
        probe_timeout: Option<Duration>,
    ) -> Result<SpacesdAttachment> {
        let key = self.key(name).await?;
        let name = key.as_str();
        let cached = self.inner.envs.lock().await.get(name).cloned();
        if let Some(mut a) = cached {
            // The client is reused; the WebSocket bearer is not: Fleet access
            // tokens are short-lived, and a bearer kept with the attachment
            // made every media bridge to a cloud Space fail (the gateway
            // answers 302) a few minutes after the daemon first connected.
            if let Some(claim) = fleet_claim_of(&a.ws_headers).map(str::to_owned) {
                let token = self.fleet()?.access_token(false).await?;
                a.ws_headers = fleet_ws_headers(&token, &claim);
            }
            return Ok(a);
        }
        let sandbox = self.handle(name).await?;
        let client = sandbox
            .spacesd_with(cua_sandbox_core::ConnectOptionsOverride {
                probe_timeout: Some(probe_timeout.unwrap_or(self.inner.env_probe_timeout)),
            })
            .await?;
        let ws_headers = match sandbox.fleet_sandbox() {
            Some(bound) => {
                let token = self.fleet()?.access_token(false).await?;
                fleet_ws_headers(&token, &bound.claim)
            }
            None => vec![],
        };
        let a = SpacesdAttachment { client, ws_headers };
        self.inner
            .envs
            .lock()
            .await
            .insert(name.to_string(), a.clone());
        Ok(a)
    }

    /// Connects an env client to an arbitrary endpoint (no sandbox).
    pub async fn env_url(&self, url: &str, token: Option<String>) -> Result<SpacesdClient> {
        let mut o = ConnectOptions::parse(url)?;
        o.token = token;
        o.probe_timeout = self.inner.env_probe_timeout;
        Ok(SpacesdClient::connect(o).await?)
    }
}

/// `kind` and `runtime` words of a persisted `runtime_type` (empty when it
/// does not say, like `fleet` or `direct`).
fn placement_words_of(runtime_type: &str) -> (String, String) {
    use cua_sandbox_core::placement::{Kind, Runtime};
    let (k, r) = cua_sandbox_core::placement_of_backend(runtime_type);
    (
        if k == Kind::Auto {
            String::new()
        } else {
            k.to_string()
        },
        if r == Runtime::Auto {
            String::new()
        } else {
            r.to_string()
        },
    )
}

fn record_of(key: &str, h: &Handle, status: Status) -> SandboxRecord {
    let sb = &h.sandbox;
    let mut endpoints = BTreeMap::new();
    for svc in sb.services().keys() {
        if let Ok(s) = sb.service(svc) {
            endpoints.insert(svc.clone(), s.url().to_string());
        }
    }
    let (kind, runtime) = sb.placement();
    let word = |auto: bool, s: String| if auto { String::new() } else { s };
    SandboxRecord {
        id: key.to_string(),
        kind: word(
            kind == cua_sandbox_core::placement::Kind::Auto,
            kind.to_string(),
        ),
        runtime: word(
            runtime == cua_sandbox_core::placement::Runtime::Auto,
            runtime.to_string(),
        ),
        name: sb.name().to_string(),
        provider: sb.provider(),
        runtime_type: sb.runtime_type().to_string(),
        status,
        ephemeral: sb.is_ephemeral(),
        services: sb.services().clone(),
        image: h.image.clone(),
        labels: h.labels.clone(),
        endpoints,
        created_at: h.created_at,
        location: sb.location().into(),
        expires_at: sb.expires_at(),
        provider_details: sb.provider_details(),
        image_info: sb.image_info().cloned(),
    }
}

/// RFC 3339 timestamps as Fleet writes them (`Z` or `+00:00`).
fn parse_rfc3339(s: &str) -> Option<SystemTime> {
    let s = s.trim();
    let s = s
        .strip_suffix("+00:00")
        .map_or_else(|| s.to_string(), |b| format!("{b}Z"));
    humantime::parse_rfc3339_weak(&s).ok()
}

/// Resolves image tags to `repo@sha256:...` through the registry (anonymous
/// or docker-config credentials, like `cua image pull`). Failures keep the
/// tag.
#[derive(Default)]
struct RegistryDigests(cua_image::RegistryClient);

#[async_trait::async_trait]
impl cua_fleet::ImageResolver for RegistryDigests {
    async fn resolve(&self, image: &str) -> Option<String> {
        let repo = strip_tag(image)?;
        match tokio::time::timeout(Duration::from_secs(10), self.0.manifest(image)).await {
            Ok(Ok((_, digest))) if digest.starts_with("sha256:") => {
                Some(format!("{repo}@{digest}"))
            }
            Ok(Ok(_)) => None,
            Ok(Err(e)) => {
                tracing::debug!(image, error = %e, "could not resolve image digest; using the tag");
                None
            }
            Err(_) => None,
        }
    }
}

/// `registry/repo:tag` -> `registry/repo` (`None` when already a digest).
fn strip_tag(image: &str) -> Option<String> {
    if image.contains('@') {
        return None;
    }
    let (head, last) = image.rsplit_once('/').unwrap_or(("", image));
    let name = last.split(':').next().unwrap_or(last);
    Some(if head.is_empty() {
        name.to_string()
    } else {
        format!("{head}/{name}")
    })
}

/// Parses a Fleet runtime name.
pub fn parse_runtime_kind(s: &str) -> Result<cua_fleet::RuntimeKind> {
    cua_fleet::parse_runtime(s)?.ok_or_else(|| Error::InvalidArgument("empty Fleet runtime".into()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn fleet_ws_headers_carry_the_claim_for_a_fresh_bearer() {
        let first = super::fleet_ws_headers("old-token", "claim-1");
        assert_eq!(super::fleet_claim_of(&first), Some("claim-1"));
        // What `env` does for a cached attachment: same claim, new bearer.
        let claim = super::fleet_claim_of(&first).unwrap().to_owned();
        let fresh = super::fleet_ws_headers("new-token", &claim);
        assert!(fresh.contains(&("authorization".into(), "Bearer new-token".into())));
        assert_eq!(super::fleet_claim_of(&fresh), Some("claim-1"));
        assert_eq!(
            super::fleet_claim_of(&[]),
            None,
            "local sandboxes send no Fleet headers"
        );
    }

    use super::strip_tag;

    #[test]
    fn fleet_timestamps_parse() {
        let a = super::parse_rfc3339("2026-09-23T10:00:00Z").unwrap();
        assert_eq!(super::parse_rfc3339("2026-09-23T10:00:00+00:00"), Some(a));
        assert!(super::parse_rfc3339("2026-09-23T10:00:00.5Z").is_some());
        assert_eq!(super::parse_rfc3339("nope"), None);
    }

    #[test]
    fn strip_tag_keeps_registry_ports_and_digests() {
        assert_eq!(
            strip_tag("public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:docker-main").as_deref(),
            Some("public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04")
        );
        assert_eq!(
            strip_tag("localhost:5000/img:1").as_deref(),
            Some("localhost:5000/img")
        );
        assert_eq!(strip_tag("ubuntu").as_deref(), Some("ubuntu"));
        assert_eq!(strip_tag("x/y@sha256:ab"), None);
    }
}
