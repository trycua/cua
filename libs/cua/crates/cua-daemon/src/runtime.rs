//! The SDK runtime shared by the embedded and daemon topologies.

use crate::{Error, Result};
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
            .field("local", &self.local.is_some())
            .finish_non_exhaustive()
    }
}

/// A pool ([`Runtime::create_pool`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PoolInfo {
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
    /// After this long without a create it keeps none ready.
    pub idle: Option<Duration>,
    /// Ready now, and starting.
    pub ready: u32,
    pub starting: u32,
    /// Why it holds what it holds, or why it cannot.
    pub status: String,
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
    /// Name. `None`: ephemeral (local) or the URL host (direct).
    pub name: Option<String>,
    /// Image.
    pub image: String,
    /// Direct URL.
    pub url: Option<String>,
    /// spacesd token.
    pub token: Option<String>,
    /// A local pool (`create_pool`): the sandbox takes its image and shape
    /// and one of its ready sandboxes.
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
    /// it).
    pub keep_on_failure: bool,
    /// A GPU option of the runtime (`cua_sandbox_core::gpu`); `None`: no
    /// GPU.
    pub gpu: Option<String>,
    /// No desktop: only cua-spacesd runs in the guest
    /// (`CreateSandboxRequest.headless`).
    pub headless: bool,
    /// Whether an image without cua-spacesd gets a static one injected
    /// (`CreateSandboxRequest.spacesd`); `None`: yes where the kind can.
    pub spacesd: Option<bool>,
    /// A size profile (`cua_sandbox_core::profile`): defaults for what is
    /// unset. `None`: the default sizes.
    pub profile: Option<String>,
    /// A fork source (a "bake" sandbox): a local QEMU VM's guest RAM is a
    /// shared file, so its checkpoints take milliseconds and forks of them
    /// share its pages. Other kinds ignore it.
    pub forkable: bool,
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
    /// `runtime_type` (`direct`, local backend).
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
    /// Provider internals (local backend).
    pub provider_details: BTreeMap<String, String>,
    /// The image as resolved and pinned at create time, when known (not
    /// carried over the daemon wire).
    pub image_info: Option<cua_sandbox_core::ImageInfo>,
    /// Why a local sandbox stopped without being asked to, in plain words
    /// ("out of memory"); `None` otherwise.
    pub stop_reason: Option<String>,
}

/// How long a listing waited for Cua Cloud before it listed without the
/// cloud sandboxes (and said so). Unused since Cua Cloud ended; kept for the
/// callers that still name it.
pub const LIST_CLOUD_TIMEOUT: Duration = Duration::from_secs(5);

/// A sandbox listing and what it could not include.
#[derive(Clone, Debug, Default)]
pub struct Listing {
    /// Sandboxes, each with its `location`.
    pub sandboxes: Vec<SandboxRecord>,
    /// Sources left out, for example "cloud sandboxes not listed: ...".
    pub warnings: Vec<String>,
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
    /// Extra headers for WebSocket upgrades. Always empty now: only a Cua
    /// Cloud sandbox needed them, and Cua Cloud has ended.
    pub ws_headers: Vec<(String, String)>,
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
    /// Which sandbox this handle (and the env connection made from it) is
    /// for: [`Runtime::incarnation`] when it was connected. A same-name
    /// sandbox created again has another one, and the handle is dropped.
    incarnation: Incarnation,
}

/// What the persisted state says about the sandbox behind a handle key.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Incarnation {
    /// The state file of this very ref: its creation time and ports, which
    /// are written once, when the sandbox is created.
    Is(String),
    /// No state file (an ephemeral sandbox, an orphan the local runtime
    /// lists, a direct connection kept in memory).
    Absent,
    /// A state file of the same name that describes another ref: a local and
    /// a cloud sandbox of one name share it. Says nothing about this one.
    Other,
}

impl Incarnation {
    /// Whether a handle made for `self` is for a sandbox that is gone or was
    /// created again, given what the state says `now`. Only a handle that
    /// knew its sandbox's state can be judged: anything else is kept, as it
    /// was before handles carried one.
    fn replaced_by(&self, now: &Incarnation) -> bool {
        match (self, now) {
            (Incarnation::Is(was), Incarnation::Is(is)) => was != is,
            (Incarnation::Is(_), Incarnation::Absent) => true,
            _ => false,
        }
    }
}

struct Inner {
    sandboxes: Sandboxes,
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
    /// `cua share` records and gateway, once the daemon serves.
    #[cfg(feature = "spaces")]
    share_hub: std::sync::OnceLock<Arc<crate::share_hub::ShareHub>>,
    #[cfg(feature = "spaces")]
    spaces: cua_spaces::Spaces,
    /// The attached extensions (the Keyvault broker, when the Cua Spaces
    /// extension is registered).
    #[cfg(feature = "spaces")]
    attached: Vec<Arc<dyn crate::extension::AttachedExtension>>,
    /// The names of this runtime's extensions ([`crate::extension`]).
    #[cfg(feature = "spaces")]
    extension_names: Vec<String>,
    /// The daemon's host half (provided Spaces and this machine's job
    /// runner), once `cua daemon` serves.
    #[cfg(feature = "spaces")]
    host_spaces: std::sync::OnceLock<Arc<cua_spaces::host_spaces::HostSpacesServer>>,
}

/// The SDK runtime. Cheap to clone.
#[derive(Clone)]
pub struct Runtime {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Runtime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runtime").finish_non_exhaustive()
    }
}

impl Runtime {
    /// Builds the runtime. Never performs network I/O (an extension may open
    /// local state, such as the Keyvault header; none unlocks anything).
    pub fn new(config: RuntimeConfig) -> Result<Self> {
        cua_spacesd_client::transport::ensure_crypto_provider();
        let mut builder = Sandboxes::builder();
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
            if let Some(h) = &config.spaces_home {
                b = b.home(h.clone());
            }
            let (home, home_explicit) = crate::extension::home_of(config.spaces_home.as_ref());
            let cx = crate::extension::ExtensionContext {
                home: &home,
                home_explicit,
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
                #[cfg(feature = "spaces")]
                host_spaces: std::sync::OnceLock::new(),
                sandboxes,
                vmm: config.vmm,
                env_probe_timeout: config.env_probe_timeout.unwrap_or(Duration::from_secs(15)),
                handles: Mutex::new(HashMap::new()),
                envs: tokio::sync::Mutex::new(HashMap::new()),
                forwards: Mutex::new(HashMap::new()),
                shares: Default::default(),
                share_host: std::sync::atomic::AtomicBool::new(false),
                #[cfg(feature = "spaces")]
                share_hub: std::sync::OnceLock::new(),
            }),
        })
    }

    /// The `cua share` hub, once the daemon started it.
    #[cfg(feature = "spaces")]
    pub fn share_hub(&self) -> Option<&Arc<crate::share_hub::ShareHub>> {
        self.inner.share_hub.get()
    }

    /// Opens the share records under `home` and starts the share gateway
    /// (the daemon calls this once).
    #[cfg(feature = "spaces")]
    pub async fn start_shares(&self, home: &std::path::Path) -> Result<()> {
        let hub = crate::share_hub::ShareHub::open(home)?;
        if self.inner.share_hub.set(hub.clone()).is_err() {
            return Ok(());
        }
        hub.start(self).await
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
    /// runtime's sandboxes.
    #[cfg(feature = "spaces")]
    pub fn spaces(&self) -> &cua_spaces::Spaces {
        &self.inner.spaces
    }

    /// The daemon's host half: this machine's job runner and the Spaces it
    /// provides (`None` in a runtime no daemon serves).
    #[cfg(feature = "spaces")]
    pub fn host_spaces(&self) -> Option<Arc<cua_spaces::host_spaces::HostSpacesServer>> {
        self.inner.host_spaces.get().cloned()
    }

    #[cfg(all(feature = "spaces", feature = "server"))]
    pub(crate) fn set_host_spaces(&self, h: Arc<cua_spaces::host_spaces::HostSpacesServer>) {
        let _ = self.inner.host_spaces.set(h);
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

    /// The cua-vmm backends, or `ProviderNotConfigured`.
    pub fn vmm(&self) -> Result<&Arc<crate::local::VmmLocal>> {
        self.inner.vmm.as_ref().ok_or_else(|| {
            Error::ProviderNotConfigured("local runtimes are disabled in this runtime".into())
        })
    }

    // ------------------------------------------------------------ lifecycle

    /// Creates (local, your cloud, a provider) or connects (direct) a
    /// sandbox. `on="cloud"` fails with [`cua_sandbox_core::CLOUD_ENDED`].
    pub async fn create(&self, mut req: CreateRequest) -> Result<SandboxRecord> {
        use cua_sandbox_core::placement::{Kind, On, Runtime};
        let non_empty = |s: &Option<String>| {
            s.as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        let location = non_empty(&req.location);
        // Unset location: the older fields (direct for a URL), else the
        // user default.
        let legacy = match (req.provider, &req.url) {
            (Some(ProviderKind::Local), _) => Some(On::Local),
            (Some(ProviderKind::Direct), u) => Some(On::Direct(u.clone().unwrap_or_default())),
            // A contrib provider is named by `location`, never this legacy path.
            (Some(ProviderKind::Contrib), _) => None,
            (None, Some(u)) if !u.is_empty() => Some(On::Direct(u.clone())),
            (None, _) => None,
        };
        let on = match &location {
            Some(l) => Some(On::parse(l)?),
            None => legacy,
        };
        // A local pool by name: the create runs its shape, kind and runtime
        // included (so it resolves exactly like the pool's sandboxes and
        // claims one of them).
        if on.as_ref().is_none_or(|o| *o == On::Local)
            && let Some(name) = req.pool.clone().filter(|p| !p.is_empty())
            && let Ok(vmm) = self.vmm()
            && let Some(pool) = vmm.pool()
            && let Some(np) = pool.named(&name)
        {
            req = crate::pool::named::merge_into(req, &np)?;
        }
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
            On::Cloud => {
                return Err(Error::Unsupported(cua_sandbox_core::CLOUD_ENDED.into()));
            }
            On::Direct(addr) => {
                if !addr.is_empty() {
                    req.url = Some(addr.clone());
                }
                ProviderKind::Direct
            }
            On::Relay(id) => {
                return Err(Error::Unsupported(format!(
                    "relay:{id}: relay computers are Spaces; open it with the Spaces API or \
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
            On::Auto => {
                return Err(Error::Unsupported(
                    "auto places a Space on the least-loaded of your machines; create it with                      create_space (on=\"auto\") or `cua spaces create --on auto`"
                        .into(),
                ));
            }
        };
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
        if req.image.is_empty() {
            return Err(Error::InvalidArgument("image is required".into()));
        }
        let image_given = req.image.clone();
        let labels = req.labels.clone();
        let location = req.location.clone();
        let mut o = options_from(req, provider, resolved.kind, resolved.runtime.clone())?;
        if provider == ProviderKind::Contrib {
            o.contrib = location.filter(|c| !c.is_empty());
        }
        // Cancellable by name ([`Runtime::cancel_create`]), and cleaned up
        // when the caller goes away mid-create.
        let sandbox = Box::pin(self.inner.sandboxes.create_tracked(o)).await?;
        let key = sandbox.id();
        let image = (!image_given.is_empty()).then_some(image_given);
        let incarnation = self.incarnation(&key);
        self.insert(key.clone(), sandbox, image, labels, incarnation);
        // A same-name sandbox created again is not the one an earlier
        // connection was made for.
        self.inner.envs.lock().await.remove(&key);
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
        let incarnation = self.incarnation(&key);
        self.insert(key.clone(), sandbox, None, labels, incarnation);
        self.cached_record(&key)
            .ok_or_else(|| Error::Internal("sandbox vanished".into()))
    }

    fn insert(
        &self,
        key: String,
        sandbox: Sandbox,
        image: Option<String>,
        labels: BTreeMap<String, String>,
        incarnation: Incarnation,
    ) {
        self.inner.handles.lock().unwrap().insert(
            key,
            Handle {
                sandbox,
                image,
                labels,
                created_at: Some(SystemTime::now()),
                incarnation,
            },
        );
    }

    /// Which sandbox the persisted state of `key` describes ([`Incarnation`]).
    /// A sandbox deleted and created again under the same name has another
    /// one (and none in between), whoever deleted it: this runtime, another
    /// `Spaces`/`Sandboxes` on the same state directory, or another process.
    fn incarnation(&self, key: &str) -> Incarnation {
        let Some(state) = self.inner.sandboxes.state().load(&Self::state_name(key)) else {
            return Incarnation::Absent;
        };
        if state.sandbox_ref().to_string() != key {
            return Incarnation::Other;
        }
        Incarnation::Is(match &state {
            cua_sandbox_core::SandboxState::Local(l) => {
                format!("{}|{}|{:?}", l.created_at, l.api_port, l.exposed_ports)
            }
            _ => state.name().to_string(),
        })
    }

    /// Drops what is cached for `key` (the handle, its env connection and its
    /// public URLs) when the sandbox it was made for is gone or was created
    /// again under the same name; returns whether it did. The Spaces runtime
    /// deletes a local Space by name in the sandbox manager
    /// ([`cua_spaces::Spaces::delete`]), which these caches never see, so
    /// without this check the new sandbox (new port, new token) was reached
    /// through the old connection until the daemon restarted.
    async fn evict_if_stale(&self, key: &str) -> bool {
        let held = match self.inner.handles.lock().unwrap().get(key) {
            Some(h) => h.incarnation.clone(),
            None => return false,
        };
        if !held.replaced_by(&self.incarnation(key)) {
            return false;
        }
        tracing::debug!(
            sandbox = key,
            "dropping the cached connection of a sandbox that was deleted or created again"
        );
        self.evict(key).await;
        true
    }

    /// [`Self::evict_if_stale`] for every held sandbox.
    async fn evict_stale(&self) {
        let keys: Vec<String> = self.inner.handles.lock().unwrap().keys().cloned().collect();
        for key in keys {
            self.evict_if_stale(&key).await;
        }
    }

    /// Drops everything this runtime caches for `key`.
    async fn evict(&self, key: &str) {
        self.forget(key).await;
        self.inner.shares.revoke_sandbox(key);
        #[cfg(feature = "spaces")]
        if let Some(hub) = self.inner.share_hub.get() {
            hub.stop_space(self, key);
        }
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
            // A held sandbox that was deleted behind this runtime must not
            // answer for its name.
            let named: Vec<String> = self
                .inner
                .handles
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, h)| h.sandbox.name() == name)
                .map(|(k, _)| k.clone())
                .collect();
            for key in named {
                self.evict_if_stale(&key).await;
            }
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
    ///
    /// A held sandbox that was deleted, or deleted and created again under
    /// the same name, is dropped on the way ([`Self::evict_if_stale`]), so
    /// no operation goes through a stale handle or env connection.
    pub(crate) async fn key(&self, input: &str) -> Result<String> {
        let held = self.inner.handles.lock().unwrap().contains_key(input);
        if held && !self.evict_if_stale(input).await {
            return Ok(input.to_string());
        }
        let key = self.resolve(input).await?.to_string();
        // Another spelling of a held sandbox (a legacy id).
        self.evict_if_stale(&key).await;
        Ok(key)
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
        // Read before connecting: a sandbox created again in between leaves
        // this handle looking stale (it is dropped and connected again)
        // rather than current.
        let incarnation = self.incarnation(&key);
        let r = self.inner.sandboxes.resolve_ref_among(&r, vec![]).await?;
        let sandbox = self.inner.sandboxes.connect_ref(&r).await?;
        self.insert(key.clone(), sandbox, None, BTreeMap::new(), incarnation);
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

    /// Whether the state file named like `key` describes `key` itself: a
    /// local and a cloud sandbox of one name share one state file, so the
    /// name alone does not say whose it is.
    fn owns_state(&self, key: &str) -> bool {
        self.inner
            .sandboxes
            .state()
            .load(&Self::state_name(key))
            .is_some_and(|s| s.sandbox_ref().to_string() == key)
    }

    /// Current record (with a fresh status).
    pub async fn record(&self, name: &str) -> Result<SandboxRecord> {
        let key = self.key(name).await?;
        let sandbox = match self.handle(&key).await {
            Ok(s) => s,
            Err(e) => {
                // A suspended sandbox has nothing to attach to, a stopped
                // local VM or container is not connected to until it is
                // resumed (nor is a stopped VM in your cloud), and a local
                // record whose VM or container is gone (`missing`) has
                // nothing to connect to: report any of them from its record.
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
                        stop_reason: info.stop_reason.map(|r| r.words()),
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
        // A sandbox deleted behind this runtime is not listed as running.
        self.evict_stale().await;
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
                    stop_reason: info.stop_reason.map(|r| r.words()),
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
                let stop_reason = out.get(key).and_then(|r| r.stop_reason.clone());
                out.insert(
                    key.clone(),
                    SandboxRecord {
                        stop_reason,
                        ..record_of(key, h, status)
                    },
                );
            }
        }
        Ok(out
            .into_values()
            .filter(|r| provider.is_none_or(|p| p == r.provider))
            .collect())
    }

    /// The sandbox listing every surface shows: the known sandboxes of
    /// `provider` (all when `None`), each tagged with its `location`.
    pub async fn list_filtered(&self, provider: Option<ProviderKind>) -> Result<Listing> {
        let out: BTreeMap<String, SandboxRecord> = self
            .list(provider)
            .await?
            .into_iter()
            .map(|r| (r.id.clone(), r))
            .collect();
        Ok(Listing {
            sandboxes: out.into_values().collect(),
            warnings: Vec::new(),
        })
    }

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

    /// Deletes (local: remove; direct: forget; your cloud: delete the
    /// machine).
    pub async fn delete(&self, name: &str) -> Result<()> {
        let key = self.key(name).await?;
        let ended = self.delete_sandbox(&key).await;
        if ended.is_ok() {
            self.forget_space(&key).await;
        }
        ended
    }

    /// Drops the Space record of a local sandbox that is gone, so
    /// `list_spaces` does not keep a stale `local:<name>` (a fork batch's
    /// forks, deleted through the sandboxes API, left theirs behind).
    async fn forget_space(&self, key: &str) {
        #[cfg(feature = "spaces")]
        if key.starts_with("local:") {
            let _ = self.inner.spaces.remove(key).await;
        }
        #[cfg(not(feature = "spaces"))]
        let _ = key;
    }

    async fn delete_sandbox(&self, key: &str) -> Result<()> {
        let key = key.to_string();
        let sandbox = match self.handle(&key).await {
            Ok(s) => s,
            // A local record whose VM or container is gone, or a local
            // instance with no state file: the manager removes what is left.
            Err(e) => {
                let state_name = Self::state_name(&key);
                if self.owns_state(&key) || key.starts_with("local:") {
                    self.forget(&key).await;
                    return match self.inner.sandboxes.delete(&state_name).await {
                        Err(cua_sandbox_core::Error::NotFound(_)) => Err(e),
                        r => Ok(r?),
                    };
                }
                return Err(e);
            }
        };
        self.evict(&key).await;
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

    /// Saves a local sandbox's state as checkpoint `checkpoint`.
    pub async fn checkpoint(
        &self,
        name: &str,
        checkpoint: &str,
    ) -> Result<cua_sandbox_core::CheckpointRecord> {
        self.checkpoint_with(name, checkpoint, false).await
    }

    /// [`Runtime::checkpoint`], pinning the new checkpoint when `pin`.
    pub async fn checkpoint_with(
        &self,
        name: &str,
        checkpoint: &str,
        pin: bool,
    ) -> Result<cua_sandbox_core::CheckpointRecord> {
        let key = self.key(name).await?;
        if !key.starts_with("local:") {
            return Err(Error::Unsupported(format!(
                "checkpoints are for local sandboxes ({key} is not one)"
            )));
        }
        Ok(self
            .inner
            .sandboxes
            .checkpoint_with(&Self::state_name(&key), checkpoint, pin)
            .await?)
    }

    /// Creates and starts `new_name` from a local sandbox or a checkpoint.
    pub async fn fork(
        &self,
        source: &str,
        new_name: &str,
        labels: BTreeMap<String, String>,
    ) -> Result<SandboxRecord> {
        // A sandbox ref or name wins; anything else names a checkpoint.
        let from = match self.key(source).await {
            Ok(key) if key.starts_with("local:") => Self::state_name(&key),
            Ok(key) => {
                return Err(Error::Unsupported(format!(
                    "forks are of local sandboxes ({key} is not one)"
                )));
            }
            Err(_) => source.to_string(),
        };
        let sandbox = self.inner.sandboxes.fork(&from, new_name).await?;
        let key = sandbox.id();
        let incarnation = self.incarnation(&key);
        self.insert(key.clone(), sandbox, None, labels, incarnation);
        // A same-name sandbox forked again is not the one an earlier
        // connection was made for.
        self.inner.envs.lock().await.remove(&key);
        self.record(&key).await
    }

    /// Every checkpoint.
    pub fn checkpoints(&self) -> Vec<cua_sandbox_core::CheckpointRecord> {
        self.inner.sandboxes.checkpoints()
    }

    /// Saves `name`'s state as checkpoint `checkpoint` where it runs: a
    /// local sandbox here ([`Runtime::checkpoint_with`]); a Space a machine
    /// of yours provides (by its name or id) on that machine, which keeps
    /// the checkpoint (`Checkpoint.host` names it).
    pub async fn checkpoint_anywhere(
        &self,
        name: &str,
        checkpoint: &str,
        pin: bool,
    ) -> Result<cua_proto::daemon::v1::Checkpoint> {
        #[cfg(feature = "spaces")]
        if self.is_space_elsewhere(name).await {
            use cua_spaces::pool_checkpoints::SpaceCheckpoint;
            return Ok(
                match self
                    .spaces()
                    .checkpoint_space(name, checkpoint, pin)
                    .await?
                {
                    SpaceCheckpoint::Local(rec) => crate::convert::checkpoint_to_pb(&rec),
                    SpaceCheckpoint::Remote { host, snapshot } => {
                        crate::convert::host_snapshot_to_pb(&snapshot, &host)
                    }
                },
            );
        }
        Ok(crate::convert::checkpoint_to_pb(
            &self.checkpoint_with(name, checkpoint, pin).await?,
        ))
    }

    /// Whether `name` is not a local sandbox but a Space the registry knows
    /// that runs elsewhere (a machine of yours, your cloud).
    #[cfg(feature = "spaces")]
    async fn is_space_elsewhere(&self, name: &str) -> bool {
        if self.key(name).await.is_ok_and(|k| k.starts_with("local:")) {
            return false;
        }
        matches!(
            self.spaces().resolve(name),
            Ok(id) if !matches!(id, cua_spaces::SpaceId::Local { .. })
        )
    }

    /// Every checkpoint: this machine's (oldest first), then those your
    /// machines that provide Spaces hold, each name once
    /// (`Checkpoint.host`).
    pub async fn checkpoints_everywhere(&self) -> Vec<cua_proto::daemon::v1::Checkpoint> {
        #[allow(unused_mut)]
        let mut out: Vec<_> = self
            .checkpoints()
            .iter()
            .map(crate::convert::checkpoint_to_pb)
            .collect();
        #[cfg(feature = "spaces")]
        match self.spaces().pool_checkpoints().await {
            Ok(all) => out.extend(all.into_iter().filter_map(|c| {
                c.host
                    .map(|h| crate::convert::host_snapshot_to_pb(&c.snapshot, &h))
            })),
            Err(e) => tracing::debug!("checkpoints of your other machines not listed: {e}"),
        }
        out
    }

    /// Deletes checkpoint `name` here and on every machine of yours that
    /// holds a copy (forks made from it keep running). A pinned one is
    /// refused until it is unpinned.
    pub async fn delete_checkpoint_everywhere(&self, name: &str) -> Result<()> {
        #[cfg(feature = "spaces")]
        {
            self.spaces().delete_checkpoint_everywhere(name).await?;
            Ok(())
        }
        #[cfg(not(feature = "spaces"))]
        {
            self.delete_checkpoint(name).await
        }
    }

    /// Forks many sandboxes from checkpoints at once, on this machine or
    /// across your machines ([`cua_spaces::fork_batch`]). Returns the
    /// batch's events as they happen; the last is `done`. The batch runs
    /// to completion even if the receiver is dropped.
    #[cfg(feature = "spaces")]
    pub async fn fork_batch(
        &self,
        req: cua_proto::daemon::v1::ForkSandboxBatchRequest,
    ) -> Result<tokio::sync::mpsc::Receiver<Result<cua_proto::daemon::v1::ForkSandboxBatchResponse>>>
    {
        use cua_proto::daemon::v1::fork_sandbox_batch_response::Event;
        let spec = cua_spaces::fork_batch::BatchSpec::from_request(req)?;
        let spaces = self.spaces().clone();
        let (events, mut from_batch) = tokio::sync::mpsc::channel(256);
        let (tx, rx) = tokio::sync::mpsc::channel(256);
        let failed = tx.clone();
        tokio::spawn(async move {
            if let Err(e) = spaces.fork_batch(spec, events).await {
                let _ = failed.send(Err(e.into())).await;
            }
        });
        let rt = self.clone();
        tokio::spawn(async move {
            while let Some(mut ev) = from_batch.recv().await {
                // A fork on this machine is a sandbox of this runtime's.
                let local = match ev.event.as_ref() {
                    Some(Event::Item(i))
                        if i.host == cua_spaces::fork_batch::THIS_MACHINE && i.error.is_empty() =>
                    {
                        Some(i.space.clone())
                    }
                    _ => None,
                };
                if let Some(space) = local
                    && let Ok(rec) = rt.record(&space).await
                {
                    ev.sandbox = Some(crate::convert::info_to_pb(&rec));
                }
                let _ = tx.send(Ok(ev)).await;
            }
        });
        Ok(rx)
    }

    /// Uploads checkpoint `name` to `fallback` (S3, or a folder every host
    /// mounts), so your other machines can copy it when this one is
    /// unreachable. Returns the bytes uploaded.
    #[cfg(feature = "spaces")]
    pub async fn publish_checkpoint(
        &self,
        name: &str,
        fallback: &cua_proto::env::v1::SnapshotFallback,
    ) -> Result<u64> {
        Ok(self.spaces().publish_snapshot(name, fallback, None).await?)
    }

    /// Deletes a checkpoint (a pinned one is refused until it is unpinned).
    pub async fn delete_checkpoint(&self, name: &str) -> Result<()> {
        Ok(self.inner.sandboxes.delete_checkpoint(name).await?)
    }

    /// Pins or unpins a checkpoint; returns it as it is now.
    pub async fn pin_checkpoint(
        &self,
        name: &str,
        pinned: bool,
    ) -> Result<cua_sandbox_core::CheckpointRecord> {
        Ok(self.inner.sandboxes.pin_checkpoint(name, pinned).await?)
    }

    /// Deletes every checkpoint that is not pinned; returns their names.
    pub async fn prune_checkpoints(&self) -> Result<Vec<String>> {
        Ok(self.inner.sandboxes.prune_checkpoints().await?)
    }

    /// The warm pool of this runtime (a daemon's; an embedded runtime has
    /// none).
    fn warm_pool(&self) -> Result<(&Arc<crate::local::VmmLocal>, &crate::pool::WarmPool)> {
        let vmm = self.vmm()?;
        let pool = vmm.pool().ok_or_else(|| {
            Error::Unsupported("local pools are kept by `cua daemon`; start it first".into())
        })?;
        Ok((vmm, pool))
    }

    /// Creates pool `name` (or updates the pool of that name; `None`: a
    /// new name): sandboxes like `spec` kept ready on this machine, `min`
    /// at all times (one starts again as soon as one is created from the
    /// pool), up to `max` (not counting those created from it) while creates
    /// come faster than sandboxes start, and none after `idle` without a
    /// create (until the next one). Applied at once; its sandboxes start in
    /// the background. Pools run on this machine only (`on="local"`).
    pub async fn create_pool(
        &self,
        name: Option<&str>,
        spec: CreateRequest,
        min: u32,
        max: u32,
        idle: Option<Duration>,
    ) -> Result<PoolInfo> {
        if let Some(on) = spec
            .location
            .as_deref()
            .map(str::trim)
            .filter(|l| !l.is_empty() && *l != "local")
        {
            return Err(Error::Unsupported(format!(
                "pools run on this machine only today: create it with on=\"local\", not {on:?}"
            )));
        }
        let (vmm, pool) = self.warm_pool()?;
        let name = name
            .filter(|n| !n.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("pool-{:06x}", rand::random::<u32>() & 0xff_ffff));
        let np = crate::pool::named::NamedPool::new(&name, spec, min, max, idle)?;
        pool.save_named(np)
            .map_err(|e| Error::Internal(format!("storing pool {name}: {e}")))?;
        vmm.apply_pools().await;
        self.get_pool(&name)
    }

    /// Pool `name`, with what it holds now.
    pub fn get_pool(&self, name: &str) -> Result<PoolInfo> {
        self.list_pools()?
            .into_iter()
            .find(|p| p.name == name)
            .ok_or_else(|| Error::NotFound(format!("no pool named {name:?}")))
    }

    /// Every pool, with what each holds now.
    pub fn list_pools(&self) -> Result<Vec<PoolInfo>> {
        let (vmm, pool) = self.warm_pool()?;
        let mut out = Vec::new();
        for np in pool.named_pools() {
            let r = np.request()?;
            let (key, err) = vmm.named_pool_state(&np.name);
            let (ready, starting, reason) = key
                .as_deref()
                .and_then(|k| pool.shape_status(k))
                .unwrap_or((0, 0, String::new()));
            out.push(PoolInfo {
                name: np.name.clone(),
                image: r.image,
                location: "local".into(),
                kind: r.kind.unwrap_or_default(),
                runtime: r.runtime.unwrap_or_default(),
                cpus: r
                    .cpus
                    .filter(|c| *c > 0)
                    .unwrap_or(crate::pool::DEFAULT_SIZE.0),
                memory_mb: r
                    .memory_mb
                    .filter(|m| *m > 0)
                    .unwrap_or(crate::pool::DEFAULT_SIZE.1),
                min: np.min,
                max: np.max,
                idle: np.idle(),
                ready,
                starting,
                status: if err.is_empty() { reason } else { err },
            });
        }
        Ok(out)
    }

    /// Deletes pool `name` (its ready sandboxes go; those created from it
    /// stay). `false`: no such pool.
    pub fn delete_pool(&self, name: &str) -> Result<bool> {
        let (_, pool) = self.warm_pool()?;
        pool.remove_named(name)
            .map_err(|e| Error::Internal(format!("deleting pool {name}: {e}")))
    }

    /// Suspends.
    pub async fn suspend(&self, name: &str) -> Result<()> {
        let key = self.key(name).await?;
        let sandbox = match self.handle(&key).await {
            Ok(s) => s,
            // Already suspended (nothing to attach to): nothing to do but go
            // through the manager, which knows. Only when its record is this
            // sandbox's own: a `cloud:box` ref never suspends `local:box`.
            Err(e) => {
                if !self.owns_state(&key) {
                    return Err(e);
                }
                return Ok(self
                    .inner
                    .sandboxes
                    .suspend(&Self::state_name(&key))
                    .await?);
            }
        };
        Ok(sandbox.suspend().await?)
    }

    /// Resumes.
    pub async fn resume(&self, name: &str) -> Result<()> {
        let key = self.key(name).await?;
        match self.handle(&key).await {
            // A local VM that was stopped boots with a new address (Lume,
            // a Mac microVM) or new host ports (a Linux microVM):
            // `refresh_handle` below points the cached handle at them.
            Ok(sandbox) => sandbox.resume().await?,
            // A stopped local VM or container is not connected to (connect
            // wants it running): the manager resumes it by its record.
            Err(e) => match self.stopped_local(&key).await {
                Some(state_name) => self.inner.sandboxes.resume(&state_name).await?,
                None => return Err(e),
            },
        }
        self.refresh_handle(&key).await;
        Ok(())
    }

    /// A sandbox that booted again may listen on new host ports (a microVM
    /// picks them on every boot): point its cached handle and env connection
    /// at the endpoints its record now holds.
    async fn refresh_handle(&self, key: &str) {
        self.inner.envs.lock().await.remove(key);
        // The Space of this sandbox caches its own connection (what the
        // Space env passthrough, and so an SDK's `Space.spacesd()`, goes
        // through): one made before the reboot dials the old run, and would
        // fail with transport errors until the daemon restarts. A Space's
        // own `start`/`stop` drop it; a resize or a sandbox-level restart or
        // resume must too. Not a Space: nothing is cached.
        #[cfg(feature = "spaces")]
        let _ = self.spaces().forget_connection(key).await;
        let Ok(r) = SandboxRef::parse(key) else {
            return;
        };
        let Ok(r) = self.inner.sandboxes.resolve_ref_among(&r, vec![]).await else {
            return;
        };
        let Ok(fresh) = self.inner.sandboxes.connect_ref(&r).await else {
            return;
        };
        if let Some(h) = self.inner.handles.lock().unwrap().get_mut(key) {
            h.sandbox = fresh;
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

    /// Drops the handle and the cached connection of `name` (a bare name
    /// or a qualified ref), so a new sandbox of that name is not reached
    /// through the old one's connection.
    pub async fn detach(&self, name: &str) {
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
        self.forget(&name).await;
    }

    async fn forget(&self, name: &str) {
        self.inner.envs.lock().await.remove(name);
        self.inner.handles.lock().unwrap().remove(name);
    }

    /// Restarts.
    pub async fn restart(&self, name: &str) -> Result<()> {
        let key = self.key(name).await?;
        self.inner.envs.lock().await.remove(&key);
        match self.handle(&key).await {
            Ok(sandbox) => sandbox.restart().await?,
            Err(e) => match self.stopped_local(&key).await {
                Some(state_name) => self.inner.sandboxes.restart(&state_name).await?,
                None => return Err(e),
            },
        }
        self.refresh_handle(&key).await;
        Ok(())
    }

    /// Gives a local sandbox another shape: it stops and boots again from
    /// its own disk (see [`cua_sandbox_core::Sandboxes::resize`]).
    pub async fn resize(&self, name: &str, shape: &cua_vmm::Resize) -> Result<()> {
        let key = self.key(name).await?;
        self.inner.envs.lock().await.remove(&key);
        match self.handle(&key).await {
            Ok(sandbox) => sandbox.resize(shape).await?,
            Err(e) => match self.stopped_local(&key).await {
                Some(state_name) => self.inner.sandboxes.resize(&state_name, shape).await?,
                None => return Err(e),
            },
        }
        self.refresh_handle(&key).await;
        Ok(())
    }

    /// Extends a sandbox lease (a no-op where the sandbox has none); gives
    /// a local sandbox a lifetime of `duration` from now (see
    /// [`Runtime::reap_expired`]).
    pub async fn keep_alive(&self, name: &str, duration: Duration) -> Result<()> {
        Ok(self.handle(name).await?.keep_alive(duration).await?)
    }

    /// [`Runtime::keep_alive`] for a local sandbox, choosing what happens
    /// when the lifetime ends (`delete` or `suspend`).
    pub async fn set_lifetime(
        &self,
        name: &str,
        duration: Duration,
        on_expire: cua_sandbox_core::LocalExpiry,
    ) -> Result<()> {
        let key = self.key(name).await?;
        Ok(self.inner.sandboxes.set_local_expiry(
            &Self::state_name(&key),
            duration,
            Some(on_expire),
        )?)
    }

    /// Ends the local sandboxes whose lifetime passed: deletes each (its
    /// Space too), or suspends it when its lifetime said so. The daemon
    /// runs this every few seconds, so a lifetime holds after the client
    /// that set it has gone. Returns the sandboxes it ended.
    pub async fn reap_expired(&self) -> Vec<String> {
        let mut ended = Vec::new();
        for (name, action) in self.inner.sandboxes.expired_local(SystemTime::now()) {
            let key = format!("local:{name}");
            let r = match action {
                cua_sandbox_core::LocalExpiry::Suspend => self.suspend(&key).await,
                cua_sandbox_core::LocalExpiry::Delete => self.delete_with_space(&key).await,
            };
            match r {
                Ok(()) => {
                    tracing::info!(sandbox = %name, action = action.as_str(), "sandbox lifetime ended");
                    ended.push(name);
                }
                Err(e) => {
                    tracing::warn!(sandbox = %name, error = %e, "could not end an expired sandbox")
                }
            }
        }
        ended
    }

    /// Deletes a sandbox and forgets its Space, when it has one.
    async fn delete_with_space(&self, key: &str) -> Result<()> {
        #[cfg(feature = "spaces")]
        if self.inner.spaces.delete(key).await.is_ok() {
            return Ok(());
        }
        self.delete(key).await
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

    /// A URL for `service` usable from this machine with no credentials: the
    /// published loopback port.
    pub async fn service_url(&self, name: &str, service: &str) -> Result<String> {
        let name = self.key(name).await?;
        let sandbox = self.handle(&name).await?;
        let svc = sandbox.service(service)?;
        Ok(svc.url().trim_end_matches('/').to_string())
    }

    /// The qualified ref of a Space by any spelling (`cua share
    /// space:<ref>:<port>` ends with the Space).
    #[cfg(feature = "spaces")]
    pub(crate) async fn resolve_key(&self, input: &str) -> Result<String> {
        self.key(input).await
    }

    /// A shareable URL for `service` that stops working after `ttl` (60 s to
    /// 24 h, default 1 h): a loopback proxy URL with its own token, hosted by
    /// the cua daemon (started if needed).
    pub async fn public_url(
        &self,
        name: &str,
        service: &str,
        ttl: Option<Duration>,
        label: Option<String>,
    ) -> Result<crate::shares::PublicUrl> {
        let _ = label;
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

    /// Revokes a public URL (a local share).
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
        Err(Error::NotFound(format!("public URL {id} not found")))
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
        if let Some(a) = cached {
            return Ok(a);
        }
        let sandbox = self.handle(name).await?;
        let client = sandbox
            .spacesd_with(cua_sandbox_core::ConnectOptionsOverride {
                probe_timeout: Some(probe_timeout.unwrap_or(self.inner.env_probe_timeout)),
                ..Default::default()
            })
            .await?;
        let a = SpacesdAttachment {
            client,
            ws_headers: vec![],
        };
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
/// does not say, like `direct`).
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

/// The sandbox-layer options of `req` on `provider`, with `kind` and
/// `runtime` resolved (explicit, else the user defaults).
pub(crate) fn options_from(
    req: CreateRequest,
    provider: ProviderKind,
    kind: cua_sandbox_core::placement::Kind,
    runtime: cua_sandbox_core::placement::Runtime,
) -> Result<CreateOptions> {
    let profile = cua_sandbox_core::profile::Profile::parse(req.profile.as_deref().unwrap_or(""))
        .map_err(Error::InvalidArgument)?;
    let mut o = CreateOptions::new(provider, req.image.clone());
    o.name = req.name.clone().filter(|n| !n.is_empty());
    if let Some(os) = req.os.filter(|s| !s.is_empty()) {
        o.os = os;
    }
    let (mut cpus_given, mut memory_given) = (false, false);
    if let Some(c) = req.cpus.filter(|c| *c > 0) {
        o.cpus = c;
        cpus_given = true;
    }
    if let Some(m) = req.memory_mb.filter(|m| *m > 0) {
        o.memory_mb = m;
        memory_given = true;
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
    o.headless = req.headless;
    o.spacesd = req.spacesd;
    o.forkable = req.forkable;
    o.kind = kind;
    o.runtime = runtime;
    o.apply_profile(profile, cpus_given, memory_given);
    Ok(o)
}

/// The options a local create of `req` runs with: its kind and runtime,
/// else the user defaults (what [`Runtime::create`] does for a local one).
pub(crate) fn local_options(req: CreateRequest) -> Result<CreateOptions> {
    use cua_sandbox_core::placement::{Kind, On, Runtime};
    let word = |s: &Option<String>| {
        s.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let kind = word(&req.kind).map(|k| Kind::parse(&k)).transpose()?;
    let runtime = word(&req.runtime).map(|r| Runtime::parse(&r)).transpose()?;
    let resolved = cua_sandbox_core::settings::Settings::load()
        .and_then(|s| s.resolve(Some(On::Local), kind, runtime))
        .map_err(|e| Error::InvalidArgument(e.to_string()))?;
    let mut o = options_from(req, ProviderKind::Local, resolved.kind, resolved.runtime)?;
    o.apply_placement()?;
    Ok(o)
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
        stop_reason: None,
    }
}
